use super::*;
use crate::style::accent_color;
use codex_app_server_protocol::McpAuthStatus;
use codex_app_server_protocol::McpServerStatus;
use codex_app_server_protocol::McpServerStatusDetail;
use codex_protocol::mcp::Resource;
use codex_protocol::mcp::ResourceTemplate;
use codex_protocol::mcp::Tool;
use ratatui::text::Span;

const MCP_MANAGER_VIEW_ID: &str = "mcp-manager";

impl ChatWidget {
    pub(crate) fn open_mcp_manager(&mut self, detail: McpServerStatusDetail) {
        self.open_mcp_manager_loading_popup();
        self.app_event_tx.send(AppEvent::FetchMcpInventory {
            detail,
            thread_id: self.thread_id(),
            target: McpInventoryTarget::Manager,
        });
        self.request_redraw();
    }

    pub(crate) fn on_mcp_manager_loaded(
        &mut self,
        result: Result<Vec<McpServerStatus>, String>,
        _detail: McpServerStatusDetail,
        _thread_id: Option<ThreadId>,
    ) {
        match result {
            Ok(mut statuses) => {
                sort_mcp_statuses(&mut statuses);
                self.replace_or_show_mcp_manager_popup(self.mcp_manager_popup_params(&statuses));
            }
            Err(err) => {
                self.replace_or_show_mcp_manager_popup(self.mcp_manager_error_popup_params(&err));
            }
        }
        self.request_redraw();
    }

    pub(crate) fn open_mcp_server_details(&mut self, status: McpServerStatus) {
        self.replace_or_show_mcp_manager_popup(self.mcp_server_detail_popup_params(status));
        self.request_redraw();
    }

    fn open_mcp_manager_loading_popup(&mut self) {
        self.replace_or_show_mcp_manager_popup(self.mcp_manager_loading_popup_params());
    }

    fn replace_or_show_mcp_manager_popup(&mut self, params: SelectionViewParams) {
        if self.bottom_pane.active_view_id() == Some(MCP_MANAGER_VIEW_ID) {
            let replaced = self
                .bottom_pane
                .replace_selection_view_if_active(MCP_MANAGER_VIEW_ID, params);
            debug_assert!(replaced);
        } else {
            self.bottom_pane.show_selection_view(params);
        }
    }

    fn mcp_manager_loading_popup_params(&self) -> SelectionViewParams {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("MCP Servers".bold()));
        header.push(Line::from("Loading configured servers...".dim()));

        SelectionViewParams {
            view_id: Some(MCP_MANAGER_VIEW_ID),
            header: Box::new(header),
            items: vec![SelectionItem {
                name: "Loading MCP servers...".to_string(),
                description: Some("This updates when server inventory is ready.".to_string()),
                is_disabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn mcp_manager_error_popup_params(&self, err: &str) -> SelectionViewParams {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("MCP Servers".bold()));
        header.push(Line::from("Failed to load server inventory.".dim()));

        SelectionViewParams {
            view_id: Some(MCP_MANAGER_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                refresh_mcp_manager_item(),
                SelectionItem {
                    name: "Inventory unavailable".to_string(),
                    description: Some(err.to_string()),
                    is_disabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }
    }

    fn mcp_manager_popup_params(&self, statuses: &[McpServerStatus]) -> SelectionViewParams {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("MCP Servers".bold()));
        header.push(Line::from(mcp_manager_summary(statuses).dim()));

        let mut items = vec![refresh_mcp_manager_item()];
        if statuses.is_empty() {
            items.push(reload_mcp_servers_item());
            items.push(mcp_setup_help_item());
            items.push(SelectionItem {
                name: "No MCP servers configured".to_string(),
                description: Some(
                    "Use Add server for the config shape and reload flow.".to_string(),
                ),
                is_disabled: true,
                ..Default::default()
            });
        } else {
            items.extend(statuses.iter().cloned().map(mcp_server_status_item));
            items.push(reload_mcp_servers_item());
            items.push(mcp_setup_help_item());
        }

        SelectionViewParams {
            view_id: Some(MCP_MANAGER_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search MCP servers".to_string()),
            col_width_mode: ColumnWidthMode::Fixed,
            ..Default::default()
        }
    }

    fn mcp_server_detail_popup_params(&self, status: McpServerStatus) -> SelectionViewParams {
        let mut header = ColumnRenderable::new();
        header.push(Line::from(format!("MCP: {}", status.name).bold()));
        header.push(Line::from(mcp_server_detail_summary(&status).dim()));
        if let Some(info) = &status.server_info
            && let Some(description) = non_empty(info.description.as_deref())
        {
            header.push(Line::from(description.to_string().dim()));
        }

        let has_inventory = !status.tools.is_empty()
            || !status.resources.is_empty()
            || !status.resource_templates.is_empty();
        let mut items = vec![refresh_mcp_manager_item()];
        if status.auth_status == McpAuthStatus::NotLoggedIn {
            items.push(mcp_oauth_login_item(status.name.clone()));
        }
        items.push(reload_mcp_servers_item());
        items.push(mcp_diagnostics_help_item(status.name.clone()));
        items.push(mcp_scale_guidance_item());
        push_tool_items(&mut items, &status);
        push_resource_items(&mut items, &status);
        push_resource_template_items(&mut items, &status);
        if !has_inventory {
            items.push(SelectionItem {
                name: "No tools or resources advertised".to_string(),
                description: Some("This server is reachable but has no inventory.".to_string()),
                is_disabled: true,
                ..Default::default()
            });
        }

        SelectionViewParams {
            view_id: Some(MCP_MANAGER_VIEW_ID),
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            is_searchable: true,
            search_placeholder: Some("Search actions, tools, and resources".to_string()),
            col_width_mode: ColumnWidthMode::Fixed,
            ..Default::default()
        }
    }
}

fn refresh_mcp_manager_item() -> SelectionItem {
    SelectionItem {
        name: "Refresh".to_string(),
        description: Some("Reload MCP server status and inventory.".to_string()),
        search_value: Some("refresh inventory status".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::OpenMcpManager {
                detail: McpServerStatusDetail::Full,
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn reload_mcp_servers_item() -> SelectionItem {
    SelectionItem {
        name: "Reload MCP tools".to_string(),
        description: Some("Reconnect loaded threads to the latest MCP config.".to_string()),
        selected_description: Some(
            "Use this after adding a server, changing command args, or updating an npm/plugin-backed MCP.".to_string(),
        ),
        search_value: Some("reload refresh reconnect tools config npm plugin".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::ReloadMcpServers);
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn mcp_setup_help_item() -> SelectionItem {
    SelectionItem {
        name: "Add server".to_string(),
        description: Some("Show config guidance for stdio and HTTP MCP servers.".to_string()),
        selected_description: Some(
            "Stdio is fine for normal local MCPs; use streamable HTTP or plugins for heavier fleets.".to_string(),
        ),
        search_value: Some("add setup configure config stdio http streamable plugin".to_string()),
        actions: vec![Box::new(|tx| {
            tx.send(AppEvent::ShowMcpSetupHelp);
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn mcp_oauth_login_item(server_name: String) -> SelectionItem {
    SelectionItem {
        name: "OAuth login".to_string(),
        description: Some("Open the browser login flow for this server.".to_string()),
        search_value: Some("oauth login auth browser".to_string()),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::StartMcpServerOauthLogin {
                name: server_name.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn mcp_diagnostics_help_item(server_name: String) -> SelectionItem {
    SelectionItem {
        name: "Diagnose".to_string(),
        description: Some(
            "Show checks for this server's command, env, auth, and inventory.".to_string(),
        ),
        search_value: Some("diagnose debug troubleshoot command env auth inventory".to_string()),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::ShowMcpDiagnosticsHelp {
                name: server_name.clone(),
            });
        })],
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn mcp_scale_guidance_item() -> SelectionItem {
    SelectionItem {
        name: "Scale guidance".to_string(),
        description: Some(
            "Many stdio MCPs spawn many local processes; HTTP/plugin MCPs reduce local load."
                .to_string(),
        ),
        search_value: Some("scale stdio http streamable plugin process load".to_string()),
        is_disabled: true,
        ..Default::default()
    }
}

fn mcp_server_status_item(status: McpServerStatus) -> SelectionItem {
    let name = status.name.clone();
    let selected_description = Some(mcp_server_selected_description(&status));
    let description = Some(mcp_server_row_description(&status));
    let search_value = Some(mcp_server_search_value(&status));
    SelectionItem {
        name,
        name_prefix_spans: vec![auth_status_prefix(status.auth_status)],
        description,
        selected_description,
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenMcpServerDetails {
                status: status.clone(),
            });
        })],
        dismiss_on_select: true,
        search_value,
        ..Default::default()
    }
}

fn push_tool_items(items: &mut Vec<SelectionItem>, status: &McpServerStatus) {
    let mut tools = status.tools.values().collect::<Vec<_>>();
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    for tool in tools {
        items.push(tool_item(tool));
    }
}

fn push_resource_items(items: &mut Vec<SelectionItem>, status: &McpServerStatus) {
    let mut resources = status.resources.iter().collect::<Vec<_>>();
    resources.sort_by(|left, right| left.name.cmp(&right.name));
    for resource in resources {
        items.push(resource_item(resource));
    }
}

fn push_resource_template_items(items: &mut Vec<SelectionItem>, status: &McpServerStatus) {
    let mut templates = status.resource_templates.iter().collect::<Vec<_>>();
    templates.sort_by(|left, right| left.name.cmp(&right.name));
    for template in templates {
        items.push(resource_template_item(template));
    }
}

fn tool_item(tool: &Tool) -> SelectionItem {
    SelectionItem {
        name: display_name(tool.title.as_deref(), &tool.name),
        name_prefix_spans: vec!["T ".fg(accent_color())],
        description: tool.description.clone().or_else(|| Some(tool.name.clone())),
        selected_description: Some(format!("Tool name: {}", tool.name)),
        is_disabled: true,
        search_value: Some(format!(
            "{} {}",
            tool.name,
            tool.description.clone().unwrap_or_default()
        )),
        ..Default::default()
    }
}

fn resource_item(resource: &Resource) -> SelectionItem {
    SelectionItem {
        name: display_name(resource.title.as_deref(), &resource.name),
        name_prefix_spans: vec!["R ".green()],
        description: Some(resource.uri.clone()),
        selected_description: resource
            .description
            .clone()
            .or_else(|| resource.mime_type.clone()),
        is_disabled: true,
        search_value: Some(format!(
            "{} {} {}",
            resource.name,
            resource.uri,
            resource.description.clone().unwrap_or_default()
        )),
        ..Default::default()
    }
}

fn resource_template_item(template: &ResourceTemplate) -> SelectionItem {
    SelectionItem {
        name: display_name(template.title.as_deref(), &template.name),
        name_prefix_spans: vec!["P ".magenta()],
        description: Some(template.uri_template.clone()),
        selected_description: template
            .description
            .clone()
            .or_else(|| template.mime_type.clone()),
        is_disabled: true,
        search_value: Some(format!(
            "{} {} {}",
            template.name,
            template.uri_template,
            template.description.clone().unwrap_or_default()
        )),
        ..Default::default()
    }
}

fn sort_mcp_statuses(statuses: &mut [McpServerStatus]) {
    statuses.sort_by(|left, right| {
        auth_status_order(left.auth_status)
            .cmp(&auth_status_order(right.auth_status))
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn auth_status_order(status: McpAuthStatus) -> usize {
    match status {
        McpAuthStatus::NotLoggedIn => 0,
        McpAuthStatus::BearerToken | McpAuthStatus::OAuth => 1,
        McpAuthStatus::Unsupported => 2,
    }
}

fn auth_status_prefix(status: McpAuthStatus) -> Span<'static> {
    match status {
        McpAuthStatus::NotLoggedIn => "! ".red(),
        McpAuthStatus::BearerToken => "K ".fg(accent_color()),
        McpAuthStatus::OAuth => "O ".green(),
        McpAuthStatus::Unsupported => "- ".dim(),
    }
}

fn auth_status_label(status: McpAuthStatus) -> &'static str {
    match status {
        McpAuthStatus::Unsupported => "No OAuth",
        McpAuthStatus::NotLoggedIn => "Login needed",
        McpAuthStatus::BearerToken => "API key",
        McpAuthStatus::OAuth => "OAuth",
    }
}

fn mcp_manager_summary(statuses: &[McpServerStatus]) -> String {
    let server_count = statuses.len();
    let tool_count: usize = statuses.iter().map(|status| status.tools.len()).sum();
    let auth_needed = statuses
        .iter()
        .filter(|status| status.auth_status == McpAuthStatus::NotLoggedIn)
        .count();
    if auth_needed > 0 {
        format!("{server_count} servers, {tool_count} tools, {auth_needed} need login")
    } else {
        format!("{server_count} servers, {tool_count} tools")
    }
}

fn mcp_server_row_description(status: &McpServerStatus) -> String {
    format!(
        "{} · {} · {} · {}",
        auth_status_label(status.auth_status),
        count_label(status.tools.len(), "tool"),
        count_label(status.resources.len(), "resource"),
        count_label(status.resource_templates.len(), "template")
    )
}

fn mcp_server_selected_description(status: &McpServerStatus) -> String {
    let mut parts = vec![mcp_server_row_description(status)];
    if let Some(info) = &status.server_info {
        parts.push(format!("{} {}", info.name, info.version));
        if let Some(title) = non_empty(info.title.as_deref()) {
            parts.push(title.to_string());
        }
        if let Some(website_url) = non_empty(info.website_url.as_deref()) {
            parts.push(website_url.to_string());
        }
    }
    parts.join(" · ")
}

fn mcp_server_detail_summary(status: &McpServerStatus) -> String {
    if let Some(info) = &status.server_info {
        let title = non_empty(info.title.as_deref()).unwrap_or(&info.name);
        format!(
            "{} · {} · {}",
            title,
            info.version,
            mcp_server_row_description(status)
        )
    } else {
        mcp_server_row_description(status)
    }
}

fn mcp_server_search_value(status: &McpServerStatus) -> String {
    let tool_names = status
        .tools
        .values()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {}",
        status.name,
        auth_status_label(status.auth_status),
        tool_names
    )
}

fn display_name(title: Option<&str>, name: &str) -> String {
    non_empty(title).unwrap_or(name).to_string()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn count_label(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
