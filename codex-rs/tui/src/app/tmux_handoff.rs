use super::App;
use super::rollout_path_is_resumable;
use crate::AppServerTarget;
use crate::tmux_handoff::TmuxHandoffDestination;
use crate::tmux_handoff::TmuxHandoffLaunchOptions;
use crate::tmux_handoff::TmuxHandoffSummary;
use codex_app_server_client::RemoteAppServerEndpoint;
use codex_app_server_protocol::ConfigLayerSource;

impl App {
    pub(super) fn prepare_tmux_handoff_from_slash(
        &mut self,
        destination: TmuxHandoffDestination,
        replace_existing: bool,
    ) -> Result<TmuxHandoffSummary, String> {
        self.prepare_tmux_handoff(
            destination,
            replace_existing,
            TmuxHandoffRunningTurnPolicy::Reject,
        )
    }

    pub(super) fn prepare_tmux_handoff_from_tool(
        &mut self,
        destination: TmuxHandoffDestination,
        replace_existing: bool,
    ) -> Result<TmuxHandoffSummary, String> {
        self.prepare_tmux_handoff(
            destination,
            replace_existing,
            TmuxHandoffRunningTurnPolicy::Allow,
        )
    }

    fn prepare_tmux_handoff(
        &mut self,
        destination: TmuxHandoffDestination,
        replace_existing: bool,
        running_turn_policy: TmuxHandoffRunningTurnPolicy,
    ) -> Result<TmuxHandoffSummary, String> {
        if running_turn_policy == TmuxHandoffRunningTurnPolicy::Reject
            && self.chat_widget.user_turn_pending_or_running()
        {
            return Err("`/tmux` is unavailable while Codewith is working.".to_string());
        }
        let Some(thread_id) = self.chat_widget.thread_id() else {
            return Err("`/tmux` is unavailable before this session starts.".to_string());
        };
        let Some(rollout_path) = self.chat_widget.rollout_path() else {
            return Err("This session is not resumable yet.".to_string());
        };
        if !rollout_path_is_resumable(&rollout_path) {
            return Err(
                "This session has not been saved yet; send a message before using `/tmux`."
                    .to_string(),
            );
        }

        let plan = crate::tmux_handoff::build_tmux_handoff_plan(
            &self.config,
            thread_id,
            destination,
            replace_existing,
            self.chat_widget.current_model(),
            &self.tmux_handoff_launch_options()?,
        )?;
        let summary = crate::tmux_handoff::create_tmux_handoff_session(&plan)?;
        self.pending_tmux_handoff = Some(summary.exit());
        Ok(summary)
    }

    fn tmux_handoff_launch_options(&self) -> Result<TmuxHandoffLaunchOptions, String> {
        Ok(TmuxHandoffLaunchOptions {
            cli_config_overrides: self.cli_kv_overrides.clone(),
            config_profile: self.tmux_config_profile(),
            remote: self.tmux_remote_arg()?,
            approval_policy: self
                .runtime_approval_policy_override
                .map(codex_app_server_protocol::AskForApproval::to_core)
                .or(self.harness_overrides.approval_policy),
            sandbox_mode: self.harness_overrides.sandbox_mode,
            additional_writable_roots: self.harness_overrides.additional_writable_roots.clone(),
            bypass_hook_trust: self.config.bypass_hook_trust,
        })
    }

    fn tmux_config_profile(&self) -> Option<String> {
        self.loader_overrides
            .user_config_profile
            .as_ref()
            .map(|profile| profile.as_str().to_string())
            .or_else(
                || match &self.config.config_layer_stack.get_active_user_layer()?.name {
                    ConfigLayerSource::User {
                        profile: Some(profile),
                        ..
                    } => Some(profile.clone()),
                    _ => None,
                },
            )
    }

    fn tmux_remote_arg(&self) -> Result<Option<String>, String> {
        match &self.app_server_target {
            AppServerTarget::Embedded => Ok(None),
            AppServerTarget::LocalDaemon { endpoint } | AppServerTarget::Remote { endpoint } => {
                tmux_remote_arg_for_endpoint(endpoint).map(Some)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TmuxHandoffRunningTurnPolicy {
    Allow,
    Reject,
}

fn tmux_remote_arg_for_endpoint(endpoint: &RemoteAppServerEndpoint) -> Result<String, String> {
    match endpoint {
        RemoteAppServerEndpoint::UnixSocket { socket_path } => {
            Ok(format!("unix://{}", socket_path.display()))
        }
        RemoteAppServerEndpoint::WebSocket {
            websocket_url,
            auth_token,
        } => {
            if auth_token.is_some() {
                Err("`/tmux` cannot preserve token-authenticated remote app-server sessions without exposing the bearer token in the tmux command. Restart manually with `codewith --remote ... --remote-auth-token-env ... resume ...`.".to_string())
            } else {
                Ok(websocket_url.clone())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;

    #[test]
    fn tmux_remote_arg_preserves_unix_socket_endpoint() {
        let endpoint = RemoteAppServerEndpoint::UnixSocket {
            socket_path: test_path_buf("/tmp/codewith.sock").abs(),
        };

        assert_eq!(
            tmux_remote_arg_for_endpoint(&endpoint).expect("remote arg"),
            format!("unix://{}", test_path_buf("/tmp/codewith.sock").display())
        );
    }

    #[test]
    fn tmux_remote_arg_preserves_websocket_without_token() {
        let endpoint = RemoteAppServerEndpoint::WebSocket {
            websocket_url: "ws://127.0.0.1:3030/".to_string(),
            auth_token: None,
        };

        assert_eq!(
            tmux_remote_arg_for_endpoint(&endpoint).expect("remote arg"),
            "ws://127.0.0.1:3030/"
        );
    }

    #[test]
    fn tmux_remote_arg_rejects_token_authenticated_websocket() {
        let endpoint = RemoteAppServerEndpoint::WebSocket {
            websocket_url: "wss://example.com:443/".to_string(),
            auth_token: Some("secret-token".to_string()),
        };

        let error =
            tmux_remote_arg_for_endpoint(&endpoint).expect_err("token endpoint should be rejected");
        assert!(error.contains("cannot preserve token-authenticated remote"));
        assert!(!error.contains("secret-token"));
    }
}
