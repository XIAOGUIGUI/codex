//! Adapter between core tool dispatch objects and rollout-trace events.
//!
//! `codex-rollout-trace` owns the event schema and writer behavior. This module
//! keeps the core-specific mapping from registry invocations/results out of the
//! registry control flow.

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use codex_diagnostics::CompatibilityDiagnostics;
use codex_diagnostics::CompatibilityEventInput;
use codex_diagnostics::CompatibilityOutcome;
use codex_diagnostics::ToolRepresentation;
use codex_rollout_trace::ExecutionStatus;
use codex_rollout_trace::ToolDispatchInvocation;
use codex_rollout_trace::ToolDispatchPayload;
use codex_rollout_trace::ToolDispatchRequester;
use codex_rollout_trace::ToolDispatchResult;
use codex_rollout_trace::ToolDispatchTraceContext;
use std::time::Instant;

/// Keeps registry early-return paths paired with trace end events.
pub(crate) struct ToolDispatchTrace {
    context: ToolDispatchTraceContext,
    compatibility: Option<CompatibilityToolTrace>,
}

struct CompatibilityToolTrace {
    diagnostics: CompatibilityDiagnostics,
    started: Instant,
    tool_name: String,
    tool_namespace: Option<String>,
    representation: ToolRepresentation,
    input_bytes: usize,
}

impl ToolDispatchTrace {
    pub(crate) fn start(invocation: &ToolInvocation) -> Self {
        let context = invocation
            .session
            .services
            .rollout_thread_trace
            .start_tool_dispatch_trace(|| tool_dispatch_invocation(invocation));
        let diagnostics = invocation
            .session
            .services
            .compatibility_diagnostics
            .clone();
        let compatibility = diagnostics.is_enabled().then(|| CompatibilityToolTrace {
            diagnostics,
            started: Instant::now(),
            tool_name: invocation.tool_name.name.clone(),
            tool_namespace: invocation.tool_name.namespace.clone(),
            representation: tool_representation(&invocation.source, &invocation.payload),
            input_bytes: tool_payload_bytes(&invocation.payload),
        });
        Self {
            context,
            compatibility,
        }
    }

    pub(crate) fn record_completed(
        &self,
        invocation: &ToolInvocation,
        call_id: &str,
        payload: &ToolPayload,
        result: &dyn ToolOutput,
    ) {
        if self.compatibility.is_none() && !self.context.is_enabled() {
            return;
        }
        let success = result.success_for_logging();
        if let Some(compatibility) = &self.compatibility {
            let output = result.log_output();
            compatibility.diagnostics.record(CompatibilityEventInput {
                phase: "tool.dispatch",
                outcome: if success {
                    CompatibilityOutcome::Success
                } else {
                    CompatibilityOutcome::Failure
                },
                tool_name: Some(&compatibility.tool_name),
                tool_namespace: compatibility.tool_namespace.as_deref(),
                representation: compatibility.representation,
                duration: compatibility.started.elapsed(),
                input_bytes: compatibility.input_bytes,
                output_bytes: output.len(),
                error: (!success).then_some(output.as_str()),
            });
        }

        let Some(result_payload) = self
            .context
            .is_enabled()
            .then(|| tool_dispatch_result(invocation, call_id, payload, result))
            .flatten()
        else {
            return;
        };
        let status = if success {
            ExecutionStatus::Completed
        } else {
            ExecutionStatus::Failed
        };
        self.context.record_completed(status, result_payload);
    }

    pub(crate) fn record_failed(&self, error: &FunctionCallError) {
        if let Some(compatibility) = &self.compatibility {
            let error_message = error.to_string();
            compatibility.diagnostics.record(CompatibilityEventInput {
                phase: "tool.dispatch",
                outcome: CompatibilityOutcome::Failure,
                tool_name: Some(&compatibility.tool_name),
                tool_namespace: compatibility.tool_namespace.as_deref(),
                representation: compatibility.representation,
                duration: compatibility.started.elapsed(),
                input_bytes: compatibility.input_bytes,
                output_bytes: error_message.len(),
                error: Some(&error_message),
            });
        }
        self.context.record_failed(error);
    }
}

fn tool_representation(source: &ToolCallSource, payload: &ToolPayload) -> ToolRepresentation {
    if matches!(source, ToolCallSource::CodeMode { .. }) {
        return ToolRepresentation::CodeMode;
    }
    match payload {
        ToolPayload::Function { .. } => ToolRepresentation::Function,
        ToolPayload::ToolSearch { .. } => ToolRepresentation::ToolSearch,
        ToolPayload::Custom { .. } => ToolRepresentation::Custom,
    }
}

fn tool_payload_bytes(payload: &ToolPayload) -> usize {
    match payload {
        ToolPayload::Function { arguments } | ToolPayload::Custom { input: arguments } => {
            arguments.len()
        }
        ToolPayload::ToolSearch { arguments } => serde_json::to_vec(arguments)
            .map(|encoded| encoded.len())
            .unwrap_or(usize::MAX),
    }
}

fn tool_dispatch_invocation(invocation: &ToolInvocation) -> Option<ToolDispatchInvocation> {
    let requester = match &invocation.source {
        ToolCallSource::Direct | ToolCallSource::DirectPlaintextMessage => {
            ToolDispatchRequester::Model {
                model_visible_call_id: invocation.call_id.clone(),
            }
        }
        ToolCallSource::CodeMode {
            cell_id,
            runtime_tool_call_id,
        } => ToolDispatchRequester::CodeCell {
            runtime_cell_id: cell_id.clone(),
            runtime_tool_call_id: runtime_tool_call_id.clone(),
        },
    };

    Some(ToolDispatchInvocation {
        thread_id: invocation.session.thread_id.to_string(),
        codex_turn_id: invocation.turn.sub_id.clone(),
        tool_call_id: invocation.call_id.clone(),
        tool_name: invocation.tool_name.name.clone(),
        tool_namespace: invocation
            .tool_name
            .namespace
            .as_ref()
            .filter(|_| !invocation.tool_name.is_default_namespace())
            .cloned(),
        requester,
        payload: tool_dispatch_payload(&invocation.payload),
    })
}

fn tool_dispatch_result(
    invocation: &ToolInvocation,
    call_id: &str,
    payload: &ToolPayload,
    result: &dyn ToolOutput,
) -> Option<ToolDispatchResult> {
    match invocation.source {
        ToolCallSource::Direct | ToolCallSource::DirectPlaintextMessage => {
            Some(ToolDispatchResult::DirectResponse {
                response_item: result.to_response_item(call_id, payload),
            })
        }
        ToolCallSource::CodeMode { .. } => Some(ToolDispatchResult::CodeModeResponse {
            value: result.code_mode_result(payload),
        }),
    }
}

fn tool_dispatch_payload(payload: &ToolPayload) -> ToolDispatchPayload {
    match payload {
        ToolPayload::Function { arguments } => ToolDispatchPayload::Function {
            arguments: arguments.clone(),
        },
        ToolPayload::ToolSearch { arguments } => ToolDispatchPayload::ToolSearch {
            arguments: arguments.clone(),
        },
        ToolPayload::Custom { input } => ToolDispatchPayload::Custom {
            input: input.clone(),
        },
    }
}

#[cfg(test)]
#[path = "tool_dispatch_trace_tests.rs"]
mod tests;
