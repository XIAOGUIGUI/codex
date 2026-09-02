use std::fs;

use anyhow::Result;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_windows_file_tools_read_search_and_list_without_powershell() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call(
                    "read-call",
                    "read_file",
                    &json!({"file_path": "source.rs", "offset": 2, "limit": 1}).to_string(),
                ),
                ev_function_call(
                    "grep-call",
                    "grep_files",
                    &json!({"pattern": "needle", "glob": "*.rs"}).to_string(),
                ),
                ev_function_call(
                    "glob-call",
                    "glob_files",
                    &json!({"pattern": "*.rs"}).to_string(),
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_assistant_message("message-1", "inspection complete"),
                ev_completed("resp-2"),
            ]),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    fs::write(test.workspace_path("source.rs"), "first\nneedle\nthird\n")?;

    test.submit_turn("inspect the source file").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let tools = requests[0].body_json()["tools"]
        .as_array()
        .expect("request should advertise tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for name in ["read_file", "grep_files", "glob_files"] {
        assert!(tools.contains(&name), "missing {name} tool");
    }
    let request = &requests[1];
    let read = function_output_json(request, "read-call");
    assert_eq!(read["content"], "2: needle\n");
    assert_eq!(read["next_offset"], 3);

    let grep = function_output_json(request, "grep-call");
    assert_eq!(grep["result_count"], 1);
    assert!(
        grep["output"]
            .as_str()
            .is_some_and(|output| output.contains("source.rs"))
    );

    let glob = function_output_json(request, "glob-call");
    assert_eq!(glob["result_count"], 1);
    assert!(
        glob["output"]
            .as_str()
            .is_some_and(|output| output.contains("source.rs"))
    );

    Ok(())
}

fn function_output_json(request: &responses::ResponsesRequest, call_id: &str) -> Value {
    let (content, success) = request
        .function_call_output_content_and_success(call_id)
        .expect("function output should be present");
    assert_eq!(success, Some(true));
    serde_json::from_str(
        content
            .as_deref()
            .expect("function output should contain text"),
    )
    .expect("function output should be JSON")
}
