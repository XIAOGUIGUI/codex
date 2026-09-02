use pretty_assertions::assert_eq;

use super::*;

#[test]
fn search_result_applies_line_limit_and_sorts_globs() {
    let output = format_search_result(
        CapturedProcess {
            stdout: b"z.rs\na.rs\nm.rs\n".to_vec(),
            truncated: false,
        },
        2,
        /*sort*/ true,
    )
    .expect("format search result");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid json");

    assert_eq!(
        output,
        serde_json::json!({
            "output": "a.rs\nm.rs\n",
            "result_count": 2,
            "truncated": true,
        })
    );
}

#[test]
fn head_limit_is_bounded() {
    assert_eq!(
        validate_head_limit("grep_files", /*requested*/ None).expect("default limit"),
        DEFAULT_RESULT_LINES
    );
    assert!(validate_head_limit("grep_files", /*requested*/ Some(0)).is_err());
    assert!(validate_head_limit("grep_files", /*requested*/ Some(MAX_RESULT_LINES + 1),).is_err());
}

#[test]
fn capture_truncation_discards_partial_utf8_lines() {
    let mut bytes = "complete\n中文".as_bytes().to_vec();
    bytes.pop();

    truncate_to_complete_line(&mut bytes);

    assert_eq!(bytes, b"complete\n");
    assert!(std::str::from_utf8(&bytes).is_ok());
}

#[test]
fn search_result_limits_long_unicode_lines_without_dropping_following_results() {
    let long_line = "界".repeat(MAX_SEARCH_LINE_BYTES);
    let output = format_search_result(
        CapturedProcess {
            stdout: format!("{long_line}\nnext.rs\n").into_bytes(),
            truncated: false,
        },
        DEFAULT_RESULT_LINES,
        /*sort*/ false,
    )
    .expect("format search result");
    let output: serde_json::Value = serde_json::from_str(&output).expect("valid JSON");
    let text = output["output"].as_str().expect("text output");

    assert_eq!(output["result_count"], 2);
    assert_eq!(output["truncated"], true);
    assert!(text.len() <= MAX_RESULT_BYTES);
    assert!(text.contains("…\nnext.rs\n"));
}
