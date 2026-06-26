//! Interactive, multi-level `/status` panel.
//!
//! `/status` (and its `/stats` alias) open a tabbed overlay — Overview, Usage,
//! Tools, Session — instead of dumping a static block into the transcript. Rows
//! that map to a setting are actionable: selecting one launches the matching
//! slash command (the Model row opens the model picker, the MCP row opens the
//! MCP control center, and so on), so the panel doubles as a navigable control
//! center in the spirit of Claude Code's `/status`.

use super::*;
use crate::bottom_pane::SelectionRowDisplay;
use crate::bottom_pane::SelectionTab;
use crate::slash_command::SlashCommand;
use crate::status::RateLimitWindowDisplay;
use crate::status::StatusAccountDisplay;
use crate::status::compose_agents_summary;
use crate::status::format_directory_display;
use crate::status::format_tokens_compact;
use crate::status::plan_type_display_name;
use crate::version::CODEX_CLI_VERSION;
use codex_utils_sandbox_summary::summarize_permission_profile;
use ratatui::widgets::Paragraph;

const OVERVIEW_TAB_ID: &str = "overview";
const USAGE_TAB_ID: &str = "usage";
const TOOLS_TAB_ID: &str = "tools";
const SESSION_TAB_ID: &str = "session";

/// Width, in cells, of the textual context-window gauge.
const GAUGE_WIDTH: usize = 12;

/// One rate-limit window rendered in the Usage tab.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StatusRateLimitRow {
    pub label: String,
    pub percent_left: f64,
    pub resets_at: Option<String>,
}

/// Pre-formatted snapshot the `/status` panel renders.
///
/// Kept free of `ChatWidget` so the tab/row builder ([`build_status_panel_params`])
/// can be unit-tested in isolation from terminal state.
#[derive(Debug, Clone, Default)]
pub(crate) struct StatusPanelData {
    pub account: Option<String>,
    pub plan: Option<String>,
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub provider: String,
    pub directory: String,
    pub git_branch: Option<String>,
    pub version: String,
    pub context_used_percent: Option<i64>,
    pub context_window: Option<i64>,
    pub tokens_in_context: Option<i64>,
    pub tokens_total: i64,
    pub tokens_input: i64,
    pub tokens_output: i64,
    pub rate_limits: Vec<StatusRateLimitRow>,
    pub agents_summary: String,
    pub thread_id: Option<String>,
    pub thread_name: Option<String>,
    pub forked_from: Option<String>,
    pub collaboration_mode: Option<String>,
    pub remote_connection: Option<String>,
    pub permissions: String,
}

impl ChatWidget {
    /// Open the interactive, tabbed `/status` panel.
    pub(crate) fn open_status_panel(&mut self) {
        let data = self.status_panel_data();
        self.show_selection_view(build_status_panel_params(&data));
    }

    fn status_panel_data(&self) -> StatusPanelData {
        let (account, account_plan) = match self.status_account_display.as_ref() {
            Some(StatusAccountDisplay::ChatGpt { email, plan }) => (
                Some(email.clone().unwrap_or_else(|| "ChatGPT".to_string())),
                plan.clone(),
            ),
            Some(StatusAccountDisplay::ApiKey) => (Some("API key".to_string()), None),
            None => (None, None),
        };
        let plan = account_plan.or_else(|| self.plan_type.map(plan_type_display_name));

        let total = self.status_line_total_usage();
        let tokens_in_context = self
            .token_info
            .as_ref()
            .map(|info| info.last_token_usage.tokens_in_context_window());

        StatusPanelData {
            account,
            plan,
            model: self.model_display_name().to_string(),
            reasoning_effort: self
                .effective_reasoning_effort()
                .map(|effort| effort.as_str().to_string()),
            provider: self.config.model_provider_id.clone(),
            directory: format_directory_display(&self.config.cwd, /*max_width*/ None),
            git_branch: self.status_line_branch.clone(),
            version: CODEX_CLI_VERSION.to_string(),
            context_used_percent: self.status_line_context_used_percent(),
            context_window: self.status_line_context_window_size(),
            tokens_in_context,
            tokens_total: total.blended_total(),
            tokens_input: total.non_cached_input(),
            tokens_output: total.output_tokens,
            rate_limits: self.status_panel_rate_limit_rows(),
            agents_summary: compose_agents_summary(&self.config, &self.instruction_source_paths),
            thread_id: self.thread_id.as_ref().map(ToString::to_string),
            thread_name: self.thread_name.clone(),
            forked_from: self.forked_from.map(|id| id.to_string()),
            collaboration_mode: self.collaboration_mode_label().map(ToString::to_string),
            remote_connection: self
                .remote_connection
                .as_ref()
                .map(|remote| format!("{} ({})", remote.address, remote.version)),
            permissions: self.status_panel_permissions_summary(),
        }
    }

    fn status_panel_rate_limit_rows(&self) -> Vec<StatusRateLimitRow> {
        let mut rows = Vec::new();
        for snapshot in self.rate_limit_snapshots_by_limit_id.values() {
            if let Some(window) = snapshot.primary.as_ref() {
                rows.push(rate_limit_row("Primary limit", window));
            }
            if let Some(window) = snapshot.secondary.as_ref() {
                rows.push(rate_limit_row("Secondary limit", window));
            }
        }
        rows
    }

    fn status_panel_permissions_summary(&self) -> String {
        let approval = self.config.permissions.approval_policy.value().to_string();
        let profile = self.config.permissions.effective_permission_profile();
        let roots = self.config.effective_workspace_roots();
        let sandbox = summarize_permission_profile(&profile, &self.config.cwd, roots.as_slice());
        format!("{approval} · {sandbox}")
    }
}

fn rate_limit_row(fallback_label: &str, window: &RateLimitWindowDisplay) -> StatusRateLimitRow {
    let label = window
        .window_minutes
        .map(window_label)
        .unwrap_or_else(|| fallback_label.to_string());
    StatusRateLimitRow {
        label,
        percent_left: (100.0 - window.used_percent).clamp(0.0, 100.0),
        resets_at: window.resets_at.clone(),
    }
}

fn window_label(minutes: i64) -> String {
    const DAY: i64 = 1440;
    const WEEK: i64 = 10080;
    match minutes {
        m if m <= 0 => "limit".to_string(),
        m if m <= 60 => format!("{m}m limit"),
        m if m < DAY => format!("{}h limit", m / 60),
        DAY => "daily limit".to_string(),
        m if m < WEEK => format!("{}h limit", m / 60),
        // A week or longer: round to the nearest week so unusual server values
        // (e.g. 10081) read as "1wk limit" rather than "168h limit".
        m => format!("{}wk limit", (m + WEEK / 2) / WEEK),
    }
}

/// Build the tabbed selection view that backs `/status`.
pub(crate) fn build_status_panel_params(data: &StatusPanelData) -> SelectionViewParams {
    let tabs = vec![
        SelectionTab {
            id: OVERVIEW_TAB_ID.to_string(),
            label: "Overview".to_string(),
            header: summary_header(overview_summary(data)),
            items: overview_items(data),
        },
        SelectionTab {
            id: USAGE_TAB_ID.to_string(),
            label: "Usage".to_string(),
            header: summary_header(usage_summary(data)),
            items: usage_items(data),
        },
        SelectionTab {
            id: TOOLS_TAB_ID.to_string(),
            label: "Tools".to_string(),
            header: summary_header("Settings and extensions".to_string()),
            items: tools_items(data),
        },
        SelectionTab {
            id: SESSION_TAB_ID.to_string(),
            label: "Session".to_string(),
            header: summary_header(session_summary(data)),
            items: session_items(data),
        },
    ];

    SelectionViewParams {
        title: Some("Status".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        tabs,
        initial_tab_id: Some(OVERVIEW_TAB_ID.to_string()),
        is_searchable: true,
        search_placeholder: Some("Search".to_string()),
        col_width_mode: ColumnWidthMode::AutoAllRows,
        row_display: SelectionRowDisplay::SingleLine,
        ..Default::default()
    }
}

fn summary_header(text: String) -> Box<dyn Renderable> {
    Box::new(Paragraph::new(vec![Line::from(text.dim())]))
}

fn overview_summary(data: &StatusPanelData) -> String {
    match &data.account {
        Some(account) => format!("{} · {}", account, data.model),
        None => data.model.clone(),
    }
}

fn usage_summary(data: &StatusPanelData) -> String {
    match data.context_used_percent {
        Some(used) => format!("{used}% of context used"),
        None => "Context usage unavailable".to_string(),
    }
}

fn session_summary(data: &StatusPanelData) -> String {
    data.thread_name
        .clone()
        .or_else(|| data.thread_id.clone())
        .unwrap_or_else(|| "No active thread".to_string())
}

fn overview_items(data: &StatusPanelData) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    if let Some(account) = &data.account {
        items.push(info_row("Account", account.clone()));
    }
    if let Some(plan) = &data.plan {
        items.push(info_row("Plan", plan.clone()));
    }
    let model_value = match &data.reasoning_effort {
        Some(effort) => format!("{} · reasoning {effort}", data.model),
        None => data.model.clone(),
    };
    items.push(launch_row("Model", model_value, SlashCommand::Model));
    items.push(launch_row(
        "Provider",
        data.provider.clone(),
        SlashCommand::Provider,
    ));
    items.push(info_row("Directory", data.directory.clone()));
    if let Some(branch) = &data.git_branch {
        items.push(info_row("Git branch", branch.clone()));
    }
    items.push(info_row("Version", data.version.clone()));
    items
}

fn usage_items(data: &StatusPanelData) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    let context_value = match (
        data.context_used_percent,
        data.context_window,
        data.tokens_in_context,
    ) {
        (Some(used), Some(window), Some(in_context)) => format!(
            "{} {used}% used · {} / {}",
            gauge_bar(used),
            format_tokens_compact(in_context),
            format_tokens_compact(window)
        ),
        (Some(used), _, _) => format!("{} {used}% used", gauge_bar(used)),
        _ => "context window not available".to_string(),
    };
    items.push(info_row("Context", context_value));
    items.push(info_row(
        "Tokens used",
        format!(
            "{} total · {} input · {} output",
            format_tokens_compact(data.tokens_total),
            format_tokens_compact(data.tokens_input),
            format_tokens_compact(data.tokens_output)
        ),
    ));
    if data.rate_limits.is_empty() {
        items.push(info_row("Rate limits", "no usage limit data yet"));
    } else {
        for limit in &data.rate_limits {
            let resets = limit
                .resets_at
                .as_ref()
                .map(|resets| format!(", resets {resets}"))
                .unwrap_or_default();
            items.push(info_row(
                limit.label.clone(),
                format!("{:.0}% left{resets}", limit.percent_left),
            ));
        }
    }
    items.push(report_row(
        "Full report",
        "append the detailed status report to the transcript",
    ));
    items
}

fn tools_items(data: &StatusPanelData) -> Vec<SelectionItem> {
    vec![
        launch_row(
            "MCP servers",
            "open the MCP control center",
            SlashCommand::Mcp,
        ),
        launch_row("Skills", "browse and toggle skills", SlashCommand::Skills),
        launch_row("Hooks", "view lifecycle hooks", SlashCommand::Hooks),
        info_row("Agent files", data.agents_summary.clone()),
    ]
}

fn session_items(data: &StatusPanelData) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    if let Some(thread_id) = &data.thread_id {
        items.push(info_row("Thread", thread_id.clone()));
    }
    if let Some(name) = &data.thread_name {
        items.push(info_row("Name", name.clone()));
    }
    if let Some(forked) = &data.forked_from {
        items.push(info_row("Forked from", forked.clone()));
    }
    if let Some(mode) = &data.collaboration_mode {
        items.push(info_row("Mode", mode.clone()));
    }
    if let Some(remote) = &data.remote_connection {
        items.push(info_row("Remote", remote.clone()));
    }
    items.push(launch_row(
        "Permissions",
        data.permissions.clone(),
        SlashCommand::Permissions,
    ));
    items
}

/// A non-actionable information row.
fn info_row(name: impl Into<String>, value: impl Into<String>) -> SelectionItem {
    SelectionItem {
        name: name.into(),
        description: Some(value.into()),
        is_disabled: true,
        ..Default::default()
    }
}

/// An actionable row that launches `command` when selected.
fn launch_row(
    name: impl Into<String>,
    value: impl Into<String>,
    command: SlashCommand,
) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx: &AppEventSender| {
        tx.send(AppEvent::DispatchSlashCommand(command));
    })];
    SelectionItem {
        name: name.into(),
        description: Some(value.into()),
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

/// An actionable row that appends the detailed text status report.
fn report_row(name: impl Into<String>, value: impl Into<String>) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx: &AppEventSender| {
        tx.send(AppEvent::ShowStatusReport);
    })];
    SelectionItem {
        name: name.into(),
        description: Some(value.into()),
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn gauge_bar(used_percent: i64) -> String {
    let used = used_percent.clamp(0, 100) as usize;
    let filled = ((used * GAUGE_WIDTH) + 50) / 100;
    let filled = filled.min(GAUGE_WIDTH);
    let empty = GAUGE_WIDTH - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> StatusPanelData {
        StatusPanelData {
            account: Some("dev@example.com".to_string()),
            plan: Some("Pro".to_string()),
            model: "gpt-5-codex".to_string(),
            reasoning_effort: Some("high".to_string()),
            provider: "openai".to_string(),
            directory: "~/code/app".to_string(),
            git_branch: Some("main".to_string()),
            version: "1.2.3".to_string(),
            context_used_percent: Some(40),
            context_window: Some(128_000),
            tokens_in_context: Some(51_200),
            tokens_total: 1_500,
            tokens_input: 1_000,
            tokens_output: 500,
            rate_limits: vec![StatusRateLimitRow {
                label: "5h limit".to_string(),
                percent_left: 73.0,
                resets_at: Some("14:30".to_string()),
            }],
            agents_summary: "AGENTS.md".to_string(),
            thread_id: Some("th_abc".to_string()),
            thread_name: Some("my thread".to_string()),
            forked_from: None,
            collaboration_mode: Some("pair".to_string()),
            remote_connection: None,
            permissions: "on-request · workspace-write".to_string(),
        }
    }

    fn row_names(items: &[SelectionItem]) -> Vec<&str> {
        items.iter().map(|item| item.name.as_str()).collect()
    }

    #[test]
    fn panel_has_four_navigable_tabs() {
        let params = build_status_panel_params(&sample_data());
        let labels: Vec<&str> = params.tabs.iter().map(|tab| tab.label.as_str()).collect();
        assert_eq!(labels, vec!["Overview", "Usage", "Tools", "Session"]);
        assert_eq!(params.initial_tab_id.as_deref(), Some(OVERVIEW_TAB_ID));
        assert!(params.is_searchable);
    }

    #[test]
    fn overview_model_row_is_actionable_and_account_is_info() {
        let params = build_status_panel_params(&sample_data());
        let overview = &params.tabs[0].items;
        let model = overview
            .iter()
            .find(|item| item.name == "Model")
            .expect("model row");
        assert!(
            !model.actions.is_empty(),
            "model row should launch the model picker"
        );
        assert!(!model.is_disabled);

        let account = overview
            .iter()
            .find(|item| item.name == "Account")
            .expect("account row");
        assert!(account.actions.is_empty());
        assert!(account.is_disabled, "account is informational");
    }

    #[test]
    fn tools_tab_launches_mcp_skills_hooks() {
        let params = build_status_panel_params(&sample_data());
        let tools = &params.tabs[2].items;
        for name in ["MCP servers", "Skills", "Hooks"] {
            let row = tools
                .iter()
                .find(|item| item.name == name)
                .unwrap_or_else(|| panic!("missing {name} row"));
            assert!(!row.actions.is_empty(), "{name} row should be actionable");
            assert!(row.dismiss_on_select);
        }
    }

    #[test]
    fn usage_tab_shows_context_gauge_and_rate_limits() {
        let params = build_status_panel_params(&sample_data());
        let usage = &params.tabs[1].items;
        let names = row_names(usage);
        assert!(names.contains(&"Context"));
        assert!(names.contains(&"Tokens used"));
        assert!(names.contains(&"5h limit"));
        let context = usage
            .iter()
            .find(|item| item.name == "Context")
            .and_then(|item| item.description.clone())
            .unwrap_or_default();
        assert!(context.contains("40% used"), "got {context:?}");
        assert!(
            context.contains('█'),
            "expected a gauge bar, got {context:?}"
        );
    }

    #[test]
    fn session_tab_shows_remote_connection_when_present() {
        let mut data = sample_data();
        data.remote_connection = Some("wss://host (v1.2.3)".to_string());
        let params = build_status_panel_params(&data);
        let session = &params.tabs[3].items;
        let remote = session
            .iter()
            .find(|item| item.name == "Remote")
            .and_then(|item| item.description.clone());
        assert_eq!(remote.as_deref(), Some("wss://host (v1.2.3)"));

        // Omitted entirely when not connected.
        let local = build_status_panel_params(&sample_data());
        assert!(
            !local.tabs[3].items.iter().any(|item| item.name == "Remote"),
            "remote row should be hidden for local sessions"
        );
    }

    #[test]
    fn usage_tab_handles_missing_rate_limits() {
        let mut data = sample_data();
        data.rate_limits.clear();
        let params = build_status_panel_params(&data);
        let usage = &params.tabs[1].items;
        assert!(row_names(usage).contains(&"Rate limits"));
    }

    #[test]
    fn gauge_bar_is_clamped_and_proportional() {
        assert_eq!(gauge_bar(0), format!("[{}]", "░".repeat(GAUGE_WIDTH)));
        assert_eq!(gauge_bar(100), format!("[{}]", "█".repeat(GAUGE_WIDTH)));
        assert_eq!(gauge_bar(150), format!("[{}]", "█".repeat(GAUGE_WIDTH)));
        let half = gauge_bar(50);
        assert_eq!(half.matches('█').count(), GAUGE_WIDTH / 2);
    }

    #[test]
    fn window_label_maps_common_durations() {
        assert_eq!(window_label(60), "60m limit");
        assert_eq!(window_label(300), "5h limit");
        assert_eq!(window_label(1440), "daily limit");
        assert_eq!(window_label(10080), "1wk limit");
    }
}
