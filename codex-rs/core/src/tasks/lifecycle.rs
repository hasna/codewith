use codex_extension_api::ExtensionData;
use codex_protocol::error::CodexErr;
use codex_protocol::error::SandboxErr;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnAbortReason;

use crate::session::session::Session;
use crate::session::turn_context::TurnContext;

impl Session {
    pub(super) async fn emit_turn_start_lifecycle(
        &self,
        turn_context: &TurnContext,
        token_usage_at_turn_start: &TokenUsage,
    ) {
        for contributor in self.services.extensions.turn_lifecycle_contributors() {
            contributor
                .on_turn_start(codex_extension_api::TurnStartInput {
                    turn_id: turn_context.sub_id.as_str(),
                    collaboration_mode: &turn_context.collaboration_mode,
                    token_usage_at_turn_start,
                    session_store: &self.services.session_extension_data,
                    thread_store: &self.services.thread_extension_data,
                    turn_store: turn_context.extension_data.as_ref(),
                })
                .await;
        }
    }

    pub(super) async fn emit_turn_stop_lifecycle(&self, turn_store: &ExtensionData) {
        for contributor in self.services.extensions.turn_lifecycle_contributors() {
            contributor
                .on_turn_stop(codex_extension_api::TurnStopInput {
                    session_store: &self.services.session_extension_data,
                    thread_store: &self.services.thread_extension_data,
                    turn_store,
                })
                .await;
        }
    }

    pub(crate) async fn emit_thread_idle_lifecycle_if_idle(&self) {
        if self.active_turn.lock().await.is_some()
            || self.input_queue.has_trigger_turn_mailbox_items().await
        {
            return;
        }

        for contributor in self.services.extensions.thread_lifecycle_contributors() {
            contributor
                .on_thread_idle(codex_extension_api::ThreadIdleInput {
                    session_store: &self.services.session_extension_data,
                    thread_store: &self.services.thread_extension_data,
                })
                .await;
        }
    }

    pub(super) async fn emit_turn_abort_lifecycle(
        &self,
        reason: TurnAbortReason,
        turn_store: &ExtensionData,
    ) {
        for contributor in self.services.extensions.turn_lifecycle_contributors() {
            contributor
                .on_turn_abort(codex_extension_api::TurnAbortInput {
                    reason: reason.clone(),
                    session_store: &self.services.session_extension_data,
                    thread_store: &self.services.thread_extension_data,
                    turn_store,
                })
                .await;
        }
    }

    pub(crate) async fn emit_turn_error_lifecycle(
        &self,
        turn_context: &TurnContext,
        error: &CodexErr,
    ) {
        self.emit_turn_error_lifecycle_with_protocol_error(
            turn_context,
            error,
            error.to_codex_protocol_error(),
        )
        .await;
    }

    pub(crate) async fn emit_turn_error_lifecycle_with_protocol_error(
        &self,
        turn_context: &TurnContext,
        error: &CodexErr,
        protocol_error: CodexErrorInfo,
    ) {
        let error_fingerprint = stable_turn_error_fingerprint(error);
        for contributor in self.services.extensions.turn_lifecycle_contributors() {
            contributor
                .on_turn_error_with_fingerprint(
                    codex_extension_api::TurnErrorInput {
                        turn_id: turn_context.sub_id.as_str(),
                        error: protocol_error.clone(),
                        session_store: &self.services.session_extension_data,
                        thread_store: &self.services.thread_extension_data,
                        turn_store: turn_context.extension_data.as_ref(),
                    },
                    error_fingerprint.as_str(),
                )
                .await;
        }
    }
}

fn stable_turn_error_fingerprint(error: &CodexErr) -> String {
    let fingerprint = match error {
        CodexErr::TurnAborted => "codex_err:turn_aborted",
        CodexErr::Stream(..) => "codex_err:stream",
        CodexErr::ContextWindowExceeded => "codex_err:context_window_exceeded",
        CodexErr::ThreadNotFound(_) => "codex_err:thread_not_found",
        CodexErr::AgentLimitReached { .. } => "codex_err:agent_limit_reached",
        CodexErr::SessionConfiguredNotFirstEvent => "codex_err:session_configured_not_first_event",
        CodexErr::Timeout => "codex_err:timeout",
        CodexErr::RequestTimeout => "codex_err:request_timeout",
        CodexErr::Spawn => "codex_err:spawn",
        CodexErr::Interrupted => "codex_err:interrupted",
        CodexErr::InvalidRequest(_) => "codex_err:invalid_request",
        CodexErr::InvalidImageRequest() => "codex_err:invalid_image_request",
        CodexErr::UsageLimitReached(_) => "codex_err:usage_limit_reached",
        CodexErr::ServerOverloaded => "codex_err:server_overloaded",
        CodexErr::CyberPolicy { .. } => "codex_err:cyber_policy",
        CodexErr::QuotaExceeded => "codex_err:quota_exceeded",
        CodexErr::UsageNotIncluded => "codex_err:usage_not_included",
        CodexErr::InternalServerError => "codex_err:internal_server_error",
        CodexErr::InternalAgentDied => "codex_err:internal_agent_died",
        CodexErr::LandlockSandboxExecutableNotProvided => {
            "codex_err:landlock_sandbox_executable_not_provided"
        }
        CodexErr::UnsupportedOperation(_) => "codex_err:unsupported_operation",
        CodexErr::RefreshTokenFailed(_) => "codex_err:refresh_token_failed",
        CodexErr::Fatal(_) => "codex_err:fatal",
        CodexErr::Io(error) => return stable_io_error_fingerprint(error.kind()),
        CodexErr::EnvVar(_) => "codex_err:env_var",
        CodexErr::Json(error) => match error.classify() {
            serde_json::error::Category::Io => "codex_err:json:io",
            serde_json::error::Category::Syntax => "codex_err:json:syntax",
            serde_json::error::Category::Data => "codex_err:json:data",
            serde_json::error::Category::Eof => "codex_err:json:eof",
        },
        CodexErr::TokioJoin(error) if error.is_cancelled() => "codex_err:tokio_join:cancelled",
        CodexErr::TokioJoin(error) if error.is_panic() => "codex_err:tokio_join:panic",
        CodexErr::TokioJoin(_) => "codex_err:tokio_join:other",
        #[cfg(target_os = "linux")]
        CodexErr::LandlockRuleset(_) => "codex_err:landlock_ruleset",
        #[cfg(target_os = "linux")]
        CodexErr::LandlockPathFd(_) => "codex_err:landlock_path_fd",
        CodexErr::UnexpectedStatus(error) => {
            return fingerprint_with_http_status(
                "codex_err:unexpected_status",
                Some(error.status.as_u16()),
            );
        }
        CodexErr::RetryLimit(error) => {
            return fingerprint_with_http_status(
                "codex_err:retry_limit",
                Some(error.status.as_u16()),
            );
        }
        CodexErr::ConnectionFailed(error) => {
            return fingerprint_with_http_status(
                "codex_err:connection_failed",
                error.source.status().map(|status| status.as_u16()),
            );
        }
        CodexErr::ResponseStreamFailed(error) => {
            return fingerprint_with_http_status(
                "codex_err:response_stream_failed",
                error.source.status().map(|status| status.as_u16()),
            );
        }
        CodexErr::Sandbox(error) => return stable_sandbox_error_fingerprint(error),
    };
    fingerprint.to_string()
}

fn fingerprint_with_http_status(kind: &str, status: Option<u16>) -> String {
    match status {
        Some(status) => format!("{kind}:http_{status}"),
        None => format!("{kind}:http_unknown"),
    }
}

fn stable_io_error_fingerprint(kind: std::io::ErrorKind) -> String {
    let kind = match kind {
        std::io::ErrorKind::NotFound => "not_found",
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::ConnectionRefused => "connection_refused",
        std::io::ErrorKind::ConnectionReset => "connection_reset",
        std::io::ErrorKind::ConnectionAborted => "connection_aborted",
        std::io::ErrorKind::NotConnected => "not_connected",
        std::io::ErrorKind::AddrInUse => "address_in_use",
        std::io::ErrorKind::AddrNotAvailable => "address_not_available",
        std::io::ErrorKind::BrokenPipe => "broken_pipe",
        std::io::ErrorKind::AlreadyExists => "already_exists",
        std::io::ErrorKind::WouldBlock => "would_block",
        std::io::ErrorKind::InvalidInput => "invalid_input",
        std::io::ErrorKind::InvalidData => "invalid_data",
        std::io::ErrorKind::TimedOut => "timed_out",
        std::io::ErrorKind::WriteZero => "write_zero",
        std::io::ErrorKind::Interrupted => "interrupted",
        std::io::ErrorKind::Unsupported => "unsupported",
        std::io::ErrorKind::UnexpectedEof => "unexpected_eof",
        std::io::ErrorKind::OutOfMemory => "out_of_memory",
        _ => "other",
    };
    format!("codex_err:io:{kind}")
}

fn stable_sandbox_error_fingerprint(error: &SandboxErr) -> String {
    let fingerprint = match error {
        SandboxErr::Denied { .. } => "codex_err:sandbox:denied",
        #[cfg(target_os = "linux")]
        SandboxErr::SeccompInstall(_) => "codex_err:sandbox:seccomp_install",
        #[cfg(target_os = "linux")]
        SandboxErr::SeccompBackend(_) => "codex_err:sandbox:seccomp_backend",
        SandboxErr::Timeout { .. } => "codex_err:sandbox:timeout",
        SandboxErr::LandlockRestrict => "codex_err:sandbox:landlock_restrict",
        SandboxErr::Signal(_) => "codex_err:sandbox:signal",
    };
    fingerprint.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::error::UnexpectedResponseError;
    use pretty_assertions::assert_eq;

    #[test]
    fn broad_protocol_errors_keep_distinct_source_fingerprints() {
        let turn_aborted = CodexErr::TurnAborted;
        let request_timeout = CodexErr::RequestTimeout;

        assert_eq!(
            turn_aborted.to_codex_protocol_error(),
            request_timeout.to_codex_protocol_error()
        );
        assert_ne!(
            stable_turn_error_fingerprint(&turn_aborted),
            stable_turn_error_fingerprint(&request_timeout)
        );
    }

    #[test]
    fn fingerprint_omits_error_payloads_and_request_ids() {
        let error = CodexErr::UnexpectedStatus(UnexpectedResponseError {
            status: reqwest::StatusCode::BAD_GATEWAY,
            body: "secret response body".to_string(),
            url: Some("https://example.invalid/private".to_string()),
            cf_ray: Some("volatile-ray".to_string()),
            request_id: Some("volatile-request-id".to_string()),
            identity_authorization_error: Some("secret authorization detail".to_string()),
            identity_error_code: Some("volatile-code".to_string()),
        });

        assert_eq!(
            "codex_err:unexpected_status:http_502",
            stable_turn_error_fingerprint(&error)
        );
    }
}
