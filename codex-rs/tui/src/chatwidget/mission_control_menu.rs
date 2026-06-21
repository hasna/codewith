//! Mission-control overview for local orchestration sessions.

use super::*;
use crate::bottom_pane::SelectionRowDisplay;
use crate::bottom_pane::SelectionTab;
use codex_app_server_protocol::ActiveSessionCapability;
use codex_app_server_protocol::LocalSessionStatus;
use codex_app_server_protocol::MissionControlCapabilities;
use codex_app_server_protocol::MissionControlOverviewResponse;
use codex_app_server_protocol::MissionControlSession;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanStatus;
use codex_app_server_protocol::ThreadGoalStatus;
use codex_app_server_protocol::ThreadPendingInteraction;
use codex_app_server_protocol::ThreadPendingInteractionKind;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
use codex_app_server_protocol::ThreadSchedule;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;
use codex_app_server_protocol::ToolRequestUserInputAnswer;
use codex_app_server_protocol::ToolRequestUserInputParams;
use codex_app_server_protocol::ToolRequestUserInputQuestion;
use ratatui::widgets::Paragraph;

const SESSIONS_TAB_ID: &str = "sessions";
const PROJECTS_TAB_ID: &str = "projects";
const QUESTIONS_TAB_ID: &str = "questions";
const WORK_QUEUE_TAB_ID: &str = "work-queue";
const GOAL_CHAINS_TAB_ID: &str = "goal-chains";
const SCHEDULES_TAB_ID: &str = "schedules";

impl ChatWidget {
    pub(crate) fn show_mission_control_overview(
        &mut self,
        response: MissionControlOverviewResponse,
    ) {
        let tabs = vec![
            SelectionTab {
                id: SESSIONS_TAB_ID.to_string(),
                label: "Sessions".to_string(),
                header: mission_control_header(&response),
                items: session_items(&response),
            },
            SelectionTab {
                id: PROJECTS_TAB_ID.to_string(),
                label: "Projects".to_string(),
                header: project_header(&response),
                items: project_items(&response),
            },
            SelectionTab {
                id: QUESTIONS_TAB_ID.to_string(),
                label: "Questions".to_string(),
                header: questions_header(&response),
                items: question_items(&response),
            },
            SelectionTab {
                id: WORK_QUEUE_TAB_ID.to_string(),
                label: "Queue".to_string(),
                header: work_queue_header(&response),
                items: work_queue_items(&response),
            },
            SelectionTab {
                id: GOAL_CHAINS_TAB_ID.to_string(),
                label: "Goals".to_string(),
                header: goal_chains_header(&response),
                items: goal_chain_items(&response),
            },
            SelectionTab {
                id: SCHEDULES_TAB_ID.to_string(),
                label: "Schedules".to_string(),
                header: schedules_header(&response),
                items: schedule_items(&response),
            },
        ];

        self.show_selection_view(SelectionViewParams {
            title: Some("Mission Control".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            tabs,
            initial_tab_id: Some(SESSIONS_TAB_ID.to_string()),
            is_searchable: true,
            search_placeholder: Some("Search".to_string()),
            col_width_mode: ColumnWidthMode::AutoAllRows,
            row_display: SelectionRowDisplay::SingleLine,
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

fn mission_control_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    Box::new(Paragraph::new(vec![Line::from(vec![
        response.sessions.len().to_string().into(),
        format!(" session{}", plural(response.sessions.len())).dim(),
        pagination_suffix(response.next_session_cursor.as_ref()).dim(),
    ])]))
}

fn project_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    Box::new(Paragraph::new(vec![Line::from(vec![
        project_count(&response.sessions).to_string().into(),
        format!(" project{}", plural(project_count(&response.sessions))).dim(),
    ])]))
}

fn questions_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let waiting = pending_interaction_count(response);
    Box::new(Paragraph::new(vec![Line::from(vec![
        waiting.to_string().into(),
        " waiting".dim(),
        pagination_suffix(response.next_pending_interaction_cursor.as_ref()).dim(),
    ])]))
}

fn work_queue_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let waiting = work_queue_attention_count(response);
    Box::new(Paragraph::new(vec![Line::from(vec![
        waiting.to_string().into(),
        " waiting".dim(),
    ])]))
}

fn goal_chains_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let stats = GoalChainStats::from_sessions(&response.sessions);
    Box::new(Paragraph::new(vec![Line::from(vec![
        stats.plans.to_string().into(),
        " plans".dim(),
    ])]))
}

fn schedules_header(response: &MissionControlOverviewResponse) -> Box<dyn Renderable> {
    let count = schedule_count(&response.sessions);
    Box::new(Paragraph::new(vec![Line::from(vec![
        count.to_string().into(),
        format!(" schedule{}", plural(count)).dim(),
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
            name: "More".to_string(),
            description: Some("available".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn session_item(session: &MissionControlSession, pending_count: usize) -> SelectionItem {
    let name = session_display_name(session);
    let actions = select_session_actions(session);

    SelectionItem {
        name,
        description: Some(session_state_label(session, pending_count)),
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
    let mut items = Vec::new();

    if !response.capabilities.durable_mailbox {
        items.push(SelectionItem {
            name: "Mailbox".to_string(),
            description: Some("off".to_string()),
            is_disabled: true,
            ..Default::default()
        });
    }

    items.extend(response.sessions.iter().filter_map(|session| {
        let pending_count = pending_by_thread
            .get(session.session.thread_id.as_str())
            .copied()
            .unwrap_or_default();
        (pending_count > 0 || session_has_limit_wait(session))
            .then(|| work_queue_item(session, pending_count))
    }));

    if items.is_empty() {
        items.push(SelectionItem {
            name: "No queued work".to_string(),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn work_queue_item(session: &MissionControlSession, pending_count: usize) -> SelectionItem {
    let description = queue_row_description(session, pending_count);
    SelectionItem {
        name: session_display_name(session),
        description: Some(description),
        actions: select_session_actions(session),
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {} {}",
            session_display_name(session),
            session.session.thread_id,
            session.session.cwd.as_path().display(),
            queue_state_label(session),
            limit_wait_label(session)
        )),
        ..Default::default()
    }
}

fn goal_chain_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    let mut items = Vec::new();
    for session in &response.sessions {
        for plan in &session.goal_plans {
            items.push(goal_plan_summary_item(session, plan));
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
        name: session_display_name(session),
        description: Some(goal_plan_row_description(plan)),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {}",
            session_display_name(session),
            plan.plan_id,
            goal_plan_row_description(plan),
            goal_plan_node_search_value(plan)
        )),
        ..Default::default()
    }
}

fn schedule_items(response: &MissionControlOverviewResponse) -> Vec<SelectionItem> {
    if !response.capabilities.scheduled_tasks {
        return vec![SelectionItem {
            name: "Schedules".to_string(),
            description: Some("off".to_string()),
            is_disabled: true,
            ..Default::default()
        }];
    }

    let mut items = Vec::new();
    for session in &response.sessions {
        for schedule in &session.schedules {
            items.push(schedule_item(session, schedule));
        }
    }

    if items.is_empty() {
        items.push(SelectionItem {
            name: "No schedules".to_string(),
            is_disabled: true,
            ..Default::default()
        });
    }

    items
}

fn schedule_item(session: &MissionControlSession, schedule: &ThreadSchedule) -> SelectionItem {
    let actions: Vec<SelectionAction> =
        if let Ok(thread_id) = ThreadId::from_string(&schedule.thread_id) {
            let schedule_id = schedule.schedule_id.clone();
            match schedule_kind(schedule) {
                ScheduleKind::Once => vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenThreadScheduleActions {
                        thread_id,
                        schedule_id: schedule_id.clone(),
                    });
                })],
                ScheduleKind::Recurring => vec![Box::new(move |tx| {
                    tx.send(AppEvent::OpenThreadLoopScheduleActions {
                        thread_id,
                        schedule_id: schedule_id.clone(),
                    });
                })],
            }
        } else {
            Vec::new()
        };
    let is_disabled = actions.is_empty();
    SelectionItem {
        name: schedule_row_name(schedule),
        description: Some(schedule_row_state(schedule)),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {} {}",
            session_display_name(session),
            session.session.thread_id,
            schedule.schedule_id,
            schedule_row_state(schedule),
            schedule.prompt
        )),
        is_disabled,
        ..Default::default()
    }
}

#[derive(Clone, Copy)]
enum ScheduleKind {
    Once,
    Recurring,
}

fn schedule_kind(schedule: &ThreadSchedule) -> ScheduleKind {
    match schedule.schedule {
        ThreadScheduleSpec::Once => ScheduleKind::Once,
        ThreadScheduleSpec::Dynamic
        | ThreadScheduleSpec::Interval { .. }
        | ThreadScheduleSpec::Cron { .. } => ScheduleKind::Recurring,
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
    let description = format!(
        "{} {}",
        pending_kind_label(interaction.kind),
        pending_status_label(interaction.status)
    );
    let disabled_reason = answerable
        .is_none()
        .then(|| disabled_question_reason(interaction));
    SelectionItem {
        name: session_name,
        description: Some(description),
        actions,
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {} {}",
            project,
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
        SelectionItem {
            name,
            description: Some(self.row_state()),
            search_value: Some(self.path),
            ..Default::default()
        }
    }

    fn row_state(&self) -> String {
        if self.waiting > 0 {
            return format!("{} waiting", self.waiting);
        }
        if self.needs_attention > 0 {
            return format!("{} attention", self.needs_attention);
        }
        if self.active > 0 {
            return format!("{} active", self.active);
        }
        format!("{} sessions", self.sessions)
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

fn session_state_label(session: &MissionControlSession, pending_count: usize) -> String {
    if pending_count > 0 {
        return format!("{pending_count} waiting");
    }
    if session_has_limit_wait(session) {
        return "limited".to_string();
    }
    if session_goal_is_blocked(session) {
        return "blocked".to_string();
    }
    if session
        .goal
        .as_ref()
        .is_some_and(|goal| goal.status == ThreadGoalStatus::Complete)
        || session
            .goal_plans
            .iter()
            .any(|plan| plan.status == ThreadGoalPlanStatus::Complete)
    {
        return "done".to_string();
    }
    session_status_label(session.session.status).to_string()
}

fn session_has_limit_wait(session: &MissionControlSession) -> bool {
    session.goal.as_ref().is_some_and(|goal| {
        matches!(
            goal.status,
            ThreadGoalStatus::UsageLimited | ThreadGoalStatus::BudgetLimited
        )
    }) || session
        .goal_plans
        .iter()
        .any(|plan| plan.usage_limited_node_count > 0 || plan.budget_limited_node_count > 0)
}

fn queue_row_description(session: &MissionControlSession, pending_count: usize) -> String {
    match (pending_count, session_has_limit_wait(session)) {
        (0, true) => "limited".to_string(),
        (0, false) => "idle".to_string(),
        (count, true) => format!("{count} waiting, limited"),
        (count, false) => format!("{count} waiting"),
    }
}

fn session_goal_is_blocked(session: &MissionControlSession) -> bool {
    session
        .goal
        .as_ref()
        .is_some_and(|goal| goal.status == ThreadGoalStatus::Blocked)
        || session
            .goal_plans
            .iter()
            .any(|plan| plan.status == ThreadGoalPlanStatus::Blocked || plan.blocked_node_count > 0)
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
        .filter(|session| session_has_limit_wait(session))
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

fn schedule_count(sessions: &[MissionControlSession]) -> usize {
    sessions.iter().map(|session| session.schedules.len()).sum()
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
    if session_has_limit_wait(session) {
        "limited".to_string()
    } else {
        "idle".to_string()
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

fn goal_plan_row_description(plan: &ThreadGoalPlan) -> String {
    if plan.ready_node_count > 0 {
        return format!("{} ready", plan.ready_node_count);
    }
    if plan.blocked_node_count > 0 || plan.status == ThreadGoalPlanStatus::Blocked {
        return "blocked".to_string();
    }
    if plan.usage_limited_node_count > 0 || plan.budget_limited_node_count > 0 {
        return "limited".to_string();
    }
    if plan.completed_node_count == plan.node_count {
        return "done".to_string();
    }
    goal_plan_status_label(plan.status).to_string()
}

fn goal_plan_node_search_value(plan: &ThreadGoalPlan) -> String {
    plan.nodes
        .iter()
        .map(|node| {
            format!(
                "{} {} {} {}",
                node.node_id,
                node.key,
                node.objective,
                goal_plan_node_status_label(node.status)
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn goal_plan_node_status_label(
    status: codex_app_server_protocol::ThreadGoalPlanNodeStatus,
) -> &'static str {
    match status {
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::Pending => "pending",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::Active => "active",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::Paused => "paused",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::Blocked => "blocked",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::UsageLimited => "usage-limited",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::BudgetLimited => "budget-limited",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::Complete => "complete",
        codex_app_server_protocol::ThreadGoalPlanNodeStatus::Cancelled => "cancelled",
    }
}

fn disabled_question_reason(interaction: &ThreadPendingInteraction) -> String {
    if interaction.kind == ThreadPendingInteractionKind::UserInput
        && serde_json::from_value::<ToolRequestUserInputParams>(interaction.request_payload.clone())
            .ok()
            .is_some_and(|params| params.questions.iter().any(question_needs_source_overlay))
    {
        "source session".to_string()
    } else {
        "unsupported here".to_string()
    }
}

fn schedule_row_name(schedule: &ThreadSchedule) -> String {
    let prompt = schedule.prompt.trim();
    if prompt.is_empty() {
        short_id(&schedule.schedule_id).to_string()
    } else {
        truncate_text(prompt, /*max_graphemes*/ 36)
    }
}

fn schedule_row_state(schedule: &ThreadSchedule) -> String {
    let id = short_schedule_id(&schedule.schedule_id);
    match schedule.status {
        ThreadScheduleStatus::Active => match schedule_kind(schedule) {
            ScheduleKind::Once => format!("scheduled {id}"),
            ScheduleKind::Recurring => format!("active {id}"),
        },
        ThreadScheduleStatus::Paused => format!("paused {id}"),
        ThreadScheduleStatus::Expired => format!("expired {id}"),
    }
}

fn short_schedule_id(value: &str) -> String {
    value.chars().take(8).collect()
}
