#![cfg(target_os = "windows")]

use std::fs;

use anyhow::Result;
use codex_config::types::ApprovalsReviewer;
use codex_core::TurnInputRequest;
use codex_core::config::Constrained;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::models::PermissionProfile;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::ThreadSettingsOverrides;
use codex_protocol::user_input::UserInput;
use core_test_support::PathExt;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn function_models_can_edit_existing_files_and_create_new_files() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(
                    "edit-call",
                    "edit_file",
                    &json!({
                        "file_path": "existing.txt",
                        "old_string": "before",
                        "new_string": "after"
                    })
                    .to_string(),
                ),
                ev_function_call(
                    "write-call",
                    "write_file",
                    &json!({
                        "file_path": "nested/new.txt",
                        "content": "created without final newline"
                    })
                    .to_string(),
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_assistant_message("message-1", "file changes complete"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_model_info_override("gpt-5.5", |model_info| {
            model_info.apply_patch_tool_type = Some(ApplyPatchToolType::Function);
        })
        .build_with_auto_env(&server)
        .await?;
    fs::write(test.workspace_path("existing.txt"), "before\r\n")?;

    test.submit_turn("edit one file and create another").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let tools = requests[0].body_json()["tools"]
        .as_array()
        .expect("request should advertise tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for name in ["edit_file", "write_file", "apply_patch"] {
        assert!(tools.contains(&name), "missing {name} tool");
    }
    for call_id in ["edit-call", "write-call"] {
        let (content, success) = requests[1]
            .function_call_output_content_and_success(call_id)
            .expect("function output should be present");
        assert_eq!(success, Some(true));
        assert!(
            content
                .as_deref()
                .is_some_and(|content| content.contains("Success. Updated"))
        );
    }
    assert_eq!(fs::read(test.workspace_path("existing.txt"))?, b"after\r\n");
    assert_eq!(
        fs::read(test.workspace_path("nested/new.txt"))?,
        b"created without final newline"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_errors_are_recoverable_and_leave_files_unchanged() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-errors-1"),
                ev_function_call(
                    "existing-write",
                    "write_file",
                    &json!({"file_path": "existing.txt", "content": "replace"}).to_string(),
                ),
                ev_function_call(
                    "missing-edit",
                    "edit_file",
                    &json!({
                        "file_path": "existing.txt",
                        "old_string": "absent",
                        "new_string": "replacement"
                    })
                    .to_string(),
                ),
                ev_function_call(
                    "ambiguous-edit",
                    "edit_file",
                    &json!({
                        "file_path": "duplicate.txt",
                        "old_string": "same",
                        "new_string": "changed"
                    })
                    .to_string(),
                ),
                ev_completed("resp-errors-1"),
            ]),
            sse(vec![
                ev_assistant_message("message-errors", "recovered from file errors"),
                ev_completed("resp-errors-2"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_model_info_override("gpt-5.5", |model_info| {
            model_info.apply_patch_tool_type = Some(ApplyPatchToolType::Function);
        })
        .build_with_auto_env(&server)
        .await?;
    fs::write(test.workspace_path("existing.txt"), "keep")?;
    fs::write(test.workspace_path("duplicate.txt"), "same\nsame\n")?;

    test.submit_turn("exercise recoverable mutation errors")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    for (call_id, expected_message) in [
        ("existing-write", "will not overwrite"),
        ("missing-edit", "could not find old_string"),
        ("ambiguous-edit", "found old_string 2 times"),
    ] {
        let (content, success) = requests[1]
            .function_call_output_content_and_success(call_id)
            .expect("failed function output should be present");
        assert_eq!(success, Some(false));
        assert!(
            content
                .as_deref()
                .is_some_and(|content| content.contains(expected_message)),
            "missing recovery guidance for {call_id}: {content:?}"
        );
    }
    assert_eq!(fs::read(test.workspace_path("existing.txt"))?, b"keep");
    assert_eq!(
        fs::read(test.workspace_path("duplicate.txt"))?,
        b"same\nsame\n"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_sandbox_denies_outside_mutation_without_writing() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let outside_path = std::env::temp_dir().join(format!(
        "codex-structured-write-denied-{}-outside.txt",
        std::process::id()
    ));
    let _ = fs::remove_file(&outside_path);
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-denied-1"),
                ev_function_call(
                    "outside-write",
                    "write_file",
                    &json!({
                        "file_path": outside_path.to_string_lossy(),
                        "content": "must not be written"
                    })
                    .to_string(),
                ),
                ev_completed("resp-denied-1"),
            ]),
            sse(vec![
                ev_assistant_message("message-denied", "outside write denied"),
                ev_completed("resp-denied-2"),
            ]),
        ],
    )
    .await;
    let permission_profile = PermissionProfile::workspace_write_with(
        &[],
        NetworkSandboxPolicy::Restricted,
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    let test = test_codex()
        .with_config(move |config| {
            config.cwd = dunce::canonicalize(config.cwd.as_path())
                .expect("test workspace should be canonicalizable")
                .abs();
            config.workspace_roots = vec![config.cwd.clone()];
            config
                .permissions
                .set_permission_profile(permission_profile)
                .expect("workspace permission profile should be accepted");
            config.set_windows_sandbox_enabled(/*value*/ true);
        })
        .with_model_info_override("gpt-5.5", |model_info| {
            model_info.apply_patch_tool_type = Some(ApplyPatchToolType::Function);
        })
        .build_with_auto_env(&server)
        .await?;

    test.submit_turn("try to write outside the workspace")
        .await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let (_, success) = requests[1]
        .function_call_output_content_and_success("outside-write")
        .expect("denied function output should be present");
    assert_ne!(success, Some(true));
    assert!(!outside_path.exists());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn structured_writes_wait_for_user_approval_and_honor_the_decision() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            structured_write_response("approve-write", "approved.txt"),
            sse(vec![
                ev_assistant_message("approved-message", "approved"),
                ev_completed("approved-follow-up"),
            ]),
            structured_write_response("deny-write", "denied.txt"),
            sse(vec![
                ev_assistant_message("denied-message", "denied"),
                ev_completed("denied-follow-up"),
            ]),
            structured_write_response("raced-write", "raced.txt"),
            sse(vec![
                ev_assistant_message("raced-message", "race rejected"),
                ev_completed("raced-follow-up"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.permissions.approval_policy =
                Constrained::allow_any(AskForApproval::UnlessTrusted);
            config
                .set_legacy_sandbox_policy(SandboxPolicy::DangerFullAccess)
                .expect("danger-full-access policy should be accepted");
        })
        .with_model_info_override("gpt-5.5", |model_info| {
            model_info.apply_patch_tool_type = Some(ApplyPatchToolType::Function);
        })
        .build_with_auto_env(&server)
        .await?;

    start_approval_turn(&test, "approve structured write").await?;
    let approval = wait_for_patch_approval(&test, "approve-write").await;
    submit_patch_decision(&test, approval.call_id, ReviewDecision::Approved).await?;
    assert_eq!(fs::read(test.workspace_path("approved.txt"))?, b"content");

    start_approval_turn(&test, "deny structured write").await?;
    let approval = wait_for_patch_approval(&test, "deny-write").await;
    submit_patch_decision(
        &test,
        approval.call_id,
        ReviewDecision::denied("rejected by user"),
    )
    .await?;
    assert!(!test.workspace_path("denied.txt").exists());

    start_approval_turn(&test, "race structured write").await?;
    let approval = wait_for_patch_approval(&test, "raced-write").await;
    fs::write(test.workspace_path("raced.txt"), "external")?;
    submit_patch_decision(&test, approval.call_id, ReviewDecision::Approved).await?;
    assert_eq!(fs::read(test.workspace_path("raced.txt"))?, b"external");

    let requests = responses.requests();
    assert_eq!(requests.len(), 6);
    let (_, approved_success) = requests[1]
        .function_call_output_content_and_success("approve-write")
        .expect("approved write should produce a tool result");
    assert_eq!(approved_success, Some(true));
    let (_, denied_success) = requests[3]
        .function_call_output_content_and_success("deny-write")
        .expect("denied write should produce a tool result");
    assert_eq!(denied_success, Some(false));
    let (_, raced_success) = requests[5]
        .function_call_output_content_and_success("raced-write")
        .expect("raced write should produce a tool result");
    assert_eq!(raced_success, Some(false));

    Ok(())
}

fn structured_write_response(call_id: &str, file_path: &str) -> String {
    let response_id = format!("{call_id}-response");
    sse(vec![
        ev_response_created(&response_id),
        ev_function_call(
            call_id,
            "write_file",
            &json!({"file_path": file_path, "content": "content"}).to_string(),
        ),
        ev_completed(&response_id),
    ])
}

async fn start_approval_turn(test: &TestCodex, prompt: &str) -> Result<()> {
    let model = test.session_configured.model.clone();
    test.codex
        .start_or_steer_turn(
            TurnInputRequest::user_input(vec![UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            }])
            .with_thread_settings(ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::UnlessTrusted),
                approvals_reviewer: Some(ApprovalsReviewer::User),
                sandbox_policy: Some(SandboxPolicy::DangerFullAccess),
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Default,
                    settings: Settings {
                        model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            }),
        )
        .await?;
    Ok(())
}

async fn wait_for_patch_approval(
    test: &TestCodex,
    expected_call_id: &str,
) -> codex_protocol::protocol::ApplyPatchApprovalRequestEvent {
    let event = wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::ApplyPatchApprovalRequest(_) | EventMsg::TurnComplete(_)
        )
    })
    .await;
    let EventMsg::ApplyPatchApprovalRequest(approval) = event else {
        panic!("expected patch approval before turn completion");
    };
    assert_eq!(approval.call_id, expected_call_id);
    approval
}

async fn submit_patch_decision(
    test: &TestCodex,
    call_id: String,
    decision: ReviewDecision,
) -> Result<()> {
    test.codex
        .submit(Op::PatchApproval {
            id: call_id,
            decision,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    Ok(())
}
