use std::sync::Arc;
use std::sync::Weak;

use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadGoal;
use codex_app_server_protocol::ThreadGoalPlan;
use codex_app_server_protocol::ThreadGoalPlanUpdatedNotification;
use codex_app_server_protocol::ThreadGoalUpdatedNotification;
use codex_app_server_protocol::ThreadNameUpdatedNotification;
use codex_core::NewThread;
use codex_core::StartThreadOptions;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::config::GoalAutoExecuteMode;
use codex_core::config::PostGoalContextAction;
use codex_extension_api::AgentSpawnFuture;
use codex_extension_api::AgentSpawner;
use codex_extension_api::ExtensionEventSink;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_goal_extension::GoalExtensionConfig;
use codex_goal_extension::GoalService;
use codex_login::AuthManager;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_rollout::state_db::StateDbHandle;

use crate::outgoing_message::OutgoingMessageSender;
use crate::request_processors::api_thread_goal_plan_from_state_for_thread;
use crate::thread_state::ThreadListenerCommand;
use crate::thread_state::ThreadStateManager;

pub(crate) fn thread_extensions<S>(
    guardian_agent_spawner: S,
    event_sink: Arc<dyn ExtensionEventSink>,
    auth_manager: Arc<AuthManager>,
    state_db: Option<StateDbHandle>,
    thread_manager: Weak<ThreadManager>,
    goal_service: Arc<GoalService>,
) -> Arc<ExtensionRegistry<Config>>
where
    S: AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> + 'static,
{
    let mut builder = ExtensionRegistryBuilder::<Config>::with_event_sink(event_sink);
    let workflow_state_db = state_db.clone();
    if let Some(state_db) = state_db {
        crate::mission_control_tools::install(
            &mut builder,
            state_db.clone(),
            thread_manager.clone(),
            |config: &Config| config.features.enabled(codex_features::Feature::Goals),
            |config: &Config| {
                config
                    .features
                    .enabled(codex_features::Feature::ScheduledTasks)
            },
        );
        codex_goal_extension::install_with_backend(
            &mut builder,
            state_db,
            codex_otel::global(),
            thread_manager,
            goal_service,
            |config: &Config| GoalExtensionConfig {
                enabled: config.features.enabled(codex_features::Feature::Goals),
                auto_execute: match config.goals.auto_execute {
                    GoalAutoExecuteMode::Off => codex_state::ThreadGoalPlanAutoExecute::Off,
                    GoalAutoExecuteMode::ReadyOnly => {
                        codex_state::ThreadGoalPlanAutoExecute::ReadyOnly
                    }
                    GoalAutoExecuteMode::AiDirected => {
                        codex_state::ThreadGoalPlanAutoExecute::AiDirected
                    }
                },
                max_auto_goals_per_plan: config.goals.max_auto_goals_per_plan,
                max_tokens_per_goal_plan: config.goals.max_tokens_per_goal_plan,
                post_goal_context: post_goal_context_action(config.goals.post_goal_context),
                post_goal_plan_context: post_goal_context_action(
                    config.goals.post_goal_plan_context,
                ),
            },
        );
    }
    codex_guardian::install(&mut builder, guardian_agent_spawner);
    codex_memories_extension::install(&mut builder, codex_otel::global());
    codex_web_search_extension::install(&mut builder, auth_manager.clone());
    codex_image_generation_extension::install(&mut builder, auth_manager);
    codex_workflows_extension::install(&mut builder, workflow_state_db, |config: &Config| {
        config.features.enabled(codex_features::Feature::Workflows)
    });
    Arc::new(builder.build())
}

fn post_goal_context_action(action: PostGoalContextAction) -> codex_state::PostGoalContextAction {
    match action {
        PostGoalContextAction::Keep => codex_state::PostGoalContextAction::Keep,
        PostGoalContextAction::Compact => codex_state::PostGoalContextAction::Compact,
    }
}

pub(crate) fn app_server_extension_event_sink(
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
    state_db: Option<StateDbHandle>,
) -> Arc<dyn ExtensionEventSink> {
    Arc::new(AppServerExtensionEventSink {
        outgoing,
        thread_state_manager,
        state_db,
    })
}

struct AppServerExtensionEventSink {
    outgoing: Arc<OutgoingMessageSender>,
    thread_state_manager: ThreadStateManager,
    state_db: Option<StateDbHandle>,
}

impl ExtensionEventSink for AppServerExtensionEventSink {
    fn emit(&self, event: Event) {
        match event.msg {
            EventMsg::ThreadGoalUpdated(thread_goal_event) => {
                let thread_id = thread_goal_event.thread_id;
                let turn_id = thread_goal_event.turn_id;
                let goal: ThreadGoal = thread_goal_event.goal.into();
                if let Some(listener_command_tx) = self
                    .thread_state_manager
                    .current_listener_command_tx(thread_id)
                {
                    let command = ThreadListenerCommand::EmitThreadGoalUpdated {
                        turn_id: turn_id.clone(),
                        goal: goal.clone(),
                    };
                    if listener_command_tx.send(command).is_ok() {
                        return;
                    }
                    tracing::warn!(
                        "failed to enqueue extension goal update for {thread_id}: listener command channel is closed"
                    );
                }
                let outgoing = Arc::clone(&self.outgoing);
                tokio::spawn(async move {
                    outgoing
                        .send_server_notification(ServerNotification::ThreadGoalUpdated(
                            ThreadGoalUpdatedNotification {
                                thread_id: thread_id.to_string(),
                                turn_id,
                                goal,
                            },
                        ))
                        .await;
                });
            }
            EventMsg::ThreadGoalPlanUpdated(thread_goal_plan_event) => {
                let thread_id = thread_goal_plan_event.thread_id;
                let turn_id = thread_goal_plan_event.turn_id;
                let plan: ThreadGoalPlan = thread_goal_plan_event.plan.into();
                let plan_id = plan.plan_id.clone();
                let outgoing = Arc::clone(&self.outgoing);
                let thread_state_manager = self.thread_state_manager.clone();
                let state_db = self.state_db.clone();
                tokio::spawn(async move {
                    if let Some(state_db) = state_db {
                        match state_db
                            .thread_goals()
                            .get_thread_goal_plan_for_thread(thread_id, plan_id.as_str())
                            .await
                        {
                            Ok(snapshot) => {
                                if let Some(snapshot) = snapshot {
                                    for target_thread_id in snapshot.participant_thread_ids() {
                                        let plan = api_thread_goal_plan_from_state_for_thread(
                                            snapshot.clone(),
                                            target_thread_id,
                                        );
                                        emit_thread_goal_plan_updated(
                                            &thread_state_manager,
                                            &outgoing,
                                            target_thread_id,
                                            turn_id.clone(),
                                            plan,
                                        )
                                        .await;
                                    }
                                    return;
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    "failed to reload full goal plan snapshot for extension event: {err}"
                                );
                            }
                        }
                    }
                    emit_thread_goal_plan_updated(
                        &thread_state_manager,
                        &outgoing,
                        thread_id,
                        turn_id,
                        plan,
                    )
                    .await;
                });
            }
            EventMsg::ThreadNameUpdated(thread_name_event) => {
                let outgoing = Arc::clone(&self.outgoing);
                tokio::spawn(async move {
                    outgoing
                        .send_server_notification(ServerNotification::ThreadNameUpdated(
                            ThreadNameUpdatedNotification {
                                thread_id: thread_name_event.thread_id.to_string(),
                                thread_name: thread_name_event.thread_name,
                            },
                        ))
                        .await;
                });
            }
            msg => {
                tracing::debug!(event_id = %event.id, ?msg, "dropping unsupported extension event");
            }
        }
    }
}

pub(crate) fn guardian_agent_spawner(
    thread_manager: Weak<ThreadManager>,
) -> impl AgentSpawner<StartThreadOptions, Spawned = NewThread, Error = CodexErr> {
    move |forked_from_thread_id: ThreadId,
          options: StartThreadOptions|
          -> AgentSpawnFuture<'static, NewThread, CodexErr> {
        let thread_manager = thread_manager.clone();
        Box::pin(async move {
            let thread_manager = thread_manager.upgrade().ok_or_else(|| {
                CodexErr::UnsupportedOperation("thread manager dropped".to_string())
            })?;
            thread_manager
                .spawn_subagent(forked_from_thread_id, options)
                .await
        })
    }
}

async fn emit_thread_goal_plan_updated(
    thread_state_manager: &ThreadStateManager,
    outgoing: &OutgoingMessageSender,
    thread_id: ThreadId,
    turn_id: Option<String>,
    plan: ThreadGoalPlan,
) {
    if let Some(listener_command_tx) = thread_state_manager.current_listener_command_tx(thread_id) {
        let command = ThreadListenerCommand::EmitThreadGoalPlanUpdated {
            turn_id: turn_id.clone(),
            plan: plan.clone(),
        };
        if listener_command_tx.send(command).is_ok() {
            return;
        }
        tracing::warn!(
            "failed to enqueue extension goal plan update for {thread_id}: listener command channel is closed"
        );
    }
    outgoing
        .send_server_notification(ServerNotification::ThreadGoalPlanUpdated(
            ThreadGoalPlanUpdatedNotification {
                thread_id: thread_id.to_string(),
                turn_id,
                plan,
            },
        ))
        .await;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use codex_analytics::AnalyticsEventsClient;
    use codex_protocol::protocol::ThreadGoal as CoreThreadGoal;
    use codex_protocol::protocol::ThreadGoalPlan as CoreThreadGoalPlan;
    use codex_protocol::protocol::ThreadGoalPlanAutoExecute as CoreThreadGoalPlanAutoExecute;
    use codex_protocol::protocol::ThreadGoalPlanStatus as CoreThreadGoalPlanStatus;
    use codex_protocol::protocol::ThreadGoalPlanUpdatedEvent;
    use codex_protocol::protocol::ThreadGoalStatus;
    use codex_protocol::protocol::ThreadGoalUpdatedEvent;
    use codex_protocol::protocol::ThreadNameUpdatedEvent;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    use crate::outgoing_message::OutgoingEnvelope;
    use crate::outgoing_message::OutgoingMessage;

    use super::*;

    #[tokio::test]
    async fn app_server_event_sink_uses_listener_fifo_for_goal_updates_and_clears() {
        let (outgoing_tx, _outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let thread_state_manager = ThreadStateManager::new();
        let thread_id = ThreadId::default();
        let (listener_command_tx, mut listener_command_rx) = mpsc::unbounded_channel();
        thread_state_manager.register_listener_command_tx(thread_id, listener_command_tx.clone());
        let sink =
            app_server_extension_event_sink(outgoing, thread_state_manager, /*state_db*/ None);

        for turn_id in ["turn-1", "turn-2"] {
            sink.emit(thread_goal_updated_event(thread_id, turn_id));
        }
        listener_command_tx
            .send(ThreadListenerCommand::EmitThreadGoalCleared)
            .expect("listener command channel should be open");

        let mut observed = Vec::new();
        for _ in 0..3 {
            let command = timeout(Duration::from_secs(1), listener_command_rx.recv())
                .await
                .expect("timed out waiting for listener command")
                .expect("listener command channel closed unexpectedly");
            match command {
                ThreadListenerCommand::EmitThreadGoalUpdated { turn_id, .. } => {
                    observed.push(turn_id.expect("extension goal updates should include turn ids"));
                }
                ThreadListenerCommand::EmitThreadGoalCleared => {
                    observed.push("cleared".to_string())
                }
                _ => panic!("unexpected listener command"),
            }
        }

        assert_eq!(
            vec![
                "turn-1".to_string(),
                "turn-2".to_string(),
                "cleared".to_string()
            ],
            observed
        );
    }

    #[tokio::test]
    async fn app_server_event_sink_fans_out_delegated_plan_past_default_list_limit() {
        let tempdir = TempDir::new().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let owner_thread_id = ThreadId::new();
        let delegate_thread_id = ThreadId::new();
        let delegate_metadata = codex_state::ThreadMetadataBuilder::new(
            delegate_thread_id,
            state_db
                .codex_home()
                .join(format!("rollout-{delegate_thread_id}.jsonl")),
            chrono::Utc::now(),
            codex_protocol::protocol::SessionSource::Cli,
        )
        .build("test-provider");
        state_db
            .upsert_thread(&delegate_metadata)
            .await
            .expect("delegate thread should be materialized");
        let created = state_db
            .thread_goals()
            .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
                thread_id: owner_thread_id,
                auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
                max_tokens: None,
                nodes: vec![codex_state::ThreadGoalPlanNodeCreateParams {
                    key: "delegated".to_string(),
                    objective: "Handle the delegated part.".to_string(),
                    assigned_thread_id: Some(delegate_thread_id),
                    title: None,
                    priority: 0,
                    token_budget: None,
                    depends_on: Vec::new(),
                }],
            })
            .await
            .expect("delegated plan should be created");
        let delegated_plan_id = created.snapshot.plan.plan_id.clone();

        tokio::time::sleep(Duration::from_millis(5)).await;
        for index in 0..codex_state::DEFAULT_THREAD_GOAL_PLAN_LIST_LIMIT {
            state_db
                .thread_goals()
                .create_thread_goal_plan(codex_state::ThreadGoalPlanCreateParams {
                    thread_id: owner_thread_id,
                    auto_execute: codex_state::ThreadGoalPlanAutoExecute::Off,
                    max_tokens: None,
                    nodes: vec![codex_state::ThreadGoalPlanNodeCreateParams {
                        key: format!("filler-{index}"),
                        objective: format!("Filler goal plan node {index}."),
                        assigned_thread_id: None,
                        title: None,
                        priority: 0,
                        token_budget: None,
                        depends_on: Vec::new(),
                    }],
                })
                .await
                .expect("newer filler plan should be created");
        }
        let first_page = state_db
            .thread_goals()
            .list_thread_goal_plans(owner_thread_id)
            .await
            .expect("owner plan list should load");
        assert!(
            !first_page
                .iter()
                .any(|snapshot| snapshot.plan.plan_id == delegated_plan_id)
        );

        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let sink = app_server_extension_event_sink(
            outgoing,
            ThreadStateManager::new(),
            Some(state_db.clone()),
        );

        sink.emit(thread_goal_plan_updated_event(
            owner_thread_id,
            "turn-delegated",
            delegated_plan_id,
        ));

        let mut observed = Vec::new();
        for _ in 0..2 {
            let envelope = timeout(Duration::from_secs(1), outgoing_rx.recv())
                .await
                .expect("timed out waiting for forwarded extension event")
                .expect("outgoing channel closed unexpectedly");
            let OutgoingEnvelope::Broadcast { message } = envelope else {
                panic!("expected broadcast notification");
            };
            let OutgoingMessage::AppServerNotification(ServerNotification::ThreadGoalPlanUpdated(
                notification,
            )) = message
            else {
                panic!("expected thread goal plan updated notification");
            };
            observed.push(notification);
        }

        let owner_thread_id = owner_thread_id.to_string();
        let delegate_thread_id = delegate_thread_id.to_string();
        let owner_update = observed
            .iter()
            .find(|notification| notification.thread_id == owner_thread_id)
            .expect("owner should receive delegated plan update");
        let delegate_update = observed
            .iter()
            .find(|notification| notification.thread_id == delegate_thread_id)
            .expect("delegate should receive delegated plan update");
        assert_eq!(0, owner_update.plan.ready_node_count);
        assert!(!owner_update.plan.nodes[0].ready);
        assert_eq!(1, delegate_update.plan.ready_node_count);
        assert!(delegate_update.plan.nodes[0].ready);
        assert_eq!(
            delegate_thread_id,
            delegate_update.plan.nodes[0].assigned_thread_id
        );
    }

    fn thread_goal_updated_event(thread_id: ThreadId, turn_id: &str) -> Event {
        Event {
            id: turn_id.to_string(),
            msg: EventMsg::ThreadGoalUpdated(ThreadGoalUpdatedEvent {
                thread_id,
                turn_id: Some(turn_id.to_string()),
                goal: CoreThreadGoal {
                    thread_id,
                    goal_id: format!("goal-{turn_id}"),
                    objective: "wire extension events".to_string(),
                    title: None,
                    status: ThreadGoalStatus::Active,
                    token_budget: Some(123),
                    tokens_used: 45,
                    time_used_seconds: 6,
                    created_at: 7,
                    updated_at: 8,
                },
            }),
        }
    }

    fn thread_goal_plan_updated_event(
        thread_id: ThreadId,
        turn_id: &str,
        plan_id: String,
    ) -> Event {
        Event {
            id: turn_id.to_string(),
            msg: EventMsg::ThreadGoalPlanUpdated(ThreadGoalPlanUpdatedEvent {
                thread_id,
                turn_id: Some(turn_id.to_string()),
                plan: CoreThreadGoalPlan {
                    plan_id,
                    thread_id,
                    status: CoreThreadGoalPlanStatus::Active,
                    auto_execute: CoreThreadGoalPlanAutoExecute::Off,
                    max_tokens: None,
                    total_tokens_used: 0,
                    total_time_used_seconds: 0,
                    remaining_tokens: None,
                    node_count: 0,
                    completed_node_count: 0,
                    ready_node_count: 0,
                    active_node_count: 0,
                    pending_node_count: 0,
                    paused_node_count: 0,
                    blocked_node_count: 0,
                    usage_limited_node_count: 0,
                    budget_limited_node_count: 0,
                    cancelled_node_count: 0,
                    created_at: 1,
                    updated_at: 1,
                    nodes: Vec::new(),
                },
            }),
        }
    }

    #[tokio::test]
    async fn app_server_event_sink_forwards_thread_name_updates() {
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel(4);
        let outgoing = Arc::new(OutgoingMessageSender::new(
            outgoing_tx,
            AnalyticsEventsClient::disabled(),
        ));
        let sink = app_server_extension_event_sink(
            outgoing,
            ThreadStateManager::new(),
            /*state_db*/ None,
        );
        let thread_id = ThreadId::default();

        sink.emit(Event {
            id: "call-1".to_string(),
            msg: EventMsg::ThreadNameUpdated(ThreadNameUpdatedEvent {
                thread_id,
                thread_name: Some("Release follow-up".to_string()),
            }),
        });

        let envelope = timeout(Duration::from_secs(1), outgoing_rx.recv())
            .await
            .expect("timed out waiting for forwarded extension event")
            .expect("outgoing channel closed unexpectedly");
        let OutgoingEnvelope::Broadcast { message } = envelope else {
            panic!("expected broadcast notification");
        };
        let OutgoingMessage::AppServerNotification(ServerNotification::ThreadNameUpdated(
            notification,
        )) = message
        else {
            panic!("expected thread name updated notification");
        };

        assert_eq!(
            ThreadNameUpdatedNotification {
                thread_id: thread_id.to_string(),
                thread_name: Some("Release follow-up".to_string()),
            },
            notification
        );
    }
}
