use std::collections::BTreeMap;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use chrono::DateTime;
use chrono::Utc;
use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::TextIntegrityAnalysis;
use crate::analyze_text_integrity;

mod category;
mod known_issues;

use category::classify;
use known_issues::known_issues;

const SCHEMA_VERSION: u16 = 2;
const DEFAULT_RETENTION_DAYS: u16 = 30;
const DEFAULT_MAX_TOTAL_MIB: u16 = 256;
const MAX_EVENT_BYTES: usize = 16 * 1024;
const MAX_ERROR_CLASSIFICATION_BYTES: usize = 16 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const EVENT_QUEUE_CAPACITY: usize = 1024;
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(60);
const MAX_REPORT_BYTES: usize = 10 * 1024 * 1024;
const MAX_SAMPLES_PER_FINGERPRINT: usize = 3;
const MAX_REPORT_SAMPLES: usize = 512;

/// User configuration for privacy-safe local compatibility diagnostics.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
#[schemars(deny_unknown_fields)]
pub struct CompatibilityDiagnosticsConfig {
    pub enabled: bool,
    pub directory: Option<AbsolutePathBuf>,
    pub retention_days: Option<u16>,
    pub max_total_mib: Option<u16>,
}

impl CompatibilityDiagnosticsConfig {
    fn retention_days(&self) -> u16 {
        self.retention_days
            .unwrap_or(DEFAULT_RETENTION_DAYS)
            .clamp(1, DEFAULT_RETENTION_DAYS)
    }

    fn max_total_bytes(&self) -> u64 {
        u64::from(
            self.max_total_mib
                .unwrap_or(DEFAULT_MAX_TOTAL_MIB)
                .clamp(1, DEFAULT_MAX_TOTAL_MIB),
        ) * 1024
            * 1024
    }
}

/// Session identity retained without endpoint or credential data.
#[derive(Clone, Debug)]
pub struct CompatibilityDiagnosticsContext {
    pub app_version: String,
    pub build_commit: Option<String>,
    pub provider: String,
    pub model: String,
    pub session_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatibilityOutcome {
    Success,
    Failure,
    Rejected,
    Timeout,
    Cancelled,
}

impl CompatibilityOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Rejected => "rejected",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolRepresentation {
    Function,
    Custom,
    ToolSearch,
    CodeMode,
    None,
}

impl ToolRepresentation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Custom => "custom",
            Self::ToolSearch => "tool_search",
            Self::CodeMode => "code_mode",
            Self::None => "none",
        }
    }
}

/// Content-free observation submitted at a compatibility-sensitive boundary.
pub struct CompatibilityEventInput<'a> {
    pub phase: &'a str,
    pub outcome: CompatibilityOutcome,
    pub tool_name: Option<&'a str>,
    pub tool_namespace: Option<&'a str>,
    pub representation: ToolRepresentation,
    pub duration: Duration,
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub error: Option<&'a str>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompatibilityMetrics {
    pub history_bytes: u64,
    pub tool_schema_bytes: u64,
    pub instruction_bytes: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
}

impl CompatibilityMetrics {
    fn add_assign(&mut self, other: &Self) {
        self.history_bytes = self.history_bytes.saturating_add(other.history_bytes);
        self.tool_schema_bytes = self
            .tool_schema_bytes
            .saturating_add(other.tool_schema_bytes);
        self.instruction_bytes = self
            .instruction_bytes
            .saturating_add(other.instruction_bytes);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(other.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(other.reasoning_output_tokens);
    }
}

/// Privacy-safe text observation. The recorder stores only length, a session-salted fingerprint,
/// and anomaly flags.
pub struct TextIntegrityEventInput<'a> {
    pub phase: &'a str,
    pub outcome: CompatibilityOutcome,
    pub tool_name: Option<&'a str>,
    pub tool_namespace: Option<&'a str>,
    pub representation: ToolRepresentation,
    pub text: &'a str,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
struct CompatibilityEvent {
    schema_version: u16,
    timestamp: String,
    process_id: u32,
    app_version: String,
    build_commit: Option<String>,
    os: String,
    arch: String,
    provider: String,
    model: String,
    session_hash: String,
    phase: String,
    category: String,
    outcome: String,
    tool_name: Option<String>,
    tool_namespace: Option<String>,
    representation: String,
    duration_ms: u64,
    input_bytes: usize,
    output_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metrics: Option<CompatibilityMetrics>,
    #[serde(default)]
    text_fingerprint: Option<String>,
    #[serde(default)]
    text_char_count: Option<usize>,
    #[serde(default)]
    integrity_flags: Vec<String>,
    error_fingerprint: Option<String>,
    error_summary: Option<String>,
    resident_memory_bytes: Option<u64>,
    private_commit_bytes: Option<u64>,
    peak_private_commit_bytes: Option<u64>,
    dropped_events: u64,
}

struct RecorderInner {
    sender: mpsc::SyncSender<CompatibilityEvent>,
    context: CompatibilityDiagnosticsContext,
    dropped_events: Arc<AtomicU64>,
}

/// Best-effort recorder. A disabled or unhealthy recorder never fails the primary operation.
#[derive(Clone, Default)]
pub struct CompatibilityDiagnostics {
    inner: Option<Arc<RecorderInner>>,
}

impl CompatibilityDiagnostics {
    pub fn start(
        config: &CompatibilityDiagnosticsConfig,
        context: CompatibilityDiagnosticsContext,
    ) -> Result<Self, String> {
        if !config.enabled {
            return Ok(Self::default());
        }
        let directory = config
            .directory
            .as_ref()
            .ok_or_else(|| "compatibility diagnostics requires an absolute directory".to_string())?
            .to_path_buf();
        std::fs::create_dir_all(&directory).map_err(|error| {
            format!("failed to create compatibility diagnostics directory: {error}")
        })?;
        cleanup_directory(
            &directory,
            config.retention_days(),
            config.max_total_bytes(),
        );
        let output = EventFile::create(&directory).map_err(|error| {
            format!("failed to create compatibility diagnostics event file: {error}")
        })?;

        let (sender, receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let dropped_events = Arc::new(AtomicU64::new(0));
        let writer_dropped_events = Arc::clone(&dropped_events);
        let writer_context = context.clone();
        let retention_days = config.retention_days();
        let max_total_bytes = config.max_total_bytes();
        std::thread::Builder::new()
            .name("codex-compat-diagnostics".to_string())
            .spawn(move || {
                writer_loop(
                    &directory,
                    output,
                    receiver,
                    writer_dropped_events,
                    writer_context,
                    retention_days,
                    max_total_bytes,
                );
            })
            .map_err(|error| {
                format!("failed to start compatibility diagnostics writer: {error}")
            })?;

        Ok(Self {
            inner: Some(Arc::new(RecorderInner {
                sender,
                context,
                dropped_events,
            })),
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    pub fn record(&self, input: CompatibilityEventInput<'_>) {
        self.record_inner(input, None, None);
    }

    pub fn record_with_metrics(
        &self,
        input: CompatibilityEventInput<'_>,
        metrics: CompatibilityMetrics,
    ) {
        self.record_inner(input, None, Some(metrics));
    }

    pub fn record_text_integrity(
        &self,
        input: TextIntegrityEventInput<'_>,
    ) -> TextIntegrityAnalysis {
        let analysis = analyze_text_integrity(input.text);
        let text_fingerprint = self
            .inner
            .as_ref()
            .map(|inner| salted_fingerprint(&inner.context.session_id, input.text));
        let error = analysis
            .is_suspicious()
            .then_some("model output text integrity signal");
        let outcome = if analysis.is_suspicious() {
            input.outcome
        } else {
            CompatibilityOutcome::Success
        };
        self.record_inner(
            CompatibilityEventInput {
                phase: input.phase,
                outcome,
                tool_name: input.tool_name,
                tool_namespace: input.tool_namespace,
                representation: input.representation,
                duration: Duration::ZERO,
                input_bytes: input.text.len(),
                output_bytes: 0,
                error,
            },
            text_fingerprint
                .as_deref()
                .map(|fingerprint| (&analysis, fingerprint)),
            None,
        );
        analysis
    }

    fn record_inner(
        &self,
        input: CompatibilityEventInput<'_>,
        text_integrity: Option<(&TextIntegrityAnalysis, &str)>,
        metrics: Option<CompatibilityMetrics>,
    ) {
        let Some(inner) = &self.inner else {
            return;
        };
        let error = input.error.filter(|error| !error.is_empty());
        let classification_error =
            error.map(|error| bounded(error, MAX_ERROR_CLASSIFICATION_BYTES));
        let category = classify(
            input.phase,
            input.tool_name,
            classification_error.as_deref(),
        );
        let snapshot = crate::snapshot().process;
        let event = CompatibilityEvent {
            schema_version: SCHEMA_VERSION,
            timestamp: Utc::now().to_rfc3339(),
            process_id: snapshot.id,
            app_version: bounded(&inner.context.app_version, 64),
            build_commit: inner
                .context
                .build_commit
                .as_deref()
                .map(|value| bounded(value, 64)),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            provider: bounded(&inner.context.provider, 128),
            model: bounded(&inner.context.model, 128),
            session_hash: fingerprint(&inner.context.session_id),
            phase: bounded(input.phase, 96),
            category: category.code.to_string(),
            outcome: input.outcome.as_str().to_string(),
            tool_name: input.tool_name.map(safe_tool_name),
            tool_namespace: input.tool_namespace.map(safe_namespace),
            representation: input.representation.as_str().to_string(),
            duration_ms: u64::try_from(input.duration.as_millis()).unwrap_or(u64::MAX),
            input_bytes: input.input_bytes,
            output_bytes: input.output_bytes,
            metrics,
            text_fingerprint: text_integrity.map(|(_, fingerprint)| fingerprint.to_string()),
            text_char_count: text_integrity.map(|(analysis, _)| analysis.char_count),
            integrity_flags: text_integrity.map_or_else(Vec::new, |(analysis, _)| {
                analysis
                    .flags
                    .iter()
                    .map(|flag| flag.as_str().to_string())
                    .collect()
            }),
            error_fingerprint: error.map(fingerprint),
            error_summary: error.map(|_| category.summary.to_string()),
            resident_memory_bytes: snapshot.resident_memory_bytes,
            private_commit_bytes: snapshot.private_commit_bytes,
            peak_private_commit_bytes: snapshot.peak_private_commit_bytes,
            dropped_events: inner.dropped_events.swap(0, Ordering::Relaxed),
        };
        if inner.sender.try_send(event).is_err() {
            inner.dropped_events.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn bounded(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn fingerprint(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn salted_fingerprint(salt: &str, value: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(salt.as_bytes());
    digest.update([0]);
    digest.update(value.as_bytes());
    format!("{:x}", digest.finalize())
}

fn safe_tool_name(value: &str) -> String {
    const BUILTIN_TOOLS: &[&str] = &[
        "apply_patch",
        "edit_file",
        "exec",
        "exec_command",
        "glob_files",
        "grep_files",
        "followup_task",
        "interrupt_agent",
        "list_agents",
        "read_file",
        "send_message",
        "shell",
        "spawn_agent",
        "wait_agent",
        "write_file",
        "write_stdin",
    ];
    if BUILTIN_TOOLS.contains(&value) {
        value.to_string()
    } else {
        format!("external:{}", &fingerprint(value)[..16])
    }
}

fn safe_namespace(value: &str) -> String {
    if value.is_empty() || matches!(value, "functions" | "collaboration" | "multi_agent_v1") {
        if value.is_empty() {
            "functions".to_string()
        } else {
            value.to_string()
        }
    } else {
        format!("external:{}", &fingerprint(value)[..16])
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)?;
    Ok(format!("{:x}", digest.finalize()))
}

fn writer_loop(
    directory: &Path,
    mut output: EventFile,
    receiver: mpsc::Receiver<CompatibilityEvent>,
    dropped_events: Arc<AtomicU64>,
    context: CompatibilityDiagnosticsContext,
    retention_days: u16,
    max_total_bytes: u64,
) {
    loop {
        match receiver.recv_timeout(RESOURCE_SAMPLE_INTERVAL) {
            Ok(event) => {
                let flush = event.outcome != "success";
                if output.write_event(&event, flush).is_err() {
                    dropped_events.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let snapshot = crate::snapshot().process;
                let event = CompatibilityEvent {
                    schema_version: SCHEMA_VERSION,
                    timestamp: Utc::now().to_rfc3339(),
                    process_id: snapshot.id,
                    app_version: bounded(&context.app_version, 64),
                    build_commit: context
                        .build_commit
                        .as_deref()
                        .map(|value| bounded(value, 64)),
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                    provider: bounded(&context.provider, 128),
                    model: bounded(&context.model, 128),
                    session_hash: fingerprint(&context.session_id),
                    phase: "process.sample".to_string(),
                    category: "process.resource_sample".to_string(),
                    outcome: "success".to_string(),
                    tool_name: None,
                    tool_namespace: None,
                    representation: "none".to_string(),
                    duration_ms: 0,
                    input_bytes: 0,
                    output_bytes: 0,
                    metrics: None,
                    text_fingerprint: None,
                    text_char_count: None,
                    integrity_flags: Vec::new(),
                    error_fingerprint: None,
                    error_summary: None,
                    resident_memory_bytes: snapshot.resident_memory_bytes,
                    private_commit_bytes: snapshot.private_commit_bytes,
                    peak_private_commit_bytes: snapshot.peak_private_commit_bytes,
                    dropped_events: dropped_events.swap(0, Ordering::Relaxed),
                };
                let _ = output.write_event(&event, true);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = output.writer.flush();
                break;
            }
        }
        if output.bytes_written >= MAX_FILE_BYTES
            && let Ok(next) = EventFile::create(directory)
        {
            output = next;
            cleanup_directory(directory, retention_days, max_total_bytes);
        }
    }
}

struct EventFile {
    writer: BufWriter<File>,
    bytes_written: u64,
}

impl EventFile {
    fn create(directory: &Path) -> std::io::Result<Self> {
        static FILE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
        for _ in 0..100 {
            let sequence = FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                "compat-{}-{}-{sequence}.jsonl",
                now_millis(),
                std::process::id()
            ));
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(file) => {
                    return Ok(Self {
                        writer: BufWriter::new(file),
                        bytes_written: 0,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique compatibility diagnostics file",
        ))
    }

    fn write_event(&mut self, event: &CompatibilityEvent, flush: bool) -> std::io::Result<()> {
        let encoded = serde_json::to_vec(event)?;
        if encoded.len() > MAX_EVENT_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "compatibility event exceeded its size limit",
            ));
        }
        self.writer.write_all(&encoded)?;
        self.writer.write_all(b"\n")?;
        self.bytes_written = self
            .bytes_written
            .saturating_add(u64::try_from(encoded.len() + 1).unwrap_or(u64::MAX));
        if flush {
            self.writer.flush()?;
        }
        Ok(())
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn cleanup_directory(directory: &Path, retention_days: u16, max_total_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            let modified = metadata.modified().ok()?;
            Some((path, metadata.len(), modified))
        })
        .collect::<Vec<_>>();
    files.sort_unstable_by_key(|(_, _, modified)| *modified);
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(u64::from(retention_days) * 86_400))
        .unwrap_or(UNIX_EPOCH);
    for (path, _, modified) in &files {
        if *modified < cutoff {
            let _ = std::fs::remove_file(path);
        }
    }
    let mut retained = files
        .into_iter()
        .filter(|(path, _, modified)| *modified >= cutoff && path.exists())
        .collect::<Vec<_>>();
    let mut total = retained.iter().map(|(_, bytes, _)| *bytes).sum::<u64>();
    for (path, bytes, _) in retained.drain(..) {
        if total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(bytes);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompatibilityDiagnosticsStatus {
    pub directory: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub newest_event: Option<String>,
}

pub fn compatibility_diagnostics_status(
    directory: &Path,
) -> std::io::Result<CompatibilityDiagnosticsStatus> {
    let mut files = 0;
    let mut bytes = 0_u64;
    let mut newest_event: Option<String> = None;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.path().extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata()?;
        files += 1;
        bytes = bytes.saturating_add(metadata.len());
        if let Ok(modified) = metadata.modified() {
            let timestamp: DateTime<Utc> = modified.into();
            let timestamp = timestamp.to_rfc3339();
            if newest_event
                .as_ref()
                .is_none_or(|current| timestamp > *current)
            {
                newest_event = Some(timestamp);
            }
        }
    }
    Ok(CompatibilityDiagnosticsStatus {
        directory: directory.to_path_buf(),
        files,
        bytes,
        newest_event,
    })
}

pub struct CompatibilityReportOptions<'a> {
    pub directory: &'a Path,
    pub output: &'a Path,
    pub since: SystemTime,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompatibilityReportSummary {
    pub schema_version: u16,
    pub generated_at: String,
    pub total_events: u64,
    pub failed_events: u64,
    pub sampled_failures: u64,
    pub malformed_lines: u64,
    pub dropped_events: u64,
    pub max_resident_memory_bytes: Option<u64>,
    pub max_private_commit_bytes: Option<u64>,
    pub peak_private_commit_bytes: Option<u64>,
    pub categories: BTreeMap<String, u64>,
    pub metrics: CompatibilityMetrics,
}

pub fn build_compatibility_report(
    options: CompatibilityReportOptions<'_>,
) -> Result<CompatibilityReportSummary, String> {
    let mut summary = CompatibilityReportSummary {
        schema_version: SCHEMA_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        total_events: 0,
        failed_events: 0,
        sampled_failures: 0,
        malformed_lines: 0,
        dropped_events: 0,
        max_resident_memory_bytes: None,
        max_private_commit_bytes: None,
        peak_private_commit_bytes: None,
        categories: BTreeMap::new(),
        metrics: CompatibilityMetrics::default(),
    };
    let mut samples = BTreeMap::<String, Vec<CompatibilityEvent>>::new();
    let mut sample_count = 0_usize;
    let entries = std::fs::read_dir(options.directory).map_err(|error| error.to_string())?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        if entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < options.since)
        {
            continue;
        }
        let Ok(file) = File::open(path) else {
            continue;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else {
                summary.malformed_lines += 1;
                continue;
            };
            if line.len() > MAX_EVENT_BYTES {
                summary.malformed_lines += 1;
                continue;
            }
            let Ok(event) = serde_json::from_str::<CompatibilityEvent>(&line) else {
                summary.malformed_lines += 1;
                continue;
            };
            let Ok(timestamp) = DateTime::parse_from_rfc3339(&event.timestamp) else {
                summary.malformed_lines += 1;
                continue;
            };
            if SystemTime::from(timestamp.with_timezone(&Utc)) < options.since {
                continue;
            }
            summary.total_events += 1;
            summary.dropped_events = summary.dropped_events.saturating_add(event.dropped_events);
            summary.max_resident_memory_bytes = maximum(
                summary.max_resident_memory_bytes,
                event.resident_memory_bytes,
            );
            summary.max_private_commit_bytes =
                maximum(summary.max_private_commit_bytes, event.private_commit_bytes);
            summary.peak_private_commit_bytes = maximum(
                summary.peak_private_commit_bytes,
                event.peak_private_commit_bytes,
            );
            *summary
                .categories
                .entry(event.category.clone())
                .or_default() += 1;
            if let Some(metrics) = &event.metrics {
                summary.metrics.add_assign(metrics);
            }
            if event.outcome != "success" {
                summary.failed_events += 1;
                if sample_count < MAX_REPORT_SAMPLES {
                    let key = event
                        .error_fingerprint
                        .clone()
                        .unwrap_or_else(|| event.category.clone());
                    let category_samples = samples.entry(key).or_default();
                    if category_samples.len() < MAX_SAMPLES_PER_FINGERPRINT {
                        category_samples.push(event);
                        sample_count += 1;
                    }
                }
            }
        }
    }
    summary.sampled_failures = u64::try_from(sample_count).unwrap_or(u64::MAX);

    let summary_json = serde_json::to_vec_pretty(&summary).map_err(|error| error.to_string())?;
    let summary_markdown = render_markdown(&summary);
    let sample_events = samples.into_values().flatten().collect::<Vec<_>>();
    let mut samples_jsonl = Vec::new();
    for sample in &sample_events {
        serde_json::to_writer(&mut samples_jsonl, sample).map_err(|error| error.to_string())?;
        samples_jsonl.push(b'\n');
    }
    let known_issues =
        serde_json::to_vec_pretty(&known_issues()).map_err(|error| error.to_string())?;
    let checksums = [
        ("summary.json", summary_json.as_slice()),
        ("summary.md", summary_markdown.as_bytes()),
        ("samples.jsonl", samples_jsonl.as_slice()),
        ("known-issues.json", known_issues.as_slice()),
    ]
    .into_iter()
    .map(|(name, contents)| format!("{}  {name}", fingerprint_bytes(contents)))
    .collect::<Vec<_>>()
    .join("\n")
        + "\n";
    let manifest = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "generated_at": summary.generated_at,
        "source_directory_hash": fingerprint(&options.directory.to_string_lossy()),
        "files": ["summary.json", "summary.md", "samples.jsonl", "known-issues.json", "SHA256SUMS"],
    }))
    .map_err(|error| error.to_string())?;
    let files = [
        ("manifest.json", manifest.as_slice()),
        ("summary.json", summary_json.as_slice()),
        ("summary.md", summary_markdown.as_bytes()),
        ("samples.jsonl", samples_jsonl.as_slice()),
        ("known-issues.json", known_issues.as_slice()),
        ("SHA256SUMS", checksums.as_bytes()),
    ];
    let projected_bytes = files.iter().map(|(_, bytes)| bytes.len()).sum::<usize>();
    if projected_bytes > MAX_REPORT_BYTES {
        return Err("compatibility report exceeded the 10 MiB limit".to_string());
    }
    write_zip(options.output, &files)?;
    Ok(summary)
}

fn maximum(current: Option<u64>, candidate: Option<u64>) -> Option<u64> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (Some(current), None) => Some(current),
        (None, candidate) => candidate,
    }
}

fn fingerprint_bytes(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn render_markdown(summary: &CompatibilityReportSummary) -> String {
    let mut output = format!(
        "# Codex compatibility diagnostics\n\nGenerated: {}\n\nTotal events: {}\n\nFailed events: {}\n\nSampled failures: {}\n\nMalformed lines: {}\n\nDropped events: {}\n\nMaximum resident memory: {}\n\nMaximum Windows private commit: {}\n\nPeak Windows private commit: {}\n\n## Token efficiency\n\n| Metric | Total |\n|---|---:|\n| History bytes | {} |\n| Tool schema bytes | {} |\n| Instruction bytes | {} |\n| Input tokens | {} |\n| Cached input tokens | {} |\n| Output tokens | {} |\n| Reasoning output tokens | {} |\n\n## Categories\n\n| Category | Count |\n|---|---:|\n",
        summary.generated_at,
        summary.total_events,
        summary.failed_events,
        summary.sampled_failures,
        summary.malformed_lines,
        summary.dropped_events,
        display_bytes(summary.max_resident_memory_bytes),
        display_bytes(summary.max_private_commit_bytes),
        display_bytes(summary.peak_private_commit_bytes),
        summary.metrics.history_bytes,
        summary.metrics.tool_schema_bytes,
        summary.metrics.instruction_bytes,
        summary.metrics.input_tokens,
        summary.metrics.cached_input_tokens,
        summary.metrics.output_tokens,
        summary.metrics.reasoning_output_tokens,
    );
    for (category, count) in &summary.categories {
        output.push_str(&format!("| `{category}` | {count} |\n"));
    }
    output
}

fn display_bytes(value: Option<u64>) -> String {
    value.map_or_else(|| "unavailable".to_string(), |value| value.to_string())
}

fn write_zip(output: &Path, files: &[(&str, &[u8])]) -> Result<(), String> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = File::create(output).map_err(|error| error.to_string())?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (name, contents) in files {
        zip.start_file(*name, options)
            .map_err(|error| error.to_string())?;
        zip.write_all(contents).map_err(|error| error.to_string())?;
    }
    zip.finish().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "compatibility_tests.rs"]
mod tests;
