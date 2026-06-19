//! Mission-control overview for local orchestration sessions.

use super::*;
use crate::bottom_pane::SelectionTab;
use codex_app_server_protocol::ActiveSessionCapability;
use codex_app_server_protocol::LocalSessionStatus;
use codex_app_server_protocol::MissionControlCapabilities;
use codex_app_server_protocol::MissionControlOverviewResponse;
use codex_app_server_protocol::MissionControlSession;
use codex_app_server_protocol::ThreadActiveFlag;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanAutoExecute;
use codex_app_server_protocol::ThreadGoalPlanNode;
use codex_app_server_protocol::ThreadGoalPlanNodeStatus;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_app_server_protocol::ThreadPendingInteraction;
use codex_app_server_protocol::ThreadPendingInteractionKind;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
use codex_app_server_protocol::ToolRequestUserInputAnswer;
use codex_app_server_protocol::ToolRequestUserInputParams;
use codex_app_server_protocol::ToolRequestUserInputQuestion;
use ratatui::widgets::Paragraph;

const SESSIONS_TAB_ID: &str = "sessions";
const PROJECTS_TAB_ID: &str = "projects";
const QUESTIONS_TAB_ID: &str = "questions";
const WORK_QUEUE_TAB_ID: &str = "work-queue";
const GOAL_CHAINS_TAB_ID: &str = "goal-chains";

impl ChatWidget {
    pub(crate) fn show_mission_control_overview(
        &mut self,
        response: MissionControlOverviewResponse,
    ) {
        let subtitle = mission_control_subtitle(&response);
        let tabs = vec![
            SelectionTab {
                id: SESSIONS_TAB_ID.to_string(),
                label: format!("Sessions ({})", response.sessions.len()),
                header: mission_control_header(&response),
                items: session_items(&response),
            },
            SelectionTab {
                id: PROJECTS_TAB_ID.to_string(),
                label: format!("Projects ({})", project_count(&response.sessions)),
                header: project_header(&response),
                items: project_items(&response),
            },
            SelectionTab {
                id: QUESTIONS_TAB_ID.to_string(),
                label: format!("Questions ({})", pending_interaction_count(&response)),
                header: questions_header(&response),
                items: question_items(&response),
            },
            SelectionTab {
                id: WORK_QUEUE_TAB_ID.to_string(),
                label: format!("Work Queue ({})", work_queue_attention_count(&response)),
                header: work_queue_header(&response),
                items: work_queue_items(&response),
            },
            SelectionTab {
                id: GOAL_CHAINS_TAB_ID.to_string(),
                label: format!("Goal Chains ({})", goal_chain_count(&response.sessions)),
                header: goal_chains_header(&response),
                items: goal_chain_items(&response),
            },
        ];

        self.show_selection_view(SelectionViewParams {
            title: Some("Mission Control".to_string()),
            subtitle: Some(subtitle),
            footer_hint: Some(standard_popup_hint_line()),
            tabs,
            initial_tab_id: Some(SESSIONS_TAB_ID.to_string()),
            is_searchable: true,
            search_placeholder: Some(
                "Search sessions, projects, questions, queues, and goal chains".to_string(),
            ),
            col_width_mode: ColumnWidthMode::AutoAllRows,
            ..Default::default()
        });
    }

    pub(crate) fn show_mission_control_answer_prompt(
        &mut self,
        interaction: ThreadPendingInteraction,
    ) {
        let Some(target) = MissionControlAnswerTarget::from_interaction(&interaction) else {
            self.add_error_message(format!(
                "Pending interaction {} cannot be answered from mission control.",
                interaction.interaction_id
            ));
            return;
        };

        let tx = self.app_event_tx.clone();
        let interaction_id = interaction.interaction_id.clone();
        let thread_id = interaction.thread_id;
        let title = target.title();
        let placeholder = target.placeholder();
        let context_label = Some(target.context_label());
        let view = CustomPromptView::new(
            title,
            placeholder,
            String::new(),
            context_label,
            Box::new(move |answer: String| {
                tx.send(AppEvent::RespondMissionControlInteraction {
                    interaction_id: interaction_id.clone(),
                    thread_id: Some(thread_id.clone()),
                    terminal_status: ThreadPendingInteractionTerminalStatus::Responded,
                    response: target.response_payload(answer),
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }
}

fn mission_control_subtitle(response: &MissionControlOverviewResponse) -> String {
    let pending_count = response
        .pending_interactions
        .iter()
        .filter(|interaction| interaction_needs_attention(interaction.status))
        .count();
    let attention_count = response
        .sessions
        .iter()
        .filter(|session| session_needs_attention(session, &response.pending_interactions))
        .count();
    format!(
        "{} session{} | {} project{} | {} waiting | {} attention | {}",
        response.sessions.len(),
        plural(response.sessions.len()),
        project_count(&response.sessions),
        plural(project_count(&response.sessions)),
        pending_count,
        attention_count,
        capability_summary(&response.capabilities)
    )
}

fn mission_control_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let active = response
        .sessions
        .iter()
        .filter(|session| session.session.status == LocalSessionStatus::Active)
        .count();
    let idle = response
        .sessions
        .iter()
        .filter(|session| session.session.status == LocalSessionStatus::Idle)
        .count();
    let unloaded = response
        .sessions
        .iter()
        .filter(|session| session.session.status == LocalSessionStatus::NotLoaded)
        .count();
    Box::new(Paragraph::new(vec![Line::from(vec![
        "Active ".dim(),
        active.to_string().into(),
        "  Idle ".dim(),
        idle.to_string().into(),
        "  Not loaded ".dim(),
        unloaded.to_string().into(),
        pagination_suffix(response.next_session_cursor.as_ref()).dim(),
    ])]))
}

fn project_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let goals = response
        .sessions
        .iter()
        .filter(|session| session.goal.is_some())
        .count();
    let plans = response
        .sessions
        .iter()
        .map(|session| session.goal_plans.len())
        .sum::<usize>();
    Box::new(Paragraph::new(vec![Line::from(vec![
        "Goals ".dim(),
        goals.to_string().into(),
        "  Plans ".dim(),
        plans.to_string().into(),
        "  Pending interactions ".dim(),
        response.pending_interactions.len().to_string().into(),
    ])]))
}

fn questions_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let answerable = response
        .pending_interactions
        .iter()
        .filter(|interaction| {
            interaction_needs_attention(interaction.status)
                && MissionControlAnswerTarget::from_interaction(interaction).is_some()
        })
        .count();
    Box::new(Paragraph::new(vec![Line::from(vec![
        "Waiting ".dim(),
        pending_interaction_count(response).to_string().into(),
        "  Answerable ".dim(),
        answerable.to_string().into(),
        pagination_suffix(response.next_pending_interaction_cursor.as_ref()).dim(),
    ])]))
}

fn work_queue_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    Box::new(Paragraph::new(vec![Line::from(vec![
        "Durable mailbox ".dim(),
        capability_label(response.capabilities.durable_mailbox).into(),
        "  Waiting ".dim(),
        pending_interaction_count(response).to_string().into(),
        "  Usage/profile waits ".dim(),
        usage_profile_wait_count(response).to_string().into(),
        "  Unsupported controls ".dim(),
        "retry/cancel".into(),
    ])]))
}

fn goal_chains_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let stats = GoalChainStats::from_sessions(&response.sessions);
    Box::new(Paragraph::new(vec![Line::from(vec![
        "Plans ".dim(),
        stats.plans.to_string().into(),
        "  Ready ".dim(),
        stats.ready.to_string().into(),
        "  Active ".dim(),
        stats.active.to_string().into(),
        "  Blocked ".dim(),
        stats.blocked.to_string().into(),
        "  Usage/budget ".dim(),
        stats.waiting_on_limits.to_string().into(),
    ])]))
}

fn session_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    if response.sessions.is_empty() {
        return vec![SelectionItem {
            name: "No sessions found".to_string(),
            description: Some(empty_sessions_description(&response.capabilities)),
            is_disabled: true,
            ..Default::default()
        }];
    }

    let pending_by_thread = pending_counts_by_thread(&response.pending_interactions);
    let mut items = response
        .sessions
        .iter()
        .map(|session| {
            let pending_count = pending_by_thread
                .get(session.session.thread_id.as_str())
                .copied()
                .unwrap_or_default();
            session_item(session, pending_count)
        })
        .collect::<Vec<_>>();

    if response.next_session_cursor.is_some() {
        items.push(SelectionItem {
            name: "More sessions available".to_string(),
            description: Some("Narrow the search or reopen for the next page".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn session_item(session: &MissionControlSession, pending_count: usize) -> SelectionItem {
    let status = session_status_label(session.session.status);
    let project = project_label(session.session.cwd.as_path());
    let goal = goal_summary(session);
    let name = session_display_name(session);
    let detail = session_detail(session, pending_count);
    let actions = select_session_actions(session);

    SelectionItem {
        name,
        description: Some(format!(
            "{status} | {project} | {} | {goal}",
            waiting_label(pending_count)
        )),
        selected_description: Some(detail),
        actions,
        dismiss_on_select: true,
        search_value: Some(session_search_value(session)),
        ..Default::default()
    }
}

fn project_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    let pending_by_thread = pending_counts_by_thread(&response.pending_interactions);
    let mut projects: BTreeMap<String, ProjectStats> = BTreeMap::new();
    for session in &response.sessions {
        let path = session.session.cwd.as_path().display().to_string();
        let pending_count = pending_by_thread
            .get(session.session.thread_id.as_str())
            .copied()
            .unwrap_or_default();
        projects
            .entry(path.clone())
            .or_insert_with(|| ProjectStats::new(path))
            .add(session, pending_count);
    }

    if projects.is_empty() {
        return vec![SelectionItem {
            name: "No projects found".to_string(),
            description: Some(empty_sessions_description(&response.capabilities)),
            is_disabled: true,
            ..Default::default()
        }];
    }

    projects
        .into_values()
        .map(ProjectStats::into_selection_item)
        .collect()
}

fn question_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    let session_names: HashMap<&str, (String, String)> = response
        .sessions
        .iter()
        .map(|session| {
            (
                session.session.thread_id.as_str(),
                (
                    session_display_name(session),
                    project_label(session.session.cwd.as_path()),
                ),
            )
        })
        .collect();
    let mut items = response
        .pending_interactions
        .iter()
        .filter(|interaction| interaction_needs_attention(interaction.status))
        .map(|interaction| question_item(interaction, &session_names))
        .collect::<Vec<_>>();

    if items.is_empty() {
        items.push(SelectionItem {
            name: "No pending questions".to_string(),
            description: Some("No local pending interactions need an answer".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }
    if response.next_pending_interaction_cursor.is_some() {
        items.push(SelectionItem {
            name: "More questions available".to_string(),
            description: Some("Narrow the search or reopen for the next page".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn work_queue_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    let pending_by_thread = pending_counts_by_thread(&response.pending_interactions);
    let mut items = vec![mailbox_capability_item(&response.capabilities)];

    items.extend(response.sessions.iter().map(|session| {
        let pending_count = pending_by_thread
            .get(session.session.thread_id.as_str())
            .copied()
            .unwrap_or_default();
        work_queue_item(session, pending_count)
    }));

    if response.sessions.is_empty() {
        items.push(SelectionItem {
            name: "No queue targets found".to_string(),
            description: Some(empty_sessions_description(&response.capabilities)),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn mailbox_capability_item(capabilities: &MissionControlCapabilities) -> SelectionItem {
    if capabilities.durable_mailbox {
        SelectionItem {
            name: "Mailbox message rows not loaded".to_string(),
            description: Some(
                "Overview shows waiting sessions; individual queued messages and receipts need mailbox list wiring".to_string(),
            ),
            selected_description: Some(
                "Retry and cancel stay disabled until this view has message ids, lease state, and a supported cancel/retry RPC.".to_string(),
            ),
            is_disabled: true,
            ..Default::default()
        }
    } else {
        SelectionItem {
            name: "Durable mailbox unavailable".to_string(),
            description: Some(
                "This app-server cannot expose queued mailbox instructions from mission control"
                    .to_string(),
            ),
            is_disabled: true,
            ..Default::default()
        }
    }
}

fn work_queue_item(session: &MissionControlSession, pending_count: usize) -> SelectionItem {
    let queue_state = queue_state_label(session);
    let limit_state = limit_wait_label(session);
    let description = format!(
        "{} | {} | {} | {queue_state}",
        project_label(session.session.cwd.as_path()),
        session_status_label(session.session.status),
        waiting_label(pending_count)
    );
    let selected_description = format!(
        "Thread {} | {} | active flags {} | peer {} | {}",
        short_id(&session.session.thread_id),
        session.session.cwd.as_path().display(),
        active_flags_label(&session.session.active_flags),
        peer_capabilities_label(session),
        limit_state
    );
    SelectionItem {
        name: session_display_name(session),
        description: Some(description),
        selected_description: Some(selected_description),
        actions: select_session_actions(session),
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {} {}",
            session_display_name(session),
            session.session.thread_id,
            session.session.cwd.as_path().display(),
            queue_state,
            limit_state
        )),
        ..Default::default()
    }
}

fn goal_chain_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    for session in &response.sessions {
        for plan in &session.goal_plans {
            items.push(goal_plan_summary_item(session, plan));
            items.extend(
                plan.nodes
                    .iter()
                    .map(|node| goal_plan_node_item(session, plan, node)),
            );
        }
    }

    if items.is_empty() {
        items.push(SelectionItem {
            name: "No goal chains found".to_string(),
            description: Some("No sessions in this overview have durable goal plans".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn goal_plan_summary_item(session: &MissionControlSession, plan: &ThreadGoalPlan) -> SelectionItem {
    let actions: Vec<SelectionAction> =
        if let Ok(thread_id) = ThreadId::from_string(&plan.thread_id) {
            let plan_for_action = plan.clone();
            vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenThreadGoalPlanDetail {
                    thread_id,
                    plan: plan_for_action.clone(),
                });
            })]
        } else {
            Vec::new()
        };
    SelectionItem {
        name: format!(
            "{} / {}",
            session_display_name(session),
            short_id(&plan.plan_id)
        ),
        description: Some(goal_plan_row_description(plan)),
        selected_description: Some(goal_plan_selected_description(session, plan)),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {}",
            session_display_name(session),
            plan.plan_id,
            goal_plan_row_description(plan)
        )),
        ..Default::default()
    }
}

fn goal_plan_node_item(
    session: &MissionControlSession,
    plan: &ThreadGoalPlan,
    node: &ThreadGoalPlanNode,
) -> SelectionItem {
    let activate_target = node
        .ready
        .then(|| ThreadId::from_string(&node.thread_id).ok())
        .flatten();
    let actions: Vec<SelectionAction> = if let Some(thread_id) = activate_target {
        let node_id = node.node_id.clone();
        vec![Box::new(move |tx| {
            tx.send(AppEvent::ActivateThreadGoalPlanNode {
                thread_id,
                node_id: node_id.clone(),
            });
        })]
    } else {
        Vec::new()
    };
    let disabled_reason = if node.ready {
        actions
            .is_empty()
            .then(|| "This goal-chain node has an invalid thread id".to_string())
    } else {
        Some("Only ready goal-chain nodes can be activated from mission control".to_string())
    };
    SelectionItem {
        name: format!(
            "  #{} {}",
            node.sequence,
            truncate_text(&node.key, /*max_graphemes*/ 22)
        ),
        description: Some(goal_node_row_description(plan, node)),
        selected_description: Some(goal_node_selected_description(node)),
        actions,
        dismiss_on_select: disabled_reason.is_none(),
        disabled_reason,
        search_value: Some(format!(
            "{} {} {} {} {}",
            session_display_name(session),
            plan.plan_id,
            node.key,
            node.node_id,
            node.objective
        )),
        ..Default::default()
    }
}

fn question_item(
    interaction: &ThreadPendingInteraction,
    session_names: &HashMap<&str, (String, String)>,
) -> SelectionItem {
    let (session_name, project) = session_names
        .get(interaction.thread_id.as_str())
        .cloned()
        .unwrap_or_else(|| {
            (
                short_id(&interaction.thread_id).to_string(),
                "unknown".to_string(),
            )
        });
    let answerable = MissionControlAnswerTarget::from_interaction(interaction);
    let actions = answerable.as_ref().map_or_else(Vec::new, |_| {
        let interaction = interaction.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenMissionControlInteractionAnswer {
                interaction: interaction.clone(),
            });
        })];
        actions
    });
    let disabled_reason = answerable
        .is_none()
        .then(|| mission_control_interaction_disabled_reason(interaction));
    let description = format!(
        "{} | {} | {} | {}",
        project,
        pending_kind_label(interaction.kind),
        pending_status_label(interaction.status),
        truncate_interaction_preview(&interaction.request_payload_preview)
    );
    let selected_description = format!(
        "Thread {} | interaction {} | {}",
        short_id(&interaction.thread_id),
        interaction.interaction_id,
        interaction.request_payload_preview
    );
    SelectionItem {
        name: session_name,
        description: Some(description),
        selected_description: Some(selected_description),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {}",
            interaction.thread_id,
            interaction.interaction_id,
            pending_kind_label(interaction.kind),
            interaction.request_payload_preview
        )),
        disabled_reason,
        ..Default::default()
    }
}

fn select_session_actions(session: &MissionControlSession) -> Vec<SelectionAction> {
    let thread_id = session.session.thread_id.clone();
    if let Ok(thread_id) = ThreadId::from_string(&thread_id) {
        vec![Box::new(move |tx| {
            tx.send(AppEvent::SelectAgentThread(thread_id));
        })]
    } else {
        Vec::new()
    }
}

#[derive(Clone)]
enum MissionControlAnswerTarget {
    RequestUserInput {
        questions: Vec<ToolRequestUserInputQuestion>,
        preview: String,
    },
    Terminal {
        preview: String,
    },
}

impl MissionControlAnswerTarget {
    fn from_interaction(interaction: &ThreadPendingInteraction) -> Option<Self> {
        match interaction.kind {
            ThreadPendingInteractionKind::UserInput => {
                let params = serde_json::from_value::<ToolRequestUserInputParams>(
                    interaction.request_payload.clone(),
                )
                .ok()?;
                if params.questions.iter().any(question_needs_source_overlay) {
                    return None;
                }
                Some(Self::RequestUserInput {
                    questions: params.questions,
                    preview: interaction.request_payload_preview.clone(),
                })
            }
            ThreadPendingInteractionKind::UsageLimit
            | ThreadPendingInteractionKind::ProfileSwitch
            | ThreadPendingInteractionKind::Blocked => Some(Self::Terminal {
                preview: interaction.request_payload_preview.clone(),
            }),
            ThreadPendingInteractionKind::CommandApproval
            | ThreadPendingInteractionKind::FileChangeApproval
            | ThreadPendingInteractionKind::McpElicitation
            | ThreadPendingInteractionKind::PermissionGrant
            | ThreadPendingInteractionKind::DynamicTool => None,
        }
    }

    fn title(&self) -> String {
        match self {
            Self::RequestUserInput { questions, .. } => {
                if questions.len() == 1 {
                    "Answer question".to_string()
                } else {
                    format!("Answer {} questions", questions.len())
                }
            }
            Self::Terminal { .. } => "Record interaction response".to_string(),
        }
    }

    fn placeholder(&self) -> String {
        match self {
            Self::RequestUserInput { questions, .. } if questions.len() > 1 => {
                "Paste one answer per line; a single line applies to every question".to_string()
            }
            Self::RequestUserInput { .. } => "Type the answer and press Enter".to_string(),
            Self::Terminal { .. } => {
                "Record a note; this does not switch profiles or resume work".to_string()
            }
        }
    }

    fn context_label(&self) -> String {
        match self {
            Self::RequestUserInput { questions, preview } => questions
                .first()
                .map(|question| {
                    format!(
                        "{}: {}",
                        question.header,
                        truncate_interaction_preview(&question.question)
                    )
                })
                .unwrap_or_else(|| truncate_interaction_preview(preview)),
            Self::Terminal { preview } => truncate_interaction_preview(preview),
        }
    }

    fn response_payload(&self, answer: String) -> ThreadPendingInteractionResponsePayload {
        match self {
            Self::RequestUserInput { questions, .. } => {
                ThreadPendingInteractionResponsePayload::RequestUserInput {
                    answers: question_answers(questions, answer),
                }
            }
            Self::Terminal { .. } => {
                ThreadPendingInteractionResponsePayload::Terminal { reason: answer }
            }
        }
    }
}

fn question_needs_source_overlay(question: &ToolRequestUserInputQuestion) -> bool {
    question.is_secret
        || question.is_other
        || question
            .options
            .as_ref()
            .is_some_and(|options| !options.is_empty())
}

fn mission_control_interaction_disabled_reason(interaction: &ThreadPendingInteraction) -> String {
    if interaction.kind == ThreadPendingInteractionKind::UserInput
        && let Ok(params) = serde_json::from_value::<ToolRequestUserInputParams>(
            interaction.request_payload.clone(),
        )
        && params.questions.iter().any(question_needs_source_overlay)
    {
        return "Use the source session for secret, option, or Other prompts so the standard masked UI is preserved".to_string();
    }

    "This interaction needs a specialized approval UI in the source session".to_string()
}

fn question_answers(
    questions: &[ToolRequestUserInputQuestion],
    answer: String,
) -> HashMap<String, ToolRequestUserInputAnswer> {
    let mut lines = answer
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(answer);
    }
    let shared_answer = (lines.len() == 1).then(|| lines[0].clone());
    questions
        .iter()
        .enumerate()
        .map(|(idx, question)| {
            let answer = shared_answer
                .clone()
                .or_else(|| lines.get(idx).cloned())
                .unwrap_or_default();
            (
                question.id.clone(),
                ToolRequestUserInputAnswer {
                    answers: vec![answer],
                },
            )
        })
        .collect()
}

#[derive(Default)]
struct ProjectStats {
    path: String,
    sessions: usize,
    active: usize,
    idle: usize,
    waiting: usize,
    goals: usize,
    plans: usize,
    needs_attention: usize,
}

impl ProjectStats {
    fn new(path: String) -> Self {
        Self {
            path,
            ..Default::default()
        }
    }

    fn add(&mut self, session: &MissionControlSession, pending_count: usize) {
        self.sessions += 1;
        match session.session.status {
            LocalSessionStatus::Active => self.active += 1,
            LocalSessionStatus::Idle => self.idle += 1,
            LocalSessionStatus::SystemError
            | LocalSessionStatus::Closing
            | LocalSessionStatus::LoadedWithoutActivePeer
            | LocalSessionStatus::NotLoaded => {}
        }
        if pending_count > 0 {
            self.waiting += pending_count;
        }
        if session.goal.is_some() {
            self.goals += 1;
        }
        self.plans += session.goal_plans.len();
        if pending_count > 0 || session_goal_needs_attention(session) {
            self.needs_attention += 1;
        }
    }

    fn into_selection_item(self) -> SelectionItem {
        let name = project_label(Path::new(&self.path));
        let description = format!(
            "{} session{} | {} active | {} idle | {} waiting | {} attention",
            self.sessions,
            plural(self.sessions),
            self.active,
            self.idle,
            self.waiting,
            self.needs_attention
        );
        let selected_description = format!(
            "{} | {} | {} goal{} | {} plan{}",
            description,
            self.path,
            self.goals,
            plural(self.goals),
            self.plans,
            plural(self.plans)
        );
        SelectionItem {
            name,
            description: Some(description),
            selected_description: Some(selected_description),
            search_value: Some(self.path),
            ..Default::default()
        }
    }
}

#[derive(Default)]
struct GoalChainStats {
    plans: usize,
    ready: i64,
    active: i64,
    blocked: i64,
    waiting_on_limits: i64,
}

impl GoalChainStats {
    fn from_sessions(sessions: &[MissionControlSession]) -> Self {
        let mut stats = Self::default();
        for plan in sessions
            .iter()
            .flat_map(|session| session.goal_plans.iter())
        {
            stats.plans += 1;
            stats.ready += plan.ready_node_count;
            stats.active += plan.active_node_count;
            stats.blocked += plan.blocked_node_count;
            stats.waiting_on_limits +=
                plan.usage_limited_node_count + plan.budget_limited_node_count;
        }
        stats
    }
}

fn pending_counts_by_thread(interactions: &[ThreadPendingInteraction]) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for interaction in interactions {
        if interaction_needs_attention(interaction.status) {
            *counts.entry(interaction.thread_id.as_str()).or_default() += 1;
        }
    }
    counts
}

fn session_needs_attention(
    session: &MissionControlSession,
    interactions: &[ThreadPendingInteraction],
) -> bool {
    interactions.iter().any(|interaction| {
        interaction.thread_id == session.session.thread_id
            && interaction_needs_attention(interaction.status)
    }) || session_goal_needs_attention(session)
        || matches!(session.session.status, LocalSessionStatus::SystemError)
}

fn session_goal_needs_attention(session: &MissionControlSession) -> bool {
    session.goal.as_ref().is_some_and(|goal| {
        matches!(
            goal.status,
            ThreadGoalStatus::Blocked
                | ThreadGoalStatus::UsageLimited
                | ThreadGoalStatus::BudgetLimited
                | ThreadGoalStatus::Cancelled
        )
    }) || session.goal_plans.iter().any(|plan| {
        matches!(
            plan.status,
            ThreadGoalPlanStatus::Blocked
                | ThreadGoalPlanStatus::BudgetLimited
                | ThreadGoalPlanStatus::Cancelled
        )
    })
}

fn session_display_name(session: &MissionControlSession) -> String {
    session
        .session
        .display_name
        .as_ref()
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| {
            let label = project_label(session.session.cwd.as_path());
            if label.is_empty() {
                short_id(&session.session.thread_id).to_string()
            } else {
                label
            }
        })
}

fn session_detail(session: &MissionControlSession, pending_count: usize) -> String {
    let branch = session
        .session
        .git_info
        .as_ref()
        .and_then(|git| git.branch.as_deref())
        .unwrap_or("no branch");
    format!(
        "Thread {} | {} | branch {branch} | {} | {} | {} | {}",
        short_id(&session.session.thread_id),
        session.session.cwd.as_path().display(),
        model_label(session),
        waiting_label(pending_count),
        goal_detail(session),
        plan_detail(session)
    )
}

fn session_search_value(session: &MissionControlSession) -> String {
    format!(
        "{} {} {} {}",
        session_display_name(session),
        session.session.thread_id,
        session.session.cwd.as_path().display(),
        model_label(session)
    )
}

fn model_label(session: &MissionControlSession) -> String {
    session
        .session
        .model
        .as_ref()
        .map(|model| format!("{}/{}", session.session.model_provider, model))
        .unwrap_or_else(|| session.session.model_provider.clone())
}

fn goal_summary(session: &MissionControlSession) -> String {
    match (&session.goal, session.goal_plans.is_empty()) {
        (Some(goal), true) => format!("goal {}", goal_status_label(goal.status)),
        (Some(goal), false) => format!(
            "goal {} | {} plan{}",
            goal_status_label(goal.status),
            session.goal_plans.len(),
            plural(session.goal_plans.len())
        ),
        (None, false) => format!(
            "{} plan{}",
            session.goal_plans.len(),
            plural(session.goal_plans.len())
        ),
        (None, true) => "no goal".to_string(),
    }
}

fn goal_detail(session: &MissionControlSession) -> String {
    session.goal.as_ref().map_or_else(
        || "no goal".to_string(),
        |goal| {
            format!(
                "goal {} {} tokens",
                goal_status_label(goal.status),
                format_tokens_compact(goal.tokens_used)
            )
        },
    )
}

fn plan_detail(session: &MissionControlSession) -> String {
    if session.goal_plans.is_empty() {
        return "no plans".to_string();
    }
    let active = session
        .goal_plans
        .iter()
        .filter(|plan| plan.status == ThreadGoalPlanStatus::Active)
        .count();
    let blocked = session
        .goal_plans
        .iter()
        .filter(|plan| plan.status == ThreadGoalPlanStatus::Blocked)
        .count();
    let ready = session
        .goal_plans
        .iter()
        .map(|plan| plan.ready_node_count)
        .sum::<i64>();
    format!(
        "{} plan{} | {} active | {} blocked | {} ready",
        session.goal_plans.len(),
        plural(session.goal_plans.len()),
        active,
        blocked,
        ready
    )
}

fn waiting_label(count: usize) -> String {
    if count == 0 {
        "no waiting".to_string()
    } else {
        format!("{count} waiting")
    }
}

fn pending_interaction_count(response: &MissionControlOverviewResponse) -> usize {
    response
        .pending_interactions
        .iter()
        .filter(|interaction| interaction_needs_attention(interaction.status))
        .count()
}

fn work_queue_attention_count(response: &MissionControlOverviewResponse) -> usize {
    pending_interaction_count(response) + usage_profile_wait_count(response)
}

fn usage_profile_wait_count(response: &MissionControlOverviewResponse) -> usize {
    let interaction_waits = response
        .pending_interactions
        .iter()
        .filter(|interaction| {
            interaction_needs_attention(interaction.status)
                && matches!(
                    interaction.kind,
                    ThreadPendingInteractionKind::UsageLimit
                        | ThreadPendingInteractionKind::ProfileSwitch
                )
        })
        .count();
    let goal_waits = response
        .sessions
        .iter()
        .filter(|session| {
            session.goal.as_ref().is_some_and(|goal| {
                matches!(
                    goal.status,
                    ThreadGoalStatus::UsageLimited | ThreadGoalStatus::BudgetLimited
                )
            }) || session
                .goal_plans
                .iter()
                .any(|plan| plan.usage_limited_node_count > 0 || plan.budget_limited_node_count > 0)
        })
        .count();
    interaction_waits + goal_waits
}

fn interaction_needs_attention(status: ThreadPendingInteractionStatus) -> bool {
    matches!(
        status,
        ThreadPendingInteractionStatus::Pending | ThreadPendingInteractionStatus::Delivered
    )
}

fn pending_kind_label(kind: ThreadPendingInteractionKind) -> &'static str {
    match kind {
        ThreadPendingInteractionKind::CommandApproval => "command approval",
        ThreadPendingInteractionKind::FileChangeApproval => "file approval",
        ThreadPendingInteractionKind::UserInput => "question",
        ThreadPendingInteractionKind::McpElicitation => "mcp elicitation",
        ThreadPendingInteractionKind::PermissionGrant => "permission grant",
        ThreadPendingInteractionKind::DynamicTool => "dynamic tool",
        ThreadPendingInteractionKind::UsageLimit => "usage limit",
        ThreadPendingInteractionKind::ProfileSwitch => "profile switch",
        ThreadPendingInteractionKind::Blocked => "blocked",
    }
}

fn pending_status_label(status: ThreadPendingInteractionStatus) -> &'static str {
    match status {
        ThreadPendingInteractionStatus::Pending => "pending",
        ThreadPendingInteractionStatus::Delivered => "delivered",
        ThreadPendingInteractionStatus::Responded => "responded",
        ThreadPendingInteractionStatus::Expired => "expired",
        ThreadPendingInteractionStatus::Cancelled => "cancelled",
        ThreadPendingInteractionStatus::Denied => "denied",
        ThreadPendingInteractionStatus::NoLongerWaiting => "no-longer-waiting",
    }
}

fn truncate_interaction_preview(value: &str) -> String {
    truncate_text(value, /*max_graphemes*/ 96)
}

fn capability_summary(capabilities: &MissionControlCapabilities) -> String {
    let remote = if capabilities.remote_dispatch {
        "remote on"
    } else {
        "remote off"
    };
    let mailbox = if capabilities.durable_mailbox {
        "mailbox on"
    } else {
        "mailbox off"
    };
    format!("{remote} | {mailbox}")
}

fn capability_label(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn empty_sessions_description(capabilities: &MissionControlCapabilities) -> String {
    if capabilities.local_sessions {
        "No local sessions matched the current filters".to_string()
    } else {
        "Local session registry unavailable".to_string()
    }
}

fn project_count(sessions: &[MissionControlSession]) -> usize {
    sessions
        .iter()
        .map(|session| session.session.cwd.as_path())
        .collect::<HashSet<_>>()
        .len()
}

fn goal_chain_count(sessions: &[MissionControlSession]) -> usize {
    sessions
        .iter()
        .map(|session| session.goal_plans.len())
        .sum()
}

fn project_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

fn pagination_suffix(cursor: Option<&String>) -> String {
    if cursor.is_some() {
        "  More available".to_string()
    } else {
        String::new()
    }
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn session_status_label(status: LocalSessionStatus) -> &'static str {
    match status {
        LocalSessionStatus::Active => "active",
        LocalSessionStatus::Idle => "idle",
        LocalSessionStatus::SystemError => "system-error",
        LocalSessionStatus::Closing => "closing",
        LocalSessionStatus::LoadedWithoutActivePeer => "loaded-no-peer",
        LocalSessionStatus::NotLoaded => "not-loaded",
    }
}

fn active_flags_label(flags: &[ThreadActiveFlag]) -> String {
    if flags.is_empty() {
        return "none".to_string();
    }
    flags
        .iter()
        .map(|flag| match flag {
            ThreadActiveFlag::WaitingOnApproval => "waiting-on-approval",
            ThreadActiveFlag::WaitingOnUserInput => "waiting-on-user-input",
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn peer_capabilities_label(session: &MissionControlSession) -> String {
    let Some(peer) = session.session.peer.as_ref() else {
        return "no live peer".to_string();
    };
    if peer.capabilities.is_empty() {
        return "peer has no message capabilities".to_string();
    }
    let labels = peer
        .capabilities
        .iter()
        .map(|capability| match capability {
            ActiveSessionCapability::ReceiveMessage => "receive",
            ActiveSessionCapability::QueueMessage => "queue",
            ActiveSessionCapability::TriggerTurn => "trigger",
            ActiveSessionCapability::ClaudeChannelBridge => "claude-bridge",
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("peer {labels}")
}

fn queue_state_label(session: &MissionControlSession) -> &'static str {
    if session.session.peer.as_ref().is_some_and(|peer| {
        peer.capabilities
            .iter()
            .any(|capability| capability == &ActiveSessionCapability::QueueMessage)
    }) {
        "queueable"
    } else if session.session.peer.as_ref().is_some_and(|peer| {
        peer.capabilities
            .iter()
            .any(|capability| capability == &ActiveSessionCapability::ReceiveMessage)
    }) {
        "live-only"
    } else {
        "not-routable"
    }
}

fn limit_wait_label(session: &MissionControlSession) -> String {
    let goal_wait = session.goal.as_ref().and_then(|goal| match goal.status {
        ThreadGoalStatus::UsageLimited => Some("goal usage-limited"),
        ThreadGoalStatus::BudgetLimited => Some("goal budget-limited"),
        ThreadGoalStatus::Active
        | ThreadGoalStatus::Paused
        | ThreadGoalStatus::Blocked
        | ThreadGoalStatus::Complete
        | ThreadGoalStatus::Cancelled => None,
    });
    let usage_limited_nodes = session
        .goal_plans
        .iter()
        .map(|plan| plan.usage_limited_node_count)
        .sum::<i64>();
    let budget_limited_nodes = session
        .goal_plans
        .iter()
        .map(|plan| plan.budget_limited_node_count)
        .sum::<i64>();
    let mut parts = goal_wait.map_or_else(Vec::new, |label| vec![label.to_string()]);
    if usage_limited_nodes > 0 {
        parts.push(format!(
            "{usage_limited_nodes} usage-limited node{}",
            plural_i64(usage_limited_nodes)
        ));
    }
    if budget_limited_nodes > 0 {
        parts.push(format!(
            "{budget_limited_nodes} budget-limited node{}",
            plural_i64(budget_limited_nodes)
        ));
    }
    if parts.is_empty() {
        "no usage/profile wait".to_string()
    } else {
        parts.join(" | ")
    }
}

fn goal_status_label(status: ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::Blocked => "blocked",
        ThreadGoalStatus::UsageLimited => "usage-limited",
        ThreadGoalStatus::BudgetLimited => "budget-limited",
        ThreadGoalStatus::Complete => "complete",
        ThreadGoalStatus::Cancelled => "cancelled",
    }
}

fn goal_plan_status_label(status: ThreadGoalPlanStatus) -> &'static str {
    match status {
        ThreadGoalPlanStatus::Active => "active",
        ThreadGoalPlanStatus::Paused => "paused",
        ThreadGoalPlanStatus::Blocked => "blocked",
        ThreadGoalPlanStatus::BudgetLimited => "budget-limited",
        ThreadGoalPlanStatus::Complete => "complete",
        ThreadGoalPlanStatus::Cancelled => "cancelled",
    }
}

fn goal_plan_auto_execute_label(auto_execute: ThreadGoalPlanAutoExecute) -> &'static str {
    match auto_execute {
        ThreadGoalPlanAutoExecute::Off => "manual",
        ThreadGoalPlanAutoExecute::ReadyOnly => "ready-only",
        ThreadGoalPlanAutoExecute::AiDirected => "ai-directed",
    }
}

fn goal_node_status_label(status: ThreadGoalPlanNodeStatus) -> &'static str {
    match status {
        ThreadGoalPlanNodeStatus::Pending => "pending",
        ThreadGoalPlanNodeStatus::Active => "active",
        ThreadGoalPlanNodeStatus::Paused => "paused",
        ThreadGoalPlanNodeStatus::Blocked => "blocked",
        ThreadGoalPlanNodeStatus::UsageLimited => "usage-limited",
        ThreadGoalPlanNodeStatus::BudgetLimited => "budget-limited",
        ThreadGoalPlanNodeStatus::Complete => "complete",
        ThreadGoalPlanNodeStatus::Cancelled => "cancelled",
    }
}

fn goal_plan_row_description(plan: &ThreadGoalPlan) -> String {
    format!(
        "{} | {}/{} complete | {} ready | auto {}",
        goal_plan_status_label(plan.status),
        plan.completed_node_count,
        plan.node_count,
        plan.ready_node_count,
        goal_plan_auto_execute_label(plan.auto_execute)
    )
}

fn goal_plan_selected_description(
    session: &MissionControlSession,
    plan: &ThreadGoalPlan,
) -> String {
    let mut parts = vec![
        format!("Thread {}", short_id(&session.session.thread_id)),
        format!("plan {}", plan.plan_id),
        project_label(session.session.cwd.as_path()),
        format!("{} active", plan.active_node_count),
        format!("{} blocked", plan.blocked_node_count),
        format!("{} usage-limited", plan.usage_limited_node_count),
        format!("{} budget-limited", plan.budget_limited_node_count),
    ];
    if plan.cancelled_node_count > 0 {
        parts.push(format!("{} cancelled", plan.cancelled_node_count));
    }
    parts.push(plan_budget_label(plan));
    parts.join(" | ")
}

fn goal_node_row_description(plan: &ThreadGoalPlan, node: &ThreadGoalPlanNode) -> String {
    let ready = if node.ready { "ready" } else { "not ready" };
    format!(
        "{} | {ready} | plan {} | auto {}",
        goal_node_status_label(node.status),
        goal_plan_status_label(plan.status),
        goal_plan_auto_execute_label(plan.auto_execute)
    )
}

fn goal_node_selected_description(node: &ThreadGoalPlanNode) -> String {
    format!(
        "Node {} | key {} | plan {} | depends {} | {} | {}",
        node.node_id,
        node.key,
        node.plan_id,
        depends_on_label(&node.depends_on),
        node_budget_label(node),
        node.objective
    )
}

fn depends_on_label(depends_on: &[String]) -> String {
    if depends_on.is_empty() {
        "none".to_string()
    } else {
        depends_on.join(",")
    }
}

fn plan_budget_label(plan: &ThreadGoalPlan) -> String {
    plan.max_tokens.map_or_else(
        || format!("{} tokens", format_tokens_compact(plan.total_tokens_used)),
        |max_tokens| {
            format!(
                "{} / {} tokens",
                format_tokens_compact(plan.total_tokens_used),
                format_tokens_compact(max_tokens)
            )
        },
    )
}

fn node_budget_label(node: &ThreadGoalPlanNode) -> String {
    node.token_budget.map_or_else(
        || format!("{} tokens", format_tokens_compact(node.tokens_used)),
        |token_budget| {
            format!(
                "{} / {} tokens",
                format_tokens_compact(node.tokens_used),
                format_tokens_compact(token_budget)
            )
        },
    )
}

fn plural_i64(count: i64) -> &'static str {
    if count == 1 { "" } else { "s" }
}
