use super::*;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
use crate::tools::context::ToolCallSource;
use crate::tools::registry::PreToolUsePayload;
use crate::turn_diff_tracker::TurnDiffTracker;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn invocation(kind: FileMutationToolKind, arguments: serde_json::Value) -> ToolInvocation {
    let (session, turn) = make_session_and_context().await;
    let turn = Arc::new(turn);
    ToolInvocation {
        session: session.into(),
        step_context: StepContext::for_test(Arc::clone(&turn)),
        turn,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-file-mutation".to_string(),
        tool_name: ToolName::plain(kind.name()),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

#[test]
fn file_paths_are_bounded_and_model_visible_text_is_truncated() {
    let boundary = "a".repeat(MAX_FILE_PATH_BYTES);
    assert!(validate_file_path(FileMutationToolKind::Edit, &boundary).is_ok());
    let oversized = format!("{boundary}a");
    assert!(validate_file_path(FileMutationToolKind::Edit, &oversized).is_err());

    let displayed = truncate_for_model(&oversized);
    assert_eq!(displayed.chars().count(), MAX_MODEL_VISIBLE_PATH_CHARS);
    assert!(displayed.ends_with('…'));
    assert!(!displayed.contains(&oversized));
}

#[tokio::test]
async fn structured_file_tools_use_apply_patch_compatible_hook_identities() {
    let edit = FileMutationToolHandler::new(FileMutationToolKind::Edit);
    let edit_input = json!({
        "file_path": "file.txt",
        "old_string": "before",
        "new_string": "after"
    });
    assert_eq!(
        edit.pre_tool_use_payload(
            &invocation(FileMutationToolKind::Edit, edit_input.clone()).await
        ),
        Some(PreToolUsePayload {
            tool_name: HookToolName::edit_file(),
            tool_input: edit_input,
        })
    );

    let write = FileMutationToolHandler::new(FileMutationToolKind::Write);
    let write_input = json!({"file_path": "new.txt", "content": "new"});
    assert_eq!(
        write.pre_tool_use_payload(
            &invocation(FileMutationToolKind::Write, write_input.clone()).await
        ),
        Some(PreToolUsePayload {
            tool_name: HookToolName::write_file(),
            tool_input: write_input,
        })
    );
}
