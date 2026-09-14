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
    let request_body = requests[0].body_json();
    let tools = request_body["tools"]
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adjacent_small_reads_expand_and_overlapping_reads_reuse_history() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let page = |response_id: &str, call_id: &str, offset: usize| {
        sse(vec![
            ev_response_created(response_id),
            ev_function_call(
                call_id,
                "read_file",
                &json!({"file_path": "source.rs", "offset": offset, "limit": 20}).to_string(),
            ),
            ev_completed(response_id),
        ])
    };
    let responses = mount_sse_sequence(
        &server,
        vec![
            page("resp-1", "read-1", 1),
            page("resp-2", "read-2", 21),
            page("resp-3", "read-3", 41),
            page("resp-4", "read-overlap", 61),
            sse(vec![
                ev_assistant_message("message-1", "inspection complete"),
                ev_completed("resp-5"),
            ]),
        ],
    )
    .await;
    let test = test_codex().build_with_auto_env(&server).await?;
    let content = (1..=300)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(test.workspace_path("source.rs"), format!("{content}\n"))?;

    test.submit_turn("inspect the source file").await?;

    let requests = responses.requests();
    assert_eq!(requests.len(), 5);
    let third = function_output_json(&requests[3], "read-3");
    assert_eq!(third["start_line"], 41);
    assert_eq!(third["end_line"], 240);
    assert_eq!(third["next_offset"], 241);
    assert_eq!(
        third["read_optimization"],
        json!({
            "requested_limit": 20,
            "effective_limit": 200,
            "reason": "expanded_after_consecutive_small_reads",
        })
    );

    let overlap = function_output_json(&requests[4], "read-overlap");
    assert_eq!(
        overlap,
        json!({
            "status": "file_unchanged",
            "requested_start_line": 61,
            "requested_end_line": 80,
            "covered_start_line": 41,
            "covered_end_line": 240,
            "next_offset": 241,
        })
    );

    Ok(())
}

fn function_output_json(request: &responses::ResponsesRequest, call_id: &str) -> Value {
    let (content, success) = request
        .function_call_output_content_and_success(call_id)
        .expect("function output should be present");
    assert_ne!(success, Some(false));
    serde_json::from_str(
        content
            .as_deref()
            .expect("function output should contain text"),
    )
    .expect("function output should be JSON")
}
