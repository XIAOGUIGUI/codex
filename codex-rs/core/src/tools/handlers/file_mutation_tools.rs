use codex_apply_patch::prepare_structured_edit;
use codex_apply_patch::prepare_structured_write;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;

use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::apply_patch::execute_structured_file_mutation;
use crate::tools::handlers::file_mutation_tools_spec::create_file_mutation_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::tools::sandboxing::ToolCtx;

const MAX_FILE_MUTATION_ARGUMENT_BYTES: usize = 8 * 1024;
// Manually reviewed P0 context item: this may exceed 1K tokens, but the combined plaintext and
// encrypted argument payload has a hard 8 KiB cap and cannot approach the 10K-token item limit.
const MAX_FILE_PATH_BYTES: usize = 1024;
const MAX_MODEL_VISIBLE_PATH_CHARS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileMutationToolKind {
    Edit,
    Write,
}

impl FileMutationToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::Edit => "edit_file",
            Self::Write => "write_file",
        }
    }
}

pub struct FileMutationToolHandler {
    kind: FileMutationToolKind,
}

impl FileMutationToolHandler {
    pub fn new(kind: FileMutationToolKind) -> Self {
        Self { kind }
    }

    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            step_context,
            cancellation_token,
            tracker,
            call_id,
            tool_name,
            payload,
            ..
        } = invocation;
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} handler received unsupported payload",
                self.kind.name()
            )));
        };
        if arguments.len() > MAX_FILE_MUTATION_ARGUMENT_BYTES {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} arguments exceed the {MAX_FILE_MUTATION_ARGUMENT_BYTES}-byte limit; use apply_patch for larger changes",
                self.kind.name()
            )));
        }
        let Some(turn_environment) =
            resolve_tool_environment(&step_context.environments, /*environment_id*/ None)?
        else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} is unavailable in this session",
                self.kind.name()
            )));
        };
        if turn_environment.environment.is_remote() {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} is only available for the local Windows environment; use apply_patch for remote environments",
                self.kind.name()
            )));
        }
        let path_arg = match self.kind {
            FileMutationToolKind::Edit => {
                let args: EditFileArgs = parse_arguments(&arguments)?;
                validate_file_path(self.kind, &args.file_path)?;
                MutationArgs::Edit(args)
            }
            FileMutationToolKind::Write => {
                let args: WriteFileArgs = parse_arguments(&arguments)?;
                validate_file_path(self.kind, &args.file_path)?;
                MutationArgs::Write(args)
            }
        };
        let path = turn_environment
            .cwd()
            .join(path_arg.file_path())
            .map_err(|err| {
                tracing::debug!(error = %err, "failed to resolve structured file mutation path");
                FunctionCallError::RespondToModel(format!(
                    "unable to resolve {} path `{}`",
                    self.kind.name(),
                    truncate_for_model(path_arg.file_path())
                ))
            })?;
        let fs = turn_environment.environment.get_filesystem();
        let sandbox = turn_environment.sandbox_context(/*additional_permissions*/ None);
        let mutation = match path_arg {
            MutationArgs::Edit(args) => {
                prepare_structured_edit(
                    path,
                    &args.old_string,
                    &args.new_string,
                    args.replace_all,
                    fs.as_ref(),
                    Some(&sandbox),
                )
                .await
            }
            MutationArgs::Write(args) => {
                prepare_structured_write(path, args.content, fs.as_ref(), Some(&sandbox)).await
            }
        }
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "{} validation failed: {err}",
                self.kind.name()
            ))
        })?;
        let tool_ctx = ToolCtx {
            session,
            step_context: std::sync::Arc::clone(&step_context),
            cancellation_token,
            call_id,
            tool_name,
        };
        let content = execute_structured_file_mutation(
            mutation,
            turn_environment.cwd(),
            turn_environment.clone(),
            Some(&tracker),
            tool_ctx,
        )
        .await?;
        Ok(boxed_tool_output(
            crate::tools::context::ApplyPatchToolOutput::from_text(content),
        ))
    }
}

impl ToolExecutor<ToolInvocation> for FileMutationToolHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.kind.name())
    }

    fn spec(&self) -> ToolSpec {
        create_file_mutation_tool(self.kind)
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for FileMutationToolHandler {
    fn model_argument_bytes_limit(&self) -> Option<usize> {
        Some(MAX_FILE_MUTATION_ARGUMENT_BYTES)
    }

    fn hook_tool_name(&self, _invocation: &ToolInvocation) -> HookToolName {
        match self.kind {
            FileMutationToolKind::Edit => HookToolName::edit_file(),
            FileMutationToolKind::Write => HookToolName::write_file(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileArgs {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileArgs {
    file_path: String,
    content: String,
}

enum MutationArgs {
    Edit(EditFileArgs),
    Write(WriteFileArgs),
}

impl MutationArgs {
    fn file_path(&self) -> &str {
        match self {
            Self::Edit(args) => &args.file_path,
            Self::Write(args) => &args.file_path,
        }
    }
}

fn validate_file_path(
    kind: FileMutationToolKind,
    file_path: &str,
) -> Result<(), FunctionCallError> {
    if file_path.is_empty()
        || file_path.len() > MAX_FILE_PATH_BYTES
        || file_path
            .chars()
            .any(|character| matches!(character, '\0' | '\r' | '\n'))
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "{}.file_path must be non-empty, at most {MAX_FILE_PATH_BYTES} bytes, and contain no control line breaks",
            kind.name(),
        )));
    }
    Ok(())
}

fn truncate_for_model(value: &str) -> String {
    let mut characters = value.chars();
    let prefix = characters
        .by_ref()
        .take(MAX_MODEL_VISIBLE_PATH_CHARS.saturating_sub(1))
        .collect::<String>();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
#[path = "file_mutation_tools_tests.rs"]
mod tests;
