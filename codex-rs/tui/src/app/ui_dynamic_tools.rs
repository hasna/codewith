use super::App;
use super::ui_management_tools::ActiveSessionsArgs;
use super::ui_management_tools::BackgroundAgentsArgs;
use super::ui_management_tools::BackgroundTerminalsArgs;
use super::ui_management_tools::CapabilitiesArgs;
use super::ui_management_tools::McpArgs;
use super::ui_management_tools::MonitorsArgs;
use super::ui_management_tools::SchedulesArgs;
use super::ui_management_tools::SessionControlArgs;
use crate::app_server_session::AppServerSession;
use crate::bottom_pane::StatusLineItem;
use crate::bottom_pane::TerminalTitleItem;
use crate::common_config_options::CommonConfigOption;
use crate::common_config_options::CommonConfigSection;
use crate::common_config_options::common_config_options;
use crate::common_config_options::common_config_sections;
use crate::legacy_core::config::edit::ConfigEditsBuilder;
use crate::tmux_handoff::TmuxHandoffDestination;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::DynamicToolCallParams;
use codex_app_server_protocol::DynamicToolCallResponse;
use codex_app_server_protocol::RequestId as AppServerRequestId;
use codex_app_server_protocol::ServerRequest;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::collections::HashSet;
use strum::IntoEnumIterator;

#[derive(Debug, Deserialize)]
struct StatusSurfaceArgs {
    action: String,
    item_ids: Option<Vec<String>>,
    use_theme_colors: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ConfigArgs {
    action: String,
    updates: Option<Vec<ConfigUpdateArg>>,
}

#[derive(Debug, Deserialize)]
struct TmuxArgs {
    explicit_user_request: Option<bool>,
    name: Option<String>,
    session: Option<String>,
    window: Option<String>,
    replace: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ConfigUpdateArg {
    option_id: String,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct StatusSurfaceOption {
    id: String,
    description: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigSectionOutput {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

#[derive(Debug, Serialize)]
struct ConfigOptionOutput {
    id: &'static str,
    section_id: &'static str,
    label: &'static str,
    description: &'static str,
    enabled: bool,
    disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    disabled_reason: Option<&'static str>,
}

impl App {
    pub(super) async fn try_handle_ui_dynamic_tool_request(
        &mut self,
        app_server: &mut AppServerSession,
        request: &ServerRequest,
    ) -> bool {
        let ServerRequest::DynamicToolCall { request_id, params } = request else {
            return false;
        };
        if !crate::ui_dynamic_tools::is_owned_ui_tool(params.namespace.as_deref(), &params.tool) {
            return false;
        }

        let result = match self.validate_ui_dynamic_tool_thread(params) {
            Ok(()) => self.handle_ui_dynamic_tool_call(app_server, params).await,
            Err(err) => Err(err),
        };
        let response = match result {
            Ok(output) => dynamic_tool_response(output, /*success*/ true),
            Err(message) => {
                self.chat_widget.add_error_message(message.clone());
                dynamic_tool_response(json!({ "error": message }), /*success*/ false)
            }
        };
        self.resolve_ui_dynamic_tool_request(app_server, request_id.clone(), response)
            .await;
        true
    }

    async fn handle_ui_dynamic_tool_call(
        &mut self,
        app_server: &mut AppServerSession,
        params: &DynamicToolCallParams,
    ) -> Result<JsonValue, String> {
        match params.tool.as_str() {
            crate::ui_dynamic_tools::STATUSLINE_TOOL => {
                let args: StatusSurfaceArgs = parse_arguments(&params.arguments)?;
                self.handle_statusline_tool(args).await
            }
            crate::ui_dynamic_tools::TERMINAL_TITLE_TOOL => {
                let args: StatusSurfaceArgs = parse_arguments(&params.arguments)?;
                self.handle_terminal_title_tool(args).await
            }
            crate::ui_dynamic_tools::CONFIG_TOOL => {
                let args: ConfigArgs = parse_arguments(&params.arguments)?;
                self.handle_config_tool(app_server, args).await
            }
            crate::ui_dynamic_tools::TMUX_TOOL => {
                let args: TmuxArgs = parse_arguments(&params.arguments)?;
                self.handle_tmux_tool(args).await
            }
            crate::ui_dynamic_tools::BACKGROUND_TERMINALS_TOOL => {
                let args: BackgroundTerminalsArgs = parse_arguments(&params.arguments)?;
                self.handle_background_terminals_tool(args).await
            }
            crate::ui_dynamic_tools::MCP_TOOL => {
                let args: McpArgs = parse_arguments(&params.arguments)?;
                self.handle_mcp_tool(app_server, args).await
            }
            crate::ui_dynamic_tools::BACKGROUND_AGENTS_TOOL => {
                let args: BackgroundAgentsArgs = parse_arguments(&params.arguments)?;
                self.handle_background_agents_tool(app_server, args).await
            }
            crate::ui_dynamic_tools::ACTIVE_SESSIONS_TOOL => {
                let args: ActiveSessionsArgs = parse_arguments(&params.arguments)?;
                self.handle_active_sessions_tool(app_server, args).await
            }
            crate::ui_dynamic_tools::SCHEDULES_TOOL => {
                let args: SchedulesArgs = parse_arguments(&params.arguments)?;
                self.handle_schedules_tool(app_server, args).await
            }
            crate::ui_dynamic_tools::MONITORS_TOOL => {
                let args: MonitorsArgs = parse_arguments(&params.arguments)?;
                self.handle_monitors_tool(app_server, args).await
            }
            crate::ui_dynamic_tools::SESSION_CONTROL_TOOL => {
                let args: SessionControlArgs = parse_arguments(&params.arguments)?;
                self.handle_session_control_tool(args).await
            }
            crate::ui_dynamic_tools::CAPABILITIES_TOOL => {
                let args: CapabilitiesArgs = parse_arguments(&params.arguments)?;
                self.handle_capabilities_tool(args).await
            }
            _ => Err(format!("unsupported Codewith UI tool `{}`", params.tool)),
        }
    }

    fn validate_ui_dynamic_tool_thread(
        &self,
        params: &DynamicToolCallParams,
    ) -> Result<(), String> {
        validate_ui_dynamic_tool_thread_id(
            &params.tool,
            &params.thread_id,
            self.chat_widget.thread_id(),
        )
    }

    async fn handle_statusline_tool(
        &mut self,
        args: StatusSurfaceArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "list_options" => Ok(json!({
                "surface": "statusline",
                "current": {
                    "item_ids": self.chat_widget.configured_status_line_items(),
                    "use_theme_colors": self.config.tui_status_line_use_colors,
                },
                "options": StatusLineItem::iter()
                    .map(|item| StatusSurfaceOption {
                        id: item.to_string(),
                        description: item.description(),
                    })
                    .collect::<Vec<_>>(),
            })),
            "set" => {
                let item_ids = required_item_ids(args.item_ids)?;
                let items = parse_statusline_items(&item_ids)?;
                let use_theme_colors = args
                    .use_theme_colors
                    .unwrap_or(self.config.tui_status_line_use_colors);
                let items_edit =
                    crate::legacy_core::config::edit::status_line_items_edit(&item_ids);
                let colors_edit =
                    crate::legacy_core::config::edit::status_line_use_colors_edit(use_theme_colors);
                ConfigEditsBuilder::for_config(&self.config)
                    .with_edits([items_edit, colors_edit])
                    .apply()
                    .await
                    .map_err(|err| format!("failed to save statusline settings: {err}"))?;
                self.config.tui_status_line = Some(item_ids.clone());
                self.config.tui_status_line_use_colors = use_theme_colors;
                self.chat_widget.setup_status_line(items, use_theme_colors);
                self.refresh_status_line();
                Ok(json!({
                    "surface": "statusline",
                    "updated": {
                        "item_ids": item_ids,
                        "use_theme_colors": use_theme_colors,
                    }
                }))
            }
            action => Err(unknown_action(action)),
        }
    }

    async fn handle_terminal_title_tool(
        &mut self,
        args: StatusSurfaceArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "list_options" => Ok(json!({
                "surface": "terminal_title",
                "current": {
                    "item_ids": self.chat_widget.configured_terminal_title_items(),
                },
                "options": TerminalTitleItem::iter()
                    .map(|item| StatusSurfaceOption {
                        id: item.to_string(),
                        description: item.description(),
                    })
                    .collect::<Vec<_>>(),
            })),
            "set" => {
                let item_ids = required_item_ids(args.item_ids)?;
                let items = parse_terminal_title_items(&item_ids)?;
                let edit = crate::legacy_core::config::edit::terminal_title_items_edit(&item_ids);
                ConfigEditsBuilder::for_config(&self.config)
                    .with_edits([edit])
                    .apply()
                    .await
                    .map_err(|err| format!("failed to save terminal title settings: {err}"))?;
                self.config.tui_terminal_title = Some(item_ids.clone());
                self.chat_widget.setup_terminal_title(items);
                Ok(json!({
                    "surface": "terminal_title",
                    "updated": {
                        "item_ids": item_ids,
                    }
                }))
            }
            action => Err(unknown_action(action)),
        }
    }

    async fn handle_config_tool(
        &mut self,
        app_server: &AppServerSession,
        args: ConfigArgs,
    ) -> Result<JsonValue, String> {
        match args.action.as_str() {
            "list_options" => Ok(json!({
                "surface": "config",
                "sections": config_section_outputs(),
                "options": config_option_outputs(&self.config),
            })),
            "set" => {
                let updates = args
                    .updates
                    .filter(|updates| !updates.is_empty())
                    .ok_or_else(|| "action=set requires non-empty updates".to_string())?;
                let options = common_config_options(&self.config);
                let mut seen = HashSet::new();
                let planned_updates = updates
                    .iter()
                    .map(|update| {
                        if !seen.insert(update.option_id.as_str()) {
                            return Err(format!(
                                "duplicate config option id `{}`",
                                update.option_id
                            ));
                        }
                        let option = options
                            .iter()
                            .find(|option| option.id == update.option_id)
                            .ok_or_else(|| {
                                format!(
                                    "unknown config option id `{}`; call action=list_options",
                                    update.option_id
                                )
                            })?;
                        let key_path = option
                            .key_path
                            .ok_or_else(|| format!("config option `{}` is managed", option.id))?;
                        Ok((
                            option.id,
                            option.section.id(),
                            key_path,
                            option.label,
                            option.value_for_enabled(update.enabled),
                            update.enabled,
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let mut applied = Vec::with_capacity(planned_updates.len());
                for (option_id, section_id, key_path, label, value, enabled) in planned_updates {
                    self.try_update_config_value_with_app_server(
                        app_server,
                        key_path.to_string(),
                        value,
                        label.to_string(),
                    )
                    .await?;
                    applied.push(json!({
                        "option_id": option_id,
                        "section_id": section_id,
                        "enabled": enabled,
                    }));
                }
                Ok(json!({
                    "surface": "config",
                    "updated": applied,
                }))
            }
            action => Err(unknown_action(action)),
        }
    }

    async fn handle_tmux_tool(&mut self, args: TmuxArgs) -> Result<JsonValue, String> {
        if args.explicit_user_request != Some(true) {
            return Err(
                "tmux handoff requires explicit_user_request=true and an explicit user request."
                    .to_string(),
            );
        }
        let destination = tmux_destination_from_args(args.name, args.session, args.window)?;
        let summary = self.prepare_tmux_handoff_from_tool(
            destination,
            args.replace.unwrap_or(/*replace_existing*/ true),
        )?;
        self.app_event_tx.send(crate::app_event::AppEvent::Exit(
            crate::app_event::ExitMode::ShutdownFirst,
        ));
        Ok(json!({
            "sessionName": summary.session_name,
            "windowName": summary.window_name,
            "target": summary.attach_target,
            "handoffCommand": summary.handoff_command,
            "attachMode": summary.attach_mode,
        }))
    }

    async fn resolve_ui_dynamic_tool_request(
        &mut self,
        app_server: &AppServerSession,
        request_id: AppServerRequestId,
        response: DynamicToolCallResponse,
    ) {
        let result = match serde_json::to_value(response) {
            Ok(result) => result,
            Err(err) => {
                self.chat_widget.add_error_message(format!(
                    "Failed to serialize Codewith UI tool response: {err}"
                ));
                return;
            }
        };
        if let Err(err) = app_server.resolve_server_request(request_id, result).await {
            self.chat_widget
                .add_error_message(format!("Failed to resolve Codewith UI tool request: {err}"));
        }
    }
}

fn parse_arguments<T>(value: &JsonValue) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(value.clone())
        .map_err(|err| format!("failed to parse Codewith UI tool arguments: {err}"))
}

fn tmux_destination_from_args(
    name: Option<String>,
    session: Option<String>,
    window: Option<String>,
) -> Result<TmuxHandoffDestination, String> {
    let name = non_empty_trimmed(name);
    let session = non_empty_trimmed(session);
    let window = non_empty_trimmed(window);
    match session {
        Some(session_name) => {
            if name.is_some() {
                return Err("tmux tool cannot combine `name` with `session`".to_string());
            }
            Ok(TmuxHandoffDestination::ExistingSession {
                session_name,
                window_name: window,
            })
        }
        None => {
            if window.is_some() {
                return Err("tmux tool `window` requires `session`".to_string());
            }
            Ok(TmuxHandoffDestination::NewSession { name })
        }
    }
}

fn non_empty_trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn dynamic_tool_response(output: JsonValue, success: bool) -> DynamicToolCallResponse {
    DynamicToolCallResponse {
        content_items: vec![DynamicToolCallOutputContentItem::InputText {
            text: serde_json::to_string_pretty(&output)
                .unwrap_or_else(|err| format!("failed to serialize output: {err}")),
        }],
        success,
    }
}

fn required_item_ids(item_ids: Option<Vec<String>>) -> Result<Vec<String>, String> {
    item_ids
        .filter(|item_ids| !item_ids.is_empty())
        .ok_or_else(|| "action=set requires non-empty item_ids".to_string())
}

fn parse_statusline_items(item_ids: &[String]) -> Result<Vec<StatusLineItem>, String> {
    parse_items(item_ids, "statusline item")
}

fn parse_terminal_title_items(item_ids: &[String]) -> Result<Vec<TerminalTitleItem>, String> {
    parse_items(item_ids, "terminal title item")
}

fn parse_items<T>(item_ids: &[String], item_label: &str) -> Result<Vec<T>, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let mut seen = HashSet::new();
    item_ids
        .iter()
        .map(|id| {
            if !seen.insert(id.as_str()) {
                return Err(format!("duplicate {item_label} id `{id}`"));
            }
            id.parse::<T>()
                .map_err(|_| format!("unknown {item_label} id `{id}`; call action=list_options"))
        })
        .collect()
}

fn config_section_outputs() -> Vec<ConfigSectionOutput> {
    common_config_sections()
        .iter()
        .copied()
        .map(config_section_output)
        .collect()
}

fn config_section_output(section: CommonConfigSection) -> ConfigSectionOutput {
    ConfigSectionOutput {
        id: section.id(),
        label: section.label(),
        description: section.description(),
    }
}

fn config_option_outputs(config: &crate::legacy_core::config::Config) -> Vec<ConfigOptionOutput> {
    common_config_options(config)
        .into_iter()
        .map(config_option_output)
        .collect()
}

fn config_option_output(option: CommonConfigOption) -> ConfigOptionOutput {
    ConfigOptionOutput {
        id: option.id,
        section_id: option.section.id(),
        label: option.label,
        description: option.description,
        enabled: option.enabled,
        disabled: option.is_disabled(),
        disabled_reason: option.disabled_reason,
    }
}

fn unknown_action(action: &str) -> String {
    format!("unknown action `{action}`; expected `list_options` or `set`")
}

fn validate_ui_dynamic_tool_thread_id(
    tool: &str,
    requested_thread_id: &str,
    current_thread_id: Option<codex_protocol::ThreadId>,
) -> Result<(), String> {
    let requested_thread_id =
        codex_protocol::ThreadId::from_string(requested_thread_id).map_err(|_| {
            format!("Codewith UI tool `{tool}` has invalid thread_id `{requested_thread_id}`")
        })?;
    let Some(current_thread_id) = current_thread_id else {
        return Err(format!(
            "Codewith UI tool `{tool}` requires an active visible thread"
        ));
    };
    if requested_thread_id != current_thread_id {
        return Err(format!(
            "Codewith UI tool `{tool}` targets thread `{requested_thread_id}`, but the visible thread is `{current_thread_id}`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::ThreadId;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_items_rejects_unknown_or_duplicate_ids() {
        let duplicate = parse_statusline_items(&["model".to_string(), "model".to_string()])
            .expect_err("duplicate ids should fail");
        assert!(duplicate.contains("duplicate"));

        let unknown = parse_terminal_title_items(&["not-real".to_string()])
            .expect_err("unknown ids should fail");
        assert!(unknown.contains("list_options"));
    }

    #[test]
    fn config_section_outputs_match_config_menu_sections() {
        let sections = config_section_outputs();
        let ids = sections
            .into_iter()
            .map(|section| section.id)
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec!["account-automation", "ai-context", "interface-privacy"]
        );
    }

    #[test]
    fn tmux_destination_from_args_preserves_legacy_name_and_explicit_target() {
        assert_eq!(
            tmux_destination_from_args(
                Some("named session".to_string()),
                /*session*/ None,
                /*window*/ None,
            ),
            Ok(TmuxHandoffDestination::NewSession {
                name: Some("named session".to_string()),
            })
        );
        assert_eq!(
            tmux_destination_from_args(
                /*name*/ None,
                Some("dev".to_string()),
                Some("codewith".to_string()),
            ),
            Ok(TmuxHandoffDestination::ExistingSession {
                session_name: "dev".to_string(),
                window_name: Some("codewith".to_string()),
            })
        );
        assert!(
            tmux_destination_from_args(
                Some("named".to_string()),
                Some("dev".to_string()),
                /*window*/ None,
            )
            .is_err()
        );
        assert!(
            tmux_destination_from_args(
                /*name*/ None,
                /*session*/ None,
                Some("codewith".to_string())
            )
            .is_err()
        );
    }

    #[test]
    fn validate_ui_dynamic_tool_thread_id_rejects_mismatch() {
        let current_thread_id = ThreadId::new();
        let other_thread_id = ThreadId::new();

        assert_eq!(
            validate_ui_dynamic_tool_thread_id(
                "manage_schedules",
                &current_thread_id.to_string(),
                Some(current_thread_id),
            ),
            Ok(())
        );

        let err = validate_ui_dynamic_tool_thread_id(
            "manage_schedules",
            &other_thread_id.to_string(),
            Some(current_thread_id),
        )
        .expect_err("mismatched thread should fail");
        assert!(err.contains("visible thread"));

        let err = validate_ui_dynamic_tool_thread_id(
            "manage_schedules",
            "not-a-thread-id",
            Some(current_thread_id),
        )
        .expect_err("invalid thread should fail");
        assert!(err.contains("invalid thread_id"));
    }
}
