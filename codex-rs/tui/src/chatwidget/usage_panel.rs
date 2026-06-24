//! Interactive `/usage` panel.
//!
//! This surface is intentionally separate from the detailed `/status` report:
//! it opens in the bottom pane, refreshes usage data in the background, and
//! never appends a transcript entry or starts a user turn.

use super::status_panel::StatusPanelData;
#[cfg(test)]
use super::status_panel::StatusRateLimitRow;
use super::*;
use crate::app_event::MiniMaxUsageRefreshOrigin;
use crate::app_event::RateLimitRefreshOrigin;
use crate::app_event::RateLimitRefreshTarget;
use crate::bottom_pane::SelectionRowDisplay;
use crate::bottom_pane::SelectionTab;
use crate::minimax_usage::MiniMaxUsageSnapshot;
use crate::minimax_usage::MiniMaxUsageWindow;
use crate::slash_command::SlashCommand;
use crate::status::format_status_limit_summary;
use codex_model_provider_info::MINIMAX_PROVIDER_ID;
use ratatui::widgets::Paragraph;

const USAGE_PANEL_VIEW_ID: &str = "usage-panel";
const USAGE_TAB_ID: &str = "usage";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum UsagePanelRateLimitState {
    #[default]
    Idle,
    Loading {
        request_id: u64,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum UsagePanelMiniMaxUsageState {
    #[default]
    Idle,
    Loading {
        request_id: u64,
    },
    Available(MiniMaxUsageSnapshot),
    Error {
        message: String,
    },
}

#[derive(Debug, Clone)]
struct UsagePanelData {
    status: StatusPanelData,
    account_limits_supported: bool,
    rate_limit_state: UsagePanelRateLimitState,
    minimax_usage_state: Option<UsagePanelMiniMaxUsageState>,
}

impl ChatWidget {
    /// Open the interactive `/usage` panel and refresh account/provider usage
    /// through background requests when a supported source is active.
    pub(crate) fn open_usage_panel(&mut self) {
        let rate_limit_request_id = if self.should_prefetch_rate_limits() {
            Some(self.next_status_request_id())
        } else {
            self.usage_panel_rate_limit_state = UsagePanelRateLimitState::Idle;
            None
        };
        let minimax_request_id = if self.is_minimax_provider_active_for_usage_panel() {
            Some(self.next_status_request_id())
        } else {
            self.usage_panel_minimax_usage_state = UsagePanelMiniMaxUsageState::Idle;
            None
        };

        if let Some(request_id) = rate_limit_request_id {
            self.usage_panel_rate_limit_state = UsagePanelRateLimitState::Loading { request_id };
        }
        if let Some(request_id) = minimax_request_id {
            self.usage_panel_minimax_usage_state =
                UsagePanelMiniMaxUsageState::Loading { request_id };
        }

        self.replace_or_show_usage_panel();

        if let Some(request_id) = rate_limit_request_id {
            self.app_event_tx.send(AppEvent::RefreshRateLimits {
                origin: RateLimitRefreshOrigin::UsagePanel { request_id },
                target: RateLimitRefreshTarget::Selected,
            });
        }
        if let Some(request_id) = minimax_request_id {
            self.app_event_tx.send(AppEvent::RefreshMiniMaxUsage {
                origin: MiniMaxUsageRefreshOrigin::UsagePanel { request_id },
            });
        }
    }

    pub(crate) fn finish_usage_panel_rate_limit_refresh(
        &mut self,
        request_id: u64,
        result: Result<(), String>,
    ) {
        if !self.is_usage_panel_rate_limit_refresh_current(request_id) {
            return;
        }

        self.usage_panel_rate_limit_state = match result {
            Ok(()) => UsagePanelRateLimitState::Idle,
            Err(message) => UsagePanelRateLimitState::Error { message },
        };
        self.refresh_usage_panel_if_active();
    }

    pub(crate) fn is_usage_panel_rate_limit_refresh_current(&self, request_id: u64) -> bool {
        matches!(
            &self.usage_panel_rate_limit_state,
            UsagePanelRateLimitState::Loading {
                request_id: pending
            } if *pending == request_id
        )
    }

    pub(crate) fn finish_usage_panel_minimax_usage_refresh(
        &mut self,
        request_id: u64,
        result: Result<MiniMaxUsageSnapshot, String>,
    ) {
        if !matches!(
            &self.usage_panel_minimax_usage_state,
            UsagePanelMiniMaxUsageState::Loading {
                request_id: pending
            } if *pending == request_id
        ) {
            return;
        }

        self.usage_panel_minimax_usage_state = match result {
            Ok(snapshot) => UsagePanelMiniMaxUsageState::Available(snapshot),
            Err(message) => UsagePanelMiniMaxUsageState::Error { message },
        };
        self.refresh_usage_panel_if_active();
    }

    pub(super) fn refresh_usage_panel_if_active(&mut self) {
        if self.bottom_pane.active_view_id() == Some(USAGE_PANEL_VIEW_ID) {
            self.replace_or_show_usage_panel();
        }
    }

    fn replace_or_show_usage_panel(&mut self) {
        let selected_idx = self
            .bottom_pane
            .selected_index_for_active_view(USAGE_PANEL_VIEW_ID);
        let mut params = build_usage_panel_params(self.usage_panel_data());
        params.initial_selected_idx = selected_idx;
        if !self
            .bottom_pane
            .replace_selection_view_if_active(USAGE_PANEL_VIEW_ID, params)
        {
            self.bottom_pane
                .show_selection_view(build_usage_panel_params(self.usage_panel_data()));
        }
    }

    fn usage_panel_data(&self) -> UsagePanelData {
        UsagePanelData {
            status: self.status_panel_data(),
            account_limits_supported: self.should_prefetch_rate_limits(),
            rate_limit_state: self.usage_panel_rate_limit_state.clone(),
            minimax_usage_state: self
                .is_minimax_provider_active_for_usage_panel()
                .then(|| self.usage_panel_minimax_usage_state.clone()),
        }
    }

    fn is_minimax_provider_active_for_usage_panel(&self) -> bool {
        self.config
            .model_provider_id
            .eq_ignore_ascii_case(MINIMAX_PROVIDER_ID)
    }
}

fn build_usage_panel_params(data: UsagePanelData) -> SelectionViewParams {
    SelectionViewParams {
        view_id: Some(USAGE_PANEL_VIEW_ID),
        title: Some("Usage".to_string()),
        subtitle: Some("Session tokens, context, and account limits".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        tabs: vec![SelectionTab {
            id: USAGE_TAB_ID.to_string(),
            label: "Usage".to_string(),
            header: usage_header(&data),
            items: usage_items(&data),
        }],
        initial_tab_id: Some(USAGE_TAB_ID.to_string()),
        is_searchable: false,
        col_width_mode: ColumnWidthMode::AutoAllRows,
        row_display: SelectionRowDisplay::SingleLine,
        ..Default::default()
    }
}

fn usage_header(data: &UsagePanelData) -> Box<dyn Renderable> {
    let account = data
        .status
        .account
        .clone()
        .unwrap_or_else(|| "Account unavailable".to_string());
    let context = data
        .status
        .context_used_percent
        .map(|used| format!("{used}% context used"))
        .unwrap_or_else(|| "context unknown".to_string());
    let tokens = format_tokens_compact(data.status.tokens_total);
    Box::new(Paragraph::new(vec![Line::from(
        format!("{account} · {context} · {tokens} total tokens").dim(),
    )]))
}

fn usage_items(data: &UsagePanelData) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    items.push(info_row(
        "Account",
        data.status
            .account
            .clone()
            .unwrap_or_else(|| "not available".to_string()),
    ));
    if let Some(plan) = data.status.plan.as_ref() {
        items.push(info_row("Plan", plan.clone()));
    }
    let model_value = match data.status.reasoning_effort.as_ref() {
        Some(effort) => format!("{} · reasoning {effort}", data.status.model),
        None => data.status.model.clone(),
    };
    items.push(info_row("Model", model_value));
    items.push(info_row("Provider", data.status.provider.clone()));
    items.push(info_row("Context", context_value(&data.status)));
    items.push(info_row(
        "Tokens",
        format!(
            "{} total · {} input · {} output",
            format_tokens_compact(data.status.tokens_total),
            format_tokens_compact(data.status.tokens_input),
            format_tokens_compact(data.status.tokens_output)
        ),
    ));
    items.extend(rate_limit_items(data));
    if let Some(minimax_state) = data.minimax_usage_state.as_ref() {
        items.extend(minimax_usage_items(minimax_state));
    }
    items.push(refresh_row(data));
    items
}

fn context_value(data: &StatusPanelData) -> String {
    match (
        data.context_used_percent,
        data.context_window,
        data.tokens_in_context,
    ) {
        (Some(used), Some(window), Some(in_context)) => format!(
            "{used}% used · {} / {}",
            format_tokens_compact(in_context),
            format_tokens_compact(window)
        ),
        (Some(used), _, _) => format!("{used}% used"),
        _ => "context window not available".to_string(),
    }
}

fn rate_limit_items(data: &UsagePanelData) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    if !data.account_limits_supported {
        items.push(info_row(
            "Account limits",
            "account limits unavailable for this provider",
        ));
        return items;
    }

    if data.status.rate_limits.is_empty() {
        let message = match &data.rate_limit_state {
            UsagePanelRateLimitState::Loading { .. } => "refreshing account limits...".to_string(),
            UsagePanelRateLimitState::Error { message } => format!("unavailable ({message})"),
            UsagePanelRateLimitState::Idle => "no usage limit data returned".to_string(),
        };
        items.push(info_row("Account limits", message));
        return items;
    }

    for limit in &data.status.rate_limits {
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
    match &data.rate_limit_state {
        UsagePanelRateLimitState::Loading { .. } => {
            items.push(info_row("Account limits", "refreshing..."));
        }
        UsagePanelRateLimitState::Error { message } => {
            items.push(info_row(
                "Account limits",
                format!("refresh failed ({message})"),
            ));
        }
        UsagePanelRateLimitState::Idle => {}
    }
    items
}

fn minimax_usage_items(state: &UsagePanelMiniMaxUsageState) -> Vec<SelectionItem> {
    match state {
        UsagePanelMiniMaxUsageState::Available(snapshot) => {
            let Some(bucket) = snapshot.primary_bucket() else {
                return vec![info_row("MiniMax usage", "not available for this key")];
            };
            let bucket_name = if bucket.name.eq_ignore_ascii_case("general") {
                "general".to_string()
            } else {
                bucket.name.clone()
            };
            vec![
                info_row("MiniMax usage", format!("Token Plan {bucket_name} bucket")),
                info_row("MiniMax 5h", minimax_window_value(&bucket.interval)),
                info_row("MiniMax weekly", minimax_window_value(&bucket.weekly)),
            ]
        }
        UsagePanelMiniMaxUsageState::Loading { .. } => {
            vec![info_row("MiniMax usage", "refreshing Token Plan usage...")]
        }
        UsagePanelMiniMaxUsageState::Error { message } => vec![info_row(
            "MiniMax usage",
            format!("not available ({message})"),
        )],
        UsagePanelMiniMaxUsageState::Idle => {
            vec![info_row("MiniMax usage", "data not available yet")]
        }
    }
}

fn minimax_window_value(window: &MiniMaxUsageWindow) -> String {
    let percent_remaining = window.remaining_percent.clamp(0.0, 100.0);
    let mut parts = vec![format_status_limit_summary(percent_remaining)];
    if let Some(counts) = minimax_usage_counts(window) {
        parts.push(counts);
    }
    if let Some(resets_at) = window.resets_at.as_ref() {
        parts.push(format!("resets {}", resets_at.format("%H:%M")));
    }
    parts.join(" · ")
}

fn minimax_usage_counts(window: &MiniMaxUsageWindow) -> Option<String> {
    match (window.used_count, window.total_count) {
        (Some(used), Some(total)) if total > 0 => Some(format!("{used} / {total} used")),
        (Some(used), _) if used > 0 => Some(format!("{used} used")),
        _ => None,
    }
}

fn refresh_row(data: &UsagePanelData) -> SelectionItem {
    let is_loading = matches!(
        &data.rate_limit_state,
        UsagePanelRateLimitState::Loading { .. }
    ) || matches!(
        data.minimax_usage_state.as_ref(),
        Some(UsagePanelMiniMaxUsageState::Loading { .. })
    );
    let can_refresh = data.account_limits_supported || data.minimax_usage_state.is_some();
    if !can_refresh {
        return info_row("Refresh", "no refreshable usage source");
    }

    let actions: Vec<SelectionAction> = vec![Box::new(|tx: &AppEventSender| {
        tx.send(AppEvent::DispatchSlashCommand(SlashCommand::Usage));
    })];
    SelectionItem {
        name: "Refresh".to_string(),
        description: Some(if is_loading {
            "refreshing usage data".to_string()
        } else {
            "refresh usage data".to_string()
        }),
        actions,
        dismiss_on_select: false,
        ..Default::default()
    }
}

fn info_row(name: impl Into<String>, value: impl Into<String>) -> SelectionItem {
    SelectionItem {
        name: name.into(),
        description: Some(value.into()),
        is_disabled: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_status() -> StatusPanelData {
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

    #[test]
    fn usage_panel_has_refresh_action_and_no_full_report_row() {
        let params = build_usage_panel_params(UsagePanelData {
            status: sample_status(),
            account_limits_supported: true,
            rate_limit_state: UsagePanelRateLimitState::Idle,
            minimax_usage_state: None,
        });
        let items = &params.tabs[0].items;

        assert!(items.iter().any(|item| item.name == "Context"));
        assert!(items.iter().any(|item| item.name == "5h limit"));
        let refresh = items
            .iter()
            .find(|item| item.name == "Refresh")
            .expect("refresh row");
        assert!(!refresh.actions.is_empty());
        assert!(!refresh.dismiss_on_select);
        assert!(
            !items.iter().any(|item| item.name == "Full report"),
            "/usage must not expose the transcript report as its default action"
        );
    }

    #[test]
    fn usage_panel_marks_account_limits_unavailable_for_api_key_provider() {
        let mut status = sample_status();
        status.account = Some("API key".to_string());
        let params = build_usage_panel_params(UsagePanelData {
            status,
            account_limits_supported: false,
            rate_limit_state: UsagePanelRateLimitState::Idle,
            minimax_usage_state: None,
        });
        let account_limits = params.tabs[0]
            .items
            .iter()
            .find(|item| item.name == "Account limits")
            .and_then(|item| item.description.as_deref());

        assert_eq!(
            account_limits,
            Some("account limits unavailable for this provider")
        );
        assert!(
            !params.tabs[0]
                .items
                .iter()
                .any(|item| item.name == "5h limit"),
            "/usage should ignore stale cached ChatGPT limits when account limits are unsupported"
        );
    }
}
