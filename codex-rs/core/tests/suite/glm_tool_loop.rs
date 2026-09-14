use anyhow::Result;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn third_party_model_receives_recovery_error_on_third_identical_tool_call() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let arguments = json!({"cmd": "echo loop"}).to_string();
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            tool_call_response("response-1", "call-1", &arguments),
            tool_call_response("response-2", "call-2", &arguments),
            tool_call_response("response-3", "call-3", &arguments),
            sse(vec![
                ev_response_created("response-4"),
                ev_assistant_message("message-4", "recovered"),
                ev_completed("response-4"),
            ]),
        ],
    )
    .await;
    let test = test_codex()
        .with_model("third-party-model")
        .build_with_auto_env(&server)
        .await?;

    test.submit_turn("exercise repeated tool-call recovery")
        .await?;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 4);
    for (request_index, call_id) in [(1, "call-1"), (2, "call-2")] {
        let output = requests[request_index]
            .function_call_output(call_id)
            .to_string();
        assert!(!output.contains("Repeated identical tool call blocked"));
    }
    let (content, _) = requests[3]
        .function_call_output_content_and_success("call-3")
        .expect("third call should receive a loop-recovery response");
    assert!(
        content
            .as_deref()
            .is_some_and(|content| content.contains("Repeated identical tool call blocked"))
    );

    Ok(())
}

fn tool_call_response(response_id: &str, call_id: &str, arguments: &str) -> String {
    sse(vec![
        ev_response_created(response_id),
        ev_function_call(call_id, "exec_command", arguments),
        ev_completed(response_id),
    ])
}
