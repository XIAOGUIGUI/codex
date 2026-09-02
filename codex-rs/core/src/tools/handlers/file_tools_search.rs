use std::collections::HashMap;
use std::time::Duration;

use codex_exec_server::ExecEnvPolicy;
use codex_exec_server::ExecOutputStream;
use codex_exec_server::ExecParams;
use codex_exec_server::ProcessId;
use codex_install_context::InstallContext;
use codex_protocol::config_types::ShellEnvironmentPolicyInherit;
use codex_protocol::exec_output::bytes_to_string_smart;
use serde::Deserialize;
use serde::Serialize;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::function_tool::FunctionCallError;
use crate::tools::handlers::file_tools_read::append_truncated_text;
use crate::tools::handlers::parse_arguments;

const MAX_RESULT_BYTES: usize = 32 * 1024;
const MAX_RG_CAPTURE_BYTES: usize = 64 * 1024;
const MAX_CONTENT_COLUMNS: usize = 4 * 1024;
const MAX_SEARCH_LINE_BYTES: usize = 8 * 1024;
const DEFAULT_RESULT_LINES: usize = 100;
const MAX_RESULT_LINES: usize = 1_000;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
struct GrepFilesArgs {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    output_mode: Option<GrepOutputMode>,
    context: Option<usize>,
    head_limit: Option<usize>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GrepOutputMode {
    FilesWithMatches,
    Content,
    Count,
}

#[derive(Deserialize)]
struct GlobFilesArgs {
    pattern: String,
    path: Option<String>,
    head_limit: Option<usize>,
}

#[derive(Serialize)]
struct SearchResult {
    output: String,
    result_count: usize,
    truncated: bool,
}

pub(super) async fn grep_files(
    environment: &crate::session::turn_context::TurnEnvironment,
    arguments: &str,
    cancellation_token: CancellationToken,
) -> Result<String, FunctionCallError> {
    let args: GrepFilesArgs = parse_arguments(arguments)?;
    if args.pattern.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "grep_files.pattern must not be empty".to_string(),
        ));
    }
    let head_limit = validate_head_limit("grep_files", args.head_limit)?;
    let mode = args.output_mode.unwrap_or(GrepOutputMode::FilesWithMatches);
    let context = args.context.unwrap_or(0);
    if context > 20 {
        return Err(FunctionCallError::RespondToModel(
            "grep_files.context cannot exceed 20".to_string(),
        ));
    }
    let search_path = resolve_search_path(environment, args.path.as_deref(), "grep_files")?;
    let mut argv = vec![
        InstallContext::current()
            .rg_command()
            .to_string_lossy()
            .into_owned(),
        "--color".to_string(),
        "never".to_string(),
        "--encoding".to_string(),
        "auto".to_string(),
    ];
    match mode {
        GrepOutputMode::FilesWithMatches => argv.push("--files-with-matches".to_string()),
        GrepOutputMode::Content => {
            argv.extend([
                "--line-number".to_string(),
                "--column".to_string(),
                "--no-heading".to_string(),
                "--max-columns".to_string(),
                MAX_CONTENT_COLUMNS.to_string(),
                "--max-columns-preview".to_string(),
            ]);
            if context > 0 {
                argv.extend(["--context".to_string(), context.to_string()]);
            }
        }
        GrepOutputMode::Count => argv.push("--count".to_string()),
    }
    if let Some(glob) = args.glob {
        argv.extend(["--glob".to_string(), glob]);
    }
    argv.extend([
        "--".to_string(),
        args.pattern,
        search_path.inferred_native_path_string(),
    ]);
    let captured = run_rg(environment, argv, cancellation_token).await?;
    format_search_result(captured, head_limit, /*sort*/ false)
}

pub(super) async fn glob_files(
    environment: &crate::session::turn_context::TurnEnvironment,
    arguments: &str,
    cancellation_token: CancellationToken,
) -> Result<String, FunctionCallError> {
    let args: GlobFilesArgs = parse_arguments(arguments)?;
    if args.pattern.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "glob_files.pattern must not be empty".to_string(),
        ));
    }
    let head_limit = validate_head_limit("glob_files", args.head_limit)?;
    let search_path = resolve_search_path(environment, args.path.as_deref(), "glob_files")?;
    let argv = vec![
        InstallContext::current()
            .rg_command()
            .to_string_lossy()
            .into_owned(),
        "--files".to_string(),
        "--sort".to_string(),
        "path".to_string(),
        "--glob".to_string(),
        args.pattern,
        "--".to_string(),
        search_path.inferred_native_path_string(),
    ];
    let captured = run_rg(environment, argv, cancellation_token).await?;
    format_search_result(captured, head_limit, /*sort*/ true)
}

fn validate_head_limit(
    tool_name: &str,
    requested: Option<usize>,
) -> Result<usize, FunctionCallError> {
    let limit = requested.unwrap_or(DEFAULT_RESULT_LINES);
    if limit == 0 || limit > MAX_RESULT_LINES {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name}.head_limit must be between 1 and {MAX_RESULT_LINES}"
        )));
    }
    Ok(limit)
}

fn resolve_search_path(
    environment: &crate::session::turn_context::TurnEnvironment,
    path: Option<&str>,
    tool_name: &str,
) -> Result<codex_utils_path_uri::PathUri, FunctionCallError> {
    path.filter(|path| !path.is_empty()).map_or_else(
        || Ok(environment.cwd().clone()),
        |path| {
            environment.cwd().join(path).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "unable to resolve {tool_name} path `{path}`: {err}"
                ))
            })
        },
    )
}

struct CapturedProcess {
    stdout: Vec<u8>,
    truncated: bool,
}

async fn run_rg(
    environment: &crate::session::turn_context::TurnEnvironment,
    argv: Vec<String>,
    cancellation_token: CancellationToken,
) -> Result<CapturedProcess, FunctionCallError> {
    if environment.environment.is_remote() {
        return Err(FunctionCallError::RespondToModel(
            "Windows native file search currently supports only the local environment".to_string(),
        ));
    }
    let started = environment
        .environment
        .get_exec_backend()
        .start(ExecParams {
            process_id: ProcessId::from(format!("file-tool-{}", Uuid::new_v4())),
            argv,
            cwd: environment.cwd().clone(),
            env_policy: Some(ExecEnvPolicy {
                inherit: ShellEnvironmentPolicyInherit::All,
                ignore_default_excludes: false,
                exclude: Vec::new(),
                r#set: HashMap::new(),
                include_only: Vec::new(),
            }),
            shell_snapshot: None,
            env: HashMap::new(),
            tty: false,
            pipe_stdin: false,
            arg0: None,
            sandbox: Some(environment.sandbox_context(/*additional_permissions*/ None)),
            enforce_managed_network: false,
            managed_network: None,
            network_proxy: None,
        })
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format!("failed to start rg: {err}")))?;
    let process = started.process;
    let deadline = Instant::now() + SEARCH_TIMEOUT;
    let mut after_seq = None;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = None;
    let mut truncated = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            let _ = process.terminate().await;
            return Err(FunctionCallError::RespondToModel(format!(
                "file search timed out after {} seconds",
                SEARCH_TIMEOUT.as_secs()
            )));
        }
        let response_result = tokio::select! {
            _ = cancellation_token.cancelled() => {
                let _ = process.terminate().await;
                return Err(FunctionCallError::RespondToModel("file search was cancelled".to_string()));
            }
            response = tokio::time::timeout(
                remaining,
                process.read(
                    /*after_seq*/ after_seq,
                    /*max_bytes*/ Some(16 * 1024),
                    /*wait_ms*/ Some(500),
                ),
            ) => response,
        };
        let response = match response_result {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                let _ = process.terminate().await;
                return Err(FunctionCallError::RespondToModel(format!(
                    "failed to read rg output: {err}"
                )));
            }
            Err(_) => {
                let _ = process.terminate().await;
                return Err(FunctionCallError::RespondToModel(format!(
                    "file search timed out after {} seconds",
                    SEARCH_TIMEOUT.as_secs()
                )));
            }
        };
        after_seq = response.next_seq.checked_sub(1).or(after_seq);
        if let Some(failure) = response.failure {
            return Err(FunctionCallError::RespondToModel(format!(
                "rg process failed: {failure}"
            )));
        }
        for chunk in response.chunks {
            match chunk.stream {
                ExecOutputStream::Stdout | ExecOutputStream::Pty => {
                    let remaining = MAX_RG_CAPTURE_BYTES.saturating_sub(stdout.len());
                    if chunk.chunk.0.len() > remaining {
                        stdout.extend_from_slice(&chunk.chunk.0[..remaining]);
                        truncated = true;
                    } else {
                        stdout.extend_from_slice(&chunk.chunk.0);
                    }
                }
                ExecOutputStream::Stderr => {
                    let remaining = MAX_RG_CAPTURE_BYTES.saturating_sub(stderr.len());
                    stderr.extend_from_slice(&chunk.chunk.0[..chunk.chunk.0.len().min(remaining)]);
                }
            }
        }
        if truncated {
            let _ = process.terminate().await;
            truncate_to_complete_line(&mut stdout);
            break;
        }
        if response.exited {
            exit_code = response.exit_code;
        }
        if response.closed {
            break;
        }
    }

    if !truncated && !matches!(exit_code, Some(0 | 1)) {
        let stderr = bytes_to_string_smart(&stderr);
        return Err(FunctionCallError::RespondToModel(format!(
            "rg exited with {}{}",
            exit_code.map_or_else(|| "unknown status".to_string(), |code| code.to_string()),
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        )));
    }

    Ok(CapturedProcess { stdout, truncated })
}

fn format_search_result(
    captured: CapturedProcess,
    head_limit: usize,
    sort: bool,
) -> Result<String, FunctionCallError> {
    let CapturedProcess {
        stdout,
        mut truncated,
    } = captured;
    let text = bytes_to_string_smart(&stdout);
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if sort {
        lines.sort();
    }
    if lines.len() > head_limit {
        lines.truncate(head_limit);
        truncated = true;
    }
    let mut output = String::new();
    let mut kept = 0usize;
    for line in lines {
        let available = MAX_RESULT_BYTES.saturating_sub(output.len());
        if available == 0 {
            truncated = true;
            break;
        }
        let line_budget = available.min(MAX_SEARCH_LINE_BYTES);
        if line.len().saturating_add(1) > line_budget {
            append_truncated_text(&mut output, &line, line_budget);
            truncated = true;
        } else {
            output.push_str(&line);
            output.push('\n');
        }
        kept += 1;
    }
    serde_json::to_string_pretty(&SearchResult {
        output,
        result_count: kept,
        truncated,
    })
    .map_err(|err| {
        FunctionCallError::Fatal(format!("failed to serialize file search output: {err}"))
    })
}

fn truncate_to_complete_line(bytes: &mut Vec<u8>) {
    if let Some(newline) = bytes.iter().rposition(|byte| *byte == b'\n') {
        bytes.truncate(newline + 1);
    } else {
        bytes.clear();
    }
}

#[cfg(test)]
#[path = "file_tools_search_tests.rs"]
mod tests;
