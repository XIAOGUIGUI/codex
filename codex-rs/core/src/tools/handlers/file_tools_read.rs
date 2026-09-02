use std::time::Duration;

use codex_exec_server::GetMetadataOptions;
use codex_protocol::exec_output::bytes_to_string_smart;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::function_tool::FunctionCallError;

pub(super) const DEFAULT_READ_LINES: usize = 400;
const MAX_READ_LINES: usize = 1_000;
pub(super) const MAX_RESULT_BYTES: usize = 32 * 1024;
const MAX_CAPTURED_LINE_BYTES: usize = MAX_RESULT_BYTES * 2;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
pub(super) struct ReadFileArgs {
    file_path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct ReadFileResult {
    path: String,
    start_line: usize,
    end_line: usize,
    content: String,
    truncated: bool,
    next_offset: Option<usize>,
}

pub(super) async fn read_file(
    environment: &crate::session::turn_context::TurnEnvironment,
    args: ReadFileArgs,
    cancellation_token: CancellationToken,
) -> Result<String, FunctionCallError> {
    let offset = args.offset.unwrap_or(1);
    if offset == 0 {
        return Err(FunctionCallError::RespondToModel(
            "read_file.offset must be at least 1".to_string(),
        ));
    }
    let limit = args.limit.unwrap_or(DEFAULT_READ_LINES);
    if limit == 0 || limit > MAX_READ_LINES {
        return Err(FunctionCallError::RespondToModel(format!(
            "read_file.limit must be between 1 and {MAX_READ_LINES}"
        )));
    }
    let path = environment.cwd().join(&args.file_path).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to resolve file path `{}`: {err}",
            args.file_path
        ))
    })?;
    let sandbox = environment.sandbox_context(/*additional_permissions*/ None);
    let fs = environment.environment.get_filesystem();
    let path_for_read = path.clone();
    let formatter = tokio::time::timeout(READ_TIMEOUT, async {
        let metadata = fs
            .get_metadata(
                &path_for_read,
                GetMetadataOptions::default(),
                Some(&sandbox),
            )
            .await?;
        if !metadata.is_file {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path is not a file",
            ));
        }
        let mut stream = fs.read_file_stream(&path_for_read, Some(&sandbox)).await?;
        let mut initial = Vec::new();
        while initial.len() < 2 {
            let next = tokio::select! {
                _ = cancellation_token.cancelled() => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "read_file was cancelled"));
                }
                next = stream.next() => next,
            };
            let Some(chunk) = next else {
                break;
            };
            initial.extend_from_slice(&chunk?);
        }

        let (encoding, initial) = LineEncoding::detect(&initial);
        let mut formatter = StreamingReadFormatter::new(offset, limit, encoding);
        let mut done = formatter.feed(initial)?;
        while !done {
            let next = tokio::select! {
                _ = cancellation_token.cancelled() => {
                    return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "read_file was cancelled"));
                }
                next = stream.next() => next,
            };
            let Some(chunk) = next else {
                formatter.finish()?;
                break;
            };
            done = formatter.feed(&chunk?)?;
        }
        Ok::<_, std::io::Error>(formatter)
    })
    .await
    .map_err(|_| {
        FunctionCallError::RespondToModel(format!(
            "read_file timed out after {} seconds",
            READ_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|err| FunctionCallError::RespondToModel(format!("read_file failed: {err}")))?;

    formatter
        .into_json(path.to_string())
        .map_err(|err| match err {
            ReadResultError::OffsetPastEnd { line_count } => FunctionCallError::RespondToModel(
                format!("read_file.offset {offset} exceeds the file's {line_count} lines"),
            ),
            ReadResultError::Serialize(err) => {
                FunctionCallError::Fatal(format!("failed to serialize read_file output: {err}"))
            }
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LineEncoding {
    ByteOriented,
    Utf16Le,
    Utf16Be,
}

impl LineEncoding {
    fn detect(bytes: &[u8]) -> (Self, &[u8]) {
        if let Some(bytes) = bytes.strip_prefix(&[0xff, 0xfe]) {
            (Self::Utf16Le, bytes)
        } else if let Some(bytes) = bytes.strip_prefix(&[0xfe, 0xff]) {
            (Self::Utf16Be, bytes)
        } else {
            (Self::ByteOriented, bytes)
        }
    }

    fn bom(self) -> &'static [u8] {
        match self {
            Self::ByteOriented => &[],
            Self::Utf16Le => &[0xff, 0xfe],
            Self::Utf16Be => &[0xfe, 0xff],
        }
    }
}

struct StreamingReadFormatter {
    offset: usize,
    limit: usize,
    encoding: LineEncoding,
    line_number: usize,
    current_line: Vec<u8>,
    current_line_has_data: bool,
    current_line_truncated: bool,
    pending_utf16_byte: Option<u8>,
    sample_bytes: usize,
    content: String,
    returned_lines: usize,
    output_truncated: bool,
    has_more_lines: bool,
    reached_eof: bool,
    done: bool,
}

impl StreamingReadFormatter {
    fn new(offset: usize, limit: usize, encoding: LineEncoding) -> Self {
        Self {
            offset,
            limit,
            encoding,
            line_number: 1,
            current_line: Vec::new(),
            current_line_has_data: false,
            current_line_truncated: false,
            pending_utf16_byte: None,
            sample_bytes: 0,
            content: String::new(),
            returned_lines: 0,
            output_truncated: false,
            has_more_lines: false,
            reached_eof: false,
            done: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> std::io::Result<bool> {
        match self.encoding {
            LineEncoding::ByteOriented => self.feed_byte_oriented(bytes)?,
            LineEncoding::Utf16Le | LineEncoding::Utf16Be => self.feed_utf16(bytes)?,
        };
        Ok(self.done)
    }

    fn feed_byte_oriented(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        for &byte in bytes {
            if self.waiting_to_confirm_more_lines() {
                self.has_more_lines = true;
                self.done = true;
                break;
            }
            if self.sample_bytes < 8 * 1024 {
                self.sample_bytes += 1;
                if byte == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "read_file only supports text files; the selected file appears to be binary",
                    ));
                }
            }
            if byte == b'\n' {
                self.finish_line()?;
                if self.done {
                    break;
                }
            } else {
                self.current_line_has_data = true;
                self.capture_bytes(&[byte]);
            }
        }
        Ok(())
    }

    fn feed_utf16(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let mut index = 0;
        if let Some(first) = self.pending_utf16_byte.take() {
            if let Some(&second) = bytes.first() {
                self.feed_utf16_unit([first, second])?;
                index = 1;
                if self.done {
                    return Ok(());
                }
            } else {
                self.pending_utf16_byte = Some(first);
                return Ok(());
            }
        }
        while index + 1 < bytes.len() {
            self.feed_utf16_unit([bytes[index], bytes[index + 1]])?;
            index += 2;
            if self.done {
                return Ok(());
            }
        }
        if index < bytes.len() {
            self.pending_utf16_byte = Some(bytes[index]);
        }
        Ok(())
    }

    fn feed_utf16_unit(&mut self, bytes: [u8; 2]) -> std::io::Result<()> {
        if self.waiting_to_confirm_more_lines() {
            self.has_more_lines = true;
            self.done = true;
            return Ok(());
        }
        let unit = match self.encoding {
            LineEncoding::Utf16Le => u16::from_le_bytes(bytes),
            LineEncoding::Utf16Be => u16::from_be_bytes(bytes),
            LineEncoding::ByteOriented => unreachable!("UTF-16 parser requires UTF-16 encoding"),
        };
        if unit == b'\n' as u16 {
            self.finish_line()
        } else {
            self.current_line_has_data = true;
            self.capture_bytes(&bytes);
            Ok(())
        }
    }

    fn capture_bytes(&mut self, bytes: &[u8]) {
        if !self.should_capture_current_line() {
            return;
        }
        let remaining = MAX_CAPTURED_LINE_BYTES.saturating_sub(self.current_line.len());
        self.current_line
            .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        self.current_line_truncated |= bytes.len() > remaining;
    }

    fn should_capture_current_line(&self) -> bool {
        self.line_number >= self.offset
            && self.returned_lines < self.limit
            && !self.output_truncated
    }

    fn waiting_to_confirm_more_lines(&self) -> bool {
        self.returned_lines >= self.limit || self.output_truncated
    }

    fn finish_line(&mut self) -> std::io::Result<()> {
        if self.should_capture_current_line() {
            let raw = std::mem::take(&mut self.current_line);
            self.append_line(&raw, self.current_line_truncated)?;
        } else {
            self.current_line.clear();
        }
        self.current_line_has_data = false;
        self.current_line_truncated = false;
        self.line_number = self.line_number.saturating_add(1);
        Ok(())
    }

    fn append_line(&mut self, raw: &[u8], raw_truncated: bool) -> std::io::Result<()> {
        let mut encoded = Vec::with_capacity(self.encoding.bom().len() + raw.len());
        encoded.extend_from_slice(self.encoding.bom());
        encoded.extend_from_slice(raw);
        if raw_truncated && self.encoding == LineEncoding::ByteOriented {
            trim_incomplete_utf8_tail(&mut encoded);
        }
        let decoded = bytes_to_string_smart(&encoded);
        if decoded.starts_with("[Binary output:") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "read_file only supports text files; the selected file appears to be binary",
            ));
        }
        let decoded = decoded.strip_prefix('\u{feff}').unwrap_or(decoded.as_str());
        let decoded = decoded.strip_suffix('\r').unwrap_or(decoded);
        let prefix = format!("{}: ", self.line_number);
        let available = MAX_RESULT_BYTES.saturating_sub(self.content.len());
        if available <= prefix.len() {
            self.output_truncated = true;
            return Ok(());
        }
        self.content.push_str(&prefix);
        let line_budget = available - prefix.len();
        if raw_truncated || decoded.len().saturating_add(1) > line_budget {
            append_truncated_text(&mut self.content, decoded, line_budget);
            self.output_truncated = true;
        } else {
            self.content.push_str(decoded);
            self.content.push('\n');
        }
        self.returned_lines += 1;
        Ok(())
    }

    fn finish(&mut self) -> std::io::Result<()> {
        if self.pending_utf16_byte.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "UTF-16 file has an incomplete final code unit",
            ));
        }
        if self.current_line_has_data {
            self.finish_line()?;
        }
        self.reached_eof = true;
        Ok(())
    }

    fn into_json(self, path: String) -> Result<String, ReadResultError> {
        let line_count = self.line_number - 1;
        let empty_file_at_start = self.reached_eof && line_count == 0 && self.offset == 1;
        if self.reached_eof && self.offset > line_count && !empty_file_at_start {
            return Err(ReadResultError::OffsetPastEnd { line_count });
        }
        let end_line = self.offset - 1 + self.returned_lines;
        let has_more_lines = self.has_more_lines || end_line < line_count;
        let truncated = has_more_lines || self.output_truncated;
        let next_offset = has_more_lines.then_some(end_line.saturating_add(1));
        serde_json::to_string_pretty(&ReadFileResult {
            path,
            start_line: self.offset,
            end_line,
            content: self.content,
            truncated,
            next_offset,
        })
        .map_err(ReadResultError::Serialize)
    }
}

#[derive(Debug)]
enum ReadResultError {
    OffsetPastEnd { line_count: usize },
    Serialize(serde_json::Error),
}

pub(super) fn append_truncated_text(output: &mut String, text: &str, max_bytes: usize) {
    let marker = "…\n";
    if max_bytes >= marker.len() {
        output.push_str(truncate_utf8(text, max_bytes - marker.len()));
        output.push_str(marker);
    } else {
        output.push_str(truncate_utf8(text, max_bytes));
    }
}

pub(super) fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    &text[..end]
}

fn trim_incomplete_utf8_tail(bytes: &mut Vec<u8>) {
    if let Err(error) = std::str::from_utf8(bytes)
        && error.error_len().is_none()
    {
        bytes.truncate(error.valid_up_to());
    }
}

#[cfg(test)]
#[path = "file_tools_read_tests.rs"]
mod tests;
