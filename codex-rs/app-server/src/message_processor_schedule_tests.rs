use super::ConnectionSessionState;
use super::MessageProcessor;
use super::MessageProcessorArgs;
use crate::analytics_utils::analytics_events_client_from_config;
use crate::config_manager::ConfigManager;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingEnvelope;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::OutgoingMessageSender;
use crate::transport::AppServerTransport;
use anyhow::Result;
use app_test_support::create_fake_rollout;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::rollout_path;
use app_test_support::write_mock_responses_config_toml;
use chrono::Utc;
use codex_analytics::AppServerRpcTransport;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::InitializeResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCRequest;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadScheduleCreateParams;
use codex_app_server_protocol::ThreadScheduleCreateResponse;
use codex_app_server_protocol::ThreadScheduleDeleteParams;
use codex_app_server_protocol::ThreadScheduleDeleteResponse;
use codex_app_server_protocol::ThreadScheduleGetParams;
use codex_app_server_protocol::ThreadScheduleGetResponse;
use codex_app_server_protocol::ThreadScheduleIntervalUnit;
use codex_app_server_protocol::ThreadScheduleListParams;
use codex_app_server_protocol::ThreadScheduleListResponse;
use codex_app_server_protocol::ThreadSchedulePauseParams;
use codex_app_server_protocol::ThreadSchedulePauseResponse;
use codex_app_server_protocol::ThreadSchedulePromptSource;
use codex_app_server_protocol::ThreadScheduleResumeParams;
use codex_app_server_protocol::ThreadScheduleResumeResponse;
use codex_app_server_protocol::ThreadScheduleRunNowParams;
use codex_app_server_protocol::ThreadScheduleRunNowResponse;
use codex_app_server_protocol::ThreadScheduleRunStatus;
use codex_app_server_protocol::ThreadScheduleRunUpdatedNotification;
use codex_app_server_protocol::ThreadScheduleSpec;
use codex_app_server_protocol::ThreadScheduleStatus;
use codex_app_server_protocol::ThreadScheduleUpdateParams;
use codex_app_server_protocol::ThreadScheduleUpdateResponse;
use codex_app_server_protocol::ThreadScheduleUpdatedNotification;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_arg0::Arg0DispatchPaths;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_config::TomlValue;
use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_exec_server::EnvironmentManager;
use codex_feedback::CodexFeedback;
use codex_login::AuthManager;
use codex_protocol::ThreadId;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TurnContextItem;
use codex_rollout::append_rollout_item_to_path;
use codex_rollout::state_db::StateDbHandle;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

const TEST_CONNECTION_ID: ConnectionId = ConnectionId(8);

fn request_from_client_request(request: ClientRequest) -> JSONRPCRequest {
    serde_json::from_value(serde_json::to_value(request).expect("serialize client request"))
        .expect("client request should convert to JSON-RPC")
}

struct ScheduleHarness {
    _server: MockServer,
    _codex_home: TempDir,
    workspace: TempDir,
    state_db: StateDbHandle,
    processor: Arc<MessageProcessor>,
    outgoing_rx: mpsc::Receiver<OutgoingEnvelope>,
    session: Arc<ConnectionSessionState>,
    next_request_id: i64,
}

impl ScheduleHarness {
    async fn new() -> Result<Self> {
        Self::new_with_cli_overrides(Vec::new()).await
    }

    async fn new_with_cli_overrides(cli_overrides: Vec<(String, TomlValue)>) -> Result<Self> {
        let server = create_mock_responses_server_repeating_assistant("Scheduled done").await;
        Self::new_with_mock_server(server, cli_overrides).await
    }

    async fn new_with_mock_server(
        server: MockServer,
        cli_overrides: Vec<(String, TomlValue)>,
    ) -> Result<Self> {
        let server_uri = server.uri();
        let codex_home = TempDir::new()?;
        let workspace = TempDir::new()?;
        let config =
            Arc::new(build_test_config(codex_home.path(), &server_uri, cli_overrides).await?);
        let state_db = codex_state::StateRuntime::init(
            config.sqlite_home.clone(),
            config.model_provider_id.clone(),
        )
        .await?;
        let (processor, outgoing_rx) = build_test_processor(config, Some(state_db.clone())).await;
        let mut harness = Self {
            _server: server,
            _codex_home: codex_home,
            workspace,
            state_db,
            processor,
            outgoing_rx,
            session: Arc::new(ConnectionSessionState::new()),
            next_request_id: 1,
        };

        let request_id = harness.request_id();
        let _: InitializeResponse = harness
            .request(ClientRequest::Initialize {
                request_id,
                params: InitializeParams {
                    client_info: ClientInfo {
                        name: "codex-app-server-schedule-tests".to_string(),
                        title: None,
                        version: "0.1.0".to_string(),
                    },
                    capabilities: Some(InitializeCapabilities {
                        experimental_api: true,
                        ..Default::default()
                    }),
                },
            })
            .await;
        assert!(harness.session.initialized());
        Ok(harness)
    }

    fn request_id(&mut self) -> RequestId {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
        RequestId::Integer(request_id)
    }

    fn workspace_cwd(&self) -> String {
        self.workspace.path().display().to_string()
    }

    async fn shutdown(self) {
        self.processor.shutdown_threads().await;
        self.processor.drain_background_tasks().await;
    }

    async fn response_request_bodies(&self) -> Vec<String> {
        self._server
            .received_requests()
            .await
            .expect("mock server should expose received requests")
            .into_iter()
            .map(|request| String::from_utf8_lossy(request.body.as_slice()).to_string())
            .collect()
    }

    async fn request<T>(&mut self, request: ClientRequest) -> T
    where
        T: serde::de::DeserializeOwned,
    {
        let request_id = match request.id() {
            RequestId::Integer(request_id) => *request_id,
            request_id => panic!("expected integer request id in test harness, got {request_id:?}"),
        };
        self.processor
            .process_request(
                TEST_CONNECTION_ID,
                request_from_client_request(request),
                &AppServerTransport::Stdio,
                Arc::clone(&self.session),
            )
            .await;
        self.read_response(request_id).await
    }

    async fn request_error(&mut self, request: ClientRequest) -> JSONRPCErrorError {
        let request_id = match request.id() {
            RequestId::Integer(request_id) => *request_id,
            request_id => panic!("expected integer request id in test harness, got {request_id:?}"),
        };
        self.processor
            .process_request(
                TEST_CONNECTION_ID,
                request_from_client_request(request),
                &AppServerTransport::Stdio,
                Arc::clone(&self.session),
            )
            .await;
        self.read_error(request_id).await
    }

    async fn start_thread(&mut self, ephemeral: bool) -> ThreadStartResponse {
        let request_id = self.request_id();
        let response: ThreadStartResponse = self
            .request(ClientRequest::ThreadStart {
                request_id,
                params: ThreadStartParams {
                    cwd: Some(self.workspace_cwd()),
                    ephemeral: Some(ephemeral),
                    ..ThreadStartParams::default()
                },
            })
            .await;
        self.read_thread_started_notification().await;
        response
    }

    async fn start_materialized_thread(&mut self) -> ThreadStartResponse {
        self.start_thread(/*ephemeral*/ false).await
    }

    async fn start_ephemeral_thread(&mut self) -> ThreadStartResponse {
        self.start_thread(/*ephemeral*/ true).await
    }

    async fn read_response<T>(&mut self, request_id: i64) -> T
    where
        T: serde::de::DeserializeOwned,
    {
        loop {
            let envelope =
                tokio::time::timeout(std::time::Duration::from_secs(5), self.outgoing_rx.recv())
                    .await
                    .expect("timed out waiting for response")
                    .expect("outgoing channel closed");
            let OutgoingEnvelope::ToConnection {
                connection_id,
                message,
                ..
            } = envelope
            else {
                continue;
            };
            if connection_id != TEST_CONNECTION_ID {
                continue;
            }
            match message {
                OutgoingMessage::Response(response)
                    if response.id == RequestId::Integer(request_id) =>
                {
                    return serde_json::from_value(response.result)
                        .expect("response payload should deserialize");
                }
                OutgoingMessage::Error(error) if error.id == RequestId::Integer(request_id) => {
                    panic!("request {request_id} failed: {:?}", error.error);
                }
                _ => {
                    continue;
                }
            }
        }
    }

    async fn read_error(&mut self, request_id: i64) -> JSONRPCErrorError {
        loop {
            let envelope =
                tokio::time::timeout(std::time::Duration::from_secs(5), self.outgoing_rx.recv())
                    .await
                    .expect("timed out waiting for error")
                    .expect("outgoing channel closed");
            let OutgoingEnvelope::ToConnection {
                connection_id,
                message,
                ..
            } = envelope
            else {
                continue;
            };
            if connection_id != TEST_CONNECTION_ID {
                continue;
            }
            match message {
                OutgoingMessage::Response(response)
                    if response.id == RequestId::Integer(request_id) =>
                {
                    panic!(
                        "request {request_id} unexpectedly succeeded: {:?}",
                        response.result
                    );
                }
                OutgoingMessage::Error(error) if error.id == RequestId::Integer(request_id) => {
                    return error.error;
                }
                _ => {
                    continue;
                }
            }
        }
    }

    async fn read_thread_started_notification(&mut self) {
        loop {
            let notification = self.read_server_notification().await;
            if matches!(notification, ServerNotification::ThreadStarted(_)) {
                return;
            }
        }
    }

    async fn read_schedule_updated(
        &mut self,
        thread_id: &str,
    ) -> ThreadScheduleUpdatedNotification {
        loop {
            let notification = self.read_server_notification().await;
            if let ServerNotification::ThreadScheduleUpdated(notification) = notification
                && notification.thread_id == thread_id
            {
                return notification;
            }
        }
    }

    async fn read_schedule_deleted(&mut self, thread_id: &str, schedule_id: &str) {
        loop {
            let notification = self.read_server_notification().await;
            if let ServerNotification::ThreadScheduleDeleted(notification) = notification
                && notification.thread_id == thread_id
                && notification.schedule_id == schedule_id
            {
                return;
            }
        }
    }

    async fn read_schedule_run_updated(
        &mut self,
        run_id: &str,
        status: ThreadScheduleRunStatus,
    ) -> ThreadScheduleRunUpdatedNotification {
        loop {
            let notification = self.read_server_notification().await;
            if let ServerNotification::ThreadScheduleRunUpdated(notification) = notification
                && notification.run.run_id == run_id
                && notification.run.status == status
            {
                return notification;
            }
        }
    }

    async fn read_completed_run_and_schedule_update(
        &mut self,
        thread_id: &str,
        run_id: &str,
    ) -> (
        ThreadScheduleUpdatedNotification,
        ThreadScheduleRunUpdatedNotification,
    ) {
        let mut schedule_update = None;
        let mut run_update = None;
        loop {
            let notification = self.read_server_notification().await;
            match notification {
                ServerNotification::ThreadScheduleUpdated(notification)
                    if notification.thread_id == thread_id =>
                {
                    schedule_update = Some(notification);
                }
                ServerNotification::ThreadScheduleRunUpdated(notification)
                    if notification.run.run_id == run_id
                        && notification.run.status == ThreadScheduleRunStatus::Completed =>
                {
                    run_update = Some(notification);
                }
                _ => {}
            }
            if let (Some(schedule_update), Some(run_update)) =
                (schedule_update.clone(), run_update.clone())
            {
                return (schedule_update, run_update);
            }
        }
    }

    async fn read_failed_run_and_schedule_update(
        &mut self,
        thread_id: &str,
        run_id: &str,
    ) -> (
        ThreadScheduleUpdatedNotification,
        ThreadScheduleRunUpdatedNotification,
    ) {
        let mut schedule_update = None;
        let mut run_update = None;
        loop {
            let notification = self.read_server_notification().await;
            match notification {
                ServerNotification::ThreadScheduleUpdated(notification)
                    if notification.thread_id == thread_id =>
                {
                    schedule_update = Some(notification);
                }
                ServerNotification::ThreadScheduleRunUpdated(notification)
                    if notification.run.run_id == run_id
                        && notification.run.status == ThreadScheduleRunStatus::Failed =>
                {
                    run_update = Some(notification);
                }
                _ => {}
            }
            if let (Some(schedule_update), Some(run_update)) =
                (schedule_update.clone(), run_update.clone())
            {
                return (schedule_update, run_update);
            }
        }
    }

    async fn read_server_notification(&mut self) -> ServerNotification {
        loop {
            let envelope = tokio::time::timeout(
                std::time::Duration::from_secs(/*secs*/ 20),
                self.outgoing_rx.recv(),
            )
            .await
            .expect("timed out waiting for server notification")
            .expect("outgoing channel closed");
            let message = match envelope {
                OutgoingEnvelope::ToConnection {
                    connection_id,
                    message,
                    ..
                } => {
                    if connection_id != TEST_CONNECTION_ID {
                        continue;
                    }
                    message
                }
                OutgoingEnvelope::Broadcast { message } => message,
            };
            if let OutgoingMessage::AppServerNotification(notification) = message {
                return notification;
            }
        }
    }
}

async fn create_mock_responses_server_unauthorized() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "message": "Incorrect API key provided: sk-test-schedule-secret",
                "type": "invalid_request_error",
                "param": null,
                "code": "invalid_api_key"
            }
        })))
        .mount(&server)
        .await;
    server
}

fn run_schedule_harness_test<F>(future: F) -> Result<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    std::thread::Builder::new()
        .name("schedule-harness".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("schedule harness runtime should build")
                .block_on(future)
        })?
        .join()
        .expect("schedule harness thread should not panic")
}

async fn build_test_config(
    codex_home: &Path,
    server_uri: &str,
    cli_overrides: Vec<(String, TomlValue)>,
) -> Result<Config> {
    write_mock_responses_config_toml(
        codex_home,
        server_uri,
        &BTreeMap::new(),
        /*auto_compact_limit*/ 8_192,
        Some(false),
        "mock_provider",
        "compact",
    )?;

    Ok(ConfigBuilder::default()
        .codex_home(codex_home.to_path_buf())
        .cli_overrides(cli_overrides)
        .build()
        .await?)
}

async fn build_test_processor(
    config: Arc<Config>,
    state_db: Option<StateDbHandle>,
) -> (Arc<MessageProcessor>, mpsc::Receiver<OutgoingEnvelope>) {
    let (outgoing_tx, outgoing_rx) = mpsc::channel(32);
    let auth_manager =
        AuthManager::shared_from_config(config.as_ref(), /*enable_codex_api_key_env*/ false).await;
    let config_manager = ConfigManager::new(
        config.codex_home.to_path_buf(),
        Vec::new(),
        LoaderOverrides::default(),
        /*strict_config*/ false,
        CloudConfigBundleLoader::default(),
        Arg0DispatchPaths::default(),
        Arc::new(codex_config::NoopThreadConfigLoader),
    );
    let analytics_events_client =
        analytics_events_client_from_config(Arc::clone(&auth_manager), config.as_ref());
    let outgoing = Arc::new(OutgoingMessageSender::new(
        outgoing_tx,
        analytics_events_client.clone(),
    ));
    let processor = Arc::new(MessageProcessor::new(MessageProcessorArgs {
        outgoing,
        analytics_events_client,
        arg0_paths: Arg0DispatchPaths::default(),
        config,
        config_manager,
        environment_manager: Arc::new(EnvironmentManager::default_for_tests()),
        feedback: CodexFeedback::new(),
        log_db: None,
        state_db,
        config_warnings: Vec::new(),
        session_source: SessionSource::VSCode,
        auth_manager,
        installation_id: "22222222-2222-4222-8222-222222222222".to_string(),
        rpc_transport: AppServerRpcTransport::Stdio,
        remote_control_handle: None,
        plugin_startup_tasks: crate::PluginStartupTasks::Start,
        background_agent_host: false,
        background_agent_worker_run_id: None,
    }));
    (processor, outgoing_rx)
}

#[test]
fn thread_schedule_requests_reject_when_feature_disabled() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new_with_cli_overrides(vec![(
            "features.scheduled_tasks".to_string(),
            TomlValue::Boolean(false),
        )])
        .await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let error = harness
            .request_error(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id,
                    prompt: "should not be scheduled".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_900_000_300),
                    expires_at: Some(1_900_604_800),
                },
            })
            .await;
        assert_eq!("scheduled_tasks feature is disabled", error.message);

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_requests_reject_ephemeral_threads() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_ephemeral_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let error = harness
            .request_error(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "should only run on materialized threads".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_900_000_300),
                    expires_at: Some(1_900_604_800),
                },
            })
            .await;
        assert_eq!(
            format!("ephemeral thread does not support scheduled tasks: {thread_id}"),
            error.message
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_create_rejects_once_without_next_run_at() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;

        let request_id = harness.request_id();
        let error = harness
            .request_error(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread.thread.id.clone(),
                    prompt: "ask me something".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Once,
                    timezone: Some("UTC".to_string()),
                    next_run_at: None,
                    expires_at: None,
                },
            })
            .await;
        assert_eq!(
            "nextRunAt is required for one-time schedules",
            error.message
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn unloaded_thread_schedule_create_records_latest_root_turn_auth() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let filename_ts = "2025-01-05T12-05-00";
        let thread_id = create_fake_rollout(
            harness._codex_home.path(),
            filename_ts,
            "2025-01-05T12:05:00Z",
            "root profile turn thread",
            Some("mock_provider"),
            /*git_info*/ None,
        )?;
        let parsed_thread_id = ThreadId::from_string(thread_id.as_str())?;
        let rollout_file_path = rollout_path(harness._codex_home.path(), filename_ts, &thread_id);
        append_rollout_item_to_path(
            &rollout_file_path,
            &RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: parsed_thread_id,
                    timestamp: "2025-01-05T12:05:01Z".to_string(),
                    cwd: harness.workspace.path().to_path_buf(),
                    originator: "codex".to_string(),
                    cli_version: "0.0.0".to_string(),
                    source: SessionSource::Cli,
                    model_provider: Some("mock_provider".to_string()),
                    auth_profile: Some(Some("account002".to_string())),
                    ..SessionMeta::default()
                },
                git: None,
            }),
        )
        .await?;
        append_rollout_item_to_path(
            &rollout_file_path,
            &RolloutItem::TurnContext(TurnContextItem {
                thread_id: Some(parsed_thread_id),
                turn_id: Some("turn-root".to_string()),
                cwd: harness.workspace.path().to_path_buf(),
                workspace_roots: None,
                current_date: None,
                timezone: None,
                approval_policy: AskForApproval::Never,
                sandbox_policy: SandboxPolicy::DangerFullAccess,
                permission_profile: None,
                network: None,
                file_system_sandbox_policy: None,
                model: "gpt-5.5".to_string(),
                model_provider_id: Some("mock_provider".to_string()),
                personality: None,
                collaboration_mode: None,
                multi_agent_version: None,
                auth_profile: Some(None),
                realtime_active: None,
                effort: None,
                summary: codex_protocol::config_types::ReasoningSummary::Auto,
            }),
        )
        .await?;

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "check the deploy".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_900_000_300),
                    expires_at: Some(1_900_604_800),
                },
            })
            .await;

        let stored = harness
            .state_db
            .thread_schedules()
            .get_thread_schedule(create_response.schedule.schedule_id.as_str())
            .await?
            .expect("created schedule should be persisted");
        assert_eq!(parsed_thread_id, stored.thread_id);
        assert_eq!(Some(None), stored.auth_profile);

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn unloaded_thread_schedule_create_records_latest_root_session_auth() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let filename_ts = "2025-01-05T12-10-00";
        let thread_id = create_fake_rollout(
            harness._codex_home.path(),
            filename_ts,
            "2025-01-05T12:10:00Z",
            "root profile switch thread",
            Some("mock_provider"),
            /*git_info*/ None,
        )?;
        let parsed_thread_id = ThreadId::from_string(thread_id.as_str())?;
        let rollout_file_path = rollout_path(harness._codex_home.path(), filename_ts, &thread_id);
        append_rollout_item_to_path(
            &rollout_file_path,
            &RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: parsed_thread_id,
                    timestamp: "2025-01-05T12:10:01Z".to_string(),
                    cwd: harness.workspace.path().to_path_buf(),
                    originator: "codex".to_string(),
                    cli_version: "0.0.0".to_string(),
                    source: SessionSource::Cli,
                    model_provider: Some("mock_provider".to_string()),
                    auth_profile: Some(Some("account002".to_string())),
                    ..SessionMeta::default()
                },
                git: None,
            }),
        )
        .await?;
        append_rollout_item_to_path(
            &rollout_file_path,
            &RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: parsed_thread_id,
                    timestamp: "2025-01-05T12:10:02Z".to_string(),
                    cwd: harness.workspace.path().to_path_buf(),
                    originator: "codex".to_string(),
                    cli_version: "0.0.0".to_string(),
                    source: SessionSource::Cli,
                    model_provider: Some("mock_provider".to_string()),
                    auth_profile: Some(None),
                    ..SessionMeta::default()
                },
                git: None,
            }),
        )
        .await?;

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "check the deploy".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_900_000_300),
                    expires_at: Some(1_900_604_800),
                },
            })
            .await;

        let stored = harness
            .state_db
            .thread_schedules()
            .get_thread_schedule(create_response.schedule.schedule_id.as_str())
            .await?
            .expect("created schedule should be persisted");
        assert_eq!(parsed_thread_id, stored.thread_id);
        assert_eq!(Some(None), stored.auth_profile);

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_update_rejects_active_once_without_next_run_at() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "ask me something".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Once,
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_900_000_300),
                    expires_at: None,
                },
            })
            .await;
        harness.read_schedule_updated(&thread_id).await;

        let request_id = harness.request_id();
        let error = harness
            .request_error(ClientRequest::ThreadScheduleUpdate {
                request_id,
                params: ThreadScheduleUpdateParams {
                    thread_id,
                    schedule_id: create_response.schedule.schedule_id,
                    prompt: None,
                    schedule: None,
                    timezone: None,
                    status: None,
                    next_run_at: Some(None),
                    expires_at: None,
                },
            })
            .await;
        assert_eq!(
            "nextRunAt is required for one-time schedules",
            error.message
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_update_recomputes_active_recurring_without_next_run_at() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "check recurring work".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: None,
                    expires_at: None,
                },
            })
            .await;
        harness.read_schedule_updated(&thread_id).await;

        let request_id = harness.request_id();
        let update_response: ThreadScheduleUpdateResponse = harness
            .request(ClientRequest::ThreadScheduleUpdate {
                request_id,
                params: ThreadScheduleUpdateParams {
                    thread_id: thread_id.clone(),
                    schedule_id: create_response.schedule.schedule_id,
                    prompt: None,
                    schedule: None,
                    timezone: None,
                    status: None,
                    next_run_at: Some(None),
                    expires_at: None,
                },
            })
            .await;
        assert_eq!(
            ThreadScheduleStatus::Active,
            update_response.schedule.status
        );
        assert!(update_response.schedule.next_run_at.is_some());
        assert_eq!(
            update_response.schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_resume_recomputes_recurring_without_next_run_at() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "check recurring work".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: None,
                    expires_at: None,
                },
            })
            .await;
        harness.read_schedule_updated(&thread_id).await;

        let claim = harness
            .state_db
            .thread_schedules()
            .claim_thread_schedule_now(
                create_response.schedule.schedule_id.as_str(),
                Utc::now(),
                "lease-fail",
                std::time::Duration::from_secs(300),
            )
            .await?
            .expect("schedule should claim for seeded failure");
        harness
            .state_db
            .thread_schedules()
            .fail_thread_schedule_run(
                create_response.schedule.schedule_id.as_str(),
                claim.run.run_id.as_str(),
                "lease-fail",
                Utc::now(),
                None,
                "model unavailable".to_string(),
            )
            .await?;
        let failed_schedule = harness
            .state_db
            .thread_schedules()
            .get_thread_schedule(create_response.schedule.schedule_id.as_str())
            .await?
            .expect("schedule should still exist after failure");
        assert_eq!(
            codex_state::ThreadScheduleStatus::Expired,
            failed_schedule.status
        );
        assert_eq!(None, failed_schedule.next_run_at);
        assert_eq!(1, failed_schedule.failure_count);

        let request_id = harness.request_id();
        let resume_response: ThreadScheduleResumeResponse = harness
            .request(ClientRequest::ThreadScheduleResume {
                request_id,
                params: ThreadScheduleResumeParams {
                    thread_id: thread_id.clone(),
                    schedule_id: create_response.schedule.schedule_id,
                },
            })
            .await;
        assert_eq!(
            ThreadScheduleStatus::Active,
            resume_response.schedule.status
        );
        assert!(resume_response.schedule.next_run_at.is_some());
        assert_eq!(0, resume_response.schedule.failure_count);
        assert_eq!(
            resume_response.schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_run_now_rejects_ambiguous_schedule_id_prefix() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();
        let mut schedules_by_prefix: BTreeMap<char, Vec<String>> = BTreeMap::new();

        for index in 0..17 {
            let request_id = harness.request_id();
            let response: ThreadScheduleCreateResponse = harness
                .request(ClientRequest::ThreadScheduleCreate {
                    request_id,
                    params: ThreadScheduleCreateParams {
                        thread_id: thread_id.clone(),
                        prompt: format!("scheduled task {index}"),
                        prompt_source: Some(ThreadSchedulePromptSource::Inline),
                        schedule: ThreadScheduleSpec::Interval {
                            amount: 1,
                            unit: ThreadScheduleIntervalUnit::Hours,
                        },
                        timezone: Some("UTC".to_string()),
                        next_run_at: Some(1_900_000_000 + index),
                        expires_at: Some(1_900_604_800),
                    },
                })
                .await;
            assert_eq!(
                response.schedule,
                harness.read_schedule_updated(&thread_id).await.schedule
            );
            let prefix = response
                .schedule
                .schedule_id
                .chars()
                .next()
                .expect("schedule id should not be empty");
            schedules_by_prefix
                .entry(prefix)
                .or_default()
                .push(response.schedule.schedule_id);
        }

        let ambiguous_prefix = schedules_by_prefix
            .into_iter()
            .find_map(|(prefix, schedule_ids)| (schedule_ids.len() > 1).then_some(prefix))
            .expect("17 UUID-like schedule ids should share at least one hex prefix")
            .to_string();
        let request_id = harness.request_id();
        let error = harness
            .request_error(ClientRequest::ThreadScheduleRunNow {
                request_id,
                params: ThreadScheduleRunNowParams {
                    thread_id,
                    schedule_id: ambiguous_prefix.clone(),
                },
            })
            .await;
        assert_eq!(
            format!("schedule id prefix is ambiguous: {ambiguous_prefix}"),
            error.message
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_crud_requests_round_trip_through_app_server() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "check the deploy".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 5,
                        unit: ThreadScheduleIntervalUnit::Minutes,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_900_000_300),
                    expires_at: Some(1_900_604_800),
                },
            })
            .await;
        let created = create_response.schedule;
        assert_eq!(thread_id, created.thread_id);
        assert_eq!("check the deploy", created.prompt);
        assert_eq!(ThreadSchedulePromptSource::Inline, created.prompt_source);
        assert_eq!(ThreadScheduleStatus::Active, created.status);
        assert_eq!(Some(1_900_000_300), created.next_run_at);
        assert_eq!(
            created,
            harness.read_schedule_updated(&thread_id).await.schedule
        );
        let schedule_id_prefix = created.schedule_id[..8].to_string();

        let request_id = harness.request_id();
        let list_response: ThreadScheduleListResponse = harness
            .request(ClientRequest::ThreadScheduleList {
                request_id,
                params: ThreadScheduleListParams {
                    thread_id: thread_id.clone(),
                    cursor: None,
                    limit: Some(10),
                },
            })
            .await;
        assert_eq!(vec![created.clone()], list_response.data);
        assert_eq!(None, list_response.next_cursor);

        let request_id = harness.request_id();
        let get_response: ThreadScheduleGetResponse = harness
            .request(ClientRequest::ThreadScheduleGet {
                request_id,
                params: ThreadScheduleGetParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule_id_prefix.clone(),
                },
            })
            .await;
        assert_eq!(Some(created.clone()), get_response.schedule);

        let request_id = harness.request_id();
        let update_response: ThreadScheduleUpdateResponse = harness
            .request(ClientRequest::ThreadScheduleUpdate {
                request_id,
                params: ThreadScheduleUpdateParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule_id_prefix.clone(),
                    prompt: Some("write the daily release handoff".to_string()),
                    schedule: Some(ThreadScheduleSpec::Cron {
                        expression: "0 9 * * 1-5".to_string(),
                    }),
                    timezone: Some("Europe/Bucharest".to_string()),
                    status: None,
                    next_run_at: Some(Some(1_900_010_000)),
                    expires_at: Some(Some(1_900_604_800)),
                },
            })
            .await;
        assert_eq!(
            "write the daily release handoff",
            update_response.schedule.prompt
        );
        assert_eq!("Europe/Bucharest", update_response.schedule.timezone);
        assert_eq!(
            ThreadScheduleSpec::Cron {
                expression: "0 9 * * 1-5".to_string()
            },
            update_response.schedule.schedule
        );
        assert_eq!(
            update_response.schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        let request_id = harness.request_id();
        let pause_response: ThreadSchedulePauseResponse = harness
            .request(ClientRequest::ThreadSchedulePause {
                request_id,
                params: ThreadSchedulePauseParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule_id_prefix.clone(),
                },
            })
            .await;
        assert_eq!(ThreadScheduleStatus::Paused, pause_response.schedule.status);
        assert_eq!(
            pause_response.schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        let request_id = harness.request_id();
        let resume_response: ThreadScheduleResumeResponse = harness
            .request(ClientRequest::ThreadScheduleResume {
                request_id,
                params: ThreadScheduleResumeParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule_id_prefix.clone(),
                },
            })
            .await;
        assert_eq!(
            ThreadScheduleStatus::Active,
            resume_response.schedule.status
        );
        assert_eq!(
            resume_response.schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        let request_id = harness.request_id();
        let delete_response: ThreadScheduleDeleteResponse = harness
            .request(ClientRequest::ThreadScheduleDelete {
                request_id,
                params: ThreadScheduleDeleteParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule_id_prefix,
                },
            })
            .await;
        assert!(delete_response.deleted);
        harness
            .read_schedule_deleted(&thread_id, created.schedule_id.as_str())
            .await;

        let request_id = harness.request_id();
        let after_delete: ThreadScheduleListResponse = harness
            .request(ClientRequest::ThreadScheduleList {
                request_id,
                params: ThreadScheduleListParams {
                    thread_id,
                    cursor: None,
                    limit: Some(10),
                },
            })
            .await;
        assert!(after_delete.data.is_empty());

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_default_prompt_reloads_from_project_file_on_execution() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();
        let project_codex = harness.workspace.path().join(".codewith");
        tokio::fs::create_dir_all(&project_codex).await?;
        tokio::fs::write(project_codex.join("loop.md"), "Project default prompt v1").await?;

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: String::new(),
                    prompt_source: Some(ThreadSchedulePromptSource::Default),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 1,
                        unit: ThreadScheduleIntervalUnit::Hours,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_800_000_000),
                    expires_at: Some(1_800_086_400),
                },
            })
            .await;
        let schedule = create_response.schedule;
        assert_eq!("Default loop prompt", schedule.prompt);
        assert_eq!(ThreadSchedulePromptSource::Default, schedule.prompt_source);
        assert_eq!(
            schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        tokio::fs::write(project_codex.join("loop.md"), "Project default prompt v2").await?;
        let request_id = harness.request_id();
        let run_now: ThreadScheduleRunNowResponse = harness
            .request(ClientRequest::ThreadScheduleRunNow {
                request_id,
                params: ThreadScheduleRunNowParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule.schedule_id.clone(),
                },
            })
            .await;
        harness
            .read_schedule_run_updated(&run_now.run.run_id, ThreadScheduleRunStatus::Running)
            .await;
        harness
            .read_completed_run_and_schedule_update(&thread_id, &run_now.run.run_id)
            .await;
        let response_request_bodies = harness.response_request_bodies().await;
        assert!(
            response_request_bodies
                .iter()
                .any(|body| body.contains("Project default prompt v2")),
            "expected refreshed default prompt in Responses request bodies: {response_request_bodies:#?}"
        );
        assert!(
            response_request_bodies.iter().any(|body| body
                .contains("You are running one new scheduled Codewith prompt")
                && body
                    .contains("Produce exactly one visible final response for this scheduled run")
                && body.contains("Do not wait, sleep, or start a timer")),
            "scheduled prompt should tell the model this is one scheduled run: {response_request_bodies:#?}"
        );
        assert!(
            !response_request_bodies
                .iter()
                .any(|body| body.contains("Default loop prompt")),
            "display placeholder should not be sent to the model: {response_request_bodies:#?}"
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_run_now_executes_and_completes_the_scheduled_turn() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "summarize the latest test status".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 1,
                        unit: ThreadScheduleIntervalUnit::Hours,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_800_000_000),
                    expires_at: Some(1_800_086_400),
                },
            })
            .await;
        let schedule = create_response.schedule;
        assert_eq!(
            schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        let request_id = harness.request_id();
        let run_now: ThreadScheduleRunNowResponse = harness
            .request(ClientRequest::ThreadScheduleRunNow {
                request_id,
                params: ThreadScheduleRunNowParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule.schedule_id.clone(),
                },
            })
            .await;
        assert_eq!(ThreadScheduleRunStatus::Leased, run_now.run.status);
        assert_eq!(schedule.schedule_id, run_now.run.schedule_id);

        let running = harness
            .read_schedule_run_updated(&run_now.run.run_id, ThreadScheduleRunStatus::Running)
            .await;
        assert_eq!(thread_id, running.thread_id);
        assert_eq!(run_now.run.scheduled_for_at, running.run.scheduled_for_at);
        assert!(running.run.turn_id.is_some());

        let (updated_schedule, completed) = harness
            .read_completed_run_and_schedule_update(&thread_id, &run_now.run.run_id)
            .await;
        assert_eq!(None, completed.run.error);
        assert!(completed.run.completed_at.is_some());
        let next_run_at = updated_schedule
            .schedule
            .next_run_at
            .expect("completed scheduled turn should compute a next run");
        let completed_at = completed
            .run
            .completed_at
            .expect("completed scheduled run should have a completion timestamp");
        assert!(next_run_at > completed_at);
        assert_eq!(None, updated_schedule.schedule.lease_expires_at);
        assert_eq!(0, updated_schedule.schedule.failure_count);
        let response_request_bodies = harness.response_request_bodies().await;
        assert!(
            response_request_bodies.iter().any(|body| body
                .contains("You are running one new scheduled Codewith prompt")
                && body.contains("Run id:")
                && body.contains(run_now.run.run_id.as_str())
                && body.contains("This is a distinct run")
                && body
                    .contains("Produce exactly one visible final response for this scheduled run")
                && body.contains("summarize the latest test status")),
            "scheduled prompt should be wrapped as a fresh visible scheduled run: {response_request_bodies:#?}"
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_run_now_records_model_errors_as_failed_runs() -> Result<()> {
    run_schedule_harness_test(async {
        let server = create_mock_responses_server_unauthorized().await;
        let mut harness = ScheduleHarness::new_with_mock_server(server, Vec::new()).await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "check whether the dev server is healthy".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 1,
                        unit: ThreadScheduleIntervalUnit::Hours,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_800_000_000),
                    expires_at: Some(1_800_086_400),
                },
            })
            .await;
        let schedule = create_response.schedule;
        assert_eq!(
            schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        let request_id = harness.request_id();
        let run_now: ThreadScheduleRunNowResponse = harness
            .request(ClientRequest::ThreadScheduleRunNow {
                request_id,
                params: ThreadScheduleRunNowParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule.schedule_id.clone(),
                },
            })
            .await;
        harness
            .read_schedule_run_updated(&run_now.run.run_id, ThreadScheduleRunStatus::Running)
            .await;

        let (updated_schedule, failed) = harness
            .read_failed_run_and_schedule_update(&thread_id, &run_now.run.run_id)
            .await;
        let error = failed
            .run
            .error
            .as_deref()
            .expect("failed scheduled run should record an error");
        assert!(error.contains("scheduled turn failed"));
        assert!(error.contains("Incorrect API key provided"));
        assert!(
            !error.contains("sk-test-schedule-secret"),
            "schedule run error should redact API keys: {error}"
        );
        assert!(failed.run.completed_at.is_some());
        assert_eq!(1, updated_schedule.schedule.failure_count);

        let request_id = harness.request_id();
        let get_response: ThreadScheduleGetResponse = harness
            .request(ClientRequest::ThreadScheduleGet {
                request_id,
                params: ThreadScheduleGetParams {
                    thread_id,
                    schedule_id: schedule.schedule_id,
                },
            })
            .await;
        assert_eq!(1, get_response.stats.total_runs);
        assert_eq!(0, get_response.stats.completed_runs);
        assert_eq!(1, get_response.stats.failed_runs);
        assert_eq!(Some(error.to_string()), get_response.stats.last_error);

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_once_clears_next_run_after_completion() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "ask one funny question".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Once,
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_800_000_000),
                    expires_at: None,
                },
            })
            .await;
        let schedule = create_response.schedule;
        assert_eq!(ThreadScheduleSpec::Once, schedule.schedule);
        assert_eq!(Some(1_800_000_000), schedule.next_run_at);
        assert_eq!(
            schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );

        let request_id = harness.request_id();
        let run_now: ThreadScheduleRunNowResponse = harness
            .request(ClientRequest::ThreadScheduleRunNow {
                request_id,
                params: ThreadScheduleRunNowParams {
                    thread_id: thread_id.clone(),
                    schedule_id: schedule.schedule_id.clone(),
                },
            })
            .await;
        harness
            .read_schedule_run_updated(&run_now.run.run_id, ThreadScheduleRunStatus::Running)
            .await;

        let (updated_schedule, completed) = harness
            .read_completed_run_and_schedule_update(&thread_id, &run_now.run.run_id)
            .await;
        assert_eq!(None, completed.run.error);
        assert_eq!(None, updated_schedule.schedule.next_run_at);
        assert_eq!(
            ThreadScheduleStatus::Expired,
            updated_schedule.schedule.status
        );
        assert_eq!(ThreadScheduleSpec::Once, updated_schedule.schedule.schedule);

        let request_id = harness.request_id();
        let error = harness
            .request_error(ClientRequest::ThreadScheduleResume {
                request_id,
                params: ThreadScheduleResumeParams {
                    thread_id,
                    schedule_id: schedule.schedule_id,
                },
            })
            .await;
        assert_eq!(
            "nextRunAt is required for one-time schedules",
            error.message
        );

        harness.shutdown().await;
        Ok(())
    })
}

#[test]
fn thread_schedule_run_now_accepts_unique_schedule_id_prefix() -> Result<()> {
    run_schedule_harness_test(async {
        let mut harness = ScheduleHarness::new().await?;
        let thread = harness.start_materialized_thread().await;
        let thread_id = thread.thread.id.clone();

        let request_id = harness.request_id();
        let create_response: ThreadScheduleCreateResponse = harness
            .request(ClientRequest::ThreadScheduleCreate {
                request_id,
                params: ThreadScheduleCreateParams {
                    thread_id: thread_id.clone(),
                    prompt: "ask one funny question".to_string(),
                    prompt_source: Some(ThreadSchedulePromptSource::Inline),
                    schedule: ThreadScheduleSpec::Interval {
                        amount: 1,
                        unit: ThreadScheduleIntervalUnit::Hours,
                    },
                    timezone: Some("UTC".to_string()),
                    next_run_at: Some(1_800_000_000),
                    expires_at: Some(1_800_086_400),
                },
            })
            .await;
        let schedule = create_response.schedule;
        assert_eq!(
            schedule,
            harness.read_schedule_updated(&thread_id).await.schedule
        );
        let short_schedule_id = schedule.schedule_id[..8].to_string();

        let request_id = harness.request_id();
        let run_now: ThreadScheduleRunNowResponse = harness
            .request(ClientRequest::ThreadScheduleRunNow {
                request_id,
                params: ThreadScheduleRunNowParams {
                    thread_id: thread_id.clone(),
                    schedule_id: short_schedule_id,
                },
            })
            .await;
        assert_eq!(schedule.schedule_id, run_now.run.schedule_id);

        harness
            .read_schedule_run_updated(&run_now.run.run_id, ThreadScheduleRunStatus::Running)
            .await;
        harness
            .read_completed_run_and_schedule_update(&thread_id, &run_now.run.run_id)
            .await;

        harness.shutdown().await;
        Ok(())
    })
}
