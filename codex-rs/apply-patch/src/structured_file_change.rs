use std::io;

use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::GetMetadataOptions;
use codex_exec_server::ReadFileOptions;
use codex_exec_server::WriteDisposition;
use similar::TextDiff;

use crate::AppliedPatchChange;
use crate::AppliedPatchDelta;
use crate::AppliedPatchFileChange;
use crate::ApplyPatchAction;
use crate::ApplyPatchError;
use crate::ApplyPatchFailure;
use crate::ApplyPatchFileChange;
use crate::ApplyPatchFileUpdateMode;
use crate::write_file_with_missing_parent_retry;
use codex_utils_path_uri::PathUri;

const MAX_STRUCTURED_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODEL_VISIBLE_PATH_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructuredFileMutationKind {
    Edit,
    Write,
}

#[derive(Debug)]
pub struct StructuredFileMutation {
    kind: StructuredFileMutationKind,
    path: PathUri,
    expected_contents: Option<Vec<u8>>,
    original_content: Option<String>,
    new_contents: Vec<u8>,
    new_content: String,
    unified_diff: String,
}

impl StructuredFileMutation {
    pub fn action(&self, cwd: &PathUri) -> ApplyPatchAction {
        let change = match self.kind {
            StructuredFileMutationKind::Edit => ApplyPatchFileChange::Update {
                unified_diff: self.unified_diff.clone(),
                move_path: None,
                new_content: self.new_content.clone(),
            },
            StructuredFileMutationKind::Write => ApplyPatchFileChange::Add {
                content: self.new_content.clone(),
            },
        };
        ApplyPatchAction {
            changes: std::collections::HashMap::from([(self.path.clone(), change)]),
            update_file_mode: ApplyPatchFileUpdateMode::PreserveLineEndings,
            patch: self.unified_diff.clone(),
            cwd: cwd.clone(),
        }
    }
}

pub async fn prepare_structured_edit(
    path: PathUri,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<StructuredFileMutation, ApplyPatchError> {
    if old_string.is_empty() {
        return Err(ApplyPatchError::StructuredFileMutation(
            "edit_file.old_string must not be empty".to_string(),
        ));
    }
    if old_string == new_string {
        return Err(ApplyPatchError::NoFilesModified);
    }
    if new_string.contains('\0') {
        return Err(ApplyPatchError::StructuredFileMutation(
            "edit_file only supports text files; new_string contains a NUL byte".to_string(),
        ));
    }

    let original_bytes = read_editable_file(&path, fs, sandbox).await?;
    let original_content = decode_utf8_text(&path, &original_bytes)?;
    let (bom, editable_content) = original_content
        .strip_prefix('\u{feff}')
        .map_or(("", original_content.as_str()), |content| {
            ("\u{feff}", content)
        });
    let normalized = normalize_line_endings_with_offsets(editable_content);
    let old_string = normalize_line_endings(old_string);
    let new_string = normalize_line_endings(new_string);
    let matches = normalized
        .text
        .match_indices(&old_string)
        .map(|(start, _)| (start, start + old_string.len()))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file could not find old_string in {}. Re-read the file and copy the exact text, including indentation.",
            display_path(&path)
        )));
    }
    if matches.len() > 1 && !replace_all {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file found old_string {} times in {}. Include more surrounding context or set replace_all to true.",
            matches.len(),
            display_path(&path)
        )));
    }

    let preferred_ending = preferred_line_ending(editable_content);
    let mut updated = editable_content.as_bytes().to_vec();
    let selected = if replace_all {
        matches.as_slice()
    } else {
        &matches[..1]
    };
    for &(normalized_start, normalized_end) in selected.iter().rev() {
        let start = normalized.original_offsets[normalized_start];
        let end = normalized.original_offsets[normalized_end];
        let replacement = restore_matched_line_endings(
            &new_string,
            &editable_content[start..end],
            preferred_ending,
        );
        updated.splice(start..end, replacement.as_bytes().iter().copied());
    }
    preserve_final_line_ending(
        &mut updated,
        has_final_line_ending(editable_content.as_bytes()),
        preferred_ending,
    );

    let mut new_contents = bom.as_bytes().to_vec();
    new_contents.extend_from_slice(&updated);
    if new_contents.len() > MAX_STRUCTURED_FILE_BYTES {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file result exceeds the {MAX_STRUCTURED_FILE_BYTES}-byte limit"
        )));
    }
    if new_contents == original_bytes {
        return Err(ApplyPatchError::NoFilesModified);
    }
    let new_content = String::from_utf8(new_contents.clone()).map_err(|err| {
        ApplyPatchError::StructuredFileMutation(format!(
            "edit_file produced invalid UTF-8 for {}: {err}",
            display_path(&path)
        ))
    })?;
    let unified_diff = TextDiff::from_lines(&original_content, &new_content)
        .unified_diff()
        .context_radius(3)
        .to_string();

    Ok(StructuredFileMutation {
        kind: StructuredFileMutationKind::Edit,
        path,
        expected_contents: Some(original_bytes),
        original_content: Some(original_content),
        new_contents,
        new_content,
        unified_diff,
    })
}

pub async fn prepare_structured_write(
    path: PathUri,
    content: String,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<StructuredFileMutation, ApplyPatchError> {
    if content.len() > MAX_STRUCTURED_FILE_BYTES {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "write_file content exceeds the {MAX_STRUCTURED_FILE_BYTES}-byte limit"
        )));
    }
    if content.contains('\0') {
        return Err(ApplyPatchError::StructuredFileMutation(
            "write_file only supports text files; content contains a NUL byte".to_string(),
        ));
    }
    match fs
        .get_metadata(&path, GetMetadataOptions::default(), sandbox)
        .await
    {
        Ok(_) => {
            return Err(ApplyPatchError::StructuredFileMutation(format!(
                "write_file will not overwrite existing path {}. Use edit_file instead.",
                display_path(&path)
            )));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(structured_io_error(
                "Failed to inspect file to create",
                &path,
                source,
            ));
        }
    }

    let unified_diff = TextDiff::from_lines("", &content)
        .unified_diff()
        .context_radius(3)
        .to_string();
    Ok(StructuredFileMutation {
        kind: StructuredFileMutationKind::Write,
        path,
        expected_contents: None,
        original_content: None,
        new_contents: content.as_bytes().to_vec(),
        new_content: content,
        unified_diff,
    })
}

pub async fn apply_structured_file_mutation(
    mutation: &StructuredFileMutation,
    follow_symlinks: bool,
    stdout: &mut impl io::Write,
    stderr: &mut impl io::Write,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    let mut delta = AppliedPatchDelta::empty();
    let result =
        apply_structured_file_mutation_inner(mutation, follow_symlinks, fs, sandbox, &mut delta)
            .await;
    match result {
        Ok(()) => {
            writeln!(stdout, "Success. Updated {}", display_path(&mutation.path))
                .map_err(|err| ApplyPatchFailure::new(ApplyPatchError::from(err), delta.clone()))?;
            Ok(delta)
        }
        Err(error) => {
            let message = error.to_string();
            writeln!(stderr, "{message}")
                .map_err(|err| ApplyPatchFailure::new(ApplyPatchError::from(err), delta.clone()))?;
            Err(ApplyPatchFailure::new(error, delta))
        }
    }
}

async fn apply_structured_file_mutation_inner(
    mutation: &StructuredFileMutation,
    follow_symlinks: bool,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
    delta: &mut AppliedPatchDelta,
) -> Result<(), ApplyPatchError> {
    if let Some(expected) = &mutation.expected_contents {
        let current = fs
            .read_file(&mutation.path, ReadFileOptions { follow_symlinks }, sandbox)
            .await
            .map_err(|source| {
                structured_io_error("Failed to re-read file before edit", &mutation.path, source)
            })?;
        if &current != expected {
            return Err(ApplyPatchError::StructuredFileMutation(format!(
                "{} changed after edit_file validation; no changes were written. Re-read the file and retry.",
                display_path(&mutation.path)
            )));
        }
    }

    if write_file_with_missing_parent_retry(
        fs,
        &mutation.path,
        mutation.new_contents.clone(),
        follow_symlinks,
        match mutation.kind {
            StructuredFileMutationKind::Edit => WriteDisposition::Overwrite,
            StructuredFileMutationKind::Write => WriteDisposition::CreateNew,
        },
        sandbox,
    )
    .await
    .is_err()
    {
        delta.exact = false;
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "Failed to write file {}. No changes were committed.",
            display_path(&mutation.path)
        )));
    }

    let change = match mutation.kind {
        StructuredFileMutationKind::Edit => AppliedPatchFileChange::Update {
            move_path: None,
            old_content: mutation.original_content.clone().unwrap_or_default(),
            overwritten_move_content: None,
            new_content: mutation.new_content.clone(),
        },
        StructuredFileMutationKind::Write => AppliedPatchFileChange::Add {
            content: mutation.new_content.clone(),
            overwritten_content: None,
        },
    };
    delta.changes.push(AppliedPatchChange {
        path: mutation.path.clone(),
        change,
    });
    Ok(())
}

async fn read_editable_file(
    path: &PathUri,
    fs: &dyn ExecutorFileSystem,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<Vec<u8>, ApplyPatchError> {
    let metadata = fs
        .get_metadata(path, GetMetadataOptions::default(), sandbox)
        .await
        .map_err(|source| structured_io_error("Failed to inspect file to edit", path, source))?;
    if !metadata.is_file {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file target is not a file: {}",
            display_path(path)
        )));
    }
    if metadata.size > MAX_STRUCTURED_FILE_BYTES as u64 {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file target exceeds the {MAX_STRUCTURED_FILE_BYTES}-byte limit: {}",
            display_path(path)
        )));
    }
    let bytes = fs
        .read_file(path, ReadFileOptions::default(), sandbox)
        .await
        .map_err(|source| structured_io_error("Failed to read file to edit", path, source))?;
    if bytes.len() > MAX_STRUCTURED_FILE_BYTES {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file target exceeds the {MAX_STRUCTURED_FILE_BYTES}-byte limit: {}",
            display_path(path)
        )));
    }
    Ok(bytes)
}

fn decode_utf8_text(path: &PathUri, bytes: &[u8]) -> Result<String, ApplyPatchError> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file supports UTF-8 text only; {} is UTF-16",
            display_path(path)
        )));
    }
    if bytes.contains(&0) {
        return Err(ApplyPatchError::StructuredFileMutation(format!(
            "edit_file only supports text files; {} appears to be binary",
            display_path(path)
        )));
    }
    String::from_utf8(bytes.to_vec()).map_err(|err| {
        ApplyPatchError::StructuredFileMutation(format!(
            "edit_file supports UTF-8 text only for {}: {err}",
            display_path(path)
        ))
    })
}

struct NormalizedText {
    text: String,
    original_offsets: Vec<usize>,
}

fn normalize_line_endings_with_offsets(input: &str) -> NormalizedText {
    let bytes = input.as_bytes();
    let mut text = String::with_capacity(input.len());
    let mut original_offsets = Vec::with_capacity(input.len() + 1);
    original_offsets.push(0);
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' {
            let ending_len = usize::from(bytes.get(index + 1) == Some(&b'\n')) + 1;
            text.push('\n');
            index += ending_len;
            original_offsets.push(index);
        } else {
            let Some(character) = input[index..].chars().next() else {
                break;
            };
            text.push(character);
            for byte_offset in 1..=character.len_utf8() {
                original_offsets.push(index + byte_offset);
            }
            index += character.len_utf8();
        }
    }
    NormalizedText {
        text,
        original_offsets,
    }
}

fn normalize_line_endings(input: &str) -> String {
    input.replace("\r\n", "\n").replace('\r', "\n")
}

fn preferred_line_ending(input: &str) -> &'static str {
    let bytes = input.as_bytes();
    for index in 0..bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => return "\r\n",
            b'\r' => return "\r",
            b'\n' => return "\n",
            _ => {}
        }
    }
    "\n"
}

fn restore_matched_line_endings(input: &str, matched: &str, fallback: &str) -> String {
    let matched_endings = line_endings(matched);
    let mut output = String::with_capacity(input.len());
    for (index, part) in input.split('\n').enumerate() {
        if index > 0 {
            output.push_str(matched_endings.get(index - 1).copied().unwrap_or(fallback));
        }
        output.push_str(part);
    }
    output
}

fn line_endings(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let mut endings = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                endings.push("\r\n");
                index += 2;
            }
            b'\r' => {
                endings.push("\r");
                index += 1;
            }
            b'\n' => {
                endings.push("\n");
                index += 1;
            }
            _ => index += 1,
        }
    }
    endings
}

fn has_final_line_ending(bytes: &[u8]) -> bool {
    bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
}

fn preserve_final_line_ending(bytes: &mut Vec<u8>, terminated: bool, preferred_ending: &str) {
    if terminated && !has_final_line_ending(bytes) {
        bytes.extend_from_slice(preferred_ending.as_bytes());
    } else if !terminated {
        while has_final_line_ending(bytes) {
            bytes.pop();
        }
    }
}

fn structured_io_error(context: &str, path: &PathUri, source: io::Error) -> ApplyPatchError {
    ApplyPatchError::StructuredFileMutation(format!(
        "{context} {} ({:?})",
        display_path(path),
        source.kind()
    ))
}

fn display_path(path: &PathUri) -> String {
    let path = path.inferred_native_path_string();
    let mut characters = path.chars();
    let prefix = characters
        .by_ref()
        .take(MAX_MODEL_VISIBLE_PATH_CHARS.saturating_sub(1))
        .collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
#[path = "structured_file_change_tests.rs"]
mod tests;
