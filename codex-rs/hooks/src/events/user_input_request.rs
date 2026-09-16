//! Blocking external user-input hook execution.
//!
//! Handlers run sequentially so only one external prompt is visible at a time.
//! The first complete response wins; failures and unhandled responses fall
//! through to the next handler and ultimately to Codex's native prompt.

use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::request_user_input::RequestUserInputQuestion;
use codex_protocol::request_user_input::RequestUserInputResponse;

use crate::engine::ClaudeHooksEngine;
use crate::engine::ConfiguredHandlerKind;
use crate::engine::HandlerRunResult;
use crate::engine::command_runner::run_command_bounded;
use crate::engine::dispatcher;
use crate::schema::NullableString;
use crate::schema::UserInputRequestCommandInput;
use crate::schema::UserInputRequestCommandOutputWire;

const MAX_OUTPUT_BYTES_PER_STREAM: usize = 64 * 1024;

#[derive(Clone)]
pub struct UserInputRequestRequest {
    pub session_id: ThreadId,
    pub turn_id: String,
    pub cwd: PathBuf,
    pub transcript_path: Option<PathBuf>,
    pub call_id: String,
    pub questions: Vec<RequestUserInputQuestion>,
    pub is_blocking: bool,
}

pub struct UserInputRequestOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
    pub response: Option<RequestUserInputResponse>,
}

pub(crate) async fn run(
    engine: &ClaudeHooksEngine,
    request: UserInputRequestRequest,
) -> UserInputRequestOutcome {
    let mut handlers = dispatcher::select_handlers(
        &engine.handlers,
        HookEventName::UserInputRequest,
        /*matcher_input*/ None,
    );
    handlers.sort_by_key(|handler| handler.display_order);
    if handlers.is_empty() {
        return UserInputRequestOutcome {
            hook_events: Vec::new(),
            response: None,
        };
    }

    let input = UserInputRequestCommandInput {
        session_id: request.session_id.to_string(),
        turn_id: request.turn_id.clone(),
        cwd: request.cwd.display().to_string(),
        transcript_path: NullableString::from_path(request.transcript_path),
        hook_event_name: "UserInputRequest".to_string(),
        tool_name: "request_user_input".to_string(),
        call_id: request.call_id,
        questions: request.questions.clone(),
        is_blocking: request.is_blocking,
    };
    let input_json = match serde_json::to_string(&input) {
        Ok(input_json) => input_json,
        Err(_) => {
            return UserInputRequestOutcome {
                hook_events: Vec::new(),
                response: None,
            };
        }
    };

    let mut hook_events = Vec::new();
    for handler in handlers {
        let ConfiguredHandlerKind::Command { command, env, .. } = &handler.kind else {
            continue;
        };
        let run_result = run_command_bounded(
            &engine.command_runtime,
            &handler,
            command,
            env,
            &input_json,
            request.cwd.as_path(),
            MAX_OUTPUT_BYTES_PER_STREAM,
        )
        .await;
        let response = parse_response(&request.questions, &run_result);
        let handled = response.is_some();
        hook_events.push(completed_event(
            &handler,
            &request.turn_id,
            &run_result,
            handled,
        ));
        if handled {
            return UserInputRequestOutcome {
                hook_events,
                response,
            };
        }
    }

    UserInputRequestOutcome {
        hook_events,
        response: None,
    }
}

fn parse_response(
    questions: &[RequestUserInputQuestion],
    run_result: &HandlerRunResult,
) -> Option<RequestUserInputResponse> {
    if run_result.error.is_some() || run_result.exit_code != Some(0) {
        return None;
    }
    let stdout = run_result.stdout.trim();
    if is_unhandled_stdout(stdout) {
        return None;
    }
    let wire: UserInputRequestCommandOutputWire = serde_json::from_str(stdout).ok()?;
    if wire.answers.len() != questions.len() || questions.is_empty() {
        return None;
    }
    for question in questions {
        let answer = wire.answers.get(&question.id)?;
        if answer.answers.is_empty() || answer.answers.iter().any(|value| value.trim().is_empty()) {
            return None;
        }
    }
    Some(RequestUserInputResponse {
        answers: wire.answers,
    })
}

fn completed_event(
    handler: &crate::engine::ConfiguredHandler,
    turn_id: &str,
    run_result: &HandlerRunResult,
    handled: bool,
) -> HookCompletedEvent {
    let failed = run_result.error.is_some() || run_result.exit_code != Some(0);
    let (status, entries) = if failed {
        (
            HookRunStatus::Failed,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "user input hook execution failed".to_string(),
            }],
        )
    } else if handled || is_unhandled_stdout(&run_result.stdout) {
        (HookRunStatus::Completed, Vec::new())
    } else {
        (
            HookRunStatus::Failed,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "user input hook returned an invalid response".to_string(),
            }],
        )
    };
    HookCompletedEvent {
        turn_id: Some(turn_id.to_string()),
        run: dispatcher::completed_summary(handler, run_result, status, entries),
    }
}

fn is_unhandled_stdout(stdout: &str) -> bool {
    let stdout = stdout.trim();
    stdout.is_empty()
        || matches!(
            serde_json::from_str::<serde_json::Value>(stdout),
            Ok(serde_json::Value::Object(object)) if object.is_empty()
        )
}

#[cfg(test)]
#[path = "user_input_request_tests.rs"]
mod tests;
