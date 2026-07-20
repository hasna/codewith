//! Cursor cloud background-agent (`bc-`) execution backend.
//!
//! `@cursor/sdk` auto-detects the cloud runtime from a `bc-` agent id and runs
//! the agent loop on a Cursor-hosted VM against a cloned copy of the repository.
//! Because that means workspace contents leave the machine, this backend is
//! gated behind an explicit [`CursorCloudEgressConsent`]: a run refuses unless
//! consent has been granted.
//!
//! The cloud lifecycle (create / resume / prompt, streaming status, artifacts,
//! and cancellation) is driven through the same Codewith-owned Node sidecar and
//! stdio protocol as the local backend ([`crate::cursor_sdk`]); the differences
//! are the config payload (`runtime: "cloud"`, no locally-bridged tools, an
//! explicit consent marker) and that builtin tools run remotely rather than
//! being disabled locally. Streamed `status` events become
//! [`ExternalAgentEvent::Status`], `artifact` events become
//! [`ExternalAgentArtifact`]s, the cloud agent id is persisted as the run's
//! `external_session_id`, and host cancellation is forwarded to the SDK's
//! `run.cancel`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_json::json;

use crate::ExternalAgentError;
use crate::ExternalAgentHost;
use crate::ExternalAgentReadiness;
use crate::ExternalAgentRequest;
use crate::ExternalAgentResult;
use crate::ExternalAgentRuntimeDescriptor;
use crate::ExternalAgentSandboxConfig;
use crate::cursor_composer_runtime_descriptor;
use crate::cursor_sdk::CursorSdkEnvironmentPolicy;
use crate::cursor_sdk::cursor_sidecar_readiness_with_env;
use crate::cursor_sdk::run_cursor_sidecar;
use crate::cursor_sdk::session_config;
use crate::cursor_sdk::validate_cursor_request;
use crate::resolve_cursor_composer_model;

/// Reason surfaced when a cloud run is attempted without egress consent.
pub const CURSOR_CLOUD_CONSENT_REQUIRED_REASON: &str =
    "Cursor cloud runs upload workspace contents to Cursor-hosted background-agent VMs; this \
requires explicit data-egress consent, which was not granted for this runtime.";

/// Whether the operator has authorized sending workspace data to Cursor cloud.
///
/// This is a hard gate on the cloud backend: constructing the backend without
/// [`CursorCloudEgressConsent::Granted`] leaves it inert (runs return
/// [`ExternalAgentError::NotReady`]). The default is [`Self::Denied`] so the safe
/// state is the one you get by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorCloudEgressConsent {
    /// The operator has NOT authorized sending workspace data to Cursor cloud.
    #[default]
    Denied,
    /// The operator explicitly authorized uploading workspace contents to
    /// Cursor-hosted background-agent VMs for this runtime.
    Granted,
}

impl CursorCloudEgressConsent {
    pub fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

/// The Cursor cloud (`bc-`) execution backend.
#[derive(Debug, Clone)]
pub struct CursorCloudBackend {
    descriptor: &'static ExternalAgentRuntimeDescriptor,
    env_policy: CursorSdkEnvironmentPolicy,
    consent: CursorCloudEgressConsent,
}

impl CursorCloudBackend {
    /// Build a cloud backend with an explicit egress-consent decision.
    pub fn new(consent: CursorCloudEgressConsent) -> Self {
        Self {
            descriptor: cursor_composer_runtime_descriptor(),
            env_policy: CursorSdkEnvironmentPolicy::default(),
            consent,
        }
    }

    pub fn descriptor(&self) -> &'static ExternalAgentRuntimeDescriptor {
        self.descriptor
    }

    pub fn consent(&self) -> CursorCloudEgressConsent {
        self.consent
    }

    pub async fn readiness_with_env(
        &self,
        source_env: &BTreeMap<String, String>,
    ) -> ExternalAgentReadiness {
        let mut readiness =
            cursor_sidecar_readiness_with_env(self.descriptor, &self.env_policy, source_env).await;
        if !self.consent.is_granted() {
            let note = "data-egress consent required before cloud runs".to_string();
            readiness.detail = Some(match readiness.detail {
                Some(detail) if !detail.trim().is_empty() => format!("{detail}; {note}"),
                _ => note,
            });
        }
        readiness
    }

    pub async fn run_sandboxed_with_env(
        &self,
        request: ExternalAgentRequest,
        host: impl ExternalAgentHost + Send + Sync,
        sandbox_config: &ExternalAgentSandboxConfig,
        source_env: BTreeMap<String, String>,
    ) -> Result<ExternalAgentResult, ExternalAgentError> {
        validate_cursor_request(self.descriptor, &request)?;
        if !self.consent.is_granted() {
            return Err(ExternalAgentError::NotReady {
                runtime: request.runtime.as_str().to_string(),
                reason: CURSOR_CLOUD_CONSENT_REQUIRED_REASON.to_string(),
            });
        }
        let config = self.cloud_config(&request);
        run_cursor_sidecar(
            self.descriptor,
            &self.env_policy,
            request,
            host,
            sandbox_config,
            source_env,
            config,
            // Cloud builtin tools execute remotely on the Cursor VM, so the local
            // "builtins disabled" invariant does not apply.
            /*expect_builtin_tools_disabled*/ false,
        )
        .await
    }

    /// Build the config payload sent to the cloud sidecar as its first line.
    pub fn cloud_config(&self, request: &ExternalAgentRequest) -> JsonValue {
        let model = resolve_cursor_composer_model(request.model.as_deref());
        json!({
            "type": "config",
            "runtime": "cloud",
            "task": request.task,
            "model": model,
            "cwd": request.cwd.to_string_lossy(),
            "mode": request.mode,
            "session": session_config(&request.session),
            "dataEgressConsent": self.consent.is_granted(),
            // Cloud tools run remotely on the Cursor VM; nothing is bridged
            // locally for a cloud run.
            "customTools": Vec::<String>::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExternalAgentActionRequest;
    use crate::ExternalAgentActionResult;
    use crate::ExternalAgentEvent;
    use crate::ExternalAgentLaunchIsolation;
    use crate::ExternalAgentLaunchSpec;
    use crate::ExternalAgentMode;
    use crate::ExternalAgentPermissionDecision;
    use crate::ExternalAgentPermissionRequest;
    use crate::ExternalAgentRunStatus;
    use crate::ExternalAgentRuntimeId;
    use crate::ExternalAgentSandboxedLaunchSpec;
    use crate::cursor_sdk::CursorSdkProcess;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct RecordingHost {
        events: Arc<Mutex<Vec<ExternalAgentEvent>>>,
    }

    impl RecordingHost {
        fn events(&self) -> Vec<ExternalAgentEvent> {
            self.events.lock().expect("events lock").clone()
        }
    }

    impl ExternalAgentHost for RecordingHost {
        async fn emit(&self, event: ExternalAgentEvent) -> Result<(), ExternalAgentError> {
            self.events.lock().expect("events lock").push(event);
            Ok(())
        }

        async fn request_permission(
            &self,
            _request: ExternalAgentPermissionRequest,
        ) -> Result<ExternalAgentPermissionDecision, ExternalAgentError> {
            Ok(ExternalAgentPermissionDecision::RejectOnce)
        }

        async fn perform_action(
            &self,
            _action: ExternalAgentActionRequest,
        ) -> Result<ExternalAgentActionResult, ExternalAgentError> {
            Ok(ExternalAgentActionResult::Rejected {
                reason: "cloud host does not execute local actions".to_string(),
            })
        }

        async fn is_cancelled(&self) -> bool {
            false
        }
    }

    fn cloud_request() -> ExternalAgentRequest {
        ExternalAgentRequest::new(
            ExternalAgentRuntimeId::CURSOR,
            "ship the feature",
            "/repo",
            ExternalAgentMode::Plan,
        )
    }

    #[test]
    fn consent_defaults_to_denied() {
        assert_eq!(
            CursorCloudEgressConsent::default(),
            CursorCloudEgressConsent::Denied
        );
        assert!(!CursorCloudEgressConsent::Denied.is_granted());
        assert!(CursorCloudEgressConsent::Granted.is_granted());
    }

    #[test]
    fn cloud_config_marks_runtime_and_consent() {
        let backend = CursorCloudBackend::new(CursorCloudEgressConsent::Granted);
        let config = backend.cloud_config(&cloud_request().with_model("composer-2"));
        assert_eq!(config["runtime"], "cloud");
        assert_eq!(config["model"], "composer-2");
        assert_eq!(config["dataEgressConsent"], true);
        assert_eq!(config["customTools"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn run_refuses_without_consent() {
        let backend = CursorCloudBackend::new(CursorCloudEgressConsent::Denied);
        let sandbox = ExternalAgentSandboxConfig::new(
            codex_protocol::models::PermissionProfile::External {
                network: codex_protocol::permissions::NetworkSandboxPolicy::Restricted,
            },
        );
        let err = backend
            .run_sandboxed_with_env(cloud_request(), RecordingHost::default(), &sandbox, BTreeMap::new())
            .await
            .expect_err("cloud run must refuse without consent");
        match err {
            ExternalAgentError::NotReady { reason, .. } => {
                assert_eq!(reason, CURSOR_CLOUD_CONSENT_REQUIRED_REASON);
            }
            other => panic!("expected NotReady, got {other:?}"),
        }
    }

    fn fake_launch(script: &Path, cwd: &Path) -> Option<ExternalAgentSandboxedLaunchSpec> {
        let python = which::which("python3").ok()?;
        let path = std::env::var("PATH").unwrap_or_default();
        let launch = ExternalAgentLaunchSpec {
            runtime: ExternalAgentRuntimeId::CURSOR.into(),
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
    async fn cloud_run_streams_status_artifacts_and_bc_session() {
        let temp = tempfile::tempdir().expect("tempdir");
        let script = temp.path().join("fake_cloud.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

config = json.loads(sys.stdin.readline())
assert config["runtime"] == "cloud"
assert config["dataEgressConsent"] is True
send({"type": "ready", "runtime": "cloud", "builtinToolsDisabled": False, "customTools": []})
send({"type": "session", "agentId": "bc-42"})
send({"type": "status", "message": "provisioning VM"})
send({"type": "status", "message": "running"})
send({"type": "artifact", "artifact": {"label": "pull request", "path": None, "mimeType": None, "uri": "https://cursor.com/agents/bc-42"}})
send({"type": "completed", "summary": "opened PR", "artifacts": []})
"#,
        )
        .expect("write fake cloud sidecar");

        let Some(launch) = fake_launch(&script, temp.path()) else {
            return;
        };
        let request = cloud_request();
        let backend = CursorCloudBackend::new(CursorCloudEgressConsent::Granted);
        let config = backend.cloud_config(&request);
        let host = RecordingHost::default();
        let mut process = CursorSdkProcess::spawn(launch, false).expect("spawn");
        let result = process
            .run(request, &host, config)
            .await
            .expect("cloud run completes");

        assert_eq!(result.status, ExternalAgentRunStatus::Completed);
        assert_eq!(result.summary.as_deref(), Some("opened PR"));
        assert_eq!(result.session.external_session_id.as_deref(), Some("bc-42"));
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(
            result.artifacts[0].uri.as_deref(),
            Some("https://cursor.com/agents/bc-42")
        );

        let events = host.events();
        let statuses = events
            .iter()
            .filter_map(|event| match event {
                ExternalAgentEvent::Status { message } => Some(message.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            statuses,
            vec!["provisioning VM".to_string(), "running".to_string()]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ExternalAgentEvent::SessionResolved { session }
                if session.external_session_id.as_deref() == Some("bc-42")
        )));
        assert!(events
            .iter()
            .any(|event| matches!(event, ExternalAgentEvent::Artifact { .. })));
    }
}
