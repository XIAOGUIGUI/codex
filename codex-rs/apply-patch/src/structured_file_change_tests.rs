use std::fs;
use std::sync::Arc;

use codex_exec_server::CopyOptions;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::ExecutorFileSystemFuture;
use codex_exec_server::FileMetadata;
use codex_exec_server::FileSystemReadStream;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::GetMetadataOptions;
use codex_exec_server::LOCAL_FS;
use codex_exec_server::ReadDirectoryEntry;
use codex_exec_server::ReadFileOptions;
use codex_exec_server::RemoveOptions;
use codex_exec_server::WalkOptions;
use codex_exec_server::WalkOutcome;
use codex_exec_server::WriteFileOptions;
use codex_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::*;

fn path_uri(path: &std::path::Path) -> PathUri {
    PathUri::from_host_native_path(path).unwrap()
}

struct GrowingAfterMetadataFileSystem {
    inner: Arc<dyn ExecutorFileSystem>,
    replacement: Vec<u8>,
}

impl ExecutorFileSystem for GrowingAfterMetadataFileSystem {
    fn canonicalize<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, PathUri> {
        self.inner.canonicalize(path, sandbox)
    }

    fn read_file<'a>(
        &'a self,
        path: &'a PathUri,
        options: ReadFileOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
        self.inner.read_file(path, options, sandbox)
    }

    fn read_file_stream<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
        self.inner.read_file_stream(path, sandbox)
    }

    fn write_file<'a>(
        &'a self,
        path: &'a PathUri,
        contents: Vec<u8>,
        options: WriteFileOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        self.inner.write_file(path, contents, options, sandbox)
    }

    fn create_directory<'a>(
        &'a self,
        path: &'a PathUri,
        options: CreateDirectoryOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        self.inner.create_directory(path, options, sandbox)
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a PathUri,
        options: GetMetadataOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
        Box::pin(async move {
            let metadata = self.inner.get_metadata(path, options, sandbox).await?;
            let native_path = path
                .to_abs_path()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            fs::write(native_path.as_path(), &self.replacement)?;
            Ok(metadata)
        })
    }

    fn read_directory<'a>(
        &'a self,
        path: &'a PathUri,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
        self.inner.read_directory(path, sandbox)
    }

    fn walk<'a>(
        &'a self,
        path: &'a PathUri,
        options: WalkOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, WalkOutcome> {
        self.inner.walk(path, options, sandbox)
    }

    fn remove<'a>(
        &'a self,
        path: &'a PathUri,
        options: RemoveOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        self.inner.remove(path, options, sandbox)
    }

    fn copy<'a>(
        &'a self,
        source_path: &'a PathUri,
        destination_path: &'a PathUri,
        options: CopyOptions,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> ExecutorFileSystemFuture<'a, ()> {
        self.inner
            .copy(source_path, destination_path, options, sandbox)
    }
}

#[test]
fn model_visible_paths_and_io_errors_are_bounded() {
    let secret_suffix = "secret-path-component";
    let long_path = format!("/{}-{secret_suffix}", "a".repeat(400));
    let path = PathUri::from_host_native_path(std::path::Path::new(&long_path)).unwrap();
    let displayed = display_path(&path);
    assert_eq!(displayed.chars().count(), MAX_MODEL_VISIBLE_PATH_CHARS);
    assert!(displayed.ends_with('…'));
    assert!(!displayed.contains(secret_suffix));

    let source = io::Error::new(io::ErrorKind::PermissionDenied, long_path);
    let error = structured_io_error("Failed to inspect file", &path, source).to_string();
    assert!(!error.contains(secret_suffix));
    assert!(error.contains("PermissionDenied"));
}

async fn apply(mutation: &StructuredFileMutation) -> Result<AppliedPatchDelta, ApplyPatchFailure> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    apply_structured_file_mutation(
        mutation,
        /*follow_symlinks*/ true,
        &mut stdout,
        &mut stderr,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
}

#[tokio::test]
async fn edit_preserves_bom_mixed_line_endings_and_unicode() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("中文 file.txt");
    let original = b"\xef\xbb\xbfalpha\r\n  \xe4\xb8\xad\xe6\x96\x87\nlast\r\nuntouched\n";
    fs::write(&path, original).unwrap();

    let mutation = prepare_structured_edit(
        path_uri(&path),
        "  中文\nlast",
        "  修改\nlast",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    apply(&mutation).await.unwrap();

    assert_eq!(
        fs::read(&path).unwrap(),
        b"\xef\xbb\xbfalpha\r\n  \xe4\xbf\xae\xe6\x94\xb9\nlast\r\nuntouched\n"
    );
}

#[tokio::test]
async fn edit_requires_an_exact_unique_match_by_default() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("duplicate.txt");
    fs::write(&path, "  same\n  same\n").unwrap();

    let indent_error = prepare_structured_edit(
        path_uri(&path),
        "same",
        "changed",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();
    assert!(
        indent_error
            .to_string()
            .contains("found old_string 2 times")
    );

    let missing_error = prepare_structured_edit(
        path_uri(&path),
        "   same",
        "changed",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();
    assert!(
        missing_error
            .to_string()
            .contains("could not find old_string")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "  same\n  same\n");
}

#[tokio::test]
async fn edit_replace_all_updates_every_exact_match() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("all.txt");
    fs::write(&path, "same\r\nsame\r\n").unwrap();

    let mutation = prepare_structured_edit(
        path_uri(&path),
        "same",
        "changed",
        true,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    apply(&mutation).await.unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"changed\r\nchanged\r\n");
}

#[tokio::test]
async fn edit_rejects_stale_file_before_writing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("stale.txt");
    fs::write(&path, "before\n").unwrap();
    let mutation = prepare_structured_edit(
        path_uri(&path),
        "before",
        "after",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    fs::write(&path, "external\n").unwrap();

    let error = apply(&mutation).await.unwrap_err();

    assert!(
        error
            .to_string()
            .contains("changed after edit_file validation")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "external\n");
}

#[tokio::test]
async fn write_creates_exact_content_and_missing_parents() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested").join("new.txt");
    let mutation = prepare_structured_write(
        path_uri(&path),
        "no final newline".to_string(),
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    apply(&mutation).await.unwrap();

    assert_eq!(fs::read(&path).unwrap(), b"no final newline");
}

#[tokio::test]
async fn write_never_overwrites_an_existing_or_concurrently_created_file() {
    let dir = tempdir().unwrap();
    let existing = dir.path().join("existing.txt");
    fs::write(&existing, "keep").unwrap();
    let existing_error = prepare_structured_write(
        path_uri(&existing),
        "replace".to_string(),
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();
    assert!(existing_error.to_string().contains("will not overwrite"));

    let raced = dir.path().join("raced.txt");
    let mutation = prepare_structured_write(
        path_uri(&raced),
        "ours".to_string(),
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    fs::write(&raced, "external").unwrap();
    let race_error = apply(&mutation).await.unwrap_err();

    assert!(race_error.to_string().contains("Failed to write file"));
    assert_eq!(fs::read_to_string(&raced).unwrap(), "external");
}

#[cfg(unix)]
#[tokio::test]
async fn write_does_not_follow_a_dangling_symlink() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    let target = dir.path().join("missing-target.txt");
    let link = dir.path().join("link.txt");
    symlink(&target, &link).unwrap();
    let mutation = prepare_structured_write(
        path_uri(&link),
        "must not be written".to_string(),
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    assert!(apply(&mutation).await.is_err());
    assert!(link.is_symlink());
    assert!(!target.exists());
}

#[tokio::test]
async fn edit_rejects_utf16_and_no_op() {
    let dir = tempdir().unwrap();
    let utf16 = dir.path().join("utf16.txt");
    fs::write(&utf16, [0xff, 0xfe, b'a', 0]).unwrap();
    let encoding_error = prepare_structured_edit(
        path_uri(&utf16),
        "a",
        "b",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();
    assert!(encoding_error.to_string().contains("is UTF-16"));

    let utf8 = dir.path().join("utf8.txt");
    fs::write(&utf8, "same").unwrap();
    assert_eq!(
        prepare_structured_edit(
            path_uri(&utf8),
            "same",
            "same",
            false,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err(),
        ApplyPatchError::NoFilesModified
    );
}

#[tokio::test]
async fn edit_rejects_empty_search_and_nul_replacement() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("input.txt");
    fs::write(&path, "before").unwrap();

    let empty_error = prepare_structured_edit(
        path_uri(&path),
        "",
        "after",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();
    assert!(empty_error.to_string().contains("must not be empty"));

    let nul_error = prepare_structured_edit(
        path_uri(&path),
        "before",
        "after\0",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();
    assert!(nul_error.to_string().contains("NUL"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "before");
}

#[tokio::test]
async fn structured_mutations_reject_non_text_and_invalid_targets() {
    let dir = tempdir().unwrap();
    let binary = dir.path().join("binary.txt");
    fs::write(&binary, [b'a', 0, b'b']).unwrap();
    assert!(
        prepare_structured_edit(
            path_uri(&binary),
            "a",
            "b",
            false,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("binary")
    );

    let invalid_utf8 = dir.path().join("invalid.txt");
    fs::write(&invalid_utf8, [0x80]).unwrap();
    assert!(
        prepare_structured_edit(
            path_uri(&invalid_utf8),
            "a",
            "b",
            false,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("UTF-8")
    );

    assert!(
        prepare_structured_edit(
            path_uri(dir.path()),
            "a",
            "b",
            false,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("not a file")
    );
    assert!(
        prepare_structured_write(
            path_uri(&dir.path().join("nul.txt")),
            "a\0b".to_string(),
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("NUL")
    );
}

#[tokio::test]
async fn structured_mutations_enforce_size_and_final_newline_boundaries() {
    let dir = tempdir().unwrap();
    let oversized = dir.path().join("oversized.txt");
    fs::write(&oversized, vec![b'a'; MAX_STRUCTURED_FILE_BYTES + 1]).unwrap();
    assert!(
        prepare_structured_edit(
            path_uri(&oversized),
            "a",
            "b",
            false,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("exceeds")
    );

    let boundary_file = dir.path().join("boundary-file.txt");
    let mut boundary_contents = vec![b'a'; MAX_STRUCTURED_FILE_BYTES];
    *boundary_contents.last_mut().unwrap() = b'x';
    fs::write(&boundary_file, boundary_contents).unwrap();
    prepare_structured_edit(
        path_uri(&boundary_file),
        "x",
        "y",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();

    let write_path = dir.path().join("boundary.txt");
    prepare_structured_write(
        path_uri(&write_path),
        "a".repeat(MAX_STRUCTURED_FILE_BYTES),
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    assert!(
        prepare_structured_write(
            path_uri(&write_path),
            "a".repeat(MAX_STRUCTURED_FILE_BYTES + 1),
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("exceeds")
    );

    let expansion = dir.path().join("expansion.txt");
    fs::write(&expansion, "x").unwrap();
    prepare_structured_edit(
        path_uri(&expansion),
        "x",
        &"a".repeat(MAX_STRUCTURED_FILE_BYTES),
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    assert!(
        prepare_structured_edit(
            path_uri(&expansion),
            "x",
            &"a".repeat(MAX_STRUCTURED_FILE_BYTES + 1),
            false,
            LOCAL_FS.as_ref(),
            /*sandbox*/ None,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("result exceeds")
    );

    let unterminated = dir.path().join("unterminated.txt");
    fs::write(&unterminated, "before").unwrap();
    let mutation = prepare_structured_edit(
        path_uri(&unterminated),
        "before",
        "after\n",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    apply(&mutation).await.unwrap();
    assert_eq!(fs::read(&unterminated).unwrap(), b"after");

    let terminated = dir.path().join("terminated.txt");
    fs::write(&terminated, "before\r\n").unwrap();
    let mutation = prepare_structured_edit(
        path_uri(&terminated),
        "before\n",
        "after",
        false,
        LOCAL_FS.as_ref(),
        /*sandbox*/ None,
    )
    .await
    .unwrap();
    apply(&mutation).await.unwrap();
    assert_eq!(fs::read(&terminated).unwrap(), b"after\r\n");
}

#[tokio::test]
async fn edit_rechecks_actual_size_after_metadata_lookup() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("growing.txt");
    fs::write(&path, "small").unwrap();
    let growing_fs = GrowingAfterMetadataFileSystem {
        inner: Arc::clone(&LOCAL_FS),
        replacement: vec![b'a'; MAX_STRUCTURED_FILE_BYTES + 1],
    };

    let error = prepare_structured_edit(
        path_uri(&path),
        "small",
        "updated",
        false,
        &growing_fs,
        /*sandbox*/ None,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("target exceeds"));
    assert_eq!(fs::metadata(path).unwrap().len(), 8 * 1024 * 1024 + 1);
}
