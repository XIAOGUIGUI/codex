use std::collections::HashMap;

use codex_protocol::models::ResponseItem;
use serde::Serialize;

use crate::context_manager::ContextManager;

use super::file_tools_read::ReadFileArgs;
use super::file_tools_read::ReadFileResult;
use super::file_tools_read::ReadOptimization;

const SMALL_READ_MAX_LINES: usize = 50;
const ADAPTIVE_READ_LINES: usize = 200;

#[derive(Clone, Debug)]
struct HistoricalRead {
    args: ReadFileArgs,
    result: ReadFileResult,
}

pub(super) struct ReadHistory {
    reads: Vec<HistoricalRead>,
    current_turn_start: usize,
}

impl ReadHistory {
    pub(super) fn collect(history: &ContextManager) -> Self {
        let mut pending = HashMap::<String, ReadFileArgs>::new();
        let mut reads = Vec::new();
        let mut current_turn_start = 0;

        for item in history.raw_items() {
            match item {
                ResponseItem::Message { role, .. } if role == "user" => {
                    current_turn_start = reads.len();
                }
                ResponseItem::FunctionCall {
                    name,
                    arguments,
                    call_id,
                    ..
                } if name == "read_file" => {
                    if let Ok(args) = serde_json::from_str(arguments) {
                        pending.insert(call_id.clone(), args);
                    }
                }
                ResponseItem::FunctionCallOutput {
                    call_id: Some(call_id),
                    output,
                    ..
                } => {
                    let Some(args) = pending.remove(call_id) else {
                        continue;
                    };
                    let Some(text) = output.text_content() else {
                        continue;
                    };
                    if let Ok(result) = serde_json::from_str(text) {
                        reads.push(HistoricalRead { args, result });
                    }
                }
                _ => {}
            }
        }

        Self {
            reads,
            current_turn_start,
        }
    }

    pub(super) fn effective_limit(&self, args: &ReadFileArgs) -> usize {
        let requested_limit = args.limit();
        if requested_limit > SMALL_READ_MAX_LINES {
            return requested_limit;
        }

        let turn_reads = &self.reads[self.current_turn_start..];
        let Some(last) = turn_reads.last() else {
            return requested_limit;
        };
        if last.args.file_path != args.file_path
            || last.result.end_line.saturating_add(1) != args.offset()
        {
            return requested_limit;
        }
        if last.result.read_optimization.is_some() {
            return ADAPTIVE_READ_LINES;
        }

        let Some(previous) = turn_reads.get(turn_reads.len().saturating_sub(2)) else {
            return requested_limit;
        };
        if previous.args.file_path == args.file_path
            && previous.args.limit() <= SMALL_READ_MAX_LINES
            && last.args.limit() <= SMALL_READ_MAX_LINES
            && previous.result.end_line.saturating_add(1) == last.args.offset()
        {
            ADAPTIVE_READ_LINES
        } else {
            requested_limit
        }
    }

    pub(super) fn optimize_result(
        &self,
        requested_limit: usize,
        mut result: ReadFileResult,
    ) -> ReadHistoryOutput {
        if let Some(covering) = self.reads.iter().rev().find(|read| {
            read.result.path == result.path
                && read.result.start_line <= result.start_line
                && read.result.end_line >= result.end_line
                && !result.content.is_empty()
                && read.result.content.contains(&result.content)
        }) {
            return ReadHistoryOutput::Unchanged(UnchangedReadResult {
                status: "file_unchanged",
                requested_start_line: result.start_line,
                requested_end_line: result.end_line,
                covered_start_line: covering.result.start_line,
                covered_end_line: covering.result.end_line,
                next_offset: covering.result.next_offset,
                saved_bytes: result.content.len(),
            });
        }

        let effective_limit = result
            .end_line
            .saturating_sub(result.start_line)
            .saturating_add(1);
        if requested_limit <= SMALL_READ_MAX_LINES && effective_limit > requested_limit {
            result.read_optimization = Some(ReadOptimization {
                requested_limit,
                effective_limit: ADAPTIVE_READ_LINES,
                reason: "expanded_after_consecutive_small_reads".to_string(),
            });
        }
        ReadHistoryOutput::Full(result)
    }
}

pub(super) enum ReadHistoryOutput {
    Full(ReadFileResult),
    Unchanged(UnchangedReadResult),
}

impl ReadHistoryOutput {
    pub(super) fn to_json(&self) -> Result<String, serde_json::Error> {
        match self {
            Self::Full(result) => serde_json::to_string_pretty(result),
            Self::Unchanged(result) => serde_json::to_string_pretty(result),
        }
    }

    pub(super) fn diagnostic(&self) -> Option<(&'static str, usize)> {
        match self {
            Self::Full(ReadFileResult {
                read_optimization: Some(_),
                content,
                ..
            }) => Some(("file.read.auto_expanded", content.len())),
            Self::Unchanged(result) => Some(("file.read.unchanged", result.saved_bytes)),
            Self::Full(_) => None,
        }
    }
}

#[derive(Serialize)]
pub(super) struct UnchangedReadResult {
    status: &'static str,
    requested_start_line: usize,
    requested_end_line: usize,
    covered_start_line: usize,
    covered_end_line: usize,
    next_offset: Option<usize>,
    #[serde(skip)]
    saved_bytes: usize,
}

#[cfg(test)]
#[path = "file_tools_read_history_tests.rs"]
mod tests;
