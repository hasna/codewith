//! Interactive workflow manager for `/workflow` and `/workflows`.

use super::*;
use crate::app_event::ThreadWorkflowAction;
use crate::bottom_pane::SelectionTab;
use crate::chatwidget::workflow_display::public_workflow_display_name;
use crate::chatwidget::workflow_display::sanitize_metadata_label;
use crate::chatwidget::workflow_display::short_id;
use crate::chatwidget::workflow_display::workflow_run_status_label;
use crate::chatwidget::workflow_display::workflow_status_label;
use codex_app_server_protocol::ThreadWorkflow;
use codex_app_server_protocol::ThreadWorkflowListResponse;
use codex_app_server_protocol::ThreadWorkflowRun;
use codex_app_server_protocol::ThreadWorkflowRunListResponse;
use codex_app_server_protocol::ThreadWorkflowRunStatus;
use codex_app_server_protocol::ThreadWorkflowStatus;

const SPECS_TAB_ID: &str = "specs";
const RUNS_TAB_ID: &str = "runs";
const APPROVALS_TAB_ID: &str = "approvals";

impl ChatWidget {
    pub(crate) fn show_thread_workflow_manager_loading(&mut self, thread_id: ThreadId) {
        self.show_selection_view(thread_workflow_manager_loading_params(thread_id));
    }

    pub(crate) fn show_thread_workflow_manager_error(
        &mut self,
        thread_id: ThreadId,
        action: &'static str,
        err: &color_eyre::Report,
    ) {
        self.show_selection_view(thread_workflow_manager_error_params(
            thread_id,
            action,
            &err.to_string(),
        ));
    }

    pub(crate) fn show_thread_workflow_manager(
        &mut self,
        thread_id: ThreadId,
        workflows: ThreadWorkflowListResponse,
        runs: ThreadWorkflowRunListResponse,
    ) {
        self.show_selection_view(thread_workflow_manager_params(
            thread_id,
            workflows.data,
            workflows.next_cursor.is_some(),
            runs.data,
            runs.next_cursor.is_some(),
        ));
    }

    pub(crate) fn show_thread_workflow_actions(
        &mut self,
        thread_id: ThreadId,
        workflow: ThreadWorkflow,
    ) {
        self.show_selection_view(thread_workflow_actions_params(thread_id, workflow));
    }

    pub(crate) fn show_thread_workflow_run_actions(
        &mut self,
        thread_id: ThreadId,
        run: ThreadWorkflowRun,
    ) {
        self.show_selection_view(thread_workflow_run_actions_params(thread_id, run));
    }

    pub(crate) fn show_thread_workflow_draft_prompt(&mut self) {
        if self.bottom_pane.is_task_running() {
            self.add_error_message(
                "'/workflow draft' is disabled while a task is in progress.".to_string(),
            );
            self.request_redraw();
            return;
        }

        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Create workflow".to_string(),
            "Describe the workflow to draft and press Enter".to_string(),
            String::new(),
            /*context_label*/ None,
            Box::new(move |request: String| {
                tx.send(AppEvent::PrefillComposer {
                    text: super::workflow_slash::workflow_generation_prompt(&request),
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }
}

fn thread_workflow_manager_loading_params(thread_id: ThreadId) -> SelectionViewParams {
    let refresh_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenThreadWorkflowManager { thread_id });
    })];
    SelectionViewParams {
        title: Some("Workflows".to_string()),
        subtitle: Some("Loading workflow specs and runs".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items: vec![SelectionItem {
            name: "Loading workflows".to_string(),
            description: Some("Fetching saved specs and recent runs for this thread".to_string()),
            actions: refresh_actions,
            dismiss_on_select: true,
            ..Default::default()
        }],
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn thread_workflow_manager_error_params(
    thread_id: ThreadId,
    action: &'static str,
    err: &str,
) -> SelectionViewParams {
    let refresh_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenThreadWorkflowManager { thread_id });
    })];
    SelectionViewParams {
        title: Some("Workflows".to_string()),
        subtitle: Some(format!("Failed to {action}")),
        footer_hint: Some(standard_popup_hint_line()),
        items: vec![
            SelectionItem {
                name: "Retry".to_string(),
                description: Some("Refresh workflow specs and runs".to_string()),
                actions: refresh_actions,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: "Workflow manager unavailable".to_string(),
                description: Some(err.to_string()),
                is_disabled: true,
                ..Default::default()
            },
        ],
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn thread_workflow_manager_params(
    thread_id: ThreadId,
    mut workflows: Vec<ThreadWorkflow>,
    workflows_truncated: bool,
    mut runs: Vec<ThreadWorkflowRun>,
    runs_truncated: bool,
) -> SelectionViewParams {
    workflows.sort_by_key(|workflow| {
        (
            workflow_status_sort_key(workflow.status),
            std::cmp::Reverse(workflow.updated_at),
            workflow.workflow_record_id.clone(),
        )
    });
    runs.sort_by_key(|run| {
        (
            workflow_run_status_sort_key(run.status),
            std::cmp::Reverse(run.updated_at),
            run.run_id.clone(),
        )
    });

    SelectionViewParams {
        title: Some("Workflows".to_string()),
        subtitle: Some(workflow_manager_subtitle(&workflows, &runs)),
        footer_hint: Some(standard_popup_hint_line()),
        items: Vec::new(),
        tabs: vec![
            SelectionTab {
                id: SPECS_TAB_ID.to_string(),
                label: format!("Specs ({})", workflows.len()),
                header: Box::new(()),
                items: workflow_spec_items(thread_id, workflows, workflows_truncated),
            },
            SelectionTab {
                id: RUNS_TAB_ID.to_string(),
                label: format!("Runs ({})", runs.len()),
                header: Box::new(()),
                items: workflow_run_items(thread_id, runs, runs_truncated),
            },
            SelectionTab {
                id: APPROVALS_TAB_ID.to_string(),
                label: "Approvals".to_string(),
                header: Box::new(()),
                items: workflow_approval_items(),
            },
        ],
        initial_tab_id: Some(SPECS_TAB_ID.to_string()),
        is_searchable: true,
        search_placeholder: Some("Search workflows".to_string()),
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn workflow_spec_items(
    thread_id: ThreadId,
    workflows: Vec<ThreadWorkflow>,
    workflows_truncated: bool,
) -> Vec<SelectionItem> {
    let create_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
        tx.send(AppEvent::OpenThreadWorkflowDraftPrompt);
    })];
    let mut items = vec![SelectionItem {
        name: "Create from prompt".to_string(),
        description: Some("Draft workflow YAML from a natural-language request".to_string()),
        actions: create_actions,
        dismiss_on_select: true,
        search_value: Some("create draft prompt workflow yaml".to_string()),
        ..Default::default()
    }];

    if workflows.is_empty() {
        items.push(SelectionItem {
            name: "No saved workflow specs".to_string(),
            description: Some(
                "Create one from a prompt or use /workflow draft <request>".to_string(),
            ),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        for workflow in workflows {
            let workflow_for_action = workflow.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenThreadWorkflowActions {
                    thread_id,
                    workflow: workflow_for_action.clone(),
                });
            })];
            items.push(SelectionItem {
                name: workflow_row_name(&workflow),
                selected_description: Some(workflow_selected_detail(&workflow)),
                actions,
                dismiss_on_select: true,
                search_value: Some(workflow_search_value(&workflow)),
                ..Default::default()
            });
        }
    }

    if workflows_truncated {
        items.push(SelectionItem {
            name: "More workflow specs available".to_string(),
            description: Some("Only the first page is shown here".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn workflow_run_items(
    thread_id: ThreadId,
    runs: Vec<ThreadWorkflowRun>,
    runs_truncated: bool,
) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    if runs.is_empty() {
        items.push(SelectionItem {
            name: "No workflow runs".to_string(),
            description: Some("Start a run from a saved spec".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    } else {
        for run in runs {
            let run_for_action = run.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenThreadWorkflowRunActions {
                    thread_id,
                    run: run_for_action.clone(),
                });
            })];
            items.push(SelectionItem {
                name: workflow_run_row_name(&run),
                selected_description: Some(workflow_run_selected_detail(&run)),
                actions,
                dismiss_on_select: true,
                search_value: Some(workflow_run_search_value(&run)),
                ..Default::default()
            });
        }
    }

    if runs_truncated {
        items.push(SelectionItem {
            name: "More workflow runs available".to_string(),
            description: Some("Only the first page is shown here".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn workflow_approval_items() -> Vec<SelectionItem> {
    vec![SelectionItem {
        name: "Workflow approval review".to_string(),
        description: Some(
            "Uses the existing approval popups; workflow-specific review queue is not exposed yet"
                .to_string(),
        ),
        is_disabled: true,
        disabled_reason: Some(
            "No workflow-scoped approval review API is available in this branch".to_string(),
        ),
        search_value: Some("approval review guardian workflow".to_string()),
        ..Default::default()
    }]
}

fn thread_workflow_actions_params(
    thread_id: ThreadId,
    workflow: ThreadWorkflow,
) -> SelectionViewParams {
    let inspect_workflow_id = workflow.workflow_record_id.clone();
    let run_workflow_id = workflow.workflow_record_id.clone();
    let mut items = vec![
        workflow_action_item(
            "Inspect spec",
            "Show sanitized spec metadata",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::ManageThreadWorkflow {
                thread_id,
                action: ThreadWorkflowAction::Show {
                    workflow_record_id: inspect_workflow_id.clone(),
                },
            },
        ),
        workflow_action_item(
            "Run",
            "Start a new workflow run",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::ManageThreadWorkflow {
                thread_id,
                action: ThreadWorkflowAction::RunStart {
                    workflow_record_id: run_workflow_id.clone(),
                },
            },
        ),
        disabled_workflow_item(
            "Delete",
            "Workflow delete is not available in the current API",
            "Missing workflow delete RPC",
        ),
        disabled_workflow_item(
            "Review approvals",
            "Workflow-specific approval review is not available yet",
            "Approval review uses the existing global approval popups",
        ),
    ];
    let back_thread_id = thread_id;
    items.push(workflow_action_item(
        "Back to workflows",
        "Return to specs and runs",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        move || AppEvent::OpenThreadWorkflowManager {
            thread_id: back_thread_id,
        },
    ));

    SelectionViewParams {
        title: Some(public_workflow_display_name(&workflow.display_name)),
        subtitle: Some(workflow_selected_detail(&workflow)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn thread_workflow_run_actions_params(
    thread_id: ThreadId,
    run: ThreadWorkflowRun,
) -> SelectionViewParams {
    let inspect_run_id = run.run_id.clone();
    let mut items = vec![workflow_action_item(
        "Inspect run",
        "Show step, verifier, and event progress",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        move || AppEvent::ManageThreadWorkflow {
            thread_id,
            action: ThreadWorkflowAction::RunShow {
                run_id: inspect_run_id.clone(),
            },
        },
    )];

    if workflow_run_can_pause(run.status) {
        let pause_run_id = run.run_id.clone();
        items.push(workflow_action_item(
            "Pause",
            "Pause this workflow run",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::ManageThreadWorkflow {
                thread_id,
                action: ThreadWorkflowAction::RunPause {
                    run_id: pause_run_id.clone(),
                },
            },
        ));
    }
    if workflow_run_can_resume(run.status) {
        let resume_run_id = run.run_id.clone();
        items.push(workflow_action_item(
            "Resume",
            "Resume this workflow run",
            /*is_disabled*/ false,
            /*disabled_reason*/ None,
            move || AppEvent::ManageThreadWorkflow {
                thread_id,
                action: ThreadWorkflowAction::RunResume {
                    run_id: resume_run_id.clone(),
                },
            },
        ));
    }
    let cancel_run_id = run.run_id.clone();
    let cannot_stop = !workflow_run_can_cancel(run.status);
    items.push(workflow_action_item(
        "Stop",
        "Request cancellation for this workflow run",
        cannot_stop,
        cannot_stop.then(|| "Completed workflow runs cannot be stopped".to_string()),
        move || AppEvent::ManageThreadWorkflow {
            thread_id,
            action: ThreadWorkflowAction::RunCancel {
                run_id: cancel_run_id.clone(),
            },
        },
    ));
    items.push(disabled_workflow_item(
        "Review approvals",
        "Workflow-specific approval review is not available yet",
        "Approval review uses the existing global approval popups",
    ));
    items.push(workflow_action_item(
        "Back to workflows",
        "Return to specs and runs",
        /*is_disabled*/ false,
        /*disabled_reason*/ None,
        move || AppEvent::OpenThreadWorkflowManager { thread_id },
    ));

    SelectionViewParams {
        title: Some(format!("Run {}", short_id(&run.run_id))),
        subtitle: Some(workflow_run_selected_detail(&run)),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        col_width_mode: ColumnWidthMode::Fixed,
        ..Default::default()
    }
}

fn workflow_action_item(
    name: &'static str,
    description: impl Into<String>,
    is_disabled: bool,
    disabled_reason: Option<String>,
    event: impl Fn() -> AppEvent + Send + Sync + 'static,
) -> SelectionItem {
    let actions: Vec<SelectionAction> = vec![Box::new(move |tx| tx.send(event()))];
    SelectionItem {
        name: name.to_string(),
        description: Some(description.into()),
        is_disabled,
        disabled_reason,
        actions,
        dismiss_on_select: true,
        ..Default::default()
    }
}

fn disabled_workflow_item(
    name: &'static str,
    description: &'static str,
    disabled_reason: &'static str,
) -> SelectionItem {
    SelectionItem {
        name: name.to_string(),
        description: Some(description.to_string()),
        is_disabled: true,
        disabled_reason: Some(disabled_reason.to_string()),
        ..Default::default()
    }
}

fn workflow_manager_subtitle(workflows: &[ThreadWorkflow], runs: &[ThreadWorkflowRun]) -> String {
    let active_runs = runs
        .iter()
        .filter(|run| workflow_run_is_active(run.status))
        .count();
    let completed_runs = runs
        .iter()
        .filter(|run| run.status == ThreadWorkflowRunStatus::Completed)
        .count();
    middle_dot(vec![
        format!("{} saved specs", workflows.len()),
        format!("{active_runs} active runs"),
        format!("{completed_runs} completed runs"),
    ])
}

fn middle_dot(parts: Vec<String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn workflow_row_name(workflow: &ThreadWorkflow) -> String {
    middle_dot(vec![
        public_workflow_display_name(&workflow.display_name),
        workflow_status_label(workflow.status).to_string(),
        format!("{} steps", workflow.step_count),
        format!("{} agents", workflow.agent_count),
    ])
}

fn workflow_selected_detail(workflow: &ThreadWorkflow) -> String {
    middle_dot(vec![
        workflow.spec_workflow_id.clone(),
        format!("{} verifiers", workflow.verifier_count),
        format!("{} model-routed steps", workflow.model_routed_step_count),
        format!("record {}", short_id(&workflow.workflow_record_id)),
        format!("updated {}", workflow.updated_at),
    ])
}

fn workflow_run_row_name(run: &ThreadWorkflowRun) -> String {
    middle_dot(vec![
        sanitize_metadata_label(&run.spec_workflow_id),
        workflow_run_status_label(run.status).to_string(),
        workflow_run_progress_label(run),
        format!("run {}", short_id(&run.run_id)),
    ])
}

fn workflow_run_selected_detail(run: &ThreadWorkflowRun) -> String {
    let mut parts = vec![
        workflow_run_activity_label(run),
        format!("{} verifiers", run.verifier_count),
        format!("{} events", run.event_count),
        format!("workflow {}", short_id(&run.workflow_record_id)),
    ];
    if let Some(reason) = run
        .status_reason
        .as_ref()
        .filter(|reason| !reason.is_empty())
    {
        parts.push(sanitize_metadata_label(reason));
    }
    middle_dot(parts)
}

fn workflow_run_progress_label(run: &ThreadWorkflowRun) -> String {
    format!(
        "{} active, {} waiting, {} done, {} failed",
        run.active_step_count,
        run.waiting_verifier_step_count,
        run.succeeded_step_count,
        run.failed_step_count
    )
}

fn workflow_run_activity_label(run: &ThreadWorkflowRun) -> String {
    let mut parts = Vec::new();
    if run.active_step_count > 0 {
        parts.push(format!("{} agent steps active", run.active_step_count));
    }
    if run.waiting_verifier_step_count > 0 {
        parts.push(format!(
            "{} steps waiting for verifiers",
            run.waiting_verifier_step_count
        ));
    }
    if run.event_count > 0 {
        parts.push(format!("{} monitor/activity events", run.event_count));
    }
    if parts.is_empty() {
        "no live agent or monitor activity".to_string()
    } else {
        parts.join(", ")
    }
}

fn workflow_search_value(workflow: &ThreadWorkflow) -> String {
    format!(
        "{} {} {} {}",
        workflow.workflow_record_id,
        workflow.spec_workflow_id,
        workflow.display_name,
        workflow.source_yaml_sha256
    )
}

fn workflow_run_search_value(run: &ThreadWorkflowRun) -> String {
    format!(
        "{} {} {} {} {:?}",
        run.run_id,
        run.workflow_record_id,
        run.spec_workflow_id,
        run.source_yaml_sha256,
        run.status
    )
}

fn workflow_status_sort_key(status: ThreadWorkflowStatus) -> u8 {
    match status {
        ThreadWorkflowStatus::Draft => 0,
        ThreadWorkflowStatus::NeedsClarification => 1,
        ThreadWorkflowStatus::Blocked => 2,
    }
}

fn workflow_run_status_sort_key(status: ThreadWorkflowRunStatus) -> u8 {
    match status {
        ThreadWorkflowRunStatus::Running
        | ThreadWorkflowRunStatus::Waiting
        | ThreadWorkflowRunStatus::Blocked
        | ThreadWorkflowRunStatus::Paused
        | ThreadWorkflowRunStatus::CancelRequested => 0,
        ThreadWorkflowRunStatus::Pending => 1,
        ThreadWorkflowRunStatus::Failed => 2,
        ThreadWorkflowRunStatus::Completed
        | ThreadWorkflowRunStatus::Cancelled
        | ThreadWorkflowRunStatus::Other => 3,
    }
}

fn workflow_run_is_active(status: ThreadWorkflowRunStatus) -> bool {
    matches!(
        status,
        ThreadWorkflowRunStatus::Pending
            | ThreadWorkflowRunStatus::Running
            | ThreadWorkflowRunStatus::Waiting
            | ThreadWorkflowRunStatus::Blocked
            | ThreadWorkflowRunStatus::Paused
            | ThreadWorkflowRunStatus::CancelRequested
    )
}

fn workflow_run_can_pause(status: ThreadWorkflowRunStatus) -> bool {
    matches!(
        status,
        ThreadWorkflowRunStatus::Pending
            | ThreadWorkflowRunStatus::Running
            | ThreadWorkflowRunStatus::Waiting
            | ThreadWorkflowRunStatus::Blocked
    )
}

fn workflow_run_can_resume(status: ThreadWorkflowRunStatus) -> bool {
    matches!(status, ThreadWorkflowRunStatus::Paused)
}

fn workflow_run_can_cancel(status: ThreadWorkflowRunStatus) -> bool {
    workflow_run_is_active(status)
}
