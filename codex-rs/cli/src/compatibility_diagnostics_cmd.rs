use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use anyhow::Context;
use clap::Parser;
use codex_core::config::ConfigBuilder;
use codex_diagnostics::CompatibilityReportOptions;
use codex_diagnostics::build_compatibility_report;
use codex_diagnostics::compatibility_diagnostics_status;
use codex_diagnostics::sha256_file;
use codex_utils_cli::CliConfigOverrides;

#[derive(Debug, Parser)]
pub(crate) struct CompatibilityDiagnosticsCommand {
    #[command(subcommand)]
    subcommand: CompatibilityDiagnosticsSubcommand,
}

#[derive(Debug, clap::Subcommand)]
enum CompatibilityDiagnosticsSubcommand {
    /// Show the configured directory and bounded storage usage.
    Status {
        /// Emit machine-readable JSON.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Export a bounded ZIP report without prompts, source, or raw tool arguments.
    Report {
        /// Time window such as 24h or 7d.
        #[arg(long, default_value = "7d")]
        since: String,

        /// Destination ZIP file.
        #[arg(long, short = 'o')]
        output: PathBuf,
    },
}

pub(crate) async fn run(
    command: CompatibilityDiagnosticsCommand,
    root_config_overrides: &CliConfigOverrides,
) -> anyhow::Result<()> {
    let cli_overrides = root_config_overrides
        .parse_overrides()
        .map_err(anyhow::Error::msg)?;
    let config = ConfigBuilder::default()
        .cli_overrides(cli_overrides)
        .build()
        .await?;
    let diagnostics_config = &config.compatibility_diagnostics;
    if !diagnostics_config.enabled {
        anyhow::bail!(
            "compatibility diagnostics are disabled; enable [compatibility_diagnostics] first"
        );
    }
    let directory = diagnostics_config
        .directory
        .as_ref()
        .context("compatibility diagnostics directory is not configured")?;

    match command.subcommand {
        CompatibilityDiagnosticsSubcommand::Status { json } => {
            let status = compatibility_diagnostics_status(directory.as_ref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("Directory: {}", status.directory.display());
                println!("Files: {}", status.files);
                println!("Bytes: {}", status.bytes);
                println!(
                    "Newest event: {}",
                    status.newest_event.as_deref().unwrap_or("none")
                );
            }
        }
        CompatibilityDiagnosticsSubcommand::Report { since, output } => {
            let window = parse_window(&since)?;
            let since = SystemTime::now()
                .checked_sub(window)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            let summary = build_compatibility_report(CompatibilityReportOptions {
                directory: directory.as_ref(),
                output: &output,
                since,
            })
            .map_err(anyhow::Error::msg)?;
            println!("Report: {}", output.display());
            println!("SHA-256: {}", sha256_file(&output)?);
            println!("Events: {}", summary.total_events);
            println!("Failures: {}", summary.failed_events);
        }
    }
    Ok(())
}

fn parse_window(value: &str) -> anyhow::Result<Duration> {
    let (number, unit) = value.split_at(value.len().saturating_sub(1));
    let number = number
        .parse::<u64>()
        .with_context(|| format!("invalid diagnostics window: {value}"))?;
    let seconds = match unit {
        "h" => number.checked_mul(60 * 60),
        "d" => number.checked_mul(24 * 60 * 60),
        _ => None,
    }
    .with_context(|| format!("invalid diagnostics window: {value}; use Nh or Nd"))?;
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::parse_window;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    #[test]
    fn parses_bounded_report_windows() {
        assert_eq!(parse_window("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_window("7d").unwrap(), Duration::from_secs(604_800));
        assert!(parse_window("7days").is_err());
    }
}
