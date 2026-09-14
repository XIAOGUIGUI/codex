use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TruncationPolicy;
use pretty_assertions::assert_eq;

use super::*;

fn args(offset: usize, limit: usize) -> ReadFileArgs {
    ReadFileArgs {
        file_path: "src/main.rs".to_string(),
        offset: Some(offset),
        limit: Some(limit),
    }
}

fn result(start_line: usize, end_line: usize, content: &str) -> ReadFileResult {
    ReadFileResult {
        path: "file:///C:/repo/src/main.rs".to_string(),
        start_line,
        end_line,
        content: content.to_string(),
        truncated: true,
        next_offset: Some(end_line + 1),
        read_optimization: None,
    }
}

fn read_items(call_id: &str, args: ReadFileArgs, result: ReadFileResult) -> [ResponseItem; 2] {
    [
        ResponseItem::FunctionCall {
            id: None,
            name: "read_file".to_string(),
            namespace: None,
            arguments: serde_json::to_string(&serde_json::json!({
                "file_path": args.file_path,
                "offset": args.offset,
                "limit": args.limit,
            }))
            .expect("serialize arguments"),
            encrypted_function_args: None,
            call_id: call_id.to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(call_id.to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text(
                serde_json::to_string_pretty(&result).expect("serialize result"),
            ),
            internal_chat_message_metadata_passthrough: None,
        },
    ]
}

fn history(reads: Vec<(ReadFileArgs, ReadFileResult)>) -> ReadHistory {
    let items = reads
        .into_iter()
        .enumerate()
        .flat_map(|(index, (args, result))| read_items(&format!("call-{index}"), args, result))
        .collect::<Vec<_>>();
    let mut history = ContextManager::new();
    history.record_items(&items, TruncationPolicy::Bytes(128 * 1024));
    ReadHistory::collect(&history)
}

#[test]
fn third_adjacent_small_read_expands() {
    let history = history(vec![
        (args(1, 20), result(1, 20, "1: first\n")),
        (args(21, 20), result(21, 40, "21: second\n")),
    ]);

    assert_eq!(history.effective_limit(&args(41, 20)), ADAPTIVE_READ_LINES);
}

#[test]
fn precise_or_non_adjacent_reads_stay_requested_size() {
    let one_read = history(vec![(args(1, 20), result(1, 20, "1: first\n"))]);
    assert_eq!(one_read.effective_limit(&args(21, 20)), 20);

    let non_adjacent = history(vec![
        (args(1, 20), result(1, 20, "1: first\n")),
        (args(40, 20), result(40, 59, "40: second\n")),
    ]);
    assert_eq!(non_adjacent.effective_limit(&args(60, 20)), 20);
    assert_eq!(non_adjacent.effective_limit(&args(60, 100)), 100);
}

#[test]
fn read_after_an_adaptive_page_remains_adaptive() {
    let mut adaptive = result(41, 240, "41: expanded\n");
    adaptive.read_optimization = Some(ReadOptimization {
        requested_limit: 20,
        effective_limit: ADAPTIVE_READ_LINES,
        reason: "expanded_after_consecutive_small_reads".to_string(),
    });
    let history = history(vec![(args(41, 20), adaptive)]);

    assert_eq!(history.effective_limit(&args(241, 20)), ADAPTIVE_READ_LINES);
}

#[test]
fn unchanged_covered_range_returns_a_small_reference() {
    let history = history(vec![(
        args(41, 200),
        result(41, 80, "41: one\n42: two\n43: three\n"),
    )]);
    let output = history.optimize_result(20, result(42, 43, "42: two\n43: three\n"));
    let json: serde_json::Value =
        serde_json::from_str(&output.to_json().expect("serialize output")).expect("valid JSON");

    assert_eq!(
        json,
        serde_json::json!({
            "status": "file_unchanged",
            "requested_start_line": 42,
            "requested_end_line": 43,
            "covered_start_line": 41,
            "covered_end_line": 80,
            "next_offset": 81,
        })
    );
    assert!(output.to_json().expect("serialize output").len() <= 512);
}

#[test]
fn changed_covered_range_returns_fresh_content() {
    let history = history(vec![(args(41, 20), result(41, 60, "42: old\n"))]);
    let fresh = result(42, 42, "42: new\n");

    assert!(matches!(
        history.optimize_result(1, fresh),
        ReadHistoryOutput::Full(_)
    ));
}
