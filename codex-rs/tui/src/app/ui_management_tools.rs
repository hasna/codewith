//! App-management dynamic tool handlers owned by the TUI.

use super::App;
use crate::app_server_session::AppServerSession;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_config::types::McpServerTransportConfig;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub(super) struct BackgroundTerminalsArgs {
    pub(super) action: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct McpArgs {
    pub(super) action: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct BackgroundAgentsArgs {
    pub(super) action: String,
    pub(super) agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SchedulesArgs {
    pub(super) action: String,
    pub(super) kind: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct MonitorsArgs {
    pub(super) action: String,
    pub(super) monitor_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SessionControlArgs {
    pub(super) action: String,
    pub(super) prompt: Option<String>,
    pub(super) name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CapabilitiesArgs {
    pub(super) action: String,
}

impl App {
    pub(super) async fn handle_background_terminals_tool(
        &mut self,
        args: BackgroundTerminalsArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "open" => {
                self.chat_widget.open_background_terminal_manager();
                Ok(json!({ "opened": "background_terminals" }))
            }
            "list" => Ok(json!({
                "processes": background_terminal_processes_json(
                    self.chat_widget.background_terminal_processes(),
                ),
            })),
            "stop_all" => {
                self.chat_widget
                    .open_background_terminal_stop_confirmation();
                Ok(json!({
                    "queued": "background_terminal_stop_confirmation",
                    "requires_user_confirmation": true,
                }))
            }
            action => Err(unknown_action_with_expected(
                action,
                "open, list, or stop_all (opens user confirmation)",
            )),
        }
    }

    pub(super) async fn handle_mcp_tool(
        &mut self,
        _app_server: &mut AppServerSession,
        args: McpArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "open" => {
                self.chat_widget.open_mcp_control_center();
                Ok(json!({ "opened": "mcp" }))
            }
            "list" => Ok(json!({
                "servers": self
                    .config
                    .mcp_servers
                    .get()
                    .iter()
                    .map(|(name, config)| json!({
                        "name": name,
                        "enabled": config.enabled,
                        "required": config.required,
                        "supports_parallel_tool_calls": config.supports_parallel_tool_calls,
                        "enabled_tools": config.enabled_tools,
                        "disabled_tools": config.disabled_tools,
                        "transport": redacted_mcp_transport(&config.transport),
                    }))
                    .collect::<Vec<_>>(),
            })),
            "add" | "set_server_enabled" | "set_tool_enabled" | "reload" => {
                self.chat_widget.open_mcp_control_center();
                Err(interactive_user_confirmation_required(args.action.as_str()))
            }
            action => Err(unknown_action_with_expected(
                action,
                "open or list; MCP mutations require the interactive /mcp UI",
            )),
        }
    }

    pub(super) async fn handle_background_agents_tool(
        &mut self,
        app_server: &mut AppServerSession,
        args: BackgroundAgentsArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "open" => {
                self.open_background_agent_manager(app_server).await;
                Ok(json!({ "opened": "background_agents" }))
            }
            "list" => {
                let thread_id = self.current_tool_thread_id()?.to_string();
                app_server
                    .agent_list()
                    .await
                    .map(|response| {
                        json!({
                            "agents": response
                                .data
                                .into_iter()
                                .filter(|agent| agent.parent_thread_id.as_deref() == Some(thread_id.as_str()))
                                .collect::<Vec<_>>()
                        })
                    })
                    .map_err(|err| format!("failed to list background agents: {err}"))
            }
            "read" | "logs" => {
                let agent_id =
                    required_string(args.agent_id, "action=read/logs requires agent_id")?;
                let thread_id = self.current_tool_thread_id()?.to_string();
                let response = app_server
                    .agent_read(agent_id.clone())
                    .await
                    .map_err(|err| format!("failed to read background agent: {err}"))?;
                let Some(agent) = response.agent.as_ref() else {
                    return Err(format!("background agent `{agent_id}` was not found"));
                };
                if agent.parent_thread_id.as_deref() != Some(thread_id.as_str()) {
                    return Err(format!(
                        "background agent `{agent_id}` does not belong to the current thread"
                    ));
                }
                if args.action == "read" {
                    Ok(json!(response))
                } else {
                    app_server
                        .agent_events_list(agent_id)
                        .await
                        .map(|response| json!(response))
                        .map_err(|err| format!("failed to list background agent logs: {err}"))
                }
            }
            "start" | "attach" | "detach" | "stop" | "delete" | "diagnostics" => {
                Err(interactive_user_confirmation_required(args.action.as_str()))
            }
            action => Err(unknown_action_with_expected(
                action,
                "open, list, read, or logs; agent mutations require the interactive /agent UI",
            )),
        }
    }

    pub(super) async fn handle_schedules_tool(
        &mut self,
        app_server: &mut AppServerSession,
        args: SchedulesArgs,
    ) -> Result<JsonValue, String> {
        let thread_id = self.current_tool_thread_id()?;
        match args.action.as_str() {
            "open" => match args.kind.as_deref().unwrap_or("once") {
                "once" | "schedule" => {
                    self.open_thread_schedule_manager(app_server, thread_id)
                        .await;
                    Ok(json!({ "opened": "schedules" }))
                }
                "loop" | "loops" => {
                    self.open_thread_loop_manager(app_server, thread_id).await;
                    Ok(json!({ "opened": "loops" }))
                }
                "all" => Err("action=open requires kind=once or kind=loop".to_string()),
                kind => Err(format!(
                    "unknown schedule kind `{kind}`; expected once, loop, or all"
                )),
            },
            "list" => {
                let kind = args.kind.unwrap_or_else(|| "all".to_string());
                let response = app_server
                    .thread_schedule_list(thread_id)
                    .await
                    .map_err(|err| format!("failed to list schedules: {err}"))?;
                let data = response
                    .data
                    .into_iter()
                    .filter(|schedule| schedule_matches_kind(schedule, &kind))
                    .collect::<Vec<_>>();
                Ok(json!({ "kind": kind, "schedules": data }))
            }
            "create" | "pause" | "resume" | "delete" | "run_now" => {
                Err(interactive_user_confirmation_required(args.action.as_str()))
            }
            action => Err(unknown_action_with_expected(
                action,
                "open or list; schedule mutations require the interactive /schedule or /loop UI",
            )),
        }
    }

    pub(super) async fn handle_monitors_tool(
        &mut self,
        app_server: &mut AppServerSession,
        args: MonitorsArgs,
    ) -> Result<JsonValue, String> {
        let thread_id = self.current_tool_thread_id()?;
        match args.action.as_str() {
            "open" => {
                self.open_thread_monitor_manager(app_server, thread_id)
                    .await;
                Ok(json!({ "opened": "monitors" }))
            }
            "list" => app_server
                .thread_monitor_list(thread_id)
                .await
                .map(|response| json!({ "monitors": response.data }))
                .map_err(|err| format!("failed to list monitors: {err}")),
            "read" => {
                let monitor_id =
                    required_string(args.monitor_id, "action=read requires monitor_id")?;
                app_server
                    .thread_monitor_read(thread_id, monitor_id)
                    .await
                    .map(|response| json!(response))
                    .map_err(|err| format!("failed to read monitor: {err}"))
            }
            "stop" | "restart" | "delete" => {
                Err(interactive_user_confirmation_required(args.action.as_str()))
            }
            action => Err(unknown_action_with_expected(
                action,
                "open, list, or read; monitor mutations require the interactive /monitor UI",
            )),
        }
    }

    pub(super) async fn handle_session_control_tool(
        &mut self,
        args: SessionControlArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "recap" => {
                let thread_id = self.current_tool_thread_id()?;
                self.chat_widget
                    .add_info_message("Generating recap...".to_string(), /*hint*/ None);
                self.app_event_tx
                    .send(crate::app_event::AppEvent::RequestSessionRecap {
                        thread_id,
                        prompt: args.prompt,
                        automatic: false,
                    });
                Ok(json!({ "queued": "recap" }))
            }
            "compact" => {
                self.chat_widget.clear_token_usage();
                self.app_event_tx.compact();
                Ok(json!({ "queued": "compact" }))
            }
            "fork" => {
                self.app_event_tx
                    .send(crate::app_event::AppEvent::ForkCurrentSession);
                Ok(json!({ "queued": "fork" }))
            }
            "rename" => {
                let name = required_string(args.name, "action=rename requires name")?;
                let Some(name) = crate::legacy_core::util::normalize_thread_name(&name) else {
                    return Err("thread name cannot be empty".to_string());
                };
                self.app_event_tx.set_thread_name(name.clone());
                Ok(json!({ "queued": "rename", "name": name }))
            }
            action => Err(unknown_action_with_expected(
                action,
                "recap, compact, fork, or rename",
            )),
        }
    }

    pub(super) async fn handle_capabilities_tool(
        &mut self,
        args: CapabilitiesArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "inspect" => Ok(self.capabilities_snapshot()),
            "propose_upgrade" => Ok(json!({
                "current": self.capabilities_snapshot(),
                "proposals": self.capability_upgrade_proposals(),
            })),
            action => Err(unknown_action_with_expected(
                action,
                "inspect or propose_upgrade",
            )),
        }
    }

    fn current_tool_thread_id(&self) -> Result<codex_protocol::ThreadId, String> {
        self.chat_widget.thread_id().ok_or_else(|| {
            "this action requires an active app-server-backed session thread".to_string()
        })
    }

    fn capabilities_snapshot(&self) -> JsonValue {
        let file_system = self.config.permissions.file_system_sandbox_policy();
        let network = self.config.permissions.network_sandbox_policy();
        json!({
            "approval_policy": self.config.permissions.approval_policy.value(),
            "approvals_reviewer": self.config.approvals_reviewer,
            "active_permission_profile": self.config.permissions.active_permission_profile(),
            "permission_profile": self.config.permissions.effective_permission_profile(),
            "filesystem": {
                "kind": file_system.kind,
                "entries": file_system.entries,
            },
            "network": network,
            "workspace_roots": self
                .config
                .permissions
                .user_visible_workspace_roots()
                .iter()
                .map(|root| root.to_string_lossy().to_string())
                .collect::<Vec<_>>(),
            "tools": {
                "background_terminals": true,
                "mcp": true,
                "background_agents": true,
                "schedules": true,
                "monitors": true,
                "session_control": true,
                "capabilities": true,
            },
            "mutation_policy": {
                "destructive_actions_require_confirmation": true,
                "permission_changes_are_read_only_suggestions": true,
            }
        })
    }

    fn capability_upgrade_proposals(&self) -> Vec<JsonValue> {
        let file_system = self.config.permissions.file_system_sandbox_policy();
        let network = self.config.permissions.network_sandbox_policy();
        let mut proposals = Vec::new();

        if !network.is_enabled() {
            proposals.push(json!({
                "id": "network-needed",
                "title": "Enable network only when a task needs it",
                "rationale": "Package installs, remote docs, and HTTP MCP servers need network access. Keep it restricted otherwise.",
                "how": "Use /permissions and choose a profile or project policy that grants the needed network scope."
            }));
        }

        if !matches!(
            file_system.kind,
            codex_protocol::permissions::FileSystemSandboxKind::Unrestricted
        ) {
            proposals.push(json!({
                "id": "workspace-write",
                "title": "Use a workspace-write permission profile for implementation tasks",
                "rationale": "Code edits, generated tests, and local build artifacts need write access to the workspace.",
                "how": "Use /permissions and choose the workspace profile, or define a custom profile with the exact writable roots."
            }));
        }

        if self.config.permissions.approval_policy.value()
            != codex_protocol::protocol::AskForApproval::OnRequest
        {
            proposals.push(json!({
                "id": "approval-on-request",
                "title": "Prefer on-request approval for autonomous app operations",
                "rationale": "The agent can proceed through safe local work while still asking before operations that need escalation.",
                "how": "Use /permissions to select an approval policy that matches the session risk."
            }));
        }

        if proposals.is_empty() {
            proposals.push(json!({
                "id": "no-upgrade-needed",
                "title": "Current capability posture is sufficient",
                "rationale": "The current permissions already provide broad filesystem/network capability for app-owned actions.",
                "how": "No automatic permission change is proposed."
            }));
        }

        proposals
    }
}

fn unknown_action_with_expected(action: &str, expected: &str) -> String {
    format!("unknown action `{action}`; expected {expected}")
}

fn required_string(value: Option<String>, message: &'static str) -> Result<String, String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| message.to_string())
}

fn interactive_user_confirmation_required(action: &str) -> String {
    format!(
        "action={action} requires interactive user confirmation; open the matching manager UI instead"
    )
}

fn redacted_mcp_transport(transport: &McpServerTransportConfig) -> JsonValue {
    match transport {
        McpServerTransportConfig::Stdio {
            command,
            args,
            env,
            env_vars,
            cwd,
        } => json!({
            "type": "stdio",
            "command": command,
            "args_count": args.len(),
            "env": env
                .as_ref()
                .map(|env| redacted_keys(env.keys()))
                .unwrap_or_default(),
            "env_vars": env_vars
                .iter()
                .map(|env_var| env_var.name().to_string())
                .collect::<Vec<_>>(),
            "cwd_configured": cwd.is_some(),
        }),
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => json!({
            "type": "streamable_http",
            "url_configured": !url.is_empty(),
            "bearer_token_env_var": bearer_token_env_var,
            "http_headers": http_headers
                .as_ref()
                .map(|headers| redacted_keys(headers.keys()))
                .unwrap_or_default(),
            "env_http_headers": env_http_headers,
        }),
    }
}

fn redacted_keys<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<JsonValue> {
    let mut keys = keys.collect::<Vec<_>>();
    keys.sort();
    keys.into_iter()
        .map(|key| {
            json!({
                "name": key,
                "value": "<redacted>",
            })
        })
        .collect()
}

fn background_terminal_processes_json(
    processes: Vec<crate::history_cell::UnifiedExecProcessDetails>,
) -> Vec<JsonValue> {
    processes
        .into_iter()
        .map(|process| {
            json!({
                "command_display": process.command_display,
                "recent_output_available": !process.recent_chunks.is_empty(),
                "recent_output_chunk_count": process.recent_chunks.len(),
            })
        })
        .collect()
}

fn schedule_matches_kind(schedule: &codex_app_server_protocol::ThreadSchedule, kind: &str) -> bool {
    match kind {
        "all" => true,
        "once" | "schedule" => matches!(schedule.schedule, ThreadScheduleSpec::Once),
        "loop" | "loops" => !matches!(schedule.schedule, ThreadScheduleSpec::Once),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;

    #[test]
    fn interactive_user_confirmation_message_is_explicit() {
        assert_eq!(
            interactive_user_confirmation_required("delete"),
            "action=delete requires interactive user confirmation; open the matching manager UI instead"
        );
    }

    #[test]
    fn mcp_transport_summary_redacts_raw_secret_values() {
        let stdio = McpServerTransportConfig::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "server".to_string(),
                "--token=arg-secret".to_string(),
            ],
            env: Some(HashMap::from([(
                "API_KEY".to_string(),
                "sk-secret".to_string(),
            )])),
            env_vars: vec![codex_config::types::McpServerEnvVar::Name(
                "SAFE_ENV_NAME".to_string(),
            )],
            cwd: Some(std::path::PathBuf::from("/tmp/secret-project")),
        };
        let summary = redacted_mcp_transport(&stdio);
        assert_eq!(summary["args_count"], 3);
        assert_eq!(summary["env"][0]["name"], "API_KEY");
        assert_eq!(summary["env"][0]["value"], "<redacted>");
        assert_eq!(summary["env_vars"][0], "SAFE_ENV_NAME");
        assert_eq!(summary["cwd_configured"], true);
        let rendered = summary.to_string();
        assert!(!rendered.contains("sk-secret"));
        assert!(!rendered.contains("arg-secret"));
        assert!(!rendered.contains("secret-project"));

        let http = McpServerTransportConfig::StreamableHttp {
            url: "https://example.com/mcp?token=query-secret".to_string(),
            bearer_token_env_var: Some("MCP_TOKEN".to_string()),
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer raw-secret".to_string(),
            )])),
            env_http_headers: Some(HashMap::from([(
                "X-Api-Key".to_string(),
                "MCP_API_KEY".to_string(),
            )])),
        };
        let summary = redacted_mcp_transport(&http);
        assert_eq!(summary["http_headers"][0]["name"], "Authorization");
        assert_eq!(summary["http_headers"][0]["value"], "<redacted>");
        assert_eq!(summary["url_configured"], true);
        assert_eq!(summary["bearer_token_env_var"], "MCP_TOKEN");
        let rendered = summary.to_string();
        assert!(!rendered.contains("raw-secret"));
        assert!(!rendered.contains("query-secret"));
    }

    #[test]
    fn background_terminal_process_list_is_metadata_only() {
        let processes = vec![crate::history_cell::UnifiedExecProcessDetails {
            command_display: "bun run dev".to_string(),
            recent_chunks: vec!["TOKEN=secret".to_string()],
        }];

        let output = background_terminal_processes_json(processes);
        assert_eq!(output[0]["command_display"], "bun run dev");
        assert_eq!(output[0]["recent_output_available"], true);
        assert_eq!(output[0]["recent_output_chunk_count"], 1);
        assert!(!output[0].to_string().contains("TOKEN=secret"));
    }
}
