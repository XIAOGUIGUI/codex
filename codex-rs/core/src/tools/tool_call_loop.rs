use std::sync::Mutex;

use serde_json::Value;
use sha1::Digest;
use sha1::Sha1;

use crate::tools::context::ToolPayload;
use crate::tools::router::ToolCall;

const RECOVERABLE_REJECTION_ATTEMPT: usize = 3;
const FATAL_REJECTION_ATTEMPT: usize = 4;
const MAX_CANONICAL_JSON_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolCallLoopDecision {
    Allow,
    Reject { attempt: usize },
    Abort { attempt: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolCallLoopPolicy {
    Disabled,
    ProtectToolCalls,
}

#[derive(Default)]
struct ToolCallLoopState {
    fingerprint: Option<[u8; 20]>,
    consecutive_count: usize,
}

/// Bounds consecutive identical direct tool calls from providers that do not
/// use the official Codex backend.
///
/// Only a fixed-size digest and counter are retained for the user turn. Tool
/// arguments are never copied into diagnostics or additional model context.
pub(crate) struct ToolCallLoopDetector {
    policy: ToolCallLoopPolicy,
    state: Mutex<ToolCallLoopState>,
}

impl ToolCallLoopDetector {
    pub(crate) fn new(policy: ToolCallLoopPolicy) -> Self {
        Self {
            policy,
            state: Mutex::new(ToolCallLoopState::default()),
        }
    }

    pub(crate) fn observe(&self, call: &ToolCall) -> ToolCallLoopDecision {
        if self.policy == ToolCallLoopPolicy::Disabled || is_polling_tool(call) {
            self.reset();
            return ToolCallLoopDecision::Allow;
        }

        let fingerprint = fingerprint(call);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.fingerprint == Some(fingerprint) {
            state.consecutive_count = state.consecutive_count.saturating_add(1);
        } else {
            state.fingerprint = Some(fingerprint);
            state.consecutive_count = 1;
        }

        match state.consecutive_count {
            FATAL_REJECTION_ATTEMPT.. => ToolCallLoopDecision::Abort {
                attempt: state.consecutive_count,
            },
            RECOVERABLE_REJECTION_ATTEMPT => ToolCallLoopDecision::Reject {
                attempt: state.consecutive_count,
            },
            _ => ToolCallLoopDecision::Allow,
        }
    }

    fn reset(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ToolCallLoopState::default();
    }
}

fn is_polling_tool(call: &ToolCall) -> bool {
    (call.tool_name.is_default_namespace()
        && matches!(call.tool_name.name.as_str(), "wait" | "write_stdin"))
        || (matches!(
            call.tool_name.namespace.as_deref(),
            Some("collaboration" | "multi_agent_v1")
        ) && call.tool_name.name == "wait_agent")
}

fn fingerprint(call: &ToolCall) -> [u8; 20] {
    let mut hasher = Sha1::new();
    let namespace = call
        .tool_name
        .namespace
        .as_deref()
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(codex_protocol::DEFAULT_FUNCTION_NAMESPACE);
    update_field(&mut hasher, namespace.as_bytes());
    update_field(&mut hasher, call.tool_name.name.as_bytes());
    match &call.payload {
        ToolPayload::Function { arguments } => {
            hasher.update([0]);
            update_canonical_json(&mut hasher, arguments);
        }
        ToolPayload::ToolSearch { arguments } => {
            hasher.update([1]);
            let arguments = serde_json::to_string(arguments).unwrap_or_default();
            update_field(&mut hasher, arguments.as_bytes());
        }
        ToolPayload::Custom { input } => {
            hasher.update([2]);
            update_field(&mut hasher, input.as_bytes());
        }
    }
    hasher.finalize().into()
}

fn update_canonical_json(hasher: &mut Sha1, arguments: &str) {
    if arguments.len() <= MAX_CANONICAL_JSON_BYTES
        && let Ok(value) = serde_json::from_str::<Value>(arguments)
        && let Ok(canonical) = serde_json::to_vec(&value)
    {
        update_field(hasher, &canonical);
        return;
    }
    update_field(hasher, arguments.as_bytes());
}

fn update_field(hasher: &mut Sha1, field: &[u8]) {
    hasher.update(field.len().to_le_bytes());
    hasher.update(field);
}

#[cfg(test)]
#[path = "tool_call_loop_tests.rs"]
mod tests;
