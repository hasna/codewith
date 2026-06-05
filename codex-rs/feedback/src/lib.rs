use std::collections::BTreeMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::collections::btree_map::Entry;
use std::fs;
use std::io::Write;
use std::io::{self};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use codex_login::AuthEnvTelemetry;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SessionSource;
use tracing::Event;
use tracing::Level;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::writer::MakeWriter;
use tracing_subscriber::registry::LookupSpan;

pub(crate) mod feedback_diagnostics;
pub use feedback_diagnostics::FEEDBACK_DIAGNOSTICS_ATTACHMENT_FILENAME;
pub use feedback_diagnostics::FeedbackDiagnostic;
pub use feedback_diagnostics::FeedbackDiagnostics;

/// Filename used for the redacted `codex doctor --json` feedback attachment.
pub const DOCTOR_REPORT_ATTACHMENT_FILENAME: &str = "codex-doctor-report.json";
/// Filename used for the Windows sandbox log feedback attachment.
pub const WINDOWS_SANDBOX_LOG_ATTACHMENT_FILENAME: &str = "windows-sandbox.log";
const DEFAULT_MAX_BYTES: usize = 4 * 1024 * 1024; // 4 MiB
const FEEDBACK_TAGS_TARGET: &str = "feedback_tags";
const FEEDBACK_REPORT_ROOT: &str = "codewith-feedback";
const FEEDBACK_REPORT_METADATA_FILENAME: &str = "metadata.txt";
const FEEDBACK_REPORT_ATTACHMENTS_DIR: &str = "attachments";
const MAX_FEEDBACK_TAGS: usize = 64;

/// Structured request/auth fields that should be attached to feedback reports.
pub struct FeedbackRequestTags<'a> {
    pub endpoint: &'a str,
    pub auth_header_attached: bool,
    pub auth_header_name: Option<&'a str>,
    pub auth_mode: Option<&'a str>,
    pub auth_retry_after_unauthorized: Option<bool>,
    pub auth_recovery_mode: Option<&'a str>,
    pub auth_recovery_phase: Option<&'a str>,
    pub auth_connection_reused: Option<bool>,
    pub auth_request_id: Option<&'a str>,
    pub auth_cf_ray: Option<&'a str>,
    pub auth_error: Option<&'a str>,
    pub auth_error_code: Option<&'a str>,
    pub auth_recovery_followup_success: Option<bool>,
    pub auth_recovery_followup_status: Option<u16>,
}

struct FeedbackRequestSnapshot<'a> {
    endpoint: &'a str,
    auth_header_attached: bool,
    auth_header_name: &'a str,
    auth_mode: &'a str,
    auth_retry_after_unauthorized: String,
    auth_recovery_mode: &'a str,
    auth_recovery_phase: &'a str,
    auth_connection_reused: String,
    auth_request_id: &'a str,
    auth_cf_ray: &'a str,
    auth_error: &'a str,
    auth_error_code: &'a str,
    auth_recovery_followup_success: String,
    auth_recovery_followup_status: String,
}

impl<'a> FeedbackRequestSnapshot<'a> {
    fn from_tags(tags: &'a FeedbackRequestTags<'a>) -> Self {
        Self {
            endpoint: tags.endpoint,
            auth_header_attached: tags.auth_header_attached,
            auth_header_name: tags.auth_header_name.unwrap_or(""),
            auth_mode: tags.auth_mode.unwrap_or(""),
            auth_retry_after_unauthorized: tags
                .auth_retry_after_unauthorized
                .map_or_else(String::new, |value| value.to_string()),
            auth_recovery_mode: tags.auth_recovery_mode.unwrap_or(""),
            auth_recovery_phase: tags.auth_recovery_phase.unwrap_or(""),
            auth_connection_reused: tags
                .auth_connection_reused
                .map_or_else(String::new, |value| value.to_string()),
            auth_request_id: tags.auth_request_id.unwrap_or(""),
            auth_cf_ray: tags.auth_cf_ray.unwrap_or(""),
            auth_error: tags.auth_error.unwrap_or(""),
            auth_error_code: tags.auth_error_code.unwrap_or(""),
            auth_recovery_followup_success: tags
                .auth_recovery_followup_success
                .map_or_else(String::new, |value| value.to_string()),
            auth_recovery_followup_status: tags
                .auth_recovery_followup_status
                .map_or_else(String::new, |value| value.to_string()),
        }
    }
}

pub fn emit_feedback_request_tags(tags: &FeedbackRequestTags<'_>) {
    let snapshot = FeedbackRequestSnapshot::from_tags(tags);
    tracing::info!(
        target: FEEDBACK_TAGS_TARGET,
        endpoint = tracing::field::debug(snapshot.endpoint),
        auth_header_attached = tracing::field::debug(snapshot.auth_header_attached),
        auth_header_name = tracing::field::debug(snapshot.auth_header_name),
        auth_mode = tracing::field::debug(snapshot.auth_mode),
        auth_retry_after_unauthorized = tracing::field::debug(&snapshot.auth_retry_after_unauthorized),
        auth_recovery_mode = tracing::field::debug(snapshot.auth_recovery_mode),
        auth_recovery_phase = tracing::field::debug(snapshot.auth_recovery_phase),
        auth_connection_reused = tracing::field::debug(&snapshot.auth_connection_reused),
        auth_request_id = tracing::field::debug(snapshot.auth_request_id),
        auth_cf_ray = tracing::field::debug(snapshot.auth_cf_ray),
        auth_error = tracing::field::debug(snapshot.auth_error),
        auth_error_code = tracing::field::debug(snapshot.auth_error_code),
        auth_recovery_followup_success = tracing::field::debug(&snapshot.auth_recovery_followup_success),
        auth_recovery_followup_status = tracing::field::debug(&snapshot.auth_recovery_followup_status),
    );
}

pub fn emit_feedback_request_tags_with_auth_env(
    tags: &FeedbackRequestTags<'_>,
    auth_env: &AuthEnvTelemetry,
) {
    let snapshot = FeedbackRequestSnapshot::from_tags(tags);
    tracing::info!(
        target: FEEDBACK_TAGS_TARGET,
        endpoint = tracing::field::debug(snapshot.endpoint),
        auth_header_attached = tracing::field::debug(snapshot.auth_header_attached),
        auth_header_name = tracing::field::debug(snapshot.auth_header_name),
        auth_mode = tracing::field::debug(snapshot.auth_mode),
        auth_retry_after_unauthorized = tracing::field::debug(&snapshot.auth_retry_after_unauthorized),
        auth_recovery_mode = tracing::field::debug(snapshot.auth_recovery_mode),
        auth_recovery_phase = tracing::field::debug(snapshot.auth_recovery_phase),
        auth_connection_reused = tracing::field::debug(&snapshot.auth_connection_reused),
        auth_request_id = tracing::field::debug(snapshot.auth_request_id),
        auth_cf_ray = tracing::field::debug(snapshot.auth_cf_ray),
        auth_error = tracing::field::debug(snapshot.auth_error),
        auth_error_code = tracing::field::debug(snapshot.auth_error_code),
        auth_recovery_followup_success = tracing::field::debug(&snapshot.auth_recovery_followup_success),
        auth_recovery_followup_status = tracing::field::debug(&snapshot.auth_recovery_followup_status),
        auth_env_openai_api_key_present = tracing::field::debug(auth_env.openai_api_key_env_present),
        auth_env_codex_api_key_present = tracing::field::debug(auth_env.codex_api_key_env_present),
        auth_env_codex_api_key_enabled = tracing::field::debug(auth_env.codex_api_key_env_enabled),
        // Custom provider `env_key` is arbitrary config text, so emit only a safe bucket.
        auth_env_provider_key_name = tracing::field::debug(
            auth_env.provider_env_key_name.as_deref().unwrap_or("")
        ),
        auth_env_provider_key_present = tracing::field::debug(
            &auth_env.provider_env_key_present.map_or_else(String::new, |value| value.to_string())
        ),
        auth_env_refresh_token_url_override_present = tracing::field::debug(
            auth_env.refresh_token_url_override_present
        ),
    );
}

#[derive(Clone)]
pub struct CodexFeedback {
    inner: Arc<FeedbackInner>,
}

impl Default for CodexFeedback {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexFeedback {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_BYTES)
    }

    pub(crate) fn with_capacity(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(FeedbackInner::new(max_bytes)),
        }
    }

    pub fn make_writer(&self) -> FeedbackMakeWriter {
        FeedbackMakeWriter {
            inner: self.inner.clone(),
        }
    }

    /// Returns a [`tracing_subscriber`] layer that captures full-fidelity logs into this feedback
    /// ring buffer.
    ///
    /// This is intended for initialization code so call sites don't have to duplicate the exact
    /// `fmt::layer()` configuration and filter logic.
    pub fn logger_layer<S>(&self) -> impl Layer<S> + Send + Sync + 'static
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        tracing_subscriber::fmt::layer()
            .with_writer(self.make_writer())
            .with_timer(tracing_subscriber::fmt::time::SystemTime)
            .with_ansi(false)
            .with_target(false)
            // Capture everything, regardless of the caller's `RUST_LOG`, so feedback includes the
            // full trace when the user prepares a report.
            .with_filter(Targets::new().with_default(Level::TRACE))
    }

    /// Returns a [`tracing_subscriber`] layer that collects structured metadata for feedback.
    ///
    /// Events with `target: "feedback_tags"` are treated as key/value tags to attach to feedback
    /// reports later.
    pub fn metadata_layer<S>(&self) -> impl Layer<S> + Send + Sync + 'static
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        FeedbackMetadataLayer {
            inner: self.inner.clone(),
        }
        .with_filter(Targets::new().with_target(FEEDBACK_TAGS_TARGET, Level::TRACE))
    }

    pub fn snapshot(&self, session_id: Option<ThreadId>) -> FeedbackSnapshot {
        let bytes = {
            #[allow(clippy::expect_used)]
            let guard = self.inner.ring.lock().expect("mutex poisoned");
            guard.snapshot_bytes()
        };
        let tags = {
            #[allow(clippy::expect_used)]
            let guard = self.inner.tags.lock().expect("mutex poisoned");
            guard.clone()
        };
        FeedbackSnapshot {
            bytes,
            tags,
            feedback_diagnostics: FeedbackDiagnostics::collect_from_env(),
            thread_id: session_id
                .map(|id| id.to_string())
                .unwrap_or("no-active-thread-".to_string() + &ThreadId::new().to_string()),
        }
    }
}

struct FeedbackInner {
    ring: Mutex<RingBuffer>,
    tags: Mutex<BTreeMap<String, String>>,
}

impl FeedbackInner {
    fn new(max_bytes: usize) -> Self {
        Self {
            ring: Mutex::new(RingBuffer::new(max_bytes)),
            tags: Mutex::new(BTreeMap::new()),
        }
    }
}

#[derive(Clone)]
pub struct FeedbackMakeWriter {
    inner: Arc<FeedbackInner>,
}

impl<'a> MakeWriter<'a> for FeedbackMakeWriter {
    type Writer = FeedbackWriter;

    fn make_writer(&'a self) -> Self::Writer {
        FeedbackWriter {
            inner: self.inner.clone(),
        }
    }
}

pub struct FeedbackWriter {
    inner: Arc<FeedbackInner>,
}

impl Write for FeedbackWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.inner.ring.lock().map_err(|_| io::ErrorKind::Other)?;
        guard.push_bytes(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct RingBuffer {
    max: usize,
    buf: VecDeque<u8>,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            max: capacity,
            buf: VecDeque::with_capacity(capacity),
        }
    }

    fn len(&self) -> usize {
        self.buf.len()
    }

    fn push_bytes(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // If the incoming chunk is larger than capacity, keep only the trailing bytes.
        if data.len() >= self.max {
            self.buf.clear();
            let start = data.len() - self.max;
            self.buf.extend(data[start..].iter().copied());
            return;
        }

        // Evict from the front if we would exceed capacity.
        let needed = self.len() + data.len();
        if needed > self.max {
            let to_drop = needed - self.max;
            for _ in 0..to_drop {
                let _ = self.buf.pop_front();
            }
        }

        self.buf.extend(data.iter().copied());
    }

    fn snapshot_bytes(&self) -> Vec<u8> {
        self.buf.iter().copied().collect()
    }
}

pub struct FeedbackSnapshot {
    bytes: Vec<u8>,
    tags: BTreeMap<String, String>,
    feedback_diagnostics: FeedbackDiagnostics,
    pub thread_id: String,
}

pub struct FeedbackAttachmentPath {
    pub path: PathBuf,
    /// Optional filename to use for the prepared attachment instead of `path`'s basename.
    pub attachment_filename_override: Option<String>,
}

/// In-memory attachment to include in a feedback report.
///
/// Use this for generated diagnostics that should not be materialized on disk,
/// such as the redacted doctor report. File-backed artifacts should use
/// `FeedbackAttachmentPath` so preparation-time read failures can be logged and
/// skipped independently.
pub struct FeedbackAttachment {
    /// Attachment filename shown in the feedback consent UI and prepared report.
    pub filename: String,
    /// Optional MIME type for consumers that render or classify attachments.
    pub content_type: Option<String>,
    /// Attachment bytes captured before feedback preparation starts.
    pub buffer: Vec<u8>,
}

/// Inputs that control one feedback report.
///
/// The caller is responsible for applying any user-consent gate before setting
/// `include_logs` or passing diagnostic attachments. This type only describes
/// what to prepare once that decision has been made.
pub struct FeedbackUploadOptions<'a> {
    pub classification: &'a str,
    pub reason: Option<&'a str>,
    pub tags: Option<&'a BTreeMap<String, String>>,
    pub include_logs: bool,
    /// Generated attachments that are already buffered and safe to include.
    ///
    /// These are included after `codex-logs.log` and before path-backed rollout
    /// attachments. They are only passed by the caller after any user consent
    /// gate has decided logs and diagnostics should be included.
    pub extra_attachments: &'a [FeedbackAttachment],
    pub extra_attachment_paths: &'a [FeedbackAttachmentPath],
    pub session_source: Option<SessionSource>,
    pub logs_override: Option<Vec<u8>>,
}

struct PreparedFeedbackAttachment {
    buffer: Vec<u8>,
    filename: String,
    content_type: Option<String>,
}

struct WrittenFeedbackAttachment {
    filename: String,
    content_type: Option<String>,
    bytes: usize,
}

pub fn feedback_report_dir(thread_id: &str) -> PathBuf {
    std::env::temp_dir()
        .join(FEEDBACK_REPORT_ROOT)
        .join(sanitize_path_component(thread_id, "thread"))
}

impl FeedbackSnapshot {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn feedback_diagnostics(&self) -> &FeedbackDiagnostics {
        &self.feedback_diagnostics
    }

    pub fn with_feedback_diagnostics(mut self, feedback_diagnostics: FeedbackDiagnostics) -> Self {
        self.feedback_diagnostics = feedback_diagnostics;
        self
    }

    pub fn feedback_diagnostics_attachment_text(&self, include_logs: bool) -> Option<String> {
        if !include_logs {
            return None;
        }

        self.feedback_diagnostics.attachment_text()
    }

    pub fn save_to_temp_file(&self) -> io::Result<PathBuf> {
        let dir = std::env::temp_dir();
        let filename = format!("codex-feedback-{}.log", self.thread_id);
        let path = dir.join(filename);
        fs::write(&path, self.as_bytes())?;
        Ok(path)
    }

    /// Prepare feedback metadata and attachments for the Codewith feedback flow.
    pub fn upload_feedback(&self, options: FeedbackUploadOptions<'_>) -> Result<()> {
        let tags = self.upload_tags(
            options.classification,
            options.reason,
            options.tags,
            options.session_source.as_ref(),
        );
        let attachments = self.feedback_attachments(
            options.include_logs,
            options.extra_attachments,
            options.extra_attachment_paths,
            options.logs_override,
        );
        let report_dir = self.write_prepared_report(&tags, options.include_logs, &attachments)?;
        tracing::info!(
            target: FEEDBACK_TAGS_TARGET,
            feedback_destination = "hasna/codewith",
            thread_id = %self.thread_id,
            classification = options.classification,
            include_logs = options.include_logs,
            report_dir = %report_dir.display(),
            attachment_count = attachments.len(),
            tag_count = tags.len(),
            "prepared Codewith feedback report"
        );
        Ok(())
    }

    fn upload_tags(
        &self,
        classification: &str,
        reason: Option<&str>,
        client_tags: Option<&BTreeMap<String, String>>,
        session_source: Option<&SessionSource>,
    ) -> BTreeMap<String, String> {
        let cli_version = env!("CARGO_PKG_VERSION");
        let mut tags = BTreeMap::from([
            (String::from("thread_id"), self.thread_id.to_string()),
            (String::from("classification"), classification.to_string()),
            (String::from("cli_version"), cli_version.to_string()),
        ]);
        if let Some(source) = session_source {
            tags.insert(String::from("session_source"), source.to_string());
        }
        if let Some(r) = reason {
            tags.insert(String::from("reason"), r.to_string());
        }

        let reserved = [
            "thread_id",
            "classification",
            "cli_version",
            "session_source",
            "reason",
        ];
        if let Some(client_tags) = client_tags {
            for (key, value) in client_tags {
                if reserved.contains(&key.as_str()) {
                    continue;
                }
                if let Entry::Vacant(entry) = tags.entry(key.clone()) {
                    entry.insert(value.clone());
                }
            }
        }
        for (key, value) in &self.tags {
            if reserved.contains(&key.as_str()) {
                continue;
            }
            if let Entry::Vacant(entry) = tags.entry(key.clone()) {
                entry.insert(value.clone());
            }
        }

        tags
    }

    fn feedback_attachments(
        &self,
        include_logs: bool,
        extra_attachments: &[FeedbackAttachment],
        extra_attachment_paths: &[FeedbackAttachmentPath],
        logs_override: Option<Vec<u8>>,
    ) -> Vec<PreparedFeedbackAttachment> {
        let mut attachments = Vec::new();

        if include_logs {
            attachments.push(PreparedFeedbackAttachment {
                buffer: logs_override.unwrap_or_else(|| self.bytes.clone()),
                filename: String::from("codex-logs.log"),
                content_type: Some("text/plain".to_string()),
            });
        }

        attachments.extend(
            extra_attachments
                .iter()
                .map(|attachment| PreparedFeedbackAttachment {
                    buffer: attachment.buffer.clone(),
                    filename: attachment.filename.clone(),
                    content_type: attachment.content_type.clone(),
                }),
        );

        if let Some(text) = self.feedback_diagnostics_attachment_text(include_logs) {
            attachments.push(PreparedFeedbackAttachment {
                buffer: text.into_bytes(),
                filename: FEEDBACK_DIAGNOSTICS_ATTACHMENT_FILENAME.to_string(),
                content_type: Some("text/plain".to_string()),
            });
        }

        for attachment_path in extra_attachment_paths {
            let data = match fs::read(&attachment_path.path) {
                Ok(data) => data,
                Err(err) => {
                    tracing::warn!(
                        path = %attachment_path.path.display(),
                        error = %err,
                        "failed to read feedback attachment; skipping"
                    );
                    continue;
                }
            };
            let filename = attachment_path
                .attachment_filename_override
                .clone()
                .unwrap_or_else(|| {
                    attachment_path
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "extra-log.log".to_string())
                });
            attachments.push(PreparedFeedbackAttachment {
                buffer: data,
                filename,
                content_type: Some("text/plain".to_string()),
            });
        }

        attachments
    }

    fn write_prepared_report(
        &self,
        tags: &BTreeMap<String, String>,
        include_logs: bool,
        attachments: &[PreparedFeedbackAttachment],
    ) -> Result<PathBuf> {
        let report_dir = feedback_report_dir(&self.thread_id);
        let attachments_dir = report_dir.join(FEEDBACK_REPORT_ATTACHMENTS_DIR);

        if let Some(report_root) = report_dir.parent() {
            ensure_directory(report_root).with_context(|| {
                format!(
                    "failed to create feedback report root {}",
                    report_root.display()
                )
            })?;
        }
        ensure_directory(&report_dir).with_context(|| {
            format!(
                "failed to create feedback report directory {}",
                report_dir.display()
            )
        })?;
        reset_directory(&attachments_dir).with_context(|| {
            format!(
                "failed to reset feedback attachments directory {}",
                attachments_dir.display()
            )
        })?;

        let written_attachments = write_feedback_attachments(&attachments_dir, attachments)
            .with_context(|| {
                format!(
                    "failed to write feedback attachments to {}",
                    attachments_dir.display()
                )
            })?;
        let metadata = feedback_report_metadata(tags, include_logs, &written_attachments);
        write_private_file(
            report_dir.join(FEEDBACK_REPORT_METADATA_FILENAME),
            metadata.as_bytes(),
        )
        .with_context(|| {
            format!(
                "failed to write feedback metadata in {}",
                report_dir.display()
            )
        })?;

        Ok(report_dir)
    }
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            fs::remove_file(path)?;
        }
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::create_dir_all(path)?;
    restrict_directory_permissions(path)
}

fn reset_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            fs::remove_file(path)?;
        }
        Ok(_) => {
            fs::remove_dir_all(path)?;
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    fs::create_dir_all(path)?;
    restrict_directory_permissions(path)
}

fn write_feedback_attachments(
    attachments_dir: &Path,
    attachments: &[PreparedFeedbackAttachment],
) -> io::Result<Vec<WrittenFeedbackAttachment>> {
    let mut used_filenames = HashSet::new();
    let mut written_attachments = Vec::new();

    for (index, attachment) in attachments.iter().enumerate() {
        let filename = unique_attachment_filename(
            &attachment.filename,
            &mut used_filenames,
            /*index*/ index + 1,
        );
        write_private_file(attachments_dir.join(&filename), &attachment.buffer)?;
        written_attachments.push(WrittenFeedbackAttachment {
            filename,
            content_type: attachment.content_type.clone(),
            bytes: attachment.buffer.len(),
        });
    }

    Ok(written_attachments)
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn write_private_file(path: impl AsRef<Path>, buffer: &[u8]) -> io::Result<()> {
    let path = path.as_ref();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::fs::PermissionsExt;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(buffer)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, buffer)
    }
}

fn feedback_report_metadata(
    tags: &BTreeMap<String, String>,
    include_logs: bool,
    attachments: &[WrittenFeedbackAttachment],
) -> String {
    let mut metadata = String::new();
    metadata.push_str("Codewith feedback report\n");
    metadata.push_str("Destination: hasna/codewith\n");
    metadata.push_str(&format!("Include logs: {include_logs}\n"));
    metadata.push_str("\nTags:\n");
    for (key, value) in tags {
        metadata.push_str(&format!("- {key}: {value}\n"));
    }

    metadata.push_str("\nAttachments:\n");
    if attachments.is_empty() {
        metadata.push_str("- none\n");
    } else {
        for attachment in attachments {
            metadata.push_str(&format!(
                "- {}/{}",
                FEEDBACK_REPORT_ATTACHMENTS_DIR, attachment.filename
            ));
            if let Some(content_type) = attachment.content_type.as_deref() {
                metadata.push_str(&format!(" ({content_type}, {} bytes)", attachment.bytes));
            } else {
                metadata.push_str(&format!(" ({} bytes)", attachment.bytes));
            }
            metadata.push('\n');
        }
    }

    metadata
}

fn unique_attachment_filename(
    filename: &str,
    used_filenames: &mut HashSet<String>,
    index: usize,
) -> String {
    let raw_name = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(filename);
    let sanitized = sanitize_path_component(raw_name, &format!("attachment-{index}.log"));
    if used_filenames.insert(sanitized.clone()) {
        return sanitized;
    }

    let path = Path::new(&sanitized);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("attachment");
    let extension = path.extension().and_then(|extension| extension.to_str());
    for suffix in 2.. {
        let candidate = match extension {
            Some(extension) if !extension.is_empty() => format!("{stem}-{suffix}.{extension}"),
            _ => format!("{stem}-{suffix}"),
        };
        if used_filenames.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix search should always return");
}

fn sanitize_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.trim_matches('.').is_empty() {
        fallback.to_string()
    } else {
        sanitized
    }
}

#[derive(Clone)]
struct FeedbackMetadataLayer {
    inner: Arc<FeedbackInner>,
}

impl<S> Layer<S> for FeedbackMetadataLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // This layer is filtered by `Targets`, but keep the guard anyway in case it is used without
        // the filter.
        if event.metadata().target() != FEEDBACK_TAGS_TARGET {
            return;
        }

        let mut visitor = FeedbackTagsVisitor::default();
        event.record(&mut visitor);
        if visitor.tags.is_empty() {
            return;
        }

        #[allow(clippy::expect_used)]
        let mut guard = self.inner.tags.lock().expect("mutex poisoned");
        for (key, value) in visitor.tags {
            if guard.len() >= MAX_FEEDBACK_TAGS && !guard.contains_key(&key) {
                continue;
            }
            guard.insert(key, value);
        }
    }
}

#[derive(Default)]
struct FeedbackTagsVisitor {
    tags: BTreeMap<String, String>,
}

impl Visit for FeedbackTagsVisitor {
    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.tags
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use super::*;
    use crate::FeedbackDiagnostic;
    use pretty_assertions::assert_eq;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    #[test]
    fn ring_buffer_drops_front_when_full() {
        let fb = CodexFeedback::with_capacity(/*max_bytes*/ 8);
        {
            let mut w = fb.make_writer().make_writer();
            w.write_all(b"abcdefgh").unwrap();
            w.write_all(b"ij").unwrap();
        }
        let snap = fb.snapshot(/*session_id*/ None);
        // Capacity 8: after writing 10 bytes, we should keep the last 8.
        pretty_assertions::assert_eq!(std::str::from_utf8(snap.as_bytes()).unwrap(), "cdefghij");
    }

    #[test]
    fn metadata_layer_records_tags_from_feedback_target() {
        let fb = CodexFeedback::new();
        let _guard = tracing_subscriber::registry()
            .with(fb.metadata_layer())
            .set_default();

        tracing::info!(target: FEEDBACK_TAGS_TARGET, model = "gpt-5", cached = true, "tags");

        let snap = fb.snapshot(/*session_id*/ None);
        pretty_assertions::assert_eq!(snap.tags.get("model").map(String::as_str), Some("gpt-5"));
        pretty_assertions::assert_eq!(snap.tags.get("cached").map(String::as_str), Some("true"));
    }

    #[test]
    fn feedback_attachments_gate_connectivity_diagnostics() {
        let extra_filename = format!("codex-feedback-extra-{}.jsonl", ThreadId::new());
        let extra_path = std::env::temp_dir().join(&extra_filename);
        let extra_attachment_path = FeedbackAttachmentPath {
            path: extra_path.clone(),
            attachment_filename_override: None,
        };
        fs::write(&extra_path, "rollout").expect("extra attachment should be written");

        let snapshot_with_diagnostics = CodexFeedback::new()
            .snapshot(/*session_id*/ None)
            .with_feedback_diagnostics(FeedbackDiagnostics::new(vec![FeedbackDiagnostic {
                headline: "Proxy environment variables are set and may affect connectivity."
                    .to_string(),
                details: vec!["HTTPS_PROXY = https://example.com:443".to_string()],
            }]));

        let attachments_with_diagnostics = snapshot_with_diagnostics.feedback_attachments(
            /*include_logs*/ true,
            &[FeedbackAttachment {
                filename: DOCTOR_REPORT_ATTACHMENT_FILENAME.to_string(),
                content_type: Some("application/json".to_string()),
                buffer: b"{\"overallStatus\":\"ok\"}".to_vec(),
            }],
            std::slice::from_ref(&extra_attachment_path),
            Some(vec![1]),
        );

        assert_eq!(
            attachments_with_diagnostics
                .iter()
                .map(|attachment| attachment.filename.as_str())
                .collect::<Vec<_>>(),
            vec![
                "codex-logs.log",
                DOCTOR_REPORT_ATTACHMENT_FILENAME,
                FEEDBACK_DIAGNOSTICS_ATTACHMENT_FILENAME,
                extra_filename.as_str()
            ]
        );
        assert_eq!(attachments_with_diagnostics[0].buffer, vec![1]);
        assert_eq!(
            attachments_with_diagnostics[1].buffer,
            b"{\"overallStatus\":\"ok\"}".to_vec()
        );
        assert_eq!(
            attachments_with_diagnostics[2].buffer,
            b"Connectivity diagnostics\n\n- Proxy environment variables are set and may affect connectivity.\n  - HTTPS_PROXY = https://example.com:443".to_vec()
        );
        assert_eq!(attachments_with_diagnostics[3].buffer, b"rollout".to_vec());
        assert_eq!(
            OsStr::new(attachments_with_diagnostics[3].filename.as_str()),
            OsStr::new(extra_filename.as_str())
        );
        let attachments_without_diagnostics = CodexFeedback::new()
            .snapshot(/*session_id*/ None)
            .with_feedback_diagnostics(FeedbackDiagnostics::default())
            .feedback_attachments(/*include_logs*/ true, &[], &[], Some(vec![1]));

        assert_eq!(
            attachments_without_diagnostics
                .iter()
                .map(|attachment| attachment.filename.as_str())
                .collect::<Vec<_>>(),
            vec!["codex-logs.log"]
        );
        assert_eq!(attachments_without_diagnostics[0].buffer, vec![1]);
        fs::remove_file(extra_path).expect("extra attachment should be removed");
    }

    #[test]
    fn upload_tags_include_client_tags_and_preserve_reserved_fields() {
        let mut tags = BTreeMap::new();
        tags.insert("thread_id".to_string(), "wrong-thread".to_string());
        tags.insert("turn_id".to_string(), "wrong-turn".to_string());
        tags.insert(
            "classification".to_string(),
            "wrong-classification".to_string(),
        );
        tags.insert("cli_version".to_string(), "wrong-version".to_string());
        tags.insert("session_source".to_string(), "wrong-source".to_string());
        tags.insert("reason".to_string(), "wrong-reason".to_string());
        tags.insert("account_id".to_string(), "actual-account".to_string());
        tags.insert("model".to_string(), "gpt-5".to_string());
        let snapshot = FeedbackSnapshot {
            bytes: Vec::new(),
            tags,
            feedback_diagnostics: FeedbackDiagnostics::default(),
            thread_id: "thread-123".to_string(),
        };
        let mut client_tags = BTreeMap::new();
        client_tags.insert("thread_id".to_string(), "wrong-client-thread".to_string());
        client_tags.insert("turn_id".to_string(), "turn-456".to_string());
        client_tags.insert(
            "classification".to_string(),
            "wrong-client-classification".to_string(),
        );
        client_tags.insert(
            "cli_version".to_string(),
            "wrong-client-version".to_string(),
        );
        client_tags.insert(
            "session_source".to_string(),
            "wrong-client-source".to_string(),
        );
        client_tags.insert("reason".to_string(), "wrong-client-reason".to_string());
        client_tags.insert("client_tag".to_string(), "from-client".to_string());

        let upload_tags = snapshot.upload_tags(
            "bug",
            Some("actual reason"),
            Some(&client_tags),
            Some(&SessionSource::Cli),
        );

        assert_eq!(
            upload_tags.get("thread_id").map(String::as_str),
            Some("thread-123")
        );
        assert_eq!(
            upload_tags.get("turn_id").map(String::as_str),
            Some("turn-456")
        );
        assert_eq!(
            upload_tags.get("classification").map(String::as_str),
            Some("bug")
        );
        assert_eq!(
            upload_tags.get("session_source").map(String::as_str),
            Some("cli")
        );
        assert_eq!(
            upload_tags.get("reason").map(String::as_str),
            Some("actual reason")
        );
        assert_eq!(
            upload_tags.get("account_id").map(String::as_str),
            Some("actual-account")
        );
        assert_eq!(
            upload_tags.get("client_tag").map(String::as_str),
            Some("from-client")
        );
        assert_eq!(upload_tags.get("model").map(String::as_str), Some("gpt-5"));
    }

    #[test]
    fn upload_feedback_prepares_codewith_report_without_network_destination() {
        let snapshot = CodexFeedback::new().snapshot(/*session_id*/ None);
        let report_dir = feedback_report_dir(&snapshot.thread_id);
        let _ = fs::remove_dir_all(&report_dir);

        let result = snapshot.upload_feedback(FeedbackUploadOptions {
            classification: "bug",
            reason: Some("something broke"),
            tags: None,
            include_logs: false,
            extra_attachments: &[],
            extra_attachment_paths: &[],
            session_source: Some(SessionSource::Cli),
            logs_override: None,
        });

        assert!(result.is_ok());
        let metadata = fs::read_to_string(report_dir.join(FEEDBACK_REPORT_METADATA_FILENAME))
            .expect("metadata should be written");
        assert!(metadata.contains("Destination: hasna/codewith"));
        assert!(metadata.contains("Include logs: false"));
        assert!(metadata.contains("- classification: bug"));
        assert!(metadata.contains("- reason: something broke"));
        assert!(metadata.contains("- none"));
        fs::remove_dir_all(report_dir).expect("report directory should be removed");
    }

    #[test]
    fn upload_feedback_writes_local_report_with_consented_attachments() {
        let thread_id = ThreadId::new();
        let feedback = CodexFeedback::new();
        {
            let mut writer = feedback.make_writer().make_writer();
            writer
                .write_all(b"ring buffer logs")
                .expect("logs should be captured");
        }
        let snapshot = feedback.snapshot(Some(thread_id));
        let report_dir = feedback_report_dir(&snapshot.thread_id);
        let _ = fs::remove_dir_all(&report_dir);

        let extra_filename = format!("codex-feedback-extra-{}.jsonl", ThreadId::new());
        let extra_path = std::env::temp_dir().join(&extra_filename);
        fs::write(&extra_path, "rollout").expect("extra attachment should be written");

        snapshot
            .upload_feedback(FeedbackUploadOptions {
                classification: "bug",
                reason: Some("something broke"),
                tags: None,
                include_logs: true,
                extra_attachments: &[FeedbackAttachment {
                    filename: DOCTOR_REPORT_ATTACHMENT_FILENAME.to_string(),
                    content_type: Some("application/json".to_string()),
                    buffer: b"{\"overallStatus\":\"ok\"}".to_vec(),
                }],
                extra_attachment_paths: &[FeedbackAttachmentPath {
                    path: extra_path.clone(),
                    attachment_filename_override: None,
                }],
                session_source: Some(SessionSource::Cli),
                logs_override: None,
            })
            .expect("feedback report should be written");

        let attachments_dir = report_dir.join(FEEDBACK_REPORT_ATTACHMENTS_DIR);
        let metadata = fs::read_to_string(report_dir.join(FEEDBACK_REPORT_METADATA_FILENAME))
            .expect("metadata should be written");
        assert!(metadata.contains("Destination: hasna/codewith"));
        assert!(metadata.contains("Include logs: true"));
        assert!(metadata.contains("- session_source: cli"));
        assert!(metadata.contains("attachments/codex-logs.log"));
        assert!(metadata.contains(DOCTOR_REPORT_ATTACHMENT_FILENAME));
        assert!(metadata.contains(extra_filename.as_str()));
        assert_eq!(
            fs::read(attachments_dir.join("codex-logs.log")).expect("logs should be written"),
            b"ring buffer logs".to_vec()
        );
        assert_eq!(
            fs::read(attachments_dir.join(DOCTOR_REPORT_ATTACHMENT_FILENAME))
                .expect("doctor report should be written"),
            b"{\"overallStatus\":\"ok\"}".to_vec()
        );
        assert_eq!(
            fs::read(attachments_dir.join(&extra_filename)).expect("rollout should be written"),
            b"rollout".to_vec()
        );

        fs::remove_file(extra_path).expect("extra attachment should be removed");
        fs::remove_dir_all(report_dir).expect("report directory should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn upload_feedback_writes_private_report_files() {
        use std::os::unix::fs::PermissionsExt;

        let thread_id = ThreadId::new();
        let feedback = CodexFeedback::new();
        {
            let mut writer = feedback.make_writer().make_writer();
            writer
                .write_all(b"private logs")
                .expect("logs should be captured");
        }
        let snapshot = feedback.snapshot(Some(thread_id));
        let report_dir = feedback_report_dir(&snapshot.thread_id);
        let report_root = report_dir
            .parent()
            .expect("feedback report directory should have root");
        let _ = fs::remove_dir_all(&report_dir);

        snapshot
            .upload_feedback(FeedbackUploadOptions {
                classification: "bug",
                reason: None,
                tags: None,
                include_logs: true,
                extra_attachments: &[],
                extra_attachment_paths: &[],
                session_source: Some(SessionSource::Cli),
                logs_override: None,
            })
            .expect("feedback report should be written");

        let attachments_dir = report_dir.join(FEEDBACK_REPORT_ATTACHMENTS_DIR);
        let metadata_path = report_dir.join(FEEDBACK_REPORT_METADATA_FILENAME);
        let logs_path = attachments_dir.join("codex-logs.log");

        for path in [report_root, report_dir.as_path(), attachments_dir.as_path()] {
            let mode = fs::metadata(path)
                .expect("directory metadata should be readable")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700);
        }
        for path in [metadata_path.as_path(), logs_path.as_path()] {
            let mode = fs::metadata(path)
                .expect("file metadata should be readable")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        fs::remove_dir_all(report_dir).expect("report directory should be removed");
    }
}
