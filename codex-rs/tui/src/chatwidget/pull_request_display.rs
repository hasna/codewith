//! Read-only pull request overview for `/pr`.

use super::*;
use crate::bottom_pane::SelectionRowDisplay;
use crate::bottom_pane::SelectionTab;
use crate::pull_request_summary::PullRequestCheckSummary;
use crate::pull_request_summary::PullRequestOverview;
use crate::pull_request_summary::PullRequestQuery;
use crate::pull_request_summary::PullRequestQueryStatus;
use crate::pull_request_summary::PullRequestSummary;
use ratatui::widgets::Paragraph;

const CURRENT_TAB_ID: &str = "current";
const OPEN_TAB_ID: &str = "open";
const PULL_REQUEST_OVERVIEW_VIEW_ID: &str = "pull_request_overview";

impl ChatWidget {
    pub(crate) fn open_pull_request_overview(&mut self) {
        self.pull_request_overview_request_id =
            self.pull_request_overview_request_id.wrapping_add(1);
        let request_id = self.pull_request_overview_request_id;
        self.show_pull_request_loading();
        let tx = self.app_event_tx.clone();
        let runner = self.workspace_command_runner.clone();
        let cwd = self
            .current_cwd
            .clone()
            .unwrap_or_else(|| self.config.cwd.to_path_buf());
        tokio::spawn(async move {
            let overview = match runner {
                Some(runner) => {
                    crate::pull_request_summary::load_pull_request_overview(runner.as_ref(), &cwd)
                        .await
                }
                None => PullRequestOverview::runner_unavailable(cwd),
            };
            tx.send(AppEvent::PullRequestOverviewLoaded {
                request_id,
                overview,
            });
        });
    }

    pub(crate) fn show_pull_request_overview(
        &mut self,
        request_id: u64,
        overview: PullRequestOverview,
    ) {
        if request_id != self.pull_request_overview_request_id {
            return;
        }
        let params = pull_request_overview_params(overview);
        let _replaced = self
            .bottom_pane
            .replace_selection_view_if_active(PULL_REQUEST_OVERVIEW_VIEW_ID, params);
    }

    fn show_pull_request_loading(&mut self) {
        let params = SelectionViewParams {
            view_id: Some(PULL_REQUEST_OVERVIEW_VIEW_ID),
            title: Some("Pull Requests".to_string()),
            subtitle: Some("Loading GitHub pull requests".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            footer_note: Some(read_only_footer_note()),
            items: vec![SelectionItem {
                name: "Loading".to_string(),
                description: Some("Running noninteractive gh commands".to_string()),
                is_disabled: true,
                ..Default::default()
            }],
            col_width_mode: ColumnWidthMode::Fixed,
            row_display: SelectionRowDisplay::SingleLine,
            ..Default::default()
        };
        if !self
            .bottom_pane
            .replace_selection_view_if_active(PULL_REQUEST_OVERVIEW_VIEW_ID, params)
        {
            self.show_selection_view(SelectionViewParams {
                view_id: Some(PULL_REQUEST_OVERVIEW_VIEW_ID),
                title: Some("Pull Requests".to_string()),
                subtitle: Some("Loading GitHub pull requests".to_string()),
                footer_hint: Some(standard_popup_hint_line()),
                footer_note: Some(read_only_footer_note()),
                items: vec![SelectionItem {
                    name: "Loading".to_string(),
                    description: Some("Running noninteractive gh commands".to_string()),
                    is_disabled: true,
                    ..Default::default()
                }],
                col_width_mode: ColumnWidthMode::Fixed,
                row_display: SelectionRowDisplay::SingleLine,
                ..Default::default()
            });
        }
    }
}

fn pull_request_overview_params(overview: PullRequestOverview) -> SelectionViewParams {
    let cwd = overview.cwd.display().to_string();
    let tabs = vec![
        SelectionTab {
            id: CURRENT_TAB_ID.to_string(),
            label: "Current".to_string(),
            header: query_header(&overview.current, "current PR"),
            items: query_items(overview.current, PullRequestQueryKind::Current),
        },
        SelectionTab {
            id: OPEN_TAB_ID.to_string(),
            label: "Open".to_string(),
            header: query_header(&overview.open, "open PR"),
            items: query_items(overview.open, PullRequestQueryKind::Open),
        },
    ];

    SelectionViewParams {
        view_id: Some(PULL_REQUEST_OVERVIEW_VIEW_ID),
        title: Some("Pull Requests".to_string()),
        subtitle: Some(cwd),
        footer_hint: Some(standard_popup_hint_line()),
        footer_note: Some(read_only_footer_note()),
        tabs,
        initial_tab_id: Some(CURRENT_TAB_ID.to_string()),
        is_searchable: true,
        search_placeholder: Some("Search PRs".to_string()),
        col_width_mode: ColumnWidthMode::AutoAllRows,
        row_display: SelectionRowDisplay::SingleLine,
        ..Default::default()
    }
}

fn query_header(query: &PullRequestQuery, label: &str) -> Box<dyn Renderable> {
    let text = match query.status {
        PullRequestQueryStatus::Ready => {
            let count = query.items.len();
            format!("{count} {label}{}", plural(count))
        }
        _ => "Needs attention".to_string(),
    };
    Box::new(Paragraph::new(vec![Line::from(text)]))
}

#[derive(Clone, Copy)]
enum PullRequestQueryKind {
    Current,
    Open,
}

fn query_items(query: PullRequestQuery, kind: PullRequestQueryKind) -> Vec<SelectionItem> {
    let mut items = vec![refresh_item()];
    match query.status {
        PullRequestQueryStatus::Ready if query.items.is_empty() => {
            items.push(empty_item(kind));
        }
        PullRequestQueryStatus::Ready => {
            items.extend(query.items.into_iter().map(pull_request_item));
        }
        status => {
            items.push(status_item(status, kind));
        }
    }
    items
}

fn refresh_item() -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(|tx| {
        tx.send(AppEvent::OpenPullRequestOverview);
    })];
    SelectionItem {
        name: "Refresh".to_string(),
        description: Some("Reload pull requests".to_string()),
        actions,
        dismiss_on_select: true,
        search_value: Some("refresh reload pull requests".to_string()),
        ..Default::default()
    }
}

fn pull_request_item(pull_request: PullRequestSummary) -> SelectionItem {
    let url = pull_request.url.clone();
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenUrlInBrowser { url: url.clone() });
    })];
    SelectionItem {
        name: pull_request_name(&pull_request),
        description: Some(pull_request_description(&pull_request)),
        selected_description: Some(pull_request_detail(&pull_request)),
        actions,
        dismiss_on_select: true,
        search_value: Some(pull_request_search_value(&pull_request)),
        ..Default::default()
    }
}

fn empty_item(kind: PullRequestQueryKind) -> SelectionItem {
    let (name, description) = match kind {
        PullRequestQueryKind::Current => (
            "No current pull request",
            "The checked-out branch is not associated with an open PR.",
        ),
        PullRequestQueryKind::Open => (
            "No open pull requests",
            "gh returned an empty open PR list.",
        ),
    };
    SelectionItem {
        name: name.to_string(),
        description: Some(description.to_string()),
        is_disabled: true,
        ..Default::default()
    }
}

fn status_item(status: PullRequestQueryStatus, kind: PullRequestQueryKind) -> SelectionItem {
    let (name, description) = status_message(status, kind);
    SelectionItem {
        name,
        description: Some(description),
        is_disabled: true,
        ..Default::default()
    }
}

fn status_message(status: PullRequestQueryStatus, kind: PullRequestQueryKind) -> (String, String) {
    match status {
        PullRequestQueryStatus::RunnerUnavailable => (
            "Workspace commands unavailable".to_string(),
            "Cannot inspect PRs before a workspace command runner is attached.".to_string(),
        ),
        PullRequestQueryStatus::NotGitRepository => (
            "Not a git repository".to_string(),
            "Open /pr from inside a git repository.".to_string(),
        ),
        PullRequestQueryStatus::GhUnavailable(message) => (
            "GitHub CLI unavailable".to_string(),
            format!("Install gh or make it available on PATH. {message}"),
        ),
        PullRequestQueryStatus::AuthRequired(message) => (
            "GitHub authentication required".to_string(),
            format!("Run gh auth login for this workspace. {message}"),
        ),
        PullRequestQueryStatus::NoCurrentPullRequest => {
            let description = match kind {
                PullRequestQueryKind::Current => {
                    "The checked-out branch is not associated with an open PR."
                }
                PullRequestQueryKind::Open => "No open PRs were returned.",
            };
            (
                "No current pull request".to_string(),
                description.to_string(),
            )
        }
        PullRequestQueryStatus::RateLimited(message) => (
            "GitHub rate limited the request".to_string(),
            format!("Wait before refreshing. {message}"),
        ),
        PullRequestQueryStatus::CommandFailed(message) => (
            "Failed to inspect pull requests".to_string(),
            format!("gh command failed. {message}"),
        ),
        PullRequestQueryStatus::ParseFailed(message) => {
            ("Failed to parse pull request data".to_string(), message)
        }
        PullRequestQueryStatus::Ready => (
            "Pull requests loaded".to_string(),
            "No pull request rows were available.".to_string(),
        ),
    }
}

fn pull_request_name(pull_request: &PullRequestSummary) -> String {
    format!(
        "#{} {}",
        pull_request.number,
        truncate_text(&pull_request.title, /*max_graphemes*/ 72)
    )
}

fn pull_request_description(pull_request: &PullRequestSummary) -> String {
    let mut parts = vec![state_label(pull_request)];
    if let Some(author) = pull_request.author.as_ref() {
        parts.push(format!("by {author}"));
    }
    parts.push(format!(
        "{} -> {}",
        pull_request.head_ref, pull_request.base_ref
    ));
    parts.push(checks_label(pull_request.checks));
    if let Some(review) = pull_request.review_decision.as_deref() {
        parts.push(format!("review {}", display_status(review)));
    }
    parts.join(" | ")
}

fn pull_request_detail(pull_request: &PullRequestSummary) -> String {
    let mut lines = vec![
        format!("URL: {}", pull_request.url),
        format!("State: {}", state_label(pull_request)),
        format!(
            "Branch: {} -> {}",
            pull_request.head_ref, pull_request.base_ref
        ),
        format!("Checks: {}", checks_label(pull_request.checks)),
    ];
    if let Some(author) = pull_request.author.as_ref() {
        lines.push(format!("Author: {author}"));
    }
    if let Some(review) = pull_request.review_decision.as_deref() {
        lines.push(format!("Review: {}", display_status(review)));
    }
    if let Some(merge_state) = pull_request.merge_state.as_deref() {
        lines.push(format!("Merge: {}", display_status(merge_state)));
    }
    lines.push("Read-only: Enter opens this PR in your browser.".to_string());
    lines.join("\n")
}

fn pull_request_search_value(pull_request: &PullRequestSummary) -> String {
    format!(
        "{} {} {} {} {} {}",
        pull_request.number,
        pull_request.title,
        pull_request.url,
        pull_request.author.as_deref().unwrap_or_default(),
        pull_request.head_ref,
        pull_request.base_ref
    )
}

fn state_label(pull_request: &PullRequestSummary) -> String {
    if pull_request.is_draft {
        "Draft".to_string()
    } else {
        display_status(&pull_request.state)
    }
}

fn checks_label(checks: PullRequestCheckSummary) -> String {
    if checks.total == 0 {
        return "checks unknown".to_string();
    }
    if checks.failing > 0 {
        return format!("{} failing/{} checks", checks.failing, checks.total);
    }
    if checks.pending > 0 {
        return format!("{} pending/{} checks", checks.pending, checks.total);
    }
    if checks.success == checks.total {
        return format!("{} checks passed", checks.total);
    }
    format!("{} checks", checks.total)
}

fn display_status(status: &str) -> String {
    status
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut word = first.to_uppercase().collect::<String>();
                    word.push_str(&chars.as_str().to_ascii_lowercase());
                    word
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn read_only_footer_note() -> Line<'static> {
    "Read-only: no comments, reviews, pushes, branch updates, merges, or auto-fix actions."
        .dim()
        .into()
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
