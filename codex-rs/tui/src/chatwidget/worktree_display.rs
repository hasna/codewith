//! Codewith-managed worktree summaries and interactive manager for `/worktree`.

use super::*;
use crate::text_formatting::truncate_text;
use chrono::DateTime;
use chrono::Local;
use codex_app_server_protocol::AgentRunStatus;
use codex_app_server_protocol::Worktree;
use codex_app_server_protocol::WorktreeCleanupPolicy;
use codex_app_server_protocol::WorktreeLifecycleStatus;
use codex_app_server_protocol::WorktreeMode;
use codex_app_server_protocol::WorktreeOwnerKind;
use codex_app_server_protocol::WorktreePolicy;
use codex_app_server_protocol::WorktreeSessionMode;
use ratatui::text::Span;
use serde_json::Value as JsonValue;

impl ChatWidget {
    pub(crate) fn show_worktree_manager(
        &mut self,
        worktrees: Vec<Worktree>,
        policy: WorktreePolicy,
    ) {
        self.show_selection_view(worktree_manager_params(
            worktrees,
            &policy,
            WorktreePrimaryAction::Actions,
        ));
    }

    pub(crate) fn show_worktree_read_selector(
        &mut self,
        worktrees: Vec<Worktree>,
        policy: WorktreePolicy,
    ) {
        self.show_selection_view(worktree_manager_params(
            worktrees,
            &policy,
            WorktreePrimaryAction::Read,
        ));
    }

    pub(crate) fn show_worktree_actions(&mut self, worktree: Worktree, policy: WorktreePolicy) {
        self.show_selection_view(worktree_actions_params(worktree, &policy));
    }

    pub(crate) fn show_worktree_read(&mut self, worktree: Worktree) {
        self.add_plain_history_lines(worktree_detail_lines(&worktree));
    }
}

fn worktree_manager_params(
    mut worktrees: Vec<Worktree>,
    policy: &WorktreePolicy,
    primary_action: WorktreePrimaryAction,
) -> SelectionViewParams {
    worktrees.sort_by_key(|worktree| {
        (
            worktree_status_sort_key(worktree.lifecycle_status),
            std::cmp::Reverse(worktree.updated_at),
            worktree.worktree_id.clone(),
        )
    });

    let mut items = Vec::with_capacity(worktrees.len() + 4);
    items.push(SelectionItem {
        name: "Managed worktree policy".to_string(),
        description: Some(worktree_policy_description(policy)),
        selected_description: Some("Creation/attach actions respect this config.".to_string()),
        is_disabled: true,
        search_value: Some(worktree_policy_search_value(policy)),
        ..Default::default()
    });
    if primary_action == WorktreePrimaryAction::Actions {
        let create_disabled_reason = if policy.current_base_repo_path.is_none() {
            "No git repository detected for this session".to_string()
        } else if !policy.enabled {
            "Managed worktrees are disabled in config".to_string()
        } else if policy.main_sessions == WorktreeSessionMode::Off {
            "Main-session worktrees are disabled in config".to_string()
        } else {
            String::new()
        };
        items.push(worktree_action_item(
            "Create worktree",
            "Create a managed isolated worktree",
            !create_disabled_reason.is_empty(),
            (!create_disabled_reason.is_empty()).then_some(create_disabled_reason),
            || AppEvent::CreateWorktree {
                name: None,
                branch: None,
                start_point: None,
            },
        ));
        let reconcile_disabled_reason = if policy.current_base_repo_path.is_none() {
            "No git repository detected for this session".to_string()
        } else if !policy.enabled {
            "Managed worktrees are disabled in config".to_string()
        } else {
            String::new()
        };
        items.push(worktree_action_item(
            "Reconcile worktrees",
            "Discover linked Codewith worktrees and update state",
            !reconcile_disabled_reason.is_empty(),
            (!reconcile_disabled_reason.is_empty()).then_some(reconcile_disabled_reason),
            || AppEvent::ReconcileWorktrees,
        ));
    }

    if worktrees.is_empty() {
        items.push(SelectionItem {
            name: "No Codewith-managed worktrees".to_string(),
            description: Some(worktree_empty_description(policy)),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        let group_counts = worktree_group_counts(&worktrees);
        let mut current_group = None;
        for worktree in worktrees {
            let group = worktree_group(worktree.lifecycle_status);
            if current_group != Some(group) {
                current_group = Some(group);
                items.push(worktree_group_item(group, group_counts[group as usize]));
            }
            let actions = worktree_row_actions(&worktree, primary_action);
            items.push(SelectionItem {
                name: worktree_row_name(&worktree),
                description: Some(worktree_row_description(&worktree)),
                selected_description: Some(worktree_detail(&worktree)),
                actions,
                dismiss_on_select: true,
                search_value: Some(worktree_search_value(&worktree)),
                ..Default::default()
            });
        }
    }

    SelectionViewParams {
        title: Some(primary_action.title().to_string()),
        subtitle: Some(worktree_manager_subtitle(policy, primary_action)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Search worktrees".to_string()),
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn worktree_actions_params(worktree: Worktree, policy: &WorktreePolicy) -> SelectionViewParams {
    let worktree_id = worktree.worktree_id.clone();
    let read_worktree_id = worktree_id.clone();
    let read_base_repo_path = Some(worktree.base_repo_path.clone());
    let use_worktree_id = worktree_id.clone();
    let use_base_repo_path = Some(worktree.base_repo_path.clone());
    let start_agent_command = format!("/agent start --worktree {worktree_id} ");
    let open_agent_id = worktree.agent.as_ref().map(|agent| agent.agent_id.clone());
    let merge_worktree_id = worktree_id.clone();
    let merge_base_repo_path = Some(worktree.base_repo_path.clone());
    let release_worktree_id = worktree_id.clone();
    let release_base_repo_path = Some(worktree.base_repo_path.clone());
    let cleanup_worktree_id = worktree_id;
    let cleanup_base_repo_path = Some(worktree.base_repo_path.clone());
    let use_disabled_reason = if !policy.enabled {
        Some("Managed worktrees are disabled in config".to_string())
    } else if policy.main_sessions == WorktreeSessionMode::Off {
        Some("Main-session worktrees are disabled in config".to_string())
    } else if worktree.lifecycle_status != WorktreeLifecycleStatus::Active {
        Some("Only active worktrees can be used by the current session".to_string())
    } else {
        None
    };
    let start_agent_disabled_reason = if !policy.enabled {
        Some("Managed worktrees are disabled in config".to_string())
    } else if policy.sub_sessions == WorktreeSessionMode::Off {
        Some("Sub-session worktrees are disabled in config".to_string())
    } else if worktree.lifecycle_status != WorktreeLifecycleStatus::Active {
        Some("Only active worktrees can start background agents".to_string())
    } else {
        None
    };
    let merge_disabled_reason = if worktree.lifecycle_status != WorktreeLifecycleStatus::Active {
        Some("Only active worktrees can refresh merge candidates".to_string())
    } else if worktree.dirty {
        Some("Dirty worktrees must be cleaned up before refreshing merge candidates".to_string())
    } else {
        None
    };
    let mut items = vec![
        worktree_action_item(
            "Read details",
            "Show lease, path, branch, cleanup, and owner state",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::ReadWorktree {
                worktree_id: Some(read_worktree_id.clone()),
                base_repo_path: read_base_repo_path.clone(),
            },
        ),
        worktree_action_item(
            "Use in current session",
            "Retarget the current session cwd to this managed worktree",
            use_disabled_reason.is_some(),
            use_disabled_reason,
            move || AppEvent::UseWorktree {
                worktree_id: use_worktree_id.clone(),
                base_repo_path: use_base_repo_path.clone(),
            },
        ),
        worktree_action_item(
            "Start agent here",
            "Prefill a background-agent start command for this worktree",
            start_agent_disabled_reason.is_some(),
            start_agent_disabled_reason,
            move || AppEvent::PrefillComposer {
                text: start_agent_command.clone(),
            },
        ),
        worktree_action_item(
            "Open owning agent",
            "Open the background-agent action menu for this worktree owner",
            open_agent_id.is_none(),
            open_agent_id
                .is_none()
                .then(|| "Owning agent record is not available".to_string()),
            move || AppEvent::OpenBackgroundAgentActions {
                agent_id: open_agent_id.clone().unwrap_or_default(),
            },
        ),
        worktree_action_item(
            "Merge candidate",
            "Dry-run merge this worktree into the current target",
            merge_disabled_reason.is_some(),
            merge_disabled_reason,
            move || AppEvent::RefreshWorktreeMergeCandidate {
                worktree_id: merge_worktree_id.clone(),
                base_repo_path: merge_base_repo_path.clone(),
                target_ref: None,
            },
        ),
        worktree_action_item(
            "Release",
            "Release this worktree and retain it on disk",
            worktree.lifecycle_status == WorktreeLifecycleStatus::Deleted,
            (worktree.lifecycle_status == WorktreeLifecycleStatus::Deleted)
                .then(|| "Deleted worktrees cannot be released".to_string()),
            move || AppEvent::ReleaseWorktree {
                worktree_id: release_worktree_id.clone(),
                base_repo_path: release_base_repo_path.clone(),
            },
        ),
        worktree_action_item(
            "Cleanup",
            "Release and delete if the worktree is clean",
            worktree.lifecycle_status == WorktreeLifecycleStatus::Deleted,
            (worktree.lifecycle_status == WorktreeLifecycleStatus::Deleted)
                .then(|| "Deleted worktrees cannot be cleaned up".to_string()),
            move || AppEvent::CleanupWorktree {
                worktree_id: cleanup_worktree_id.clone(),
                base_repo_path: cleanup_base_repo_path.clone(),
                force_delete: false,
            },
        ),
    ];
    items.push(worktree_action_item(
        "Back to worktrees",
        "Return to all managed worktrees",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        || AppEvent::OpenWorktreeManager,
    ));

    SelectionViewParams {
        title: Some(format!(
            "Worktree {}",
            short_worktree_id(&worktree.worktree_id)
        )),
        subtitle: Some(worktree_detail(&worktree)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn worktree_action_item(
    name: impl Into<String>,
    description: impl Into<String>,
    is_disabled: bool,
    disabled_reason: Option<String>,
    event: impl Fn() -> AppEvent + Send + Sync + 'static,
) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(event());
    })];
    SelectionItem {
        name: name.into(),
        description: Some(description.into()),
        is_disabled,
        disabled_reason,
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorktreePrimaryAction {
    Actions,
    Read,
}

impl WorktreePrimaryAction {
    fn title(self) -> &'static str {
        match self {
            WorktreePrimaryAction::Actions => "Worktrees",
            WorktreePrimaryAction::Read => "Read worktree",
        }
    }
}

fn worktree_row_actions(
    worktree: &Worktree,
    primary_action: WorktreePrimaryAction,
) -> Vec<SelectionAction> {
    let worktree_id = worktree.worktree_id.clone();
    let base_repo_path = Some(worktree.base_repo_path.clone());
    match primary_action {
        WorktreePrimaryAction::Actions => vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenWorktreeActions {
                worktree_id: worktree_id.clone(),
                base_repo_path: base_repo_path.clone(),
            });
        })],
        WorktreePrimaryAction::Read => vec![Box::new(move |tx| {
            tx.send(AppEvent::ReadWorktree {
                worktree_id: Some(worktree_id.clone()),
                base_repo_path: base_repo_path.clone(),
            });
        })],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum WorktreeGroup {
    Active = 0,
    CleanupPending = 1,
    Released = 2,
    Deleted = 3,
}

impl WorktreeGroup {
    const COUNT: usize = 4;

    fn label(self) -> &'static str {
        match self {
            WorktreeGroup::Active => "Active",
            WorktreeGroup::CleanupPending => "Cleanup pending",
            WorktreeGroup::Released => "Released",
            WorktreeGroup::Deleted => "Deleted",
        }
    }

    fn description(self) -> &'static str {
        match self {
            WorktreeGroup::Active => "leased and available to inspect",
            WorktreeGroup::CleanupPending => "released but retained because cleanup needs review",
            WorktreeGroup::Released => "released but retained",
            WorktreeGroup::Deleted => "deleted or tombstoned",
        }
    }
}

fn worktree_group_counts(worktrees: &[Worktree]) -> [usize; WorktreeGroup::COUNT] {
    let mut counts = [0; WorktreeGroup::COUNT];
    for worktree in worktrees {
        counts[worktree_group(worktree.lifecycle_status) as usize] += 1;
    }
    counts
}

fn worktree_group_item(group: WorktreeGroup, count: usize) -> SelectionItem {
    SelectionItem {
        name: format!("{} ({count})", group.label()),
        description: Some(group.description().to_string()),
        is_disabled: true,
        search_value: Some(group.label().to_string()),
        ..Default::default()
    }
}

fn worktree_detail_lines(worktree: &Worktree) -> Vec<Line<'static>> {
    let mut lines = vec![worktree_header_line(worktree)];
    lines.push(Line::from(vec![
        "  mode ".dim(),
        worktree_mode_name(worktree.mode).into(),
        "  dirty ".dim(),
        worktree.dirty.to_string().into(),
        "  cleanup ".dim(),
        worktree_cleanup_policy_name(worktree.cleanup_policy).into(),
        "  updated ".dim(),
        format_timestamp(worktree.updated_at).into(),
    ]));
    lines.push(Line::from(vec![
        "  owner ".dim(),
        worktree_owner_summary(worktree).into(),
        "  force delete ".dim(),
        worktree.force_delete_requested.to_string().into(),
    ]));
    lines.push(Line::from(vec![
        "  base ".dim(),
        truncate_text(&worktree.base_repo_path, /*max_graphemes*/ 120).into(),
    ]));
    lines.push(Line::from(vec![
        "  path ".dim(),
        truncate_text(&worktree.worktree_path, /*max_graphemes*/ 120).into(),
    ]));
    if let Some(branch) = worktree.branch.as_ref() {
        lines.push(Line::from(vec![
            "  branch ".dim(),
            truncate_text(branch, /*max_graphemes*/ 96).into(),
        ]));
    }
    if let Some(head_sha) = worktree.head_sha.as_ref() {
        lines.push(Line::from(vec![
            "  head ".dim(),
            truncate_text(head_sha, /*max_graphemes*/ 64).into(),
        ]));
    }
    if let Some(base_sha) = worktree.base_sha.as_ref() {
        lines.push(Line::from(vec![
            "  base sha ".dim(),
            truncate_text(base_sha, /*max_graphemes*/ 64).into(),
        ]));
    }
    if let Some(cleanup_after) = worktree.cleanup_after {
        lines.push(Line::from(vec![
            "  cleanup after ".dim(),
            format_timestamp(cleanup_after).into(),
        ]));
    }
    if let Some(released_at) = worktree.released_at {
        lines.push(Line::from(vec![
            "  released ".dim(),
            format_timestamp(released_at).into(),
        ]));
    }
    if let Some(deleted_at) = worktree.deleted_at {
        lines.push(Line::from(vec![
            "  deleted ".dim(),
            format_timestamp(deleted_at).into(),
        ]));
    }
    lines.push(Line::from(vec![
        "  snapshot ".dim(),
        truncate_text(
            &json_summary(&worktree.status_snapshot),
            /*max_graphemes*/ 120,
        )
        .into(),
    ]));
    lines
}

fn worktree_header_line(worktree: &Worktree) -> Line<'static> {
    Line::from(vec![
        "  ".into(),
        worktree_status_label(worktree.lifecycle_status),
        " ".into(),
        short_worktree_id(&worktree.worktree_id).bold(),
        " ".into(),
        worktree_mode_name(worktree.mode).dim(),
        " ".into(),
        format_timestamp(worktree.updated_at).dim(),
    ])
}

fn worktree_row_name(worktree: &Worktree) -> String {
    format!(
        "{}  {}",
        short_worktree_id(&worktree.worktree_id),
        worktree_status_name(worktree.lifecycle_status)
    )
}

fn worktree_row_description(worktree: &Worktree) -> String {
    let mut parts = vec![
        worktree_mode_name(worktree.mode).to_string(),
        worktree_owner_summary(worktree),
        if worktree.dirty { "dirty" } else { "clean" }.to_string(),
        format!("updated {}", format_timestamp(worktree.updated_at)),
    ];
    if let Some(branch) = worktree.branch.as_ref() {
        parts.push(format!(
            "branch {}",
            truncate_text(branch, /*max_graphemes*/ 40)
        ));
    }
    parts.push(truncate_text(
        &worktree.worktree_path,
        /*max_graphemes*/ 80,
    ));
    parts.join(" - ")
}

fn worktree_detail(worktree: &Worktree) -> String {
    format!(
        "{} - {} - {} - {}",
        worktree_status_name(worktree.lifecycle_status),
        worktree_mode_name(worktree.mode),
        worktree_owner_summary(worktree),
        truncate_text(&worktree.worktree_path, /*max_graphemes*/ 96)
    )
}

fn worktree_search_value(worktree: &Worktree) -> String {
    format!(
        "{} {} {} {} {} {:?} {}",
        worktree.worktree_id,
        worktree.agent_id.clone().unwrap_or_default(),
        worktree.base_repo_path,
        worktree.worktree_path,
        worktree.branch.clone().unwrap_or_default(),
        worktree.lifecycle_status,
        worktree
            .agent
            .as_ref()
            .map(|agent| agent_run_status_name(agent.status))
            .unwrap_or_default()
    )
}

fn worktree_policy_description(policy: &WorktreePolicy) -> String {
    format!(
        "enabled {} | main {} | sub {} | cleanup {}{}{}",
        policy.enabled,
        worktree_session_mode_name(policy.main_sessions),
        worktree_session_mode_name(policy.sub_sessions),
        worktree_cleanup_policy_name(policy.cleanup_default),
        policy
            .root
            .as_ref()
            .map(|root| format!(" | root {}", truncate_text(root, /*max_graphemes*/ 60)))
            .unwrap_or_default(),
        policy
            .current_base_repo_path
            .as_ref()
            .map(|path| format!(" | repo {}", truncate_text(path, /*max_graphemes*/ 60)))
            .unwrap_or_else(|| " | repo unresolved".to_string())
    )
}

fn worktree_policy_search_value(policy: &WorktreePolicy) -> String {
    format!(
        "policy config enabled {} main {:?} sub {:?} repo {}",
        policy.enabled,
        policy.main_sessions,
        policy.sub_sessions,
        policy.current_base_repo_path.clone().unwrap_or_default()
    )
}

fn worktree_manager_subtitle(
    policy: &WorktreePolicy,
    primary_action: WorktreePrimaryAction,
) -> String {
    let prefix = match primary_action {
        WorktreePrimaryAction::Actions => "Codewith-managed worktrees",
        WorktreePrimaryAction::Read => "Choose a managed worktree to read",
    };
    policy
        .current_base_repo_path
        .as_ref()
        .map(|path| format!("{prefix} for {}", truncate_text(path, /*max_graphemes*/ 72)))
        .unwrap_or_else(|| "No git repository detected for this session".to_string())
}

fn worktree_empty_description(policy: &WorktreePolicy) -> String {
    if policy.current_base_repo_path.is_none() {
        return "No git repository was detected for this session".to_string();
    }
    if !policy.enabled {
        return "Managed worktrees are disabled by config".to_string();
    }
    if policy.main_sessions == WorktreeSessionMode::Off
        && policy.sub_sessions == WorktreeSessionMode::Off
    {
        return "Managed worktrees are disabled for main and sub sessions".to_string();
    }
    "Managed session and agent worktrees will appear here".to_string()
}

fn worktree_status_sort_key(status: WorktreeLifecycleStatus) -> u8 {
    worktree_group(status) as u8
}

fn worktree_group(status: WorktreeLifecycleStatus) -> WorktreeGroup {
    match status {
        WorktreeLifecycleStatus::Active => WorktreeGroup::Active,
        WorktreeLifecycleStatus::CleanupPending => WorktreeGroup::CleanupPending,
        WorktreeLifecycleStatus::Released => WorktreeGroup::Released,
        WorktreeLifecycleStatus::Deleted => WorktreeGroup::Deleted,
    }
}

fn worktree_status_label(status: WorktreeLifecycleStatus) -> Span<'static> {
    match status {
        WorktreeLifecycleStatus::Active => "active".green(),
        WorktreeLifecycleStatus::CleanupPending => "cleanup-pending".magenta(),
        WorktreeLifecycleStatus::Released => "released".fg(accent_color()),
        WorktreeLifecycleStatus::Deleted => "deleted".dim(),
    }
}

fn worktree_status_name(status: WorktreeLifecycleStatus) -> &'static str {
    match status {
        WorktreeLifecycleStatus::Active => "active",
        WorktreeLifecycleStatus::CleanupPending => "cleanup-pending",
        WorktreeLifecycleStatus::Released => "released",
        WorktreeLifecycleStatus::Deleted => "deleted",
    }
}

fn worktree_mode_name(mode: WorktreeMode) -> &'static str {
    match mode {
        WorktreeMode::IsolatedWorktree => "isolated",
        WorktreeMode::SharedRepository => "shared-repo",
    }
}

fn worktree_session_mode_name(mode: WorktreeSessionMode) -> &'static str {
    match mode {
        WorktreeSessionMode::Off => "off",
        WorktreeSessionMode::Manual => "manual",
        WorktreeSessionMode::Auto => "auto",
    }
}

fn worktree_cleanup_policy_name(policy: WorktreeCleanupPolicy) -> &'static str {
    match policy {
        WorktreeCleanupPolicy::Retain => "retain",
        WorktreeCleanupPolicy::DeleteIfClean => "delete-if-clean",
        WorktreeCleanupPolicy::ForceDelete => "force-delete",
    }
}

fn worktree_owner_kind_name(owner_kind: WorktreeOwnerKind) -> &'static str {
    match owner_kind {
        WorktreeOwnerKind::Manual => "manual",
        WorktreeOwnerKind::MainSession => "main session",
        WorktreeOwnerKind::SubSession => "sub session",
        WorktreeOwnerKind::BackgroundAgent => "background agent",
    }
}

fn worktree_owner_summary(worktree: &Worktree) -> String {
    if let Some(agent) = worktree.agent.as_ref() {
        return format!(
            "agent {} {}",
            short_worktree_id(&agent.agent_id),
            agent_run_status_name(agent.status)
        );
    }
    if let Some(agent_id) = worktree.owner_agent_run_id.as_ref() {
        return format!(
            "{} {}",
            worktree_owner_kind_name(worktree.owner_kind),
            short_worktree_id(agent_id)
        );
    }
    if let Some(thread_id) = worktree.owner_thread_id.as_ref() {
        return format!(
            "{} {}",
            worktree_owner_kind_name(worktree.owner_kind),
            short_worktree_id(thread_id)
        );
    }
    worktree_owner_kind_name(worktree.owner_kind).to_string()
}

fn agent_run_status_name(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Queued => "queued",
        AgentRunStatus::Starting => "starting",
        AgentRunStatus::Running => "running",
        AgentRunStatus::WaitingOnApproval => "waitingOnApproval",
        AgentRunStatus::WaitingOnUser => "waitingOnUser",
        AgentRunStatus::Stopping => "stopping",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
        AgentRunStatus::Orphaned => "orphaned",
    }
}

fn json_summary(value: &JsonValue) -> String {
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    for key in ["summary", "message", "text", "reason", "phase", "status"] {
        if let Some(text) = value.get(key).and_then(JsonValue::as_str) {
            return text.to_string();
        }
    }
    serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".to_string())
}

fn short_worktree_id(worktree_id: &str) -> String {
    worktree_id.chars().take(8).collect()
}

fn format_timestamp(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|datetime| {
            datetime
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| timestamp.to_string())
}
