//! TUI-owned dynamic tool support for agent-requested MCP management.

use super::App;
use super::ui_dynamic_tools::UiDynamicToolOutcome;
use super::ui_mcp_tool_helpers::*;
use crate::app_server_session::AppServerSession;
use crate::chatwidget::McpAgentMutationApprovalSummary;
use codex_app_server_protocol::RequestId as AppServerRequestId;
use codex_config::types::McpServerConfig;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::collections::HashMap;

const RESERVED_MCP_SERVER_NAMES: &[&str] = &["codex_apps"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct McpArgs {
    pub(super) action: String,
    name: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env_vars: Option<Vec<String>>,
    url: Option<String>,
    bearer_token_env_var: Option<String>,
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
    enabled_tools: Option<Vec<String>>,
    disabled_tools: Option<Vec<String>>,
    default_tools_approval_mode: Option<String>,
    server: Option<String>,
    tool: Option<String>,
    enabled: Option<bool>,
}

#[derive(Debug, Clone)]
pub(super) struct PendingMcpDynamicToolMutation {
    mutation: McpDynamicToolMutation,
    approval: McpAgentMutationApprovalSummary,
}

#[derive(Debug, Clone)]
enum McpDynamicToolMutation {
    AddServer {
        name: String,
        config: JsonValue,
    },
    SetServerEnabled {
        name: String,
        enabled: bool,
    },
    SetToolEnabled {
        server: String,
        tool: String,
        enabled: bool,
    },
}

impl App {
    pub(super) async fn handle_mcp_tool(
        &mut self,
        app_server: &mut AppServerSession,
        request_id: AppServerRequestId,
        args: McpArgs,
    ) -> Result<UiDynamicToolOutcome, String> {
        match args.action.as_str() {
            "open" => {
                self.chat_widget.open_mcp_control_center();
                Ok(UiDynamicToolOutcome::Respond(json!({ "opened": "mcp" })))
            }
            "list" => Ok(UiDynamicToolOutcome::Respond(json!({
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
            }))),
            "reload" => {
                self.reload_mcp_servers(app_server);
                Ok(UiDynamicToolOutcome::Respond(json!({
                    "queued": "mcp_reload",
                    "refresh": "Loaded threads pick up refreshed MCP tools before the next turn.",
                })))
            }
            "add_stdio" | "add_streamable_http" | "set_server_enabled" | "set_tool_enabled" => {
                if !self.pending_mcp_dynamic_tool_mutations.is_empty() {
                    return Err(
                        "another agent-requested MCP config change is awaiting user approval"
                            .to_string(),
                    );
                }
                let pending = self.build_mcp_dynamic_tool_mutation(args)?;
                self.chat_widget.open_mcp_agent_mutation_confirmation(
                    request_id.clone(),
                    pending.approval.clone(),
                );
                self.pending_mcp_dynamic_tool_mutations
                    .insert(request_id, pending);
                Ok(UiDynamicToolOutcome::Pending)
            }
            "add" => Err("use action=add_stdio or action=add_streamable_http".to_string()),
            action => Err(format!(
                "unknown action `{action}`; expected open, list, reload, add_stdio, add_streamable_http, set_server_enabled, or set_tool_enabled"
            )),
        }
    }

    pub(super) async fn confirm_agent_mcp_mutation(
        &mut self,
        app_server: &mut AppServerSession,
        request_id: AppServerRequestId,
    ) {
        let Some(pending) = self.pending_mcp_dynamic_tool_mutations.remove(&request_id) else {
            self.chat_widget.add_error_message(
                "No pending agent-requested MCP config change matches this approval.".to_string(),
            );
            return;
        };

        let result = self
            .apply_mcp_dynamic_tool_mutation(app_server, pending)
            .await;
        let response = match result {
            Ok(output) => {
                super::ui_dynamic_tools::dynamic_tool_response(output, /*success*/ true)
            }
            Err(message) => {
                self.chat_widget.add_error_message(message.clone());
                super::ui_dynamic_tools::dynamic_tool_response(
                    json!({ "approved": true, "error": message }),
                    /*success*/ false,
                )
            }
        };
        self.resolve_ui_dynamic_tool_request(app_server, request_id, response)
            .await;
    }

    pub(super) async fn deny_agent_mcp_mutation(
        &mut self,
        app_server: &AppServerSession,
        request_id: AppServerRequestId,
    ) {
        if self
            .pending_mcp_dynamic_tool_mutations
            .remove(&request_id)
            .is_none()
        {
            self.chat_widget.add_error_message(
                "No pending agent-requested MCP config change matches this denial.".to_string(),
            );
            return;
        }
        self.chat_widget.add_info_message(
            "Agent-requested MCP config change denied.".to_string(),
            None,
        );
        let response = super::ui_dynamic_tools::dynamic_tool_response(
            json!({
                "approved": false,
                "denied": true,
                "message": "The user denied the MCP config change.",
            }),
            /*success*/ false,
        );
        self.resolve_ui_dynamic_tool_request(app_server, request_id, response)
            .await;
    }

    pub(super) fn clear_pending_agent_mcp_mutation(&mut self, request_id: &AppServerRequestId) {
        if self
            .pending_mcp_dynamic_tool_mutations
            .remove(request_id)
            .is_some()
        {
            self.chat_widget.dismiss_mcp_agent_mutation_confirmation();
        }
    }

    fn build_mcp_dynamic_tool_mutation(
        &self,
        args: McpArgs,
    ) -> Result<PendingMcpDynamicToolMutation, String> {
        match args.action.as_str() {
            "add_stdio" => self.build_stdio_mcp_mutation(args),
            "add_streamable_http" => self.build_streamable_http_mcp_mutation(args),
            "set_server_enabled" => self.build_server_enablement_mcp_mutation(args),
            "set_tool_enabled" => self.build_tool_enablement_mcp_mutation(args),
            action => Err(format!("action `{action}` is not an MCP config mutation")),
        }
    }

    fn build_stdio_mcp_mutation(
        &self,
        args: McpArgs,
    ) -> Result<PendingMcpDynamicToolMutation, String> {
        let name = self.validate_new_agent_mcp_server_name(required_string(
            args.name,
            "action=add_stdio requires name",
        )?)?;
        let command = required_string(args.command, "action=add_stdio requires command")?;
        validate_argv_command(&command)?;
        let argv = normalize_args(args.args)?;
        let env_vars = normalize_env_var_names(args.env_vars.unwrap_or_default())?;
        let cwd = args
            .cwd
            .map(|cwd| required_string(Some(cwd), "cwd must not be empty"))
            .transpose()?;
        let tool_options = normalize_tool_options(
            args.enabled_tools,
            args.disabled_tools,
            args.default_tools_approval_mode,
        )?;

        let mut config = json!({
            "command": command,
            "args": argv,
            "enabled": true,
        });
        if !env_vars.is_empty() {
            config["env_vars"] = json!(env_vars);
        }
        if let Some(cwd) = &cwd {
            config["cwd"] = json!(cwd);
        }
        apply_tool_options_to_config(&mut config, &tool_options)?;

        let title = format!("Add MCP server `{name}`");
        let scope_and_refresh = format!("{}; {}", mcp_server_scope_label(), refresh_label());
        Ok(pending_add_server(
            name.clone(),
            config,
            vec![
                ("Server", name),
                ("Transport", format!("stdio; command: {command}")),
                (
                    "Args / cwd",
                    format!(
                        "args: {}; cwd: {}",
                        list_or_none(&argv),
                        cwd.unwrap_or_else(|| "not set".to_string())
                    ),
                ),
                (
                    "Env / headers",
                    format!(
                        "env vars: {}; headers: not applicable",
                        list_or_none(&env_vars)
                    ),
                ),
                (
                    "Tool config",
                    format!(
                        "default approval: {}; enabled: {}; disabled: {}",
                        tool_options.approval_label(),
                        list_or_none(&tool_options.enabled_tools),
                        list_or_none(&tool_options.disabled_tools)
                    ),
                ),
                ("Scope / refresh", scope_and_refresh),
            ],
            title,
        ))
    }

    fn build_streamable_http_mcp_mutation(
        &self,
        args: McpArgs,
    ) -> Result<PendingMcpDynamicToolMutation, String> {
        let name = self.validate_new_agent_mcp_server_name(required_string(
            args.name,
            "action=add_streamable_http requires name",
        )?)?;
        let url = required_string(args.url, "action=add_streamable_http requires url")?;
        validate_mcp_http_url(&url)?;
        let bearer_token_env_var = args
            .bearer_token_env_var
            .map(|name| validate_env_var_name(&name).map(|_| name))
            .transpose()?;
        let http_headers = normalize_plain_http_headers(args.http_headers.unwrap_or_default())?;
        let env_http_headers =
            normalize_env_http_headers(args.env_http_headers.unwrap_or_default())?;
        let tool_options = normalize_tool_options(
            args.enabled_tools,
            args.disabled_tools,
            args.default_tools_approval_mode,
        )?;

        let mut config = json!({
            "url": url,
            "enabled": true,
        });
        if let Some(bearer_token_env_var) = &bearer_token_env_var {
            config["bearer_token_env_var"] = json!(bearer_token_env_var);
        }
        if !http_headers.is_empty() {
            config["http_headers"] = json!(http_headers);
        }
        if !env_http_headers.is_empty() {
            config["env_http_headers"] = json!(env_http_headers);
        }
        apply_tool_options_to_config(&mut config, &tool_options)?;

        let header_summary = http_header_summary(&http_headers, &env_http_headers);
        let title = format!("Add MCP server `{name}`");
        let scope_and_refresh = format!("{}; {}", mcp_server_scope_label(), refresh_label());
        Ok(pending_add_server(
            name.clone(),
            config,
            vec![
                ("Server", name),
                ("Transport", format!("streamable_http; url: {url}")),
                (
                    "Auth / headers",
                    format!(
                        "bearer env: {}; headers: {header_summary}",
                        optional_or_none(bearer_token_env_var)
                    ),
                ),
                (
                    "Env / cwd",
                    "env vars: not applicable; cwd: not applicable".to_string(),
                ),
                (
                    "Tool config",
                    format!(
                        "default approval: {}; enabled: {}; disabled: {}",
                        tool_options.approval_label(),
                        list_or_none(&tool_options.enabled_tools),
                        list_or_none(&tool_options.disabled_tools)
                    ),
                ),
                ("Scope / refresh", scope_and_refresh),
            ],
            title,
        ))
    }

    fn build_server_enablement_mcp_mutation(
        &self,
        args: McpArgs,
    ) -> Result<PendingMcpDynamicToolMutation, String> {
        let name = required_string(args.name, "action=set_server_enabled requires name")?;
        let config = self.direct_mcp_server_config(&name)?;
        let enabled = args
            .enabled
            .ok_or_else(|| "action=set_server_enabled requires enabled".to_string())?;
        let title = format!(
            "{} MCP server `{name}`",
            if enabled { "Enable" } else { "Disable" }
        );
        let mut rows = existing_server_approval_rows(&name, config);
        rows.extend([
            (
                "Change".to_string(),
                if enabled {
                    "enabled = true"
                } else {
                    "enabled = false"
                }
                .to_string(),
            ),
            ("Persistence scope".to_string(), mcp_server_scope_label()),
            ("Refresh".to_string(), refresh_label()),
        ]);
        Ok(PendingMcpDynamicToolMutation {
            mutation: McpDynamicToolMutation::SetServerEnabled {
                name: name.clone(),
                enabled,
            },
            approval: McpAgentMutationApprovalSummary { title, rows },
        })
    }

    fn build_tool_enablement_mcp_mutation(
        &self,
        args: McpArgs,
    ) -> Result<PendingMcpDynamicToolMutation, String> {
        let server = required_string(args.server, "action=set_tool_enabled requires server")?;
        let config = self.direct_mcp_server_config(&server)?;
        let tool = required_string(args.tool, "action=set_tool_enabled requires tool")?;
        validate_tool_name(&tool)?;
        let enabled = args
            .enabled
            .ok_or_else(|| "action=set_tool_enabled requires enabled".to_string())?;
        let title = format!(
            "{} MCP tool `{server}.{tool}`",
            if enabled { "Enable" } else { "Disable" }
        );
        let mut rows = existing_server_approval_rows(&server, config);
        rows.extend([
            ("Tool".to_string(), tool.clone()),
            (
                "Change".to_string(),
                if enabled {
                    "remove from disabled_tools / add to enabled_tools"
                } else {
                    "add to disabled_tools / remove from enabled_tools"
                }
                .to_string(),
            ),
            ("Persistence scope".to_string(), mcp_server_scope_label()),
            ("Refresh".to_string(), refresh_label()),
        ]);
        Ok(PendingMcpDynamicToolMutation {
            mutation: McpDynamicToolMutation::SetToolEnabled {
                server: server.clone(),
                tool,
                enabled,
            },
            approval: McpAgentMutationApprovalSummary { title, rows },
        })
    }

    async fn apply_mcp_dynamic_tool_mutation(
        &mut self,
        app_server: &mut AppServerSession,
        pending: PendingMcpDynamicToolMutation,
    ) -> Result<JsonValue, String> {
        match pending.mutation {
            McpDynamicToolMutation::AddServer { name, config } => {
                self.add_mcp_server_from_config(app_server, name.clone(), config)
                    .await?;
                Ok(json!({
                    "approved": true,
                    "action": "add_server",
                    "server": name,
                    "refresh": refresh_label(),
                    "available_next_turn": true,
                }))
            }
            McpDynamicToolMutation::SetServerEnabled { name, enabled } => {
                self.set_mcp_server_enabled(app_server, name.clone(), enabled)
                    .await?;
                Ok(json!({
                    "approved": true,
                    "action": "set_server_enabled",
                    "server": name,
                    "enabled": enabled,
                    "refresh": refresh_label(),
                    "available_next_turn": true,
                }))
            }
            McpDynamicToolMutation::SetToolEnabled {
                server,
                tool,
                enabled,
            } => {
                self.set_mcp_tool_enabled(app_server, server.clone(), tool.clone(), enabled)
                    .await?;
                Ok(json!({
                    "approved": true,
                    "action": "set_tool_enabled",
                    "server": server,
                    "tool": tool,
                    "enabled": enabled,
                    "refresh": refresh_label(),
                    "available_next_turn": true,
                }))
            }
        }
    }

    fn validate_new_agent_mcp_server_name(&self, name: String) -> Result<String, String> {
        super::mcp_config_actions::validate_mcp_server_name(&name)?;
        if RESERVED_MCP_SERVER_NAMES
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
        {
            return Err(format!(
                "MCP server name `{name}` is reserved for host-managed tools"
            ));
        }
        if self.config.mcp_servers.get().contains_key(&name) {
            return Err(format!(
                "MCP server `{name}` already exists; use set_server_enabled or set_tool_enabled for existing direct config"
            ));
        }
        Ok(name)
    }

    fn direct_mcp_server_config(&self, name: &str) -> Result<&McpServerConfig, String> {
        super::mcp_config_actions::validate_mcp_server_name(name)?;
        self.config
            .mcp_servers
            .get()
            .get(name)
            .ok_or_else(|| format!("MCP server `{name}` is not directly configured in mcp_servers"))
    }
}

fn pending_add_server(
    name: String,
    config: JsonValue,
    rows: Vec<(&'static str, String)>,
    title: String,
) -> PendingMcpDynamicToolMutation {
    PendingMcpDynamicToolMutation {
        mutation: McpDynamicToolMutation::AddServer { name, config },
        approval: approval_summary(title, rows),
    }
}

fn approval_summary(
    title: String,
    rows: Vec<(&'static str, String)>,
) -> McpAgentMutationApprovalSummary {
    McpAgentMutationApprovalSummary {
        title,
        rows: rows
            .into_iter()
            .map(|(label, value)| (label.to_string(), value))
            .collect(),
    }
}
