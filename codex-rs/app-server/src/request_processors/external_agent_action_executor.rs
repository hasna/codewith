//! Guarded execution of external-agent managed actions.
//!
//! [`ExternalAgentActionExecutor`] is the app-server realization of the
//! [`ExternalAgentActionGuard`] policy defined in `codex-external-agent`. It is
//! what turns a runtime's *request* to read/write/run/call-MCP into a real,
//! safe host effect -- always performed by Codewith, never by the runtime.
//!
//! For each action the executor:
//!   1. classifies the request with the guard,
//!   2. realizes the resulting decision:
//!        - [`PerformRead`]   -> a confined read (canonicalize + `can_read`
//!          recheck against the run's [`PermissionProfile`] + a byte cap),
//!        - [`PromoteWrite`]  -> the write promoted through Codewith's native
//!          `apply_patch` pipeline,
//!        - [`DelegateCommand`] -> the command delegated to the native exec /
//!          `ToolOrchestrator` path under the run sandbox,
//!        - [`DelegateMcp`]   -> the MCP call routed through the native MCP
//!          approval path,
//!        - [`RecordProposal`]/[`Deny`] -> recorded / refused without executing,
//!   3. audits the action + decision + outcome to the transcript.
//!
//! The confined-read logic here is the migration target for the read path that
//! was previously inline in the `#301` `perform_action` match; the native
//! delegations for write/command/MCP are injected by the caller (Comp4) via the
//! [`ManagedActionBackend`] seam so that this module stays independently
//! testable while binding to the real native services at wiring time.
//!
//! [`PerformRead`]: ExternalAgentActionDecision::PerformRead
//! [`PromoteWrite`]: ExternalAgentActionDecision::PromoteWrite
//! [`DelegateCommand`]: ExternalAgentActionDecision::DelegateCommand
//! [`DelegateMcp`]: ExternalAgentActionDecision::DelegateMcp
//! [`RecordProposal`]: ExternalAgentActionDecision::RecordProposal
//! [`Deny`]: ExternalAgentActionDecision::Deny

use async_trait::async_trait;
use codex_external_agent::ExternalAgentActionDecision;
use codex_external_agent::ExternalAgentActionGuard;
use codex_external_agent::ExternalAgentActionRequest;
use codex_external_agent::ExternalAgentActionResult;
use codex_protocol::models::PermissionProfile;
use serde_json::Value as JsonValue;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

/// Default cap on a single confined read, in bytes (5 MiB). Files larger than
/// this are refused rather than truncated so the runtime never acts on a
/// partial view of a file.
pub const DEFAULT_MAX_READ_BYTES: usize = 5 * 1024 * 1024;

/// Failure raised by a [`ManagedActionBackend`] delegation.
#[derive(Debug, thiserror::Error)]
pub enum ManagedActionError {
    /// The action was refused by the native safety/approval path (e.g. a write
    /// outside the writable roots, or a denied approval prompt). The message is
    /// surfaced to the runtime as a rejection reason.
    #[error("{0}")]
    Rejected(String),
    /// The native path failed unexpectedly while executing an approved action.
    #[error("managed action failed: {0}")]
    Failed(String),
}

/// Output of a delegated command, mirroring
/// [`ExternalAgentActionResult::CommandOutput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedCommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// What ultimately happened to an action, recorded in the audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAgentActionOutcome {
    /// Codewith realized the effect (read/write/command/MCP succeeded).
    Performed,
    /// Codewith recorded the request as a reviewable proposal, unexecuted.
    Recorded,
    /// Codewith refused the request by policy.
    Denied,
    /// Codewith attempted the effect but it failed.
    Failed,
}

/// One audit record appended to the transcript for a handled action.
#[derive(Debug, Clone)]
pub struct ExternalAgentActionAudit {
    pub action: ExternalAgentActionRequest,
    pub decision: ExternalAgentActionDecision,
    pub outcome: ExternalAgentActionOutcome,
    /// Populated on [`ExternalAgentActionOutcome::Denied`] /
    /// [`ExternalAgentActionOutcome::Failed`] with the surfaced reason.
    pub detail: Option<String>,
}

/// Native Codewith services the executor delegates managed effects to.
///
/// Every method here is performed by *Codewith*, never by the external runtime.
/// Comp4 implements this against the run's native `apply_patch` pipeline, the
/// sandboxed exec / `ToolOrchestrator` path, and the MCP approval path, and
/// appends audit records to the active transcript.
#[async_trait]
pub trait ManagedActionBackend: Send + Sync {
    /// Promote a confined write into a staged-and-applied patch via the native
    /// `apply_patch` pipeline (which performs its own write-safety assessment).
    async fn apply_write(&self, path: PathBuf, content: String) -> Result<(), ManagedActionError>;

    /// Delegate a command to the native sandboxed exec / `ToolOrchestrator`
    /// path, running under the run sandbox with the given working directory.
    async fn run_command(
        &self,
        command: Vec<String>,
        cwd: PathBuf,
    ) -> Result<ManagedCommandOutput, ManagedActionError>;

    /// Route an MCP tool call through the native MCP approval path.
    async fn call_mcp_tool(
        &self,
        server: String,
        tool: String,
        arguments: JsonValue,
    ) -> Result<JsonValue, ManagedActionError>;

    /// Append an audit record for a handled (performed, recorded, denied, or
    /// failed) action to the active transcript.
    async fn audit(&self, record: ExternalAgentActionAudit);
}

/// Executes runtime-requested actions under the [`ExternalAgentActionGuard`].
pub struct ExternalAgentActionExecutor {
    guard: ExternalAgentActionGuard,
    /// The run's read policy. Reads are rechecked against this after
    /// canonicalization. Writes/commands/MCP are governed by the native backend
    /// (which owns their own, distinct write/exec/approval policy).
    permission_profile: PermissionProfile,
    cwd: PathBuf,
    max_read_bytes: usize,
    backend: Arc<dyn ManagedActionBackend>,
}

impl ExternalAgentActionExecutor {
    /// Build an executor for one run.
    ///
    /// `guard` classifies actions; `permission_profile` is the run's (read)
    /// filesystem policy used to recheck confined reads; `cwd` is the run
    /// working directory; `backend` provides the native write/command/MCP/audit
    /// services.
    pub fn new(
        guard: ExternalAgentActionGuard,
        permission_profile: PermissionProfile,
        cwd: impl Into<PathBuf>,
        backend: Arc<dyn ManagedActionBackend>,
    ) -> Self {
        Self {
            guard,
            permission_profile,
            cwd: cwd.into(),
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
            backend,
        }
    }

    /// Override the confined-read byte cap (default [`DEFAULT_MAX_READ_BYTES`]).
    #[must_use]
    pub fn with_max_read_bytes(mut self, max_read_bytes: usize) -> Self {
        self.max_read_bytes = max_read_bytes;
        self
    }

    #[cfg(test)]
    fn guard(&self) -> &ExternalAgentActionGuard {
        &self.guard
    }

    /// Classify, realize, and audit a single action.
    pub async fn execute(&self, action: ExternalAgentActionRequest) -> ExternalAgentActionResult {
        let decision = self.guard.decide(&action);
        // Belt-and-suspenders: the guard's contract is that no decision ever
        // authorizes the runtime to perform the effect itself.
        debug_assert!(
            !decision.authorizes_runtime_side_effect(),
            "guard produced a decision that authorizes a runtime side effect"
        );

        let result = self.realize(&action, &decision).await;

        let (outcome, detail) = classify_outcome(&decision, &result);
        self.backend
            .audit(ExternalAgentActionAudit {
                action,
                decision,
                outcome,
                detail,
            })
            .await;

        result
    }

    async fn realize(
        &self,
        action: &ExternalAgentActionRequest,
        decision: &ExternalAgentActionDecision,
    ) -> ExternalAgentActionResult {
        match decision {
            ExternalAgentActionDecision::Deny { reason } => rejected(reason.clone()),
            ExternalAgentActionDecision::RecordProposal => rejected(
                "recorded as a proposal; Codewith does not execute this action in the current mode",
            ),
            ExternalAgentActionDecision::PerformRead => self.perform_confined_read(action).await,
            ExternalAgentActionDecision::PromoteWrite => self.promote_write(action).await,
            ExternalAgentActionDecision::DelegateCommand => self.delegate_command(action).await,
            ExternalAgentActionDecision::DelegateMcp => self.delegate_mcp(action).await,
        }
    }

    /// Confined read: canonicalize the requested path (resolving symlinks and
    /// `..` so the recheck sees the real target), recheck `can_read` against the
    /// run's permission profile, then read the file bounded by the byte cap.
    async fn perform_confined_read(
        &self,
        action: &ExternalAgentActionRequest,
    ) -> ExternalAgentActionResult {
        let ExternalAgentActionRequest::ReadFile { path } = action else {
            return rejected("internal error: non-read action routed to the read path");
        };

        // Canonicalize first; this both requires the file to exist and collapses
        // symlinks/`..`, defeating attempts to escape the readable roots via a
        // symlink that lives inside them.
        let canonical = match tokio::fs::canonicalize(path).await {
            Ok(canonical) => canonical,
            Err(err) => {
                return rejected(format!("cannot access `{}`: {err}", path.display()));
            }
        };

        // Recheck against the run's read policy on the *resolved* path.
        if !self
            .permission_profile
            .file_system_sandbox_policy()
            .can_read_path_with_cwd(&canonical, &self.cwd)
        {
            return rejected(format!(
                "read of `{}` is outside the permitted read roots",
                canonical.display()
            ));
        }

        let metadata = match tokio::fs::metadata(&canonical).await {
            Ok(metadata) => metadata,
            Err(err) => {
                return rejected(format!("cannot stat `{}`: {err}", canonical.display()));
            }
        };
        if !metadata.is_file() {
            return rejected(format!(
                "`{}` is not a regular file",
                canonical.display()
            ));
        }

        match read_file_capped(&canonical, self.max_read_bytes).await {
            Ok(ReadOutcome::Content(content)) => {
                ExternalAgentActionResult::FileContent { content }
            }
            Ok(ReadOutcome::TooLarge) => rejected(format!(
                "`{}` exceeds the {}-byte read cap",
                canonical.display(),
                self.max_read_bytes
            )),
            Err(err) => rejected(format!("cannot read `{}`: {err}", canonical.display())),
        }
    }

    /// Promote a write through the native `apply_patch` pipeline. The pipeline
    /// performs its own write-safety assessment against the run's write policy,
    /// so the executor does not re-gate the path here (the external-agent read
    /// profile intentionally grants no write roots).
    async fn promote_write(
        &self,
        action: &ExternalAgentActionRequest,
    ) -> ExternalAgentActionResult {
        let ExternalAgentActionRequest::WriteFile { path, content } = action else {
            return rejected("internal error: non-write action routed to the write path");
        };
        match self
            .backend
            .apply_write(path.clone(), content.clone())
            .await
        {
            Ok(()) => ExternalAgentActionResult::WriteAccepted,
            Err(err) => rejected(err.to_string()),
        }
    }

    /// Delegate a command to the native sandboxed exec path. The command runs
    /// under the run sandbox; its working directory is confined to the run cwd.
    async fn delegate_command(
        &self,
        action: &ExternalAgentActionRequest,
    ) -> ExternalAgentActionResult {
        let ExternalAgentActionRequest::RunCommand { command, cwd } = action else {
            return rejected("internal error: non-command action routed to the command path");
        };
        if command.is_empty() {
            return rejected("cannot run an empty command");
        }
        let cwd = match cwd {
            Some(requested) => match confine_to_cwd(&self.cwd, requested) {
                Ok(cwd) => cwd,
                Err(reason) => return rejected(reason),
            },
            None => self.cwd.clone(),
        };
        match self.backend.run_command(command.clone(), cwd).await {
            Ok(output) => ExternalAgentActionResult::CommandOutput {
                exit_code: output.exit_code,
                stdout: output.stdout,
                stderr: output.stderr,
            },
            Err(err) => rejected(err.to_string()),
        }
    }

    /// Route an MCP tool call through the native MCP approval path.
    async fn delegate_mcp(
        &self,
        action: &ExternalAgentActionRequest,
    ) -> ExternalAgentActionResult {
        let ExternalAgentActionRequest::McpToolCall {
            server,
            tool,
            arguments,
        } = action
        else {
            return rejected("internal error: non-MCP action routed to the MCP path");
        };
        match self
            .backend
            .call_mcp_tool(server.clone(), tool.clone(), arguments.clone())
            .await
        {
            Ok(result) => ExternalAgentActionResult::McpToolResult { result },
            Err(err) => rejected(err.to_string()),
        }
    }
}

fn rejected(reason: impl Into<String>) -> ExternalAgentActionResult {
    ExternalAgentActionResult::Rejected {
        reason: reason.into(),
    }
}

/// Derive the audit outcome (and any surfaced detail) from a decision and its
/// realized result.
fn classify_outcome(
    decision: &ExternalAgentActionDecision,
    result: &ExternalAgentActionResult,
) -> (ExternalAgentActionOutcome, Option<String>) {
    match (decision, result) {
        (ExternalAgentActionDecision::Deny { reason }, _) => {
            (ExternalAgentActionOutcome::Denied, Some(reason.clone()))
        }
        (ExternalAgentActionDecision::RecordProposal, _) => {
            (ExternalAgentActionOutcome::Recorded, None)
        }
        // An executable decision that still returned a rejection means the
        // native realization refused or failed.
        (_, ExternalAgentActionResult::Rejected { reason }) => {
            (ExternalAgentActionOutcome::Failed, Some(reason.clone()))
        }
        _ => (ExternalAgentActionOutcome::Performed, None),
    }
}

enum ReadOutcome {
    Content(String),
    TooLarge,
}

/// Read at most `max_bytes` from `path`. If the file has more than `max_bytes`
/// of content, report [`ReadOutcome::TooLarge`] rather than returning a
/// truncated view. Reads one byte past the cap to detect overflow even when the
/// on-disk size races the stat above.
async fn read_file_capped(path: &Path, max_bytes: usize) -> std::io::Result<ReadOutcome> {
    let file = tokio::fs::File::open(path).await?;
    let cap = max_bytes as u64;
    let mut reader = file.take(cap.saturating_add(1));
    let mut buf = Vec::new();
    reader.read_to_end(&mut buf).await?;
    if buf.len() > max_bytes {
        return Ok(ReadOutcome::TooLarge);
    }
    Ok(ReadOutcome::Content(
        String::from_utf8_lossy(&buf).into_owned(),
    ))
}

/// Confine a requested command working directory to the run cwd (lexically).
/// The run sandbox is the authoritative enforcement boundary; this is
/// defense-in-depth that refuses an obvious escape before delegating.
fn confine_to_cwd(cwd: &Path, requested: &Path) -> Result<PathBuf, String> {
    let base = normalize_lexical(cwd);
    let candidate = if requested.is_absolute() {
        normalize_lexical(requested)
    } else {
        normalize_lexical(&base.join(requested))
    };
    if candidate.starts_with(&base) {
        Ok(candidate)
    } else {
        Err(format!(
            "command cwd `{}` is outside the run cwd `{}`",
            candidate.display(),
            base.display()
        ))
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_external_agent::ExternalAgentCapabilities;
    use codex_external_agent::ExternalAgentMode;
    use codex_protocol::permissions::FileSystemAccessMode;
    use codex_protocol::permissions::FileSystemPath;
    use codex_protocol::permissions::FileSystemSandboxEntry;
    use codex_protocol::permissions::FileSystemSandboxPolicy;
    use codex_protocol::permissions::NetworkSandboxPolicy;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use serde_json::json;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[derive(Default)]
    struct FakeBackend {
        writes: Mutex<Vec<(PathBuf, String)>>,
        commands: Mutex<Vec<(Vec<String>, PathBuf)>>,
        mcp_calls: Mutex<Vec<(String, String, JsonValue)>>,
        audits: Mutex<Vec<ExternalAgentActionAudit>>,
        fail_write: bool,
    }

    #[async_trait]
    impl ManagedActionBackend for FakeBackend {
        async fn apply_write(
            &self,
            path: PathBuf,
            content: String,
        ) -> Result<(), ManagedActionError> {
            self.writes.lock().unwrap().push((path, content));
            if self.fail_write {
                Err(ManagedActionError::Rejected(
                    "write outside writable roots".to_string(),
                ))
            } else {
                Ok(())
            }
        }

        async fn run_command(
            &self,
            command: Vec<String>,
            cwd: PathBuf,
        ) -> Result<ManagedCommandOutput, ManagedActionError> {
            self.commands.lock().unwrap().push((command, cwd));
            Ok(ManagedCommandOutput {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            })
        }

        async fn call_mcp_tool(
            &self,
            server: String,
            tool: String,
            arguments: JsonValue,
        ) -> Result<JsonValue, ManagedActionError> {
            self.mcp_calls
                .lock()
                .unwrap()
                .push((server, tool, arguments));
            Ok(json!({"ok": true}))
        }

        async fn audit(&self, record: ExternalAgentActionAudit) {
            self.audits.lock().unwrap().push(record);
        }
    }

    fn read_only_profile_for(root: &Path) -> PermissionProfile {
        let entry = FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: AbsolutePathBuf::from_absolute_path(root).expect("absolute root"),
            },
            access: FileSystemAccessMode::Read,
        };
        let policy = FileSystemSandboxPolicy::restricted(vec![entry]);
        PermissionProfile::from_runtime_permissions(&policy, NetworkSandboxPolicy::Enabled)
    }

    fn managed_executor(
        cwd: &Path,
        backend: Arc<FakeBackend>,
    ) -> ExternalAgentActionExecutor {
        let guard = ExternalAgentActionGuard::new(
            ExternalAgentMode::Managed,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Managed),
        );
        ExternalAgentActionExecutor::new(guard, read_only_profile_for(cwd), cwd, backend)
    }

    /// Canonicalize a temp dir so read-root comparisons match the canonical
    /// paths the executor derives (e.g. `/tmp` -> `/private/tmp` on macOS).
    fn canonical_dir(dir: &TempDir) -> PathBuf {
        std::fs::canonicalize(dir.path()).expect("canonicalize temp dir")
    }

    #[tokio::test]
    async fn read_within_roots_returns_file_content() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let file = cwd.join("hello.txt");
        tokio::fs::write(&file, "hello world").await.unwrap();

        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());
        let result = executor
            .execute(ExternalAgentActionRequest::ReadFile { path: file })
            .await;

        assert_eq!(
            result,
            ExternalAgentActionResult::FileContent {
                content: "hello world".to_string()
            }
        );
        let audits = backend.audits.lock().unwrap();
        assert_eq!(audits.len(), 1);
        assert_eq!(audits[0].outcome, ExternalAgentActionOutcome::Performed);
    }

    #[tokio::test]
    async fn read_of_missing_file_is_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::ReadFile {
                path: cwd.join("nope.txt"),
            })
            .await;

        assert!(matches!(
            result,
            ExternalAgentActionResult::Rejected { .. }
        ));
        assert_eq!(
            backend.audits.lock().unwrap()[0].outcome,
            ExternalAgentActionOutcome::Failed
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escaping_the_roots_is_rejected_after_canonicalization() {
        // secret lives outside the readable root; a symlink inside the root
        // points at it. Canonicalization must resolve the symlink so the
        // can_read recheck sees the real (out-of-root) target.
        let outside = TempDir::new().unwrap();
        let secret = canonical_dir(&outside).join("secret.txt");
        tokio::fs::write(&secret, "top secret").await.unwrap();

        let root = TempDir::new().unwrap();
        let cwd = canonical_dir(&root);
        let link = cwd.join("link.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();

        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend);
        let result = executor
            .execute(ExternalAgentActionRequest::ReadFile { path: link })
            .await;

        match result {
            ExternalAgentActionResult::Rejected { reason } => {
                assert!(reason.contains("outside the permitted read roots"), "{reason}");
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_exceeding_the_cap_is_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let file = cwd.join("big.txt");
        tokio::fs::write(&file, "0123456789").await.unwrap();

        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend).with_max_read_bytes(4);
        let result = executor
            .execute(ExternalAgentActionRequest::ReadFile { path: file })
            .await;

        match result {
            ExternalAgentActionResult::Rejected { reason } => {
                assert!(reason.contains("read cap"), "{reason}");
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn managed_write_is_promoted_through_apply_patch() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::WriteFile {
                path: cwd.join("out.rs"),
                content: "fn main() {}".to_string(),
            })
            .await;

        assert_eq!(result, ExternalAgentActionResult::WriteAccepted);
        assert_eq!(backend.writes.lock().unwrap().len(), 1);
        assert_eq!(
            backend.audits.lock().unwrap()[0].outcome,
            ExternalAgentActionOutcome::Performed
        );
    }

    #[tokio::test]
    async fn managed_write_rejection_from_backend_is_surfaced_and_audited_as_failed() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend {
            fail_write: true,
            ..FakeBackend::default()
        });
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::WriteFile {
                path: cwd.join("out.rs"),
                content: "x".to_string(),
            })
            .await;

        match result {
            ExternalAgentActionResult::Rejected { reason } => {
                assert!(reason.contains("writable roots"), "{reason}");
            }
            other => panic!("expected rejection, got {other:?}"),
        }
        assert_eq!(
            backend.audits.lock().unwrap()[0].outcome,
            ExternalAgentActionOutcome::Failed
        );
    }

    #[tokio::test]
    async fn managed_command_is_delegated_under_the_run_cwd() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::RunCommand {
                command: vec!["echo".to_string(), "hi".to_string()],
                cwd: None,
            })
            .await;

        assert_eq!(
            result,
            ExternalAgentActionResult::CommandOutput {
                exit_code: 0,
                stdout: "ok".to_string(),
                stderr: String::new(),
            }
        );
        let commands = backend.commands.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].1, cwd);
    }

    #[tokio::test]
    async fn command_cwd_escaping_the_run_cwd_is_rejected() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::RunCommand {
                command: vec!["echo".to_string()],
                cwd: Some(cwd.join("..").join("elsewhere")),
            })
            .await;

        assert!(matches!(
            result,
            ExternalAgentActionResult::Rejected { .. }
        ));
        assert!(backend.commands.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn managed_mcp_is_routed_to_the_backend() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::McpToolCall {
                server: "docs".to_string(),
                tool: "search".to_string(),
                arguments: json!({"q": "x"}),
            })
            .await;

        assert_eq!(
            result,
            ExternalAgentActionResult::McpToolResult {
                result: json!({"ok": true})
            }
        );
        assert_eq!(backend.mcp_calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn propose_mode_write_is_recorded_not_executed() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let guard = ExternalAgentActionGuard::new(
            ExternalAgentMode::Propose,
            ExternalAgentCapabilities::for_mode(ExternalAgentMode::Propose),
        );
        let executor = ExternalAgentActionExecutor::new(
            guard,
            read_only_profile_for(&cwd),
            &cwd,
            backend.clone(),
        );

        let result = executor
            .execute(ExternalAgentActionRequest::WriteFile {
                path: cwd.join("out.rs"),
                content: "x".to_string(),
            })
            .await;

        assert!(matches!(
            result,
            ExternalAgentActionResult::Rejected { .. }
        ));
        assert!(backend.writes.lock().unwrap().is_empty());
        assert_eq!(
            backend.audits.lock().unwrap()[0].outcome,
            ExternalAgentActionOutcome::Recorded
        );
    }

    #[tokio::test]
    async fn unsupported_action_is_denied() {
        let dir = TempDir::new().unwrap();
        let cwd = canonical_dir(&dir);
        let backend = Arc::new(FakeBackend::default());
        let executor = managed_executor(&cwd, backend.clone());

        let result = executor
            .execute(ExternalAgentActionRequest::Other {
                label: "teleport".to_string(),
                payload: json!({}),
            })
            .await;

        assert!(matches!(
            result,
            ExternalAgentActionResult::Rejected { .. }
        ));
        assert_eq!(
            backend.audits.lock().unwrap()[0].outcome,
            ExternalAgentActionOutcome::Denied
        );
        // The guard the executor uses is genuinely in managed mode.
        assert!(executor.guard().is_managed());
    }
}
