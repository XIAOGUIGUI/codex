use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookSource;
use codex_protocol::request_user_input::RequestUserInputAnswer;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_protocol::request_user_input::RequestUserInputResponse;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::UserInputRequestRequest;
use super::parse_response;
use super::run;
use crate::engine::ClaudeHooksEngine;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::ConfiguredHandlerKind;
use crate::engine::HandlerRunResult;
use crate::engine::command_runner::CommandHookRuntime;
use crate::mcp::HookMcpCall;
use crate::mcp::HookMcpExecutor;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::FutureExt;
use futures::future::BoxFuture;

#[test]
fn accepts_complete_answers_and_preserves_values() {
    let questions = vec![question("first"), question("second")];
    let run_result = successful_run(
        r#"{"answers":{"first":{"answers":["A"]},"second":{"answers":["custom value"]}}}"#,
    );

    assert_eq!(
        parse_response(&questions, &run_result),
        Some(RequestUserInputResponse {
            answers: [
                (
                    "first".to_string(),
                    RequestUserInputAnswer {
                        answers: vec!["A".to_string()],
                    },
                ),
                (
                    "second".to_string(),
                    RequestUserInputAnswer {
                        answers: vec!["custom value".to_string()],
                    },
                ),
            ]
            .into_iter()
            .collect(),
        })
    );
}

#[test]
fn rejects_unhandled_or_incomplete_answers() {
    let questions = vec![question("first"), question("second")];
    for stdout in [
        "",
        "{}",
        "{  }",
        "not json",
        r#"{"answers":{"first":{"answers":["A"]}}}"#,
        r#"{"answers":{"first":{"answers":["A"]},"unknown":{"answers":["B"]}}}"#,
        r#"{"answers":{"first":{"answers":["A"]},"second":{"answers":[" "]}}}"#,
    ] {
        assert_eq!(parse_response(&questions, &successful_run(stdout)), None);
    }
}

#[test]
fn rejects_failed_command_even_when_stdout_contains_answers() {
    let mut run_result = successful_run(r#"{"answers":{"first":{"answers":["secret"]}}}"#);
    run_result.exit_code = Some(1);

    assert_eq!(parse_response(&[question("first")], &run_result), None);
}

#[tokio::test]
async fn runs_handlers_in_order_and_stops_after_first_complete_answer() {
    let temp = tempdir().expect("create temp dir");
    let order_path = temp.path().join("order.txt");
    let commands = [
        hook_command(temp.path(), &order_path, "first", "{}"),
        hook_command(
            temp.path(),
            &order_path,
            "second",
            r#"{"answers":{"first":{"answers":["A"]}}}"#,
        ),
        hook_command(temp.path(), &order_path, "third", "{}"),
    ];
    let (result_sender, _result_receiver) = async_channel::unbounded();
    let runtime = CommandHookRuntime::new(
        CommandShell {
            program: String::new(),
            args: Vec::new(),
        },
        Arc::new(std::env::vars_os().collect()),
        ThreadId::new(),
        result_sender,
    );
    let mut engine = ClaudeHooksEngine::new(
        /*enabled*/ true,
        /*bypass_hook_trust*/ false,
        /*config_layer_stack*/ None,
        Vec::new(),
        Vec::new(),
        runtime,
        Arc::new(NoopMcpExecutor),
    );
    engine.handlers = commands
        .into_iter()
        .enumerate()
        .map(|(display_order, command)| ConfiguredHandler {
            builtin: false,
            event_name: HookEventName::UserInputRequest,
            matcher: None,
            timeout_sec: 10,
            status_message: None,
            additional_context_limit: Default::default(),
            source_path: AbsolutePathBuf::try_from(temp.path().join("hooks.json"))
                .expect("absolute hooks path")
                .into(),
            source: HookSource::User,
            display_order: display_order as i64,
            kind: ConfiguredHandlerKind::Command {
                command,
                env: HashMap::new(),
                r#async: false,
            },
        })
        .collect();

    let outcome = run(
        &engine,
        UserInputRequestRequest {
            session_id: ThreadId::new(),
            turn_id: "turn-1".to_string(),
            cwd: temp.path().to_path_buf(),
            transcript_path: None,
            call_id: "call-1".to_string(),
            questions: vec![question("first")],
            is_blocking: true,
        },
    )
    .await;

    assert_eq!(
        outcome.response,
        Some(RequestUserInputResponse {
            answers: [(
                "first".to_string(),
                RequestUserInputAnswer {
                    answers: vec!["A".to_string()],
                },
            )]
            .into_iter()
            .collect(),
        })
    );
    assert_eq!(outcome.hook_events.len(), 2);
    assert_eq!(
        fs::read_to_string(order_path).expect("read hook order"),
        "first\nsecond\n"
    );
}

fn hook_command(
    directory: &std::path::Path,
    order_path: &std::path::Path,
    name: &str,
    output: &str,
) -> String {
    #[cfg(unix)]
    {
        let script_path = directory.join(format!("{name}.sh"));
        fs::write(
            &script_path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{name}' >> '{}'\nprintf '%s\\n' '{}'\n",
                order_path.display(),
                output
            ),
        )
        .expect("write hook script");
        format!("sh '{}'", script_path.display())
    }
    #[cfg(windows)]
    {
        let script_path = directory.join(format!("{name}.cmd"));
        fs::write(
            &script_path,
            format!(
                "@echo off\r\necho {name}>>\"{}\"\r\necho {output}\r\n",
                order_path.display()
            ),
        )
        .expect("write hook script");
        format!("\"{}\"", script_path.display())
    }
}

struct NoopMcpExecutor;

impl HookMcpExecutor for NoopMcpExecutor {
    fn execute(&self, _call: HookMcpCall) -> BoxFuture<'_, anyhow::Result<String>> {
        async { anyhow::bail!("MCP hooks are not used in this test") }.boxed()
    }
}

fn question(id: &str) -> RequestUserInputQuestion {
    RequestUserInputQuestion {
        id: id.to_string(),
        header: "Header".to_string(),
        question: "Question?".to_string(),
        is_other: true,
        is_secret: false,
        options: None,
    }
}

fn successful_run(stdout: &str) -> HandlerRunResult {
    HandlerRunResult {
        started_at: 1,
        completed_at: 2,
        duration_ms: 1,
        exit_code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
        error: None,
    }
}
