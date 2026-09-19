use codex_tools::ToolName;

use super::ToolCallLoopDecision;
use super::ToolCallLoopDetector;
use super::ToolCallLoopPolicy;
use crate::tools::context::ToolPayload;
use crate::tools::router::ToolCall;

fn function_call(name: ToolName, arguments: &str) -> ToolCall {
    ToolCall {
        tool_name: name,
        call_id: "ignored-call-id".to_string(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
        encrypted_function_args: None,
    }
}

#[test]
fn protected_provider_rejects_third_identical_call_and_aborts_fourth() {
    let detector = ToolCallLoopDetector::new(ToolCallLoopPolicy::ProtectToolCalls);
    let call = function_call(ToolName::plain("exec_command"), r#"{"cmd":"echo loop"}"#);

    assert_eq!(detector.observe(&call), ToolCallLoopDecision::Allow);
    assert_eq!(detector.observe(&call), ToolCallLoopDecision::Allow);
    assert_eq!(
        detector.observe(&call),
        ToolCallLoopDecision::Reject { attempt: 3 }
    );
    assert_eq!(
        detector.observe(&call),
        ToolCallLoopDecision::Abort { attempt: 4 }
    );
}

#[test]
fn canonical_json_and_default_namespace_share_a_fingerprint() {
    let detector = ToolCallLoopDetector::new(ToolCallLoopPolicy::ProtectToolCalls);
    let first = function_call(ToolName::plain("tool"), r#"{"b":2,"a":1}"#);
    let second = function_call(
        ToolName::namespaced("functions", "tool"),
        r#"{ "a": 1, "b": 2 }"#,
    );

    assert_eq!(detector.observe(&first), ToolCallLoopDecision::Allow);
    assert_eq!(detector.observe(&second), ToolCallLoopDecision::Allow);
    assert_eq!(
        detector.observe(&first),
        ToolCallLoopDecision::Reject { attempt: 3 }
    );
}

#[test]
fn changed_arguments_reset_the_sequence() {
    let detector = ToolCallLoopDetector::new(ToolCallLoopPolicy::ProtectToolCalls);
    let first = function_call(ToolName::plain("exec_command"), r#"{"cmd":"one"}"#);
    let changed = function_call(ToolName::plain("exec_command"), r#"{"cmd":"two"}"#);

    assert_eq!(detector.observe(&first), ToolCallLoopDecision::Allow);
    assert_eq!(detector.observe(&first), ToolCallLoopDecision::Allow);
    assert_eq!(detector.observe(&changed), ToolCallLoopDecision::Allow);
    assert_eq!(detector.observe(&first), ToolCallLoopDecision::Allow);
}

#[test]
fn polling_tools_are_exempt() {
    let detector = ToolCallLoopDetector::new(ToolCallLoopPolicy::ProtectToolCalls);
    let calls = [
        function_call(ToolName::plain("wait"), r#"{"cell_id":"cell"}"#),
        function_call(ToolName::plain("write_stdin"), r#"{"session_id":1}"#),
        function_call(
            ToolName::namespaced("collaboration", "wait_agent"),
            r#"{"timeout_ms":30000}"#,
        ),
        function_call(
            ToolName::namespaced("multi_agent_v1", "wait_agent"),
            r#"{"targets":["agent"],"timeout_ms":30000}"#,
        ),
    ];

    for call in calls {
        for _ in 0..5 {
            assert_eq!(detector.observe(&call), ToolCallLoopDecision::Allow);
        }
    }
}

#[test]
fn disabled_policy_does_not_reject_repeated_calls() {
    let detector = ToolCallLoopDetector::new(ToolCallLoopPolicy::Disabled);
    let call = function_call(ToolName::plain("exec_command"), r#"{"cmd":"echo"}"#);

    for _ in 0..5 {
        assert_eq!(detector.observe(&call), ToolCallLoopDecision::Allow);
    }
}
