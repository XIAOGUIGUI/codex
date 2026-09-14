use std::fs::File;
use std::io::Read;
use std::time::Duration;
use std::time::SystemTime;

use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use zip::ZipArchive;

use super::CompatibilityDiagnostics;
use super::CompatibilityDiagnosticsConfig;
use super::CompatibilityDiagnosticsContext;
use super::CompatibilityEventInput;
use super::CompatibilityOutcome;
use super::CompatibilityReportOptions;
use super::TextIntegrityEventInput;
use super::ToolRepresentation;
use super::build_compatibility_report;
use super::classify;

#[test]
fn classifies_known_windows_and_provider_failures() {
    let cases = [
        (
            "windows sandbox: setup refresh failed",
            "windows.sandbox.setup",
        ),
        (
            "cannot enforce split writable root sets directly",
            "windows.sandbox.split_roots",
        ),
        (
            "provider does not support custom/freeform tools",
            "provider.tool_schema.custom_unsupported",
        ),
        (
            "Failed to find expected lines in secret.rs: company source",
            "file.apply_patch.context_mismatch",
        ),
        (
            "The first line of the patch must be '*** Begin Patch'",
            "file.apply_patch.missing_begin",
        ),
        (
            "Unexpected line found in update hunk: import request",
            "file.apply_patch.hunk_format",
        ),
        (
            "unresolved git merge conflict marker <<<<<<< HEAD",
            "file.merge_conflict",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(
            classify("tool.dispatch", Some("apply_patch"), Some(error)).code,
            expected
        );
    }
}

#[test]
fn configured_retention_cannot_exceed_the_privacy_limits() {
    let config = CompatibilityDiagnosticsConfig {
        enabled: true,
        directory: None,
        retention_days: Some(u16::MAX),
        max_total_mib: Some(u16::MAX),
    };

    assert_eq!(config.retention_days(), 30);
    assert_eq!(config.max_total_bytes(), 256 * 1024 * 1024);
}

#[test]
fn report_contains_only_bounded_structured_failure_data() {
    let temp = TempDir::new().unwrap();
    let directory = AbsolutePathBuf::try_from(temp.path().to_path_buf()).unwrap();
    let recorder = CompatibilityDiagnostics::start(
        &CompatibilityDiagnosticsConfig {
            enabled: true,
            directory: Some(directory),
            retention_days: Some(30),
            max_total_mib: Some(1),
        },
        CompatibilityDiagnosticsContext {
            app_version: "0.153.4".to_string(),
            build_commit: Some("test".to_string()),
            provider: "third-party".to_string(),
            model: "test-model".to_string(),
            session_id: "session-secret".to_string(),
        },
    )
    .unwrap();
    recorder.record(CompatibilityEventInput {
        phase: "tool.dispatch",
        outcome: CompatibilityOutcome::Failure,
        tool_name: Some("apply_patch"),
        tool_namespace: None,
        representation: ToolRepresentation::Function,
        duration: Duration::from_millis(3),
        input_bytes: 42,
        output_bytes: 120,
        error: Some("Failed to find expected lines in C:\\secret\\source.rs: TOP_SECRET_SOURCE"),
    });
    recorder.record(CompatibilityEventInput {
        phase: "tool.dispatch",
        outcome: CompatibilityOutcome::Failure,
        tool_name: Some("company_secret_tool"),
        tool_namespace: Some("company_secret_namespace"),
        representation: ToolRepresentation::Function,
        duration: Duration::from_millis(4),
        input_bytes: 24,
        output_bytes: 48,
        error: Some("invalid function arguments: INTERNAL_ARGUMENT"),
    });
    drop(recorder);
    std::thread::sleep(Duration::from_millis(100));

    let output = temp.path().join("report.zip");
    let summary = build_compatibility_report(CompatibilityReportOptions {
        directory: temp.path(),
        output: &output,
        since: SystemTime::UNIX_EPOCH,
    })
    .unwrap();
    assert_eq!(summary.total_events, 2);
    assert_eq!(summary.failed_events, 2);

    let mut archive = ZipArchive::new(File::open(output).unwrap()).unwrap();
    let mut samples = String::new();
    archive
        .by_name("samples.jsonl")
        .unwrap()
        .read_to_string(&mut samples)
        .unwrap();
    assert!(!samples.contains("TOP_SECRET_SOURCE"));
    assert!(!samples.contains("C:\\secret"));
    assert!(!samples.contains("session-secret"));
    assert!(!samples.contains("company_secret_tool"));
    assert!(!samples.contains("company_secret_namespace"));
    assert!(!samples.contains("INTERNAL_ARGUMENT"));
    assert!(samples.contains("external:"));
    assert!(samples.contains("file.apply_patch.context_mismatch"));
}

#[test]
fn text_integrity_event_records_only_fingerprint_lengths_and_flags() {
    let temp = TempDir::new().unwrap();
    let directory = AbsolutePathBuf::try_from(temp.path().to_path_buf()).unwrap();
    let recorder = CompatibilityDiagnostics::start(
        &CompatibilityDiagnosticsConfig {
            enabled: true,
            directory: Some(directory),
            retention_days: Some(30),
            max_total_mib: Some(1),
        },
        CompatibilityDiagnosticsContext {
            app_version: "0.153.4".to_string(),
            build_commit: Some("test".to_string()),
            provider: "third-party".to_string(),
            model: "test-model".to_string(),
            session_id: "session-secret".to_string(),
        },
    )
    .unwrap();
    let secret_text = "INTERNAL_SECRET andтобы";
    let analysis = recorder.record_text_integrity(TextIntegrityEventInput {
        phase: "model.text_integrity.assistant.completed",
        outcome: CompatibilityOutcome::Failure,
        tool_name: None,
        tool_namespace: None,
        representation: ToolRepresentation::None,
        text: secret_text,
    });
    drop(recorder);
    std::thread::sleep(Duration::from_millis(100));

    let event_path = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .expect("event log");
    let mut event = String::new();
    File::open(event_path)
        .unwrap()
        .read_to_string(&mut event)
        .unwrap();

    assert!(!event.contains(secret_text));
    assert_eq!(analysis.byte_count, secret_text.len());
    assert!(event.contains("\"text_fingerprint\":\""));
    assert!(event.contains("mixed_alphabetic_scripts"));
    assert!(event.contains("provider.output.text_integrity"));
}
