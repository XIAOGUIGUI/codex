use super::*;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

#[test]
fn preview_is_utf8_safe_and_bounded() {
    let text = "开始".repeat(2_000);
    let preview = build_preview(&text, text.len(), "$CODEX_HOME/tool-results/test.txt", 1024);

    assert!(preview.len() <= 1024);
    assert!(preview.contains("Full tool output"));
    assert!(preview.contains('…'));
}

#[tokio::test]
async fn spill_persists_exact_content_before_returning_preview() {
    let home = tempdir().expect("temp dir");
    let thread_id = ThreadId::new();
    let text = "large output\n".repeat(1_000);

    let preview = spill_text(home.path(), thread_id, "call-1", &text, 1024)
        .await
        .expect("spill succeeds");
    let directory = home.path().join("tool-results").join(thread_id.to_string());
    let paths = std::fs::read_dir(directory)
        .expect("spill directory")
        .map(|entry| entry.expect("directory entry").path())
        .collect::<Vec<_>>();

    assert_eq!(paths.len(), 1);
    assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), text);
    assert!(preview.len() <= 1024);
}

#[tokio::test]
async fn repeated_identical_spill_reuses_complete_file() {
    let home = tempdir().expect("temp dir");
    let thread_id = ThreadId::new();
    let text = "same output".repeat(1_000);

    let first = spill_text(home.path(), thread_id, "call-1", &text, 1024)
        .await
        .expect("first spill succeeds");
    let second = spill_text(home.path(), thread_id, "call-1", &text, 1024)
        .await
        .expect("second spill succeeds");

    assert_eq!(first, second);
}

#[tokio::test]
async fn write_failure_preserves_inline_output() {
    let home = tempdir().expect("temp dir");
    let invalid_home = home.path().join("not-a-directory");
    std::fs::write(&invalid_home, "file").expect("create blocking file");
    let original = "important output".repeat(1_000);
    let mut items = vec![ResponseItemEnvelope::new(
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call-1".to_string()),
            name: Some("exec_command".to_string()),
            namespace: None,
            output: FunctionCallOutputPayload::from_text(original.clone()),
            internal_chat_message_metadata_passthrough: None,
        },
    )];
    let config = ToolOutputSpillConfig {
        enabled: true,
        threshold_bytes: 8 * 1024,
        preview_bytes: 1024,
    };

    spill_large_tool_outputs(
        &invalid_home,
        ThreadId::new(),
        &config,
        &CompatibilityDiagnostics::default(),
        &mut items,
    )
    .await;

    let ResponseItem::FunctionCallOutput { output, .. } = &items[0].item else {
        panic!("expected function output");
    };
    assert_eq!(output.text_content(), Some(original.as_str()));
}
