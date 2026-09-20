use codex_diagnostics::CompatibilityDiagnostics;
use codex_diagnostics::CompatibilityEventInput;
use codex_diagnostics::CompatibilityOutcome;
use codex_diagnostics::ToolRepresentation;
use codex_history::ResponseItemEnvelope;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ToolOutputSpillConfig;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ResponseItem;
use sha1::Digest;
use sha1::Sha1;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::warn;

pub(super) async fn spill_large_tool_outputs(
    codex_home: &Path,
    thread_id: ThreadId,
    config: &ToolOutputSpillConfig,
    diagnostics: &CompatibilityDiagnostics,
    items: &mut [ResponseItemEnvelope],
) {
    if !config.enabled {
        return;
    }

    for envelope in items {
        let (call_id, output, representation) = match &mut envelope.item {
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } => (
                call_id.as_deref().unwrap_or("function-call-output"),
                output,
                ToolRepresentation::Function,
            ),
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => (call_id.as_str(), output, ToolRepresentation::Custom),
            _ => continue,
        };
        let FunctionCallOutputBody::Text(text) = &mut output.body else {
            continue;
        };
        if text.len() <= config.threshold_bytes {
            continue;
        }

        let started = Instant::now();
        let input_bytes = text.len();
        match spill_text(codex_home, thread_id, call_id, text, config.preview_bytes).await {
            Ok(preview) => {
                let output_bytes = preview.len();
                *text = preview;
                diagnostics.record(CompatibilityEventInput {
                    phase: "tool.output_spill",
                    outcome: CompatibilityOutcome::Success,
                    tool_name: None,
                    tool_namespace: None,
                    representation,
                    duration: started.elapsed(),
                    input_bytes,
                    output_bytes,
                    error: None,
                });
            }
            Err(error) => {
                warn!(%error, "failed to spill large tool output; preserving inline output");
                let error_text = error.to_string();
                diagnostics.record(CompatibilityEventInput {
                    phase: "tool.output_spill",
                    outcome: CompatibilityOutcome::Failure,
                    tool_name: None,
                    tool_namespace: None,
                    representation,
                    duration: started.elapsed(),
                    input_bytes,
                    output_bytes: input_bytes,
                    error: Some(&error_text),
                });
            }
        }
    }
}

async fn spill_text(
    codex_home: &Path,
    thread_id: ThreadId,
    call_id: &str,
    text: &str,
    preview_bytes: usize,
) -> io::Result<String> {
    let mut hasher = Sha1::new();
    hasher.update(call_id.as_bytes());
    hasher.update([0]);
    hasher.update(text.as_bytes());
    let filename = format!("{:x}.txt", hasher.finalize());
    let relative_path = PathBuf::from("tool-results")
        .join(thread_id.to_string())
        .join(filename);
    let path = codex_home.join(&relative_path);
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("tool output spill path has no parent"))?;
    fs::create_dir_all(parent).await?;

    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
    {
        Ok(mut file) => {
            if let Err(error) = async {
                file.write_all(text.as_bytes()).await?;
                file.flush().await?;
                file.sync_data().await
            }
            .await
            {
                drop(file);
                let _ = fs::remove_file(&path).await;
                return Err(error);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let size = fs::metadata(&path).await?.len();
            if size != u64::try_from(text.len()).unwrap_or(u64::MAX) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "existing tool output spill has an unexpected size",
                ));
            }
        }
        Err(error) => return Err(error),
    }

    let display_path = format!("$CODEX_HOME/{}", relative_path.to_string_lossy());
    Ok(build_preview(
        text,
        text.len(),
        &display_path,
        preview_bytes,
    ))
}

fn build_preview(text: &str, original_bytes: usize, path: &str, max_bytes: usize) -> String {
    let mut notice = format!("[Full tool output: {original_bytes} bytes; saved to {path}]\n");
    if notice.len() >= max_bytes {
        notice.truncate(floor_char_boundary(&notice, max_bytes));
        return notice;
    }

    const SEPARATOR: &str = "\n…\n";
    let remaining = max_bytes.saturating_sub(notice.len() + SEPARATOR.len());
    let head_budget = remaining.saturating_mul(3) / 4;
    let tail_budget = remaining.saturating_sub(head_budget);
    let head_end = floor_char_boundary(text, head_budget);
    let tail_start = ceil_char_boundary(text, text.len().saturating_sub(tail_budget));
    notice.push_str(&text[..head_end]);
    notice.push_str(SEPARATOR);
    notice.push_str(&text[tail_start..]);
    notice
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index = index.saturating_sub(1);
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index = index.saturating_add(1).min(text.len());
    }
    index
}

#[cfg(test)]
#[path = "tool_output_spill_tests.rs"]
mod tests;
