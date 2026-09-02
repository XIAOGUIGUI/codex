use pretty_assertions::assert_eq;

use super::*;

fn format_bytes(bytes: &[u8], offset: usize, limit: usize) -> Result<String, ReadResultError> {
    let (encoding, bytes) = LineEncoding::detect(bytes);
    let mut formatter = StreamingReadFormatter::new(offset, limit, encoding);
    if !formatter.feed(bytes).expect("feed bytes") {
        formatter.finish().expect("finish stream");
    }
    formatter.into_json("file:///C:/repo/source.txt".to_string())
}

#[test]
fn next_offset_continues_after_a_line_larger_than_eight_mib() {
    let mut bytes = vec![b'x'; 8 * 1024 * 1024 + 1];
    bytes.extend_from_slice(b"\nsecond");

    let first: serde_json::Value = serde_json::from_str(
        &format_bytes(&bytes, 1, DEFAULT_READ_LINES).expect("format first page"),
    )
    .expect("valid JSON");
    assert_eq!(first["end_line"], 1);
    assert_eq!(first["next_offset"], 2);
    assert!(
        first["content"]
            .as_str()
            .is_some_and(|content| content.len() <= MAX_RESULT_BYTES)
    );

    let second: serde_json::Value =
        serde_json::from_str(&format_bytes(&bytes, 2, 1).expect("format second page"))
            .expect("valid JSON");
    assert_eq!(second["content"], "2: second\n");
    assert_eq!(second["next_offset"], serde_json::Value::Null);
}

#[test]
fn a_single_truncated_line_does_not_claim_there_is_a_next_offset() {
    let bytes = vec![b'x'; MAX_CAPTURED_LINE_BYTES + 1];

    let output: serde_json::Value =
        serde_json::from_str(&format_bytes(&bytes, 1, 1).expect("format truncated line"))
            .expect("valid JSON");

    assert_eq!(output["end_line"], 1);
    assert_eq!(output["truncated"], true);
    assert_eq!(output["next_offset"], serde_json::Value::Null);
}

#[test]
fn offset_past_end_reports_requested_and_actual_line_counts() {
    let Err(ReadResultError::OffsetPastEnd { line_count }) = format_bytes(b"one\ntwo\n", 3, 1)
    else {
        panic!("offset past end should fail");
    };
    assert_eq!(line_count, 2);
}

#[test]
fn empty_file_at_first_offset_returns_an_empty_result() {
    let output: serde_json::Value =
        serde_json::from_str(&format_bytes(b"", 1, 1).expect("format empty file"))
            .expect("valid JSON");
    assert_eq!(
        output,
        serde_json::json!({
            "path": "file:///C:/repo/source.txt",
            "start_line": 1,
            "end_line": 0,
            "content": "",
            "truncated": false,
            "next_offset": null,
        })
    );
}

#[test]
fn read_result_pages_by_line() {
    let output: serde_json::Value =
        serde_json::from_str(&format_bytes(b"one\ntwo\nthree\n", 2, 1).expect("format page"))
            .expect("valid JSON");
    assert_eq!(
        output,
        serde_json::json!({
            "path": "file:///C:/repo/source.txt",
            "start_line": 2,
            "end_line": 2,
            "content": "2: two\n",
            "truncated": true,
            "next_offset": 3,
        })
    );
}

#[test]
fn decoder_handles_split_utf16_units_and_rejects_binary_data() {
    let utf16 = [0xff, 0xfe, b'h', 0, b'i', 0, b'\n', 0];
    let (encoding, bytes) = LineEncoding::detect(&utf16);
    let mut formatter = StreamingReadFormatter::new(1, 1, encoding);
    for byte in bytes {
        formatter.feed(&[*byte]).expect("feed UTF-16 byte");
    }
    formatter.finish().expect("finish UTF-16 stream");
    let output: serde_json::Value = serde_json::from_str(
        &formatter
            .into_json("file:///C:/repo/utf16.txt".to_string())
            .expect("format UTF-16 result"),
    )
    .expect("valid JSON");
    assert_eq!(output["content"], "1: hi\n");

    let (encoding, bytes) = LineEncoding::detect(&[0, 1, 2, 3]);
    let mut formatter = StreamingReadFormatter::new(1, 1, encoding);
    assert!(formatter.feed(bytes).is_err());
}

#[test]
fn completed_page_stops_after_confirming_the_next_line() {
    let (encoding, bytes) = LineEncoding::detect(b"one\ntwo\nthree\n");
    let mut formatter = StreamingReadFormatter::new(1, 1, encoding);

    assert!(formatter.feed(bytes).expect("feed page"));
    assert_eq!(formatter.line_number, 2);
    assert!(formatter.has_more_lines);
}
