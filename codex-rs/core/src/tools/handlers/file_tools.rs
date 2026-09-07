use std::time::Duration;
use std::time::Instant;

use codex_protocol::items::CommandExecutionItem;
use codex_protocol::items::CommandExecutionStatus;
use codex_protocol::items::TurnItem;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::ExecCommandSource;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::file_tools_read::read_file;
use crate::tools::handlers::file_tools_search::glob_files;
use crate::tools::handlers::file_tools_search::grep_files;
use crate::tools::handlers::file_tools_spec::create_file_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileToolKind {
    Read,
    Grep,
    Glob,
}

impl FileToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::Read => "read_file",
            Self::Grep => "grep_files",
            Self::Glob => "glob_files",
        }
    }
}

pub struct FileToolHandler {
    kind: FileToolKind,
}

impl FileToolHandler {
    pub fn new(kind: FileToolKind) -> Self {
        Self { kind }
    }

    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            step_context,
            cancellation_token,
            call_id,
            payload,
            ..
        } = invocation;
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} handler received unsupported payload",
                self.kind.name()
            )));
        };
        let Some(turn_environment) =
            resolve_tool_environment(&step_context.environments, /*environment_id*/ None)?
        else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} is unavailable in this session",
                self.kind.name()
            )));
        };

        let presentation = file_tool_presentation(self.kind, &arguments);
        let started_at = Instant::now();
        emit_file_tool_started(
            session.as_ref(),
            turn.as_ref(),
            &call_id,
            turn_environment.cwd(),
            &presentation,
        )
        .await;
        let result = match self.kind {
            FileToolKind::Read => match parse_arguments(&arguments) {
                Ok(args) => read_file(turn_environment, args, cancellation_token).await,
                Err(err) => Err(err),
            },
            FileToolKind::Grep => {
                grep_files(turn_environment, &arguments, cancellation_token).await
            }
            FileToolKind::Glob => {
                glob_files(turn_environment, &arguments, cancellation_token).await
            }
        };
        let status = if result.is_ok() {
            CommandExecutionStatus::Completed
        } else {
            CommandExecutionStatus::Failed
        };
        emit_file_tool_completed(
            session.as_ref(),
            turn.as_ref(),
            &call_id,
            turn_environment.cwd(),
            presentation,
            started_at.elapsed(),
            status,
        )
        .await;
        let result = result?;

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            result,
            /*success*/ Some(true),
        )))
    }
}

impl ToolExecutor<ToolInvocation> for FileToolHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.kind.name())
    }

    fn spec(&self) -> ToolSpec {
        create_file_tool(self.kind)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for FileToolHandler {}

struct FileToolPresentation {
    command: Vec<String>,
    parsed_cmd: Vec<ParsedCommand>,
}

fn file_tool_presentation(kind: FileToolKind, arguments: &str) -> FileToolPresentation {
    let arguments = serde_json::from_str::<serde_json::Value>(arguments).unwrap_or_default();
    let string_arg = |name: &str| {
        arguments
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let path = match kind {
        FileToolKind::Read => string_arg("file_path"),
        FileToolKind::Grep | FileToolKind::Glob => string_arg("path"),
    };
    let command_text = path.as_ref().map_or_else(
        || kind.name().to_string(),
        |path| format!("{} {path}", kind.name()),
    );
    let parsed_cmd = match kind {
        FileToolKind::Read => path.map_or_else(
            || {
                vec![ParsedCommand::Unknown {
                    cmd: command_text.clone(),
                }]
            },
            |path| {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
                vec![ParsedCommand::Read {
                    cmd: command_text.clone(),
                    name,
                    path: path.into(),
                }]
            },
        ),
        FileToolKind::Grep => vec![ParsedCommand::Search {
            cmd: command_text.clone(),
            query: string_arg("pattern"),
            path,
        }],
        FileToolKind::Glob => vec![ParsedCommand::ListFiles {
            cmd: command_text.clone(),
            path: string_arg("pattern"),
        }],
    };
    FileToolPresentation {
        command: vec![command_text],
        parsed_cmd,
    }
}

async fn emit_file_tool_started(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    call_id: &str,
    cwd: &codex_utils_path_uri::PathUri,
    presentation: &FileToolPresentation,
) {
    session
        .emit_turn_item_started(
            turn,
            &TurnItem::CommandExecution(command_execution_item(
                call_id,
                cwd,
                presentation,
                CommandExecutionStatus::InProgress,
                /*duration*/ None,
            )),
        )
        .await;
}

async fn emit_file_tool_completed(
    session: &crate::session::session::Session,
    turn: &crate::session::turn_context::TurnContext,
    call_id: &str,
    cwd: &codex_utils_path_uri::PathUri,
    presentation: FileToolPresentation,
    duration: Duration,
    status: CommandExecutionStatus,
) {
    session
        .emit_turn_item_completed(
            turn,
            TurnItem::CommandExecution(command_execution_item(
                call_id,
                cwd,
                &presentation,
                status,
                Some(duration),
            )),
        )
        .await;
}

fn command_execution_item(
    call_id: &str,
    cwd: &codex_utils_path_uri::PathUri,
    presentation: &FileToolPresentation,
    status: CommandExecutionStatus,
    duration: Option<Duration>,
) -> CommandExecutionItem {
    CommandExecutionItem {
        id: call_id.to_string(),
        plugin_id: None,
        script_path: None,
        process_id: None,
        command: presentation.command.clone(),
        cwd: cwd.clone(),
        parsed_cmd: presentation.parsed_cmd.clone(),
        source: ExecCommandSource::Agent,
        interaction_input: None,
        status,
        stdout: None,
        stderr: None,
        aggregated_output: None,
        exit_code: match status {
            CommandExecutionStatus::Completed => Some(0),
            CommandExecutionStatus::Failed | CommandExecutionStatus::Declined => Some(1),
            CommandExecutionStatus::InProgress => None,
        },
        duration,
        formatted_output: None,
    }
}

#[cfg(test)]
#[path = "file_tools_tests.rs"]
mod tests;
