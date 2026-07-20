//! Grok Build run helpers and end-to-end coverage over the shared ACP harness.
//!
//! Grok Build is xAI's coding agent, reached over an ACP stdio transport
//! (`grok --no-auto-update agent stdio`). The vendor-specific knobs (auth
//! environment, ACP auth-method preferences, CLI resolution, readiness) live in
//! [`crate::GrokBuildAcpAdapter`], which plugs into the shared
//! [`crate::AcpStdioHarness`] via the [`crate::AcpAgentAdapter`] seam;
//! [`crate::grok_build_acp_harness`] returns a harness already wired with it.
//!
//! This module adds the pieces specific to *running* Grok Build through that
//! harness rather than redefining the seam:
//!
//! * ergonomic request builders, including [`grok_build_resume_request`] for
//!   persisting and resuming a Grok session id across runs; and
//! * fake-runtime integration tests that prove the composed harness + adapter
//!   route permissions, honor cancellation, filter replayed session updates,
//!   confine filesystem paths to the run `cwd`, and persist/resume session ids.

use std::path::PathBuf;

use crate::ExternalAgentMode;
use crate::ExternalAgentRequest;
use crate::ExternalAgentRuntimeId;
use crate::ExternalAgentSessionRequest;

/// Build a fresh Grok Build run request for the given task and workspace.
pub fn grok_build_request(
    task: impl Into<String>,
    cwd: impl Into<PathBuf>,
    mode: ExternalAgentMode,
) -> ExternalAgentRequest {
    ExternalAgentRequest::new(ExternalAgentRuntimeId::GROK_BUILD, task, cwd, mode)
}

/// Build a Grok Build run request that resumes a previously persisted session.
///
/// Codewith persists the `external_session_id` surfaced on a run (via
/// [`crate::ExternalAgentEvent::RunStarted`] / `SessionResolved` and the
/// returned [`crate::ExternalAgentResult`]). Passing it back here drives an ACP
/// `session/load` so Grok Build continues the same conversation instead of
/// starting a new one.
pub fn grok_build_resume_request(
    task: impl Into<String>,
    cwd: impl Into<PathBuf>,
    mode: ExternalAgentMode,
    external_session_id: impl Into<String>,
) -> ExternalAgentRequest {
    let mut request = grok_build_request(task, cwd, mode);
    request.session = ExternalAgentSessionRequest::Resume {
        external_session_id: external_session_id.into(),
    };
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AcpAgentAdapter;
    use crate::ExternalAgentActionRequest;
    use crate::ExternalAgentActionResult;
    use crate::ExternalAgentError;
    use crate::ExternalAgentEvent;
    use crate::ExternalAgentHarness;
    use crate::ExternalAgentHarnessKind;
    use crate::ExternalAgentHost;
    use crate::ExternalAgentLaunchIsolation;
    use crate::ExternalAgentLaunchSpec;
    use crate::ExternalAgentPermissionDecision;
    use crate::ExternalAgentPermissionOption;
    use crate::ExternalAgentPermissionRequest;
    use crate::ExternalAgentRunStatus;
    use crate::ExternalAgentRuntime;
    use crate::ExternalAgentSandboxedLaunchSpec;
    use crate::GrokBuildAcpAdapter;
    use crate::grok_build_acp_harness;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[test]
    fn grok_build_harness_is_wired_with_the_grok_adapter() {
        let harness = grok_build_acp_harness().expect("grok-build harness");

        assert_eq!(harness.id(), ExternalAgentRuntimeId::from("grok-build"));
        assert_eq!(harness.harness_kind(), ExternalAgentHarnessKind::AcpStdio);
        assert_eq!(harness.descriptor().display_name, "Grok Build");
        assert_eq!(harness.descriptor().command.program, "grok");
        assert_eq!(
            harness.descriptor().command.args,
            ["--no-auto-update", "agent", "stdio"]
        );
        // The built-in registry must select the Grok adapter's knobs.
        assert_eq!(harness.adapter().auth_env_vars(), &["XAI_API_KEY"]);
        assert_eq!(
            harness.adapter().preferred_auth_methods(),
            GrokBuildAcpAdapter.preferred_auth_methods()
        );
        assert_eq!(
            harness.adapter().mode_id(ExternalAgentMode::Plan),
            Some("plan")
        );
        assert_eq!(harness.adapter().mode_id(ExternalAgentMode::Propose), None);
    }

    #[test]
    fn grok_build_resume_request_loads_persisted_session_id() {
        let request = grok_build_resume_request(
            "continue the audit",
            "/repo",
            ExternalAgentMode::Plan,
            "grok-session-42",
        );

        assert_eq!(request.runtime, ExternalAgentRuntimeId::from("grok-build"));
        assert_eq!(
            request.session,
            ExternalAgentSessionRequest::Resume {
                external_session_id: "grok-session-42".to_string(),
            }
        );
    }

    #[derive(Clone)]
    struct TestHost {
        events: Arc<Mutex<Vec<ExternalAgentEvent>>>,
        actions: Arc<Mutex<Vec<ExternalAgentActionRequest>>>,
        decision: ExternalAgentPermissionDecision,
        cancelled: bool,
    }

    impl TestHost {
        fn new(decision: ExternalAgentPermissionDecision) -> Self {
            Self {
                events: Arc::new(Mutex::new(Vec::new())),
                actions: Arc::new(Mutex::new(Vec::new())),
                decision,
                cancelled: false,
            }
        }

        fn cancelled() -> Self {
            let mut host = Self::new(ExternalAgentPermissionDecision::RejectOnce);
            host.cancelled = true;
            host
        }

        fn events(&self) -> Vec<ExternalAgentEvent> {
            self.events
                .lock()
                .unwrap_or_else(|err| panic!("events lock: {err}"))
                .clone()
        }

        fn actions(&self) -> Vec<ExternalAgentActionRequest> {
            self.actions
                .lock()
                .unwrap_or_else(|err| panic!("actions lock: {err}"))
                .clone()
        }

        fn output_text(&self) -> Vec<String> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    ExternalAgentEvent::OutputTextDelta { text } => Some(text),
                    _ => None,
                })
                .collect()
        }

        fn permission_requests(&self) -> Vec<ExternalAgentPermissionRequest> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    ExternalAgentEvent::PermissionRequested { request } => Some(request),
                    _ => None,
                })
                .collect()
        }

        fn run_started_session_ids(&self) -> Vec<Option<String>> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    ExternalAgentEvent::RunStarted { session } => Some(session.external_session_id),
                    _ => None,
                })
                .collect()
        }
    }

    impl ExternalAgentHost for TestHost {
        async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
            self.events
                .lock()
                .unwrap_or_else(|err| panic!("events lock: {err}"))
                .push(event);
            Ok(())
        }

        async fn request_permission(
            &self,
            _request: ExternalAgentPermissionRequest,
        ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
            Ok(self.decision)
        }

        async fn perform_action(
            &self,
            action: ExternalAgentActionRequest,
        ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
            self.actions
                .lock()
                .unwrap_or_else(|err| panic!("actions lock: {err}"))
                .push(action.clone());
            match action {
                ExternalAgentActionRequest::ReadFile { .. } => {
                    Ok(ExternalAgentActionResult::FileContent {
                        content: "host file contents".to_string(),
                    })
                }
                _ => Ok(ExternalAgentActionResult::Rejected {
                    reason: "test host does not execute this action".to_string(),
                }),
            }
        }

        async fn is_cancelled(&self) -> bool {
            self.cancelled
        }
    }

    /// Write a fake Grok ACP server script and wrap it as an (unenforced) launch
    /// so the composed harness + adapter can be driven end to end without the
    /// real `grok` CLI or the platform sandbox. Returns `None` when `python3` is
    /// unavailable so the test skips instead of failing on minimal images.
    fn fake_grok_launch(script_body: &str, cwd: &Path) -> Option<ExternalAgentSandboxedLaunchSpec> {
        let python = which::which("python3").ok()?;
        let script = cwd.join("fake_grok_acp.py");
        std::fs::write(&script, script_body).unwrap_or_else(|err| panic!("write fake grok: {err}"));
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::from("grok-build"),
            program: python,
            args: vec![script.display().to_string()],
            arg0: None,
            cwd: cwd.to_path_buf(),
            env: BTreeMap::from([("PATH".to_string(), path)]),
            isolation: ExternalAgentLaunchIsolation::test_only_unenforced(),
        };
        Some(ExternalAgentSandboxedLaunchSpec::test_only_unenforced(
            launch,
        ))
    }

    #[tokio::test]
    async fn routes_permission_requests_through_the_host() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let Some(launch) = fake_grok_launch(
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "grok-session"}})
    elif method == "session/prompt":
        send({
            "jsonrpc": "2.0",
            "id": "perm-1",
            "method": "session/request_permission",
            "params": {
                "sessionId": "grok-session",
                "toolCall": {"title": "Apply change"},
                "options": [
                    {"optionId": "allow-once", "kind": "allow_once", "name": "Allow"},
                    {"optionId": "reject-once", "kind": "reject_once", "name": "Reject"}
                ]
            }
        })
        response = json.loads(sys.stdin.readline())
        outcome = response.get("result", {}).get("outcome", {})
        if outcome.get("outcome") == "selected" and outcome.get("optionId") == "allow-once":
            send({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "grok-session",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "granted"}
                    }
                }
            })
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32000, "message": "permission mismatch"}})
"#,
            temp_dir.path(),
        ) else {
            return;
        };
        let harness = grok_build_acp_harness().expect("grok-build harness");
        let request = grok_build_request(
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Propose,
        );
        let host = TestHost::new(ExternalAgentPermissionDecision::AllowOnce);

        let result = harness
            .run_test_launch(request, host.clone(), launch)
            .await
            .unwrap_or_else(|err| panic!("grok run should complete: {err}"));

        assert_eq!(result.status, ExternalAgentRunStatus::Completed);
        assert_eq!(result.summary.as_deref(), Some("granted"));
        let permissions = host.permission_requests();
        assert_eq!(permissions.len(), 1);
        assert_eq!(
            permissions[0].options,
            vec![
                ExternalAgentPermissionOption::AllowOnce,
                ExternalAgentPermissionOption::RejectOnce,
            ]
        );
    }

    #[tokio::test]
    async fn confines_file_reads_to_the_run_cwd() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let Some(launch) = fake_grok_launch(
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "grok-session"}})
    elif method == "session/prompt":
        send({
            "jsonrpc": "2.0",
            "id": "read-1",
            "method": "fs/read_text_file",
            "params": {"sessionId": "grok-session", "path": "../escape.txt"}
        })
        response = json.loads(sys.stdin.readline())
        error = response.get("error", {})
        if error.get("code") == -32011:
            send({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "grok-session",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "confined"}
                    }
                }
            })
            send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        else:
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32000, "message": "confinement bypassed"}})
"#,
            temp_dir.path(),
        ) else {
            return;
        };
        let harness = grok_build_acp_harness().expect("grok-build harness");
        let request = grok_build_request(
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Propose,
        );
        let host = TestHost::new(ExternalAgentPermissionDecision::AllowOnce);

        let result = harness
            .run_test_launch(request, host.clone(), launch)
            .await
            .unwrap_or_else(|err| panic!("grok run should complete: {err}"));

        assert_eq!(result.status, ExternalAgentRunStatus::Completed);
        assert_eq!(result.summary.as_deref(), Some("confined"));
        // The confined path is rejected before the host ever performs the read.
        assert!(
            host.actions().is_empty(),
            "a confined read must not reach the host"
        );
    }

    #[tokio::test]
    async fn filters_replayed_session_updates_from_other_sessions() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let Some(launch) = fake_grok_launch(
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def update(session_id, text):
    send({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text}
            }
        }
    })

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "grok-session"}})
    elif method == "session/prompt":
        update("stale-session", "stale")
        update("grok-session", "current")
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
"#,
            temp_dir.path(),
        ) else {
            return;
        };
        let harness = grok_build_acp_harness().expect("grok-build harness");
        let request = grok_build_request(
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Propose,
        );
        let host = TestHost::new(ExternalAgentPermissionDecision::AllowOnce);

        let result = harness
            .run_test_launch(request, host.clone(), launch)
            .await
            .unwrap_or_else(|err| panic!("grok run should complete: {err}"));

        assert_eq!(result.status, ExternalAgentRunStatus::Completed);
        assert_eq!(host.output_text(), vec!["current".to_string()]);
    }

    #[tokio::test]
    async fn cancellation_stops_the_run_and_emits_a_cancelled_event() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let Some(launch) = fake_grok_launch(
            r#"
import sys
import time

sys.stdin.readline()
time.sleep(30)
"#,
            temp_dir.path(),
        ) else {
            return;
        };
        let harness = grok_build_acp_harness().expect("grok-build harness");
        let request =
            grok_build_request("inspect README", temp_dir.path(), ExternalAgentMode::Plan);
        let host = TestHost::cancelled();

        let err = harness
            .run_test_launch(request, host.clone(), launch)
            .await
            .expect_err("cancelled host should stop the run");

        assert!(matches!(err, ExternalAgentError::Cancelled));
        assert!(
            host.events()
                .iter()
                .any(|event| matches!(event, ExternalAgentEvent::Cancelled { .. })),
            "a cancelled event should be emitted"
        );
    }

    #[tokio::test]
    async fn persists_the_resolved_grok_session_id() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let Some(launch) = fake_grok_launch(
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": "grok-session-new"}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
"#,
            temp_dir.path(),
        ) else {
            return;
        };
        let harness = grok_build_acp_harness().expect("grok-build harness");
        let request = grok_build_request(
            "inspect README",
            temp_dir.path(),
            ExternalAgentMode::Propose,
        );
        let host = TestHost::new(ExternalAgentPermissionDecision::AllowOnce);

        let result = harness
            .run_test_launch(request, host.clone(), launch)
            .await
            .unwrap_or_else(|err| panic!("grok run should complete: {err}"));

        assert_eq!(
            result.session.external_session_id.as_deref(),
            Some("grok-session-new")
        );
        assert_eq!(
            host.run_started_session_ids(),
            vec![Some("grok-session-new".to_string())]
        );
    }

    #[tokio::test]
    async fn resumes_a_persisted_grok_session_id() {
        let temp_dir = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        // The fake agent echoes the loaded session id back with a suffix so the
        // test proves the harness forwarded the persisted id into session/load.
        let Some(launch) = fake_grok_launch(
            r#"
import json
import sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    params = message.get("params", {})
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request_id, "result": {"protocolVersion": 1}})
    elif method == "session/load":
        loaded = params.get("sessionId", "") + "-loaded"
        send({"jsonrpc": "2.0", "id": request_id, "result": {"sessionId": loaded}})
    elif method == "session/prompt":
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
"#,
            temp_dir.path(),
        ) else {
            return;
        };
        let harness = grok_build_acp_harness().expect("grok-build harness");
        let request = grok_build_resume_request(
            "continue the audit",
            temp_dir.path(),
            ExternalAgentMode::Propose,
            "prior-session",
        );
        let host = TestHost::new(ExternalAgentPermissionDecision::AllowOnce);

        let result = harness
            .run_test_launch(request, host.clone(), launch)
            .await
            .unwrap_or_else(|err| panic!("grok resume should complete: {err}"));

        assert_eq!(
            result.session.external_session_id.as_deref(),
            Some("prior-session-loaded")
        );
        assert_eq!(
            host.run_started_session_ids(),
            vec![Some("prior-session-loaded".to_string())]
        );
    }
}
