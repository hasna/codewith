use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use chrono::DateTime;
use chrono::Utc;
use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
use codex_core::ThreadManager;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_protocol::ThreadId;
use codex_rollout::state_db::StateDbHandle;
use codex_tools::JsonSchema;
use codex_tools::ToolExposure;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

const MISSION_CONTROL_OVERVIEW_TOOL_NAME: &str = "mission_control_overview";
const MISSION_CONTROL_ENQUEUE_INSTRUCTION_TOOL_NAME: &str = "mission_control_enqueue_instruction";
const MISSION_CONTROL_MAILBOX_RECEIPTS_TOOL_NAME: &str = "mission_control_mailbox_receipts";
const MISSION_CONTROL_RESPOND_INTERACTION_TOOL_NAME: &str = "mission_control_respond_interaction";
const MISSION_CONTROL_PREVIEW_CHARS: usize = 240;
const DEFAULT_MISSION_CONTROL_SESSION_LIMIT: usize = 25;
const MAX_MISSION_CONTROL_SESSION_LIMIT: usize = 100;
const DEFAULT_MISSION_CONTROL_MAX_ATTEMPTS: i64 = 10;
const MAX_MISSION_CONTROL_SENDER_LABEL_CHARS: usize = 120;

#[derive(Clone)]
struct MissionControlExtension<C> {
    state_db: StateDbHandle,
    thread_manager: Weak<ThreadManager>,
    mission_control_enabled: Arc<dyn Fn(&C) -> bool + Send + Sync>,
    scheduled_tasks_enabled: Arc<dyn Fn(&C) -> bool + Send + Sync>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MissionControlExtensionConfig {
    enabled: bool,
    scheduled_tasks_enabled: bool,
}

struct MissionControlExtensionState {
    enabled: Arc<AtomicBool>,
    scheduled_tasks_enabled: Arc<AtomicBool>,
    current_thread_id: ThreadId,
    tools_available_for_thread: bool,
}

impl MissionControlExtensionState {
    fn new(
        config: MissionControlExtensionConfig,
        current_thread_id: ThreadId,
        tools_available_for_thread: bool,
    ) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(config.enabled)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(config.scheduled_tasks_enabled)),
            current_thread_id,
            tools_available_for_thread,
        }
    }

    fn set_config(&self, config: MissionControlExtensionConfig) {
        self.enabled.store(config.enabled, Ordering::Relaxed);
        self.scheduled_tasks_enabled
            .store(config.scheduled_tasks_enabled, Ordering::Relaxed);
    }

    fn tools_enabled(&self) -> bool {
        self.tools_available_for_thread && self.enabled.load(Ordering::Relaxed)
    }
}

impl<C> MissionControlExtension<C> {
    fn new(
        state_db: StateDbHandle,
        thread_manager: Weak<ThreadManager>,
        mission_control_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
        scheduled_tasks_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            state_db,
            thread_manager,
            mission_control_enabled: Arc::new(mission_control_enabled),
            scheduled_tasks_enabled: Arc::new(scheduled_tasks_enabled),
        }
    }

    fn config(&self, config: &C) -> MissionControlExtensionConfig {
        MissionControlExtensionConfig {
            enabled: (self.mission_control_enabled)(config),
            scheduled_tasks_enabled: (self.scheduled_tasks_enabled)(config),
        }
    }
}

#[async_trait::async_trait]
impl<C> ThreadLifecycleContributor<C> for MissionControlExtension<C>
where
    C: Send + Sync + 'static,
{
    async fn on_thread_start(&self, input: ThreadStartInput<'_, C>) {
        let config = self.config(input.config);
        input.thread_store.insert(config);
        let Ok(current_thread_id) = ThreadId::from_string(input.thread_store.level_id()) else {
            return;
        };
        input
            .thread_store
            .get_or_init(|| {
                MissionControlExtensionState::new(
                    config,
                    current_thread_id,
                    input.persistent_thread_state_available,
                )
            })
            .set_config(config);
    }
}

impl<C> ConfigContributor<C> for MissionControlExtension<C>
where
    C: Send + Sync + 'static,
{
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &C,
        new_config: &C,
    ) {
        let config = self.config(new_config);
        thread_store.insert(config);
        if let Some(state) = thread_store.get::<MissionControlExtensionState>() {
            state.set_config(config);
        }
    }
}

impl<C> ToolContributor for MissionControlExtension<C>
where
    C: Send + Sync + 'static,
{
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(state) = thread_store.get::<MissionControlExtensionState>() else {
            return Vec::new();
        };
        if !state.tools_enabled() {
            return Vec::new();
        }

        let runtime = MissionControlRuntime {
            state_db: self.state_db.clone(),
            thread_manager: self.thread_manager.clone(),
            enabled: Arc::clone(&state.enabled),
            scheduled_tasks_enabled: Arc::clone(&state.scheduled_tasks_enabled),
            current_thread_id: state.current_thread_id,
        };
        vec![
            Arc::new(MissionControlTool::new(
                MissionControlToolKind::Overview,
                runtime.clone(),
            )),
            Arc::new(MissionControlTool::new(
                MissionControlToolKind::EnqueueInstruction,
                runtime.clone(),
            )),
            Arc::new(MissionControlTool::new(
                MissionControlToolKind::MailboxReceipts,
                runtime.clone(),
            )),
            Arc::new(MissionControlTool::new(
                MissionControlToolKind::RespondInteraction,
                runtime,
            )),
        ]
    }
}

#[derive(Clone)]
struct MissionControlRuntime {
    state_db: StateDbHandle,
    thread_manager: Weak<ThreadManager>,
    enabled: Arc<AtomicBool>,
    scheduled_tasks_enabled: Arc<AtomicBool>,
    current_thread_id: ThreadId,
}

struct MissionControlTool {
    kind: MissionControlToolKind,
    runtime: MissionControlRuntime,
}

impl MissionControlTool {
    fn new(kind: MissionControlToolKind, runtime: MissionControlRuntime) -> Self {
        Self { kind, runtime }
    }

    fn ensure_enabled(&self) -> Result<(), FunctionCallError> {
        if self.runtime.enabled.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(FunctionCallError::RespondToModel(
                "mission-control tools are unavailable because the goals feature is disabled"
                    .to_string(),
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MissionControlToolKind {
    Overview,
    EnqueueInstruction,
    MailboxReceipts,
    RespondInteraction,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolCall> for MissionControlTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.kind.name())
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: self.kind.name().to_string(),
            description: self.kind.description().to_string(),
            strict: self.kind.strict(),
            defer_loading: None,
            parameters: self.kind.parameters(),
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    async fn handle(&self, invocation: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        self.ensure_enabled()?;
        match self.kind {
            MissionControlToolKind::Overview => {
                let args = parse_arguments::<OverviewArgs>(
                    MISSION_CONTROL_OVERVIEW_TOOL_NAME,
                    &invocation,
                )?;
                self.runtime.handle_overview(args).await
            }
            MissionControlToolKind::EnqueueInstruction => {
                let args = parse_arguments::<EnqueueInstructionArgs>(
                    MISSION_CONTROL_ENQUEUE_INSTRUCTION_TOOL_NAME,
                    &invocation,
                )?;
                self.runtime.handle_enqueue_instruction(args).await
            }
            MissionControlToolKind::MailboxReceipts => {
                let args = parse_arguments::<MailboxReceiptsArgs>(
                    MISSION_CONTROL_MAILBOX_RECEIPTS_TOOL_NAME,
                    &invocation,
                )?;
                self.runtime.handle_mailbox_receipts(args).await
            }
            MissionControlToolKind::RespondInteraction => {
                let args = parse_arguments::<RespondInteractionArgs>(
                    MISSION_CONTROL_RESPOND_INTERACTION_TOOL_NAME,
                    &invocation,
                )?;
                self.runtime.handle_respond_interaction(args).await
            }
        }
    }
}

impl MissionControlToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::Overview => MISSION_CONTROL_OVERVIEW_TOOL_NAME,
            Self::EnqueueInstruction => MISSION_CONTROL_ENQUEUE_INSTRUCTION_TOOL_NAME,
            Self::MailboxReceipts => MISSION_CONTROL_MAILBOX_RECEIPTS_TOOL_NAME,
            Self::RespondInteraction => MISSION_CONTROL_RESPOND_INTERACTION_TOOL_NAME,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Overview => {
                "Inspect local mission-control state: persisted local sessions, live thread ids, pending user interactions, and coordination capabilities. This tool only reads state."
            }
            Self::EnqueueInstruction => {
                "Queue a durable user instruction for another existing thread. This tool writes only to the thread mailbox; it does not execute shell commands, mutate files, create workflows, or spawn agents."
            }
            Self::MailboxReceipts => {
                "Read delivery receipts for a mission-control mailbox message. This tool only reads state."
            }
            Self::RespondInteraction => {
                "Record a durable response for an existing pending interaction. This tool updates pending-interaction state only and does not execute commands, mutate files, or change workflows."
            }
        }
    }

    fn strict(self) -> bool {
        !matches!(self, Self::RespondInteraction)
    }

    fn parameters(self) -> JsonSchema {
        match self {
            Self::Overview => strict_object(BTreeMap::from([
                (
                    "pending_cursor".to_string(),
                    nullable_string("Cursor for pending interaction pagination."),
                ),
                (
                    "pending_limit".to_string(),
                    nullable_integer("Maximum pending interactions to return."),
                ),
                (
                    "local_limit".to_string(),
                    nullable_integer("Maximum local sessions to return."),
                ),
                (
                    "local_cursor".to_string(),
                    nullable_string("Cursor for local session pagination."),
                ),
                (
                    "include_live_sessions".to_string(),
                    nullable_boolean("Whether to include live thread ids from the local process."),
                ),
                (
                    "include_payloads".to_string(),
                    nullable_boolean(
                        "Whether to include full pending-interaction payloads and schedule prompts instead of compact previews.",
                    ),
                ),
            ])),
            Self::EnqueueInstruction => strict_object(BTreeMap::from([
                (
                    "target_thread_id".to_string(),
                    JsonSchema::string(Some("Existing durable target thread id.".to_string())),
                ),
                (
                    "message".to_string(),
                    JsonSchema::string(Some("Instruction text to enqueue.".to_string())),
                ),
                (
                    "sender_thread_id".to_string(),
                    nullable_string("Optional sender thread id; defaults to the calling thread."),
                ),
                (
                    "sender_label".to_string(),
                    nullable_string("Optional sender label for mailbox attribution."),
                ),
                (
                    "idempotency_key".to_string(),
                    nullable_string("Optional idempotency key for safe retries."),
                ),
                (
                    "resume".to_string(),
                    nullable_boolean("If true, mark the message for resume-and-trigger dispatch."),
                ),
                (
                    "dry_run".to_string(),
                    nullable_boolean("If true, validate and preview without enqueuing."),
                ),
            ])),
            Self::MailboxReceipts => strict_object(BTreeMap::from([(
                "message_id".to_string(),
                JsonSchema::string(Some("Mailbox message id.".to_string())),
            )])),
            Self::RespondInteraction => non_strict_object(
                BTreeMap::from([
                    (
                        "interaction_id".to_string(),
                        JsonSchema::string(Some("Pending interaction id.".to_string())),
                    ),
                    (
                        "thread_id".to_string(),
                        nullable_string(
                            "Optional thread id; defaults to the calling thread and is required when responding to another thread's interaction.",
                        ),
                    ),
                    (
                        "terminal_status".to_string(),
                        JsonSchema::string_enum(
                            vec![
                                json!("responded"),
                                json!("expired"),
                                json!("cancelled"),
                                json!("denied"),
                                json!("no_longer_waiting"),
                            ],
                            Some("Terminal status to record for the interaction.".to_string()),
                        ),
                    ),
                    (
                        "response".to_string(),
                        JsonSchema::object(
                            BTreeMap::new(),
                            /*required*/ None,
                            Some(true.into()),
                        ),
                    ),
                    (
                        "response_preview".to_string(),
                        nullable_string("Optional redacted preview of the response."),
                    ),
                    (
                        "dry_run".to_string(),
                        nullable_boolean("If true, validate and preview without updating."),
                    ),
                ]),
                vec!["interaction_id", "terminal_status", "response"],
            ),
        }
    }
}

impl MissionControlRuntime {
    async fn handle_overview(
        &self,
        args: OverviewArgs,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let include_live_sessions = args.include_live_sessions.unwrap_or(true);
        let include_payloads = args.include_payloads.unwrap_or(false);
        let live_thread_ids = if include_live_sessions {
            match self.thread_manager.upgrade() {
                Some(thread_manager) => thread_manager
                    .list_thread_ids()
                    .await
                    .into_iter()
                    .map(|thread_id| thread_id.to_string())
                    .collect::<Vec<_>>(),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let local_limit = args
            .local_limit
            .map(|value| value as usize)
            .unwrap_or(DEFAULT_MISSION_CONTROL_SESSION_LIMIT)
            .clamp(1, MAX_MISSION_CONTROL_SESSION_LIMIT);
        let local_anchor = args
            .local_cursor
            .as_deref()
            .map(parse_local_session_cursor)
            .transpose()?;
        let local_sessions = self
            .state_db
            .list_threads(
                local_limit,
                codex_state::ThreadFilterOptions {
                    archived_only: false,
                    allowed_sources: &[],
                    model_providers: None,
                    cwd_filters: None,
                    anchor: local_anchor.as_ref(),
                    sort_key: codex_state::SortKey::UpdatedAt,
                    sort_direction: codex_state::SortDirection::Desc,
                    search_term: None,
                },
            )
            .await
            .map_err(|err| model_error(format!("failed to list local sessions: {err}")))?;
        let pending_limit = args
            .pending_limit
            .unwrap_or(codex_state::DEFAULT_PENDING_INTERACTION_LIST_LIMIT);
        let page = self
            .state_db
            .list_thread_pending_interactions(codex_state::PendingInteractionListParams {
                thread_id: None,
                statuses: vec![
                    codex_state::PendingInteractionStatus::Pending,
                    codex_state::PendingInteractionStatus::Delivered,
                ],
                kinds: Vec::new(),
                cursor: args.pending_cursor,
                limit: pending_limit,
            })
            .await
            .map_err(|err| model_error(format!("failed to list pending interactions: {err}")))?;

        let include_schedules = self.scheduled_tasks_enabled.load(Ordering::Relaxed);
        let mut local_session_values = Vec::with_capacity(local_sessions.items.len());
        for metadata in local_sessions.items {
            let schedules = if include_schedules {
                self.state_db
                    .thread_schedules()
                    .list_thread_schedules(metadata.id)
                    .await
                    .map_err(|err| model_error(format!("failed to list thread schedules: {err}")))?
            } else {
                Vec::new()
            };
            local_session_values.push(local_session_json(
                metadata,
                &live_thread_ids,
                schedules,
                include_payloads,
            ));
        }

        Ok(json_output(json!({
            "liveThreadIds": live_thread_ids,
            "localSessions": local_session_values,
            "localSessionLimit": local_limit,
            "nextLocalSessionCursor": local_sessions
                .next_anchor
                .map(|anchor| anchor.ts.timestamp_millis().to_string()),
            "pendingInteractions": page
                .data
                .into_iter()
                .map(|interaction| pending_interaction_json(interaction, include_payloads))
                .collect::<Vec<_>>(),
            "nextPendingInteractionCursor": page.next_cursor,
            "capabilities": mission_control_capabilities_json(include_schedules),
        })))
    }

    async fn handle_enqueue_instruction(
        &self,
        args: EnqueueInstructionArgs,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let target_thread_id = parse_thread_id("target_thread_id", &args.target_thread_id)?;
        ensure_thread_exists(self.state_db.as_ref(), target_thread_id).await?;
        let sender_thread_id = args
            .sender_thread_id
            .as_deref()
            .map(|thread_id| parse_thread_id("sender_thread_id", thread_id))
            .transpose()?
            .or(Some(self.current_thread_id));
        let sender_label = normalize_optional_label(args.sender_label)?;
        let idempotency_key = normalize_optional_token("idempotency_key", args.idempotency_key)?;
        let text = validate_required_text("message", args.message)?;
        let preview = truncate_preview(text.as_str());
        let delivery_policy = if args.resume.unwrap_or(false) {
            "resumeAndTrigger"
        } else {
            "liveOnly"
        };

        if args.dry_run.unwrap_or(false) {
            return Ok(json_output(json!({
                "dryRun": true,
                "deliveryPolicy": delivery_policy,
                "preview": preview,
                "message": null,
                "created": null,
            })));
        }

        let payload = if args.resume.unwrap_or(false) {
            json!({ "text": text, "delivery": "resumeAndTrigger" })
        } else {
            json!({ "text": text })
        };
        let outcome = self
            .state_db
            .mailbox_messages()
            .enqueue_message(codex_state::MailboxEnqueueParams {
                target_thread_id,
                sender_thread_id,
                sender_label,
                idempotency_key,
                kind: codex_state::MailboxMessageKind::UserInstruction,
                payload_json: payload,
                payload_preview: preview.clone(),
                priority: 0,
                max_attempts: DEFAULT_MISSION_CONTROL_MAX_ATTEMPTS,
                next_attempt_at: None,
                expires_at: None,
            })
            .await
            .map_err(|err| model_error(format!("failed to enqueue instruction: {err}")))?;

        Ok(json_output(json!({
            "dryRun": false,
            "deliveryPolicy": delivery_policy,
            "preview": preview,
            "message": mailbox_message_json(outcome.message),
            "created": outcome.created,
        })))
    }

    async fn handle_mailbox_receipts(
        &self,
        args: MailboxReceiptsArgs,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let message_id = validate_required_text("message_id", args.message_id)?;
        let message = self
            .state_db
            .mailbox_messages()
            .get_message(message_id.as_str())
            .await
            .map_err(|err| model_error(format!("failed to read mailbox message: {err}")))?
            .ok_or_else(|| model_error(format!("mailbox message not found: {message_id}")))?;
        let receipts = self
            .state_db
            .mailbox_messages()
            .list_receipts(message_id.as_str())
            .await
            .map_err(|err| model_error(format!("failed to list mailbox receipts: {err}")))?;

        Ok(json_output(json!({
            "message": mailbox_message_json(message),
            "data": receipts.into_iter().map(mailbox_receipt_json).collect::<Vec<_>>(),
        })))
    }

    async fn handle_respond_interaction(
        &self,
        args: RespondInteractionArgs,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let interaction_id = validate_required_text("interaction_id", args.interaction_id)?;
        let thread_id = args
            .thread_id
            .as_deref()
            .map(|thread_id| parse_thread_id("thread_id", thread_id))
            .transpose()?
            .unwrap_or(self.current_thread_id);
        let terminal_status = parse_terminal_status(args.terminal_status.as_str())?;
        let terminal_status_state =
            crate::request_processors::thread_pending_interaction_processor::api_pending_interaction_terminal_status_to_state(
                terminal_status,
            );
        let interaction =
            crate::request_processors::thread_pending_interaction_processor::read_pending_interaction(
                self.state_db.as_ref(),
                interaction_id.as_str(),
                Some(thread_id),
            )
            .await
            .map_err(model_error_from_json_rpc)?;
        crate::request_processors::thread_pending_interaction_processor::validate_response_matches_interaction(
            interaction.kind,
            &args.response,
        )
        .map_err(model_error_from_json_rpc)?;
        crate::request_processors::thread_pending_interaction_processor::validate_response_status_matches_payload(
            &args.response,
            terminal_status,
        )
        .map_err(model_error_from_json_rpc)?;
        let stored_response =
            crate::request_processors::thread_pending_interaction_processor::redacted_response_payload(
                &args.response,
            );
        let response_preview = if stored_response.redactions.is_empty() {
            args.response_preview
                .as_deref()
                .map(str::trim)
                .filter(|preview| !preview.is_empty())
                .map(truncate_preview)
                .unwrap_or_else(|| stored_response.preview.clone())
        } else {
            stored_response.preview.clone()
        };

        if interaction.server_request_id_json.is_some() {
            return Err(model_error(
                "pending interaction is tied to a live client request; use the app-server pending-interaction response path so the waiting client is notified",
            ));
        }

        if args.dry_run.unwrap_or(false) {
            return Ok(json_output(json!({
                "dryRun": true,
                "updated": false,
                "interaction": pending_interaction_json(interaction, /*include_payloads*/ false),
                "responsePreview": response_preview,
            })));
        }

        let updated = self
            .state_db
            .respond_thread_pending_interaction(&codex_state::PendingInteractionRespondParams {
                interaction_id: interaction.interaction_id.clone(),
                response_payload_json: stored_response.payload,
                response_payload_preview: response_preview.clone(),
                response_redactions_json: json!(stored_response.redactions),
                terminal_status: terminal_status_state,
            })
            .await
            .map_err(|err| {
                model_error(format!("failed to respond to pending interaction: {err}"))
            })?;
        let interaction = self
            .state_db
            .get_thread_pending_interaction(interaction.interaction_id.as_str())
            .await
            .map_err(|err| model_error(format!("failed to reload pending interaction: {err}")))?;

        Ok(json_output(json!({
            "dryRun": false,
            "updated": updated,
            "interaction": interaction
                .map(|interaction| pending_interaction_json(interaction, /*include_payloads*/ false)),
            "responsePreview": response_preview,
        })))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OverviewArgs {
    pending_cursor: Option<String>,
    pending_limit: Option<u32>,
    local_cursor: Option<String>,
    local_limit: Option<u32>,
    include_live_sessions: Option<bool>,
    include_payloads: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnqueueInstructionArgs {
    target_thread_id: String,
    message: String,
    sender_thread_id: Option<String>,
    sender_label: Option<String>,
    idempotency_key: Option<String>,
    resume: Option<bool>,
    dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MailboxReceiptsArgs {
    message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondInteractionArgs {
    interaction_id: String,
    thread_id: Option<String>,
    terminal_status: String,
    response: ThreadPendingInteractionResponsePayload,
    response_preview: Option<String>,
    dry_run: Option<bool>,
}

fn parse_arguments<T>(tool_name: &str, invocation: &ToolCall) -> Result<T, FunctionCallError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(invocation.function_arguments()?)
        .map_err(|err| model_error(format!("invalid {tool_name} arguments: {err}")))
}

fn parse_local_session_cursor(cursor: &str) -> Result<codex_state::Anchor, FunctionCallError> {
    let millis = cursor
        .trim()
        .parse::<i64>()
        .map_err(|err| model_error(format!("invalid local_cursor: {err}")))?;
    let ts = DateTime::<Utc>::from_timestamp_millis(millis)
        .ok_or_else(|| model_error("invalid local_cursor timestamp"))?;
    Ok(codex_state::Anchor { ts })
}

fn strict_object(properties: BTreeMap<String, JsonSchema>) -> JsonSchema {
    let required = properties.keys().cloned().collect();
    JsonSchema::object(properties, Some(required), Some(false.into()))
}

fn non_strict_object(properties: BTreeMap<String, JsonSchema>, required: Vec<&str>) -> JsonSchema {
    JsonSchema::object(
        properties,
        Some(required.into_iter().map(str::to_string).collect()),
        Some(false.into()),
    )
}

fn nullable_string(description: &str) -> JsonSchema {
    JsonSchema::any_of(
        vec![
            JsonSchema::string(/*description*/ None),
            JsonSchema::null(/*description*/ None),
        ],
        Some(description.to_string()),
    )
}

fn nullable_integer(description: &str) -> JsonSchema {
    JsonSchema::any_of(
        vec![
            JsonSchema::integer(/*description*/ None),
            JsonSchema::null(/*description*/ None),
        ],
        Some(description.to_string()),
    )
}

fn nullable_boolean(description: &str) -> JsonSchema {
    JsonSchema::any_of(
        vec![
            JsonSchema::boolean(/*description*/ None),
            JsonSchema::null(/*description*/ None),
        ],
        Some(description.to_string()),
    )
}

fn parse_thread_id(field_name: &str, value: &str) -> Result<ThreadId, FunctionCallError> {
    ThreadId::from_string(value).map_err(|err| model_error(format!("invalid {field_name}: {err}")))
}

fn model_error_from_json_rpc(
    error: codex_app_server_protocol::JSONRPCErrorError,
) -> FunctionCallError {
    model_error(error.message)
}

async fn ensure_thread_exists(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<(), FunctionCallError> {
    state_db
        .get_thread(thread_id)
        .await
        .map_err(|err| model_error(format!("failed to read thread metadata: {err}")))?
        .ok_or_else(|| model_error(format!("thread not found: {thread_id}")))?;
    Ok(())
}

fn parse_terminal_status(
    value: &str,
) -> Result<ThreadPendingInteractionTerminalStatus, FunctionCallError> {
    let status = match value {
        "responded" => ThreadPendingInteractionTerminalStatus::Responded,
        "expired" => ThreadPendingInteractionTerminalStatus::Expired,
        "cancelled" => ThreadPendingInteractionTerminalStatus::Cancelled,
        "denied" => ThreadPendingInteractionTerminalStatus::Denied,
        "no_longer_waiting" => ThreadPendingInteractionTerminalStatus::NoLongerWaiting,
        other => {
            return Err(model_error(format!(
                "terminal_status must be one of responded, expired, cancelled, denied, no_longer_waiting; got {other}"
            )));
        }
    };
    Ok(status)
}

fn normalize_optional_label(value: Option<String>) -> Result<Option<String>, FunctionCallError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Err(model_error("sender_label must not be empty"));
            }
            if value.chars().count() > MAX_MISSION_CONTROL_SENDER_LABEL_CHARS {
                return Err(model_error(format!(
                    "sender_label must be at most {MAX_MISSION_CONTROL_SENDER_LABEL_CHARS} characters"
                )));
            }
            Ok(value.to_string())
        })
        .transpose()
}

fn normalize_optional_token(
    field_name: &str,
    value: Option<String>,
) -> Result<Option<String>, FunctionCallError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Err(model_error(format!("{field_name} must not be empty")));
            }
            Ok(value.to_string())
        })
        .transpose()
}

fn validate_required_text(field_name: &str, value: String) -> Result<String, FunctionCallError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(model_error(format!("{field_name} must not be empty")));
    }
    Ok(value.to_string())
}

fn truncate_preview(value: &str) -> String {
    value.chars().take(MISSION_CONTROL_PREVIEW_CHARS).collect()
}

fn mailbox_message_json(message: codex_state::MailboxMessage) -> Value {
    json!({
        "messageId": message.message_id,
        "targetThreadId": message.target_thread_id.to_string(),
        "senderThreadId": message.sender_thread_id.map(|thread_id| thread_id.to_string()),
        "senderLabel": message.sender_label,
        "kind": message.kind.as_str(),
        "status": message.status.as_str(),
        "payloadSha256": message.payload_sha256,
        "payloadPreview": message.payload_preview,
        "priority": message.priority,
        "attemptCount": message.attempt_count,
        "maxAttempts": message.max_attempts,
        "nextAttemptAt": message.next_attempt_at.timestamp(),
        "leaseExpiresAt": message.lease_expires_at.map(|timestamp| timestamp.timestamp()),
        "lastError": message.last_error,
        "expiresAt": message.expires_at.map(|timestamp| timestamp.timestamp()),
        "acknowledgedAt": message.acknowledged_at.map(|timestamp| timestamp.timestamp()),
        "terminalAt": message.terminal_at.map(|timestamp| timestamp.timestamp()),
        "createdAt": message.created_at.timestamp(),
        "updatedAt": message.updated_at.timestamp(),
    })
}

fn mailbox_receipt_json(receipt: codex_state::MailboxReceipt) -> Value {
    json!({
        "receiptId": receipt.receipt_id,
        "messageId": receipt.message_id,
        "attemptId": receipt.attempt_id,
        "threadId": receipt.thread_id.to_string(),
        "kind": receipt.kind.as_str(),
        "statusAfter": receipt.status_after.as_str(),
        "payload": receipt.payload_json,
        "createdAt": receipt.created_at.timestamp(),
    })
}

fn local_session_json(
    metadata: codex_state::ThreadMetadata,
    live_thread_ids: &[String],
    schedules: Vec<codex_state::ThreadSchedule>,
    include_payloads: bool,
) -> Value {
    let thread_id = metadata.id.to_string();
    let live = live_thread_ids.contains(&thread_id);
    let schedule_count = schedules.len();
    json!({
        "threadId": thread_id,
        "live": live,
        "cwd": metadata.cwd.display().to_string(),
        "title": metadata.title,
        "preview": metadata.preview,
        "modelProvider": metadata.model_provider,
        "model": metadata.model,
        "source": metadata.source,
        "threadSource": metadata
            .thread_source
            .map(|source| source.as_str().to_string()),
        "agentNickname": metadata.agent_nickname,
        "agentRole": metadata.agent_role,
        "agentPath": metadata.agent_path,
        "createdAt": metadata.created_at.timestamp(),
        "updatedAt": metadata.updated_at.timestamp(),
        "path": metadata.rollout_path.display().to_string(),
        "scheduleCount": schedule_count,
        "schedules": schedules
            .into_iter()
            .map(|schedule| schedule_summary_json(schedule, include_payloads))
            .collect::<Vec<_>>(),
    })
}

fn schedule_summary_json(schedule: codex_state::ThreadSchedule, include_payloads: bool) -> Value {
    let prompt = if include_payloads {
        json!(&schedule.prompt)
    } else {
        Value::Null
    };
    json!({
        "threadId": schedule.thread_id.to_string(),
        "scheduleId": schedule.schedule_id,
        "promptPreview": truncate_preview(&schedule.prompt),
        "promptChars": schedule.prompt.chars().count(),
        "prompt": prompt,
        "promptSource": schedule.prompt_source.as_str(),
        "schedule": schedule_spec_json(schedule.schedule),
        "timezone": schedule.timezone,
        "status": schedule.status.as_str(),
        "nextRunAt": schedule.next_run_at.map(|timestamp| timestamp.timestamp()),
        "lastRunAt": schedule.last_run_at.map(|timestamp| timestamp.timestamp()),
        "expiresAt": schedule.expires_at.map(|timestamp| timestamp.timestamp()),
        "failureCount": schedule.failure_count,
        "leaseExpiresAt": schedule.lease_expires_at.map(|timestamp| timestamp.timestamp()),
        "createdAt": schedule.created_at.timestamp(),
        "updatedAt": schedule.updated_at.timestamp(),
    })
}

fn schedule_spec_json(schedule: codex_state::ThreadScheduleSpec) -> Value {
    match schedule {
        codex_state::ThreadScheduleSpec::Once => json!({ "type": "once" }),
        codex_state::ThreadScheduleSpec::Dynamic => json!({ "type": "dynamic" }),
        codex_state::ThreadScheduleSpec::Interval(interval) => json!({
            "type": "interval",
            "amount": interval.amount,
            "unit": interval.unit.as_str(),
        }),
        codex_state::ThreadScheduleSpec::Cron { expression } => json!({
            "type": "cron",
            "expression": expression,
        }),
    }
}

fn pending_interaction_json(
    interaction: codex_state::PendingInteraction,
    include_payloads: bool,
) -> Value {
    let request_payload = if include_payloads {
        interaction.request_payload_json
    } else {
        Value::Null
    };
    let response_payload = if include_payloads {
        interaction
            .response_payload_json
            .map_or(Value::Null, |payload| payload)
    } else {
        Value::Null
    };
    json!({
        "interactionId": interaction.interaction_id,
        "threadId": interaction.thread_id.to_string(),
        "sourceKind": interaction.source_kind.as_str(),
        "sourceId": interaction.source_id,
        "turnId": interaction.turn_id,
        "workerRequestId": interaction.worker_request_id,
        "kind": interaction.kind.as_str(),
        "status": interaction.status.as_str(),
        "requestPayload": request_payload,
        "requestPayloadSha256": interaction.request_payload_sha256,
        "requestPayloadPreview": interaction.request_payload_preview,
        "requestRedactions": interaction.request_redactions_json,
        "responsePayload": response_payload,
        "responsePayloadSha256": interaction.response_payload_sha256,
        "responsePayloadPreview": interaction.response_payload_preview,
        "responseRedactions": interaction.response_redactions_json,
        "noClientPolicy": interaction.no_client_policy,
        "timeoutAt": interaction.timeout_at.map(|timestamp| timestamp.timestamp()),
        "createdAt": interaction.created_at.timestamp(),
        "deliveredAt": interaction.delivered_at.map(|timestamp| timestamp.timestamp()),
        "respondedAt": interaction.responded_at.map(|timestamp| timestamp.timestamp()),
        "terminalAt": interaction.terminal_at.map(|timestamp| timestamp.timestamp()),
        "updatedAt": interaction.updated_at.timestamp(),
    })
}

fn mission_control_capabilities_json(scheduled_tasks_enabled: bool) -> Value {
    json!({
        "localSessions": true,
        "durableMailbox": true,
        "pendingInteractions": true,
        "goals": true,
        "scheduledTasks": scheduled_tasks_enabled,
        "remoteDispatch": false,
        "workflowMutation": false,
        "shellExecution": false,
        "filesystemMutation": false,
    })
}

fn json_output(value: Value) -> Box<dyn ToolOutput> {
    Box::new(JsonToolOutput::new(value))
}

fn model_error(message: impl Into<String>) -> FunctionCallError {
    FunctionCallError::RespondToModel(message.into())
}

pub(crate) fn install<C>(
    registry: &mut ExtensionRegistryBuilder<C>,
    state_db: StateDbHandle,
    thread_manager: Weak<ThreadManager>,
    mission_control_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
    scheduled_tasks_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
) where
    C: Send + Sync + 'static,
{
    let extension = Arc::new(MissionControlExtension::new(
        state_db,
        thread_manager,
        mission_control_enabled,
        scheduled_tasks_enabled,
    ));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use codex_app_server_protocol::RequestId;
    use codex_app_server_protocol::ToolRequestUserInputAnswer;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::ThreadStartInput;
    use codex_extension_api::ToolPayload;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::ResponseInputItem;
    use codex_protocol::protocol::SessionSource;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn installed_extension_hides_tools_when_disabled() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let state_db = test_state_db().await;
            let mut builder = ExtensionRegistryBuilder::<bool>::new();
            install(
                &mut builder,
                state_db,
                Weak::new(),
                |enabled| *enabled,
                |_| true,
            );
            let registry = builder.build();
            let session_store = ExtensionData::new("session");
            let thread_store = ExtensionData::new(test_thread_id().to_string());
            let lifecycle = &registry.thread_lifecycle_contributors()[0];
            lifecycle
                .on_thread_start(ThreadStartInput {
                    config: &false,
                    session_source: &SessionSource::Cli,
                    persistent_thread_state_available: true,
                    session_store: &session_store,
                    thread_store: &thread_store,
                })
                .await;

            let tool_names = registry.tool_contributors()[0]
                .tools(&session_store, &thread_store)
                .into_iter()
                .map(|tool| tool.tool_name())
                .collect::<Vec<_>>();

            assert_eq!(Vec::<ToolName>::new(), tool_names);
        });
    }

    #[tokio::test]
    async fn installed_extension_contributes_mission_control_tools_when_enabled() {
        let state_db = test_state_db().await;
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(
            &mut builder,
            state_db,
            Weak::new(),
            |enabled| *enabled,
            |_| true,
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new(test_thread_id().to_string());
        registry.thread_lifecycle_contributors()[0]
            .on_thread_start(ThreadStartInput {
                config: &true,
                session_source: &SessionSource::Cli,
                persistent_thread_state_available: true,
                session_store: &session_store,
                thread_store: &thread_store,
            })
            .await;

        let tool_names = registry.tool_contributors()[0]
            .tools(&session_store, &thread_store)
            .into_iter()
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>();

        assert_eq!(
            vec![
                ToolName::plain(MISSION_CONTROL_OVERVIEW_TOOL_NAME),
                ToolName::plain(MISSION_CONTROL_ENQUEUE_INSTRUCTION_TOOL_NAME),
                ToolName::plain(MISSION_CONTROL_MAILBOX_RECEIPTS_TOOL_NAME),
                ToolName::plain(MISSION_CONTROL_RESPOND_INTERACTION_TOOL_NAME),
            ],
            tool_names
        );
    }

    #[tokio::test]
    async fn enqueue_instruction_supports_dry_run_idempotency_and_receipts() {
        let state_db = test_state_db().await;
        let current_thread_id = test_thread_id();
        let target_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid thread id");
        upsert_test_thread(state_db.as_ref(), target_thread_id).await;
        let runtime = MissionControlRuntime {
            state_db,
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id,
        };
        let dry_run = output_json(
            runtime
                .handle_enqueue_instruction(EnqueueInstructionArgs {
                    target_thread_id: target_thread_id.to_string(),
                    message: "  Plan the rollout  ".to_string(),
                    sender_thread_id: None,
                    sender_label: None,
                    idempotency_key: Some("daily-rollout".to_string()),
                    resume: Some(true),
                    dry_run: Some(true),
                })
                .await
                .expect("dry-run should return output"),
        )
        .await;
        assert_eq!(dry_run["dryRun"], true);
        assert_eq!(dry_run["deliveryPolicy"], "resumeAndTrigger");
        assert_eq!(dry_run["preview"], "Plan the rollout");

        let first = output_json(
            runtime
                .handle_enqueue_instruction(EnqueueInstructionArgs {
                    target_thread_id: target_thread_id.to_string(),
                    message: "Plan the rollout".to_string(),
                    sender_thread_id: None,
                    sender_label: None,
                    idempotency_key: Some("daily-rollout".to_string()),
                    resume: Some(true),
                    dry_run: None,
                })
                .await
                .expect("enqueue should return output"),
        )
        .await;
        let second = output_json(
            runtime
                .handle_enqueue_instruction(EnqueueInstructionArgs {
                    target_thread_id: target_thread_id.to_string(),
                    message: "Plan the rollout".to_string(),
                    sender_thread_id: None,
                    sender_label: None,
                    idempotency_key: Some("daily-rollout".to_string()),
                    resume: Some(true),
                    dry_run: None,
                })
                .await
                .expect("idempotent enqueue should return output"),
        )
        .await;

        assert_eq!(first["created"], true);
        assert_eq!(second["created"], false);
        assert_eq!(
            first["message"]["messageId"],
            second["message"]["messageId"]
        );
        assert_eq!(
            first["message"]["senderThreadId"],
            current_thread_id.to_string()
        );

        let receipts = output_json(
            runtime
                .handle_mailbox_receipts(MailboxReceiptsArgs {
                    message_id: first["message"]["messageId"]
                        .as_str()
                        .expect("message id")
                        .to_string(),
                })
                .await
                .expect("receipt list should return output"),
        )
        .await;
        assert_eq!(receipts["data"].as_array().expect("receipts").len(), 1);
        assert_eq!(receipts["data"][0]["kind"], "enqueued");
    }

    #[tokio::test]
    async fn overview_and_respond_interaction_use_pending_interaction_state() {
        let state_db = test_state_db().await;
        let thread_id = test_thread_id();
        let interaction_id = "int-1";
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("worker-1".to_string()),
                server_request_id_json: None,
                kind: codex_state::PendingInteractionKind::Blocked,
                request_payload_json: json!({ "reason": "needs user decision" }),
                request_payload_preview: "needs user decision".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "persist".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should be created");
        state_db
            .thread_schedules()
            .create_thread_schedule(codex_state::ThreadScheduleCreateParams {
                thread_id,
                prompt: "Check schedule visibility".to_string(),
                prompt_source: codex_state::ThreadSchedulePromptSource::Inline,
                schedule: codex_state::ThreadScheduleSpec::Once,
                timezone: "UTC".to_string(),
                status: codex_state::ThreadScheduleStatus::Active,
                next_run_at: DateTime::<Utc>::from_timestamp(1_900, 0),
                expires_at: None,
            })
            .await
            .expect("schedule should be created");
        let runtime = MissionControlRuntime {
            state_db,
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: thread_id,
        };

        let overview = output_json(
            runtime
                .handle_overview(OverviewArgs {
                    pending_cursor: None,
                    pending_limit: None,
                    local_cursor: None,
                    local_limit: None,
                    include_live_sessions: Some(false),
                    include_payloads: None,
                })
                .await
                .expect("overview should return output"),
        )
        .await;
        assert_eq!(overview["liveThreadIds"], json!([]));
        assert_eq!(
            overview["localSessions"][0]["threadId"],
            thread_id.to_string()
        );
        assert_eq!(overview["localSessions"][0]["live"], false);
        assert_eq!(
            overview["pendingInteractions"][0]["interactionId"],
            interaction_id
        );
        assert_eq!(
            overview["pendingInteractions"][0]["requestPayloadPreview"],
            "needs user decision"
        );
        assert!(
            overview["pendingInteractions"][0]["requestPayload"].is_null(),
            "mission-control overview should not include full request payloads by default"
        );
        assert!(
            overview["pendingInteractions"][0]["responsePayload"].is_null(),
            "mission-control overview should not include full response payloads by default"
        );
        let payload_overview = output_json(
            runtime
                .handle_overview(OverviewArgs {
                    pending_cursor: None,
                    pending_limit: None,
                    local_cursor: None,
                    local_limit: None,
                    include_live_sessions: Some(false),
                    include_payloads: Some(true),
                })
                .await
                .expect("overview should return payloads when requested"),
        )
        .await;
        assert_eq!(
            payload_overview["pendingInteractions"][0]["requestPayload"],
            json!({ "reason": "needs user decision" })
        );
        assert_eq!(
            payload_overview["localSessions"][0]["schedules"][0]["prompt"],
            "Check schedule visibility"
        );
        assert_eq!(
            overview["localSessions"][0]["schedules"]
                .as_array()
                .expect("schedules")
                .len(),
            1
        );
        assert_eq!(overview["localSessions"][0]["scheduleCount"], 1);
        assert_eq!(
            overview["localSessions"][0]["schedules"][0]["promptPreview"],
            "Check schedule visibility"
        );
        assert_eq!(
            overview["localSessions"][0]["schedules"][0]["promptChars"],
            25
        );
        assert!(
            overview["localSessions"][0]["schedules"][0]["prompt"].is_null(),
            "mission-control overview should not include full schedule prompts by default"
        );
        assert_eq!(overview["capabilities"]["scheduledTasks"], true);
        assert_eq!(overview["capabilities"]["workflowMutation"], false);

        runtime
            .scheduled_tasks_enabled
            .store(false, Ordering::Relaxed);
        let schedules_disabled_overview = output_json(
            runtime
                .handle_overview(OverviewArgs {
                    pending_cursor: None,
                    pending_limit: None,
                    local_cursor: None,
                    local_limit: None,
                    include_live_sessions: Some(false),
                    include_payloads: None,
                })
                .await
                .expect("overview should return output when schedules are disabled"),
        )
        .await;
        assert_eq!(
            schedules_disabled_overview["capabilities"]["scheduledTasks"],
            false
        );
        assert_eq!(
            schedules_disabled_overview["localSessions"][0]["schedules"],
            json!([])
        );

        let dry_run = output_json(
            runtime
                .handle_respond_interaction(RespondInteractionArgs {
                    interaction_id: interaction_id.to_string(),
                    thread_id: None,
                    terminal_status: "responded".to_string(),
                    response: ThreadPendingInteractionResponsePayload::Terminal {
                        reason: "go ahead".to_string(),
                    },
                    response_preview: None,
                    dry_run: Some(true),
                })
                .await
                .expect("respond dry-run should return output"),
        )
        .await;
        assert_eq!(dry_run["dryRun"], true);
        assert_eq!(dry_run["updated"], false);

        let response = output_json(
            runtime
                .handle_respond_interaction(RespondInteractionArgs {
                    interaction_id: interaction_id.to_string(),
                    thread_id: None,
                    terminal_status: "responded".to_string(),
                    response: ThreadPendingInteractionResponsePayload::Terminal {
                        reason: "go ahead".to_string(),
                    },
                    response_preview: Some("go ahead".to_string()),
                    dry_run: None,
                })
                .await
                .expect("respond should return output"),
        )
        .await;
        assert_eq!(response["updated"], true);
        assert_eq!(response["interaction"]["status"], "responded");
        assert_eq!(response["responsePreview"], "go ahead");
    }

    #[tokio::test]
    async fn respond_interaction_rejects_kind_mismatch() {
        let state_db = test_state_db().await;
        let thread_id = test_thread_id();
        let interaction_id = "int-kind-mismatch";
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("worker-1".to_string()),
                server_request_id_json: None,
                kind: codex_state::PendingInteractionKind::UserInput,
                request_payload_json: json!({ "questions": [] }),
                request_payload_preview: "question".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "persist".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should be created");
        let runtime = MissionControlRuntime {
            state_db: state_db.clone(),
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: thread_id,
        };

        let Err(err) = runtime
            .handle_respond_interaction(RespondInteractionArgs {
                interaction_id: interaction_id.to_string(),
                thread_id: None,
                terminal_status: "responded".to_string(),
                response: ThreadPendingInteractionResponsePayload::Terminal {
                    reason: "wrong shape".to_string(),
                },
                response_preview: None,
                dry_run: None,
            })
            .await
        else {
            panic!("kind mismatch should be rejected");
        };

        assert!(
            err.to_string()
                .contains("pending interaction response kind mismatch"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn respond_interaction_rejects_status_payload_mismatch() {
        let state_db = test_state_db().await;
        let thread_id = test_thread_id();
        let interaction_id = "int-status-mismatch";
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("worker-1".to_string()),
                server_request_id_json: None,
                kind: codex_state::PendingInteractionKind::UserInput,
                request_payload_json: json!({ "questions": [] }),
                request_payload_preview: "question".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "persist".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should be created");
        let runtime = MissionControlRuntime {
            state_db: state_db.clone(),
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: thread_id,
        };

        let Err(err) = runtime
            .handle_respond_interaction(RespondInteractionArgs {
                interaction_id: interaction_id.to_string(),
                thread_id: None,
                terminal_status: "denied".to_string(),
                response: ThreadPendingInteractionResponsePayload::RequestUserInput {
                    answers: HashMap::new(),
                },
                response_preview: None,
                dry_run: Some(true),
            })
            .await
        else {
            panic!("status/payload mismatch should be rejected");
        };

        assert!(err.to_string().contains("must be responded"), "{err}");
    }

    #[tokio::test]
    async fn respond_interaction_defaults_to_current_thread_scope() {
        let state_db = test_state_db().await;
        let current_thread_id = test_thread_id();
        let other_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid thread id");
        upsert_test_thread(state_db.as_ref(), other_thread_id).await;
        let interaction_id = "int-other-thread";
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id: other_thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("worker-1".to_string()),
                server_request_id_json: None,
                kind: codex_state::PendingInteractionKind::Blocked,
                request_payload_json: json!({ "reason": "needs user decision" }),
                request_payload_preview: "needs user decision".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "persist".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should be created");
        let runtime = MissionControlRuntime {
            state_db,
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id,
        };

        let Err(err) = runtime
            .handle_respond_interaction(RespondInteractionArgs {
                interaction_id: interaction_id.to_string(),
                thread_id: None,
                terminal_status: "responded".to_string(),
                response: ThreadPendingInteractionResponsePayload::Terminal {
                    reason: "go ahead".to_string(),
                },
                response_preview: None,
                dry_run: Some(true),
            })
            .await
        else {
            panic!("unqualified cross-thread interaction response should be rejected");
        };
        assert!(
            err.to_string()
                .contains("pending interaction not found: int-other-thread"),
            "{err}"
        );

        let output = output_json(
            runtime
                .handle_respond_interaction(RespondInteractionArgs {
                    interaction_id: interaction_id.to_string(),
                    thread_id: Some(other_thread_id.to_string()),
                    terminal_status: "responded".to_string(),
                    response: ThreadPendingInteractionResponsePayload::Terminal {
                        reason: "go ahead".to_string(),
                    },
                    response_preview: None,
                    dry_run: None,
                })
                .await
                .expect("explicit target thread should be allowed"),
        )
        .await;
        assert_eq!(output["updated"], true);
        assert_eq!(
            output["interaction"]["threadId"],
            other_thread_id.to_string()
        );
    }

    #[tokio::test]
    async fn respond_interaction_redacts_sensitive_response_payloads() {
        let state_db = test_state_db().await;
        let thread_id = test_thread_id();
        let interaction_id = "int-user-input";
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("worker-1".to_string()),
                server_request_id_json: None,
                kind: codex_state::PendingInteractionKind::UserInput,
                request_payload_json: json!({ "questions": [] }),
                request_payload_preview: "question".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "persist".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should be created");
        let runtime = MissionControlRuntime {
            state_db: state_db.clone(),
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: thread_id,
        };
        let response = ThreadPendingInteractionResponsePayload::RequestUserInput {
            answers: HashMap::from([(
                "token".to_string(),
                ToolRequestUserInputAnswer {
                    answers: vec!["secret-value".to_string()],
                },
            )]),
        };

        let output = output_json(
            runtime
                .handle_respond_interaction(RespondInteractionArgs {
                    interaction_id: interaction_id.to_string(),
                    thread_id: None,
                    terminal_status: "responded".to_string(),
                    response,
                    response_preview: Some("secret-value".to_string()),
                    dry_run: None,
                })
                .await
                .expect("typed user-input response should be recorded"),
        )
        .await;

        assert_eq!(output["updated"], true);
        assert_eq!(output["responsePreview"], "1 user input answer(s)");
        let stored = state_db
            .get_thread_pending_interaction(interaction_id)
            .await
            .expect("pending interaction should reload")
            .expect("pending interaction should exist");
        assert_eq!(
            stored.response_payload_json,
            Some(json!({ "type": "requestUserInput", "answerCount": 1 }))
        );
        assert_eq!(
            stored.response_redactions_json,
            Some(json!(["responsePayload"]))
        );
        assert_eq!(
            stored.response_payload_preview,
            Some("1 user input answer(s)".to_string())
        );
    }

    #[tokio::test]
    async fn respond_interaction_rejects_live_client_requests() {
        let state_db = test_state_db().await;
        let thread_id = test_thread_id();
        let interaction_id = "int-live-request";
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some("worker-1".to_string()),
                server_request_id_json: Some(
                    serde_json::to_value(RequestId::Integer(7))
                        .expect("request id should serialize"),
                ),
                kind: codex_state::PendingInteractionKind::Blocked,
                request_payload_json: json!({ "reason": "needs user decision" }),
                request_payload_preview: "needs user decision".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "persist".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should be created");
        let runtime = MissionControlRuntime {
            state_db: state_db.clone(),
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: thread_id,
        };

        let Err(err) = runtime
            .handle_respond_interaction(RespondInteractionArgs {
                interaction_id: interaction_id.to_string(),
                thread_id: None,
                terminal_status: "responded".to_string(),
                response: ThreadPendingInteractionResponsePayload::Terminal {
                    reason: "go ahead".to_string(),
                },
                response_preview: None,
                dry_run: Some(true),
            })
            .await
        else {
            panic!("live client request dry-run should be rejected");
        };

        assert!(
            err.to_string()
                .contains("pending interaction is tied to a live client request"),
            "{err}"
        );

        let stored = state_db
            .get_thread_pending_interaction(interaction_id)
            .await
            .expect("pending interaction should reload")
            .expect("pending interaction should exist");
        assert_eq!(
            codex_state::PendingInteractionStatus::Pending,
            stored.status
        );

        let Err(err) = runtime
            .handle_respond_interaction(RespondInteractionArgs {
                interaction_id: interaction_id.to_string(),
                thread_id: None,
                terminal_status: "responded".to_string(),
                response: ThreadPendingInteractionResponsePayload::Terminal {
                    reason: "go ahead".to_string(),
                },
                response_preview: None,
                dry_run: None,
            })
            .await
        else {
            panic!("live client request should be rejected");
        };

        assert!(
            err.to_string()
                .contains("pending interaction is tied to a live client request"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn tool_spec_is_model_only_and_non_workflow_mutating() {
        let runtime = MissionControlRuntime {
            state_db: test_state_db().await,
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: test_thread_id(),
        };
        let tool = MissionControlTool::new(MissionControlToolKind::EnqueueInstruction, runtime);

        assert_eq!(tool.exposure(), ToolExposure::DirectModelOnly);
        let ToolSpec::Function(spec) = tool.spec() else {
            panic!("mission-control tool should be a function tool");
        };
        assert_eq!(spec.name, MISSION_CONTROL_ENQUEUE_INSTRUCTION_TOOL_NAME);
        assert!(spec.strict);
        assert!(spec.description.contains("mailbox"));
        assert!(spec.description.contains("does not execute shell commands"));
        assert!(spec.description.contains("create workflows"));
    }

    #[tokio::test]
    async fn mission_control_tool_specs_serialize_for_responses_api() {
        let runtime = MissionControlRuntime {
            state_db: test_state_db().await,
            thread_manager: Weak::new(),
            enabled: Arc::new(AtomicBool::new(true)),
            scheduled_tasks_enabled: Arc::new(AtomicBool::new(true)),
            current_thread_id: test_thread_id(),
        };

        for kind in [
            MissionControlToolKind::Overview,
            MissionControlToolKind::EnqueueInstruction,
            MissionControlToolKind::MailboxReceipts,
            MissionControlToolKind::RespondInteraction,
        ] {
            let spec = MissionControlTool::new(kind, runtime.clone()).spec();
            let ToolSpec::Function(function) = &spec else {
                panic!("mission-control tool should be a function tool");
            };
            assert_eq!(function.strict, kind.strict());
            if function.strict {
                assert_required_matches_properties(&function.parameters);
            }
            let tool_json = codex_tools::create_tools_json_for_responses_api(&[spec])
                .expect("mission-control tool schema should serialize for Responses");
            assert_mission_control_schema_regression(kind, &tool_json[0]);
        }
    }

    fn assert_required_matches_properties(schema: &JsonSchema) {
        let properties = schema
            .properties
            .as_ref()
            .expect("strict object should have properties")
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let required = schema
            .required
            .as_ref()
            .expect("strict object should have required properties")
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();

        assert_eq!(properties, required);
    }

    fn assert_mission_control_schema_regression(kind: MissionControlToolKind, tool_json: &Value) {
        match kind {
            MissionControlToolKind::Overview => {
                assert_eq!(
                    tool_json.pointer("/parameters/required"),
                    Some(&json!([
                        "include_live_sessions",
                        "include_payloads",
                        "local_cursor",
                        "local_limit",
                        "pending_cursor",
                        "pending_limit"
                    ]))
                );
                assert_nullable_property(tool_json, "include_live_sessions");
                assert_nullable_property(tool_json, "include_payloads");
            }
            MissionControlToolKind::EnqueueInstruction => {
                assert_nullable_property(tool_json, "resume");
                assert_nullable_property(tool_json, "dry_run");
            }
            MissionControlToolKind::MailboxReceipts => {
                assert_eq!(
                    tool_json.pointer("/parameters/required"),
                    Some(&json!(["message_id"]))
                );
            }
            MissionControlToolKind::RespondInteraction => {
                assert_eq!(tool_json.get("strict"), Some(&json!(false)));
                assert_eq!(
                    tool_json.pointer("/parameters/additionalProperties"),
                    Some(&json!(false))
                );
                assert_nullable_property(tool_json, "thread_id");
            }
        }
    }

    fn assert_nullable_property(tool_json: &Value, property_name: &str) {
        let variants = tool_json
            .pointer(format!("/parameters/properties/{property_name}/anyOf").as_str())
            .and_then(Value::as_array)
            .expect("nullable property should use anyOf");
        assert!(
            variants
                .iter()
                .any(|variant| variant.get("type") == Some(&json!("null"))),
            "{property_name} should allow null"
        );
    }

    async fn output_json(output: Box<dyn ToolOutput>) -> Value {
        let payload = ToolPayload::Function {
            arguments: "{}".to_string(),
        };
        let ResponseInputItem::FunctionCallOutput { output, .. } =
            output.to_response_item("call-1", &payload)
        else {
            panic!("expected function output");
        };
        let FunctionCallOutputBody::Text(text) = output.body else {
            panic!("expected text output");
        };
        serde_json::from_str(&text).expect("tool output should be json")
    }

    async fn test_state_db() -> StateDbHandle {
        let tempdir = TempDir::new().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        upsert_test_thread(state_db.as_ref(), test_thread_id()).await;
        state_db
    }

    async fn upsert_test_thread(state_db: &codex_state::StateRuntime, thread_id: ThreadId) {
        let mut metadata = codex_state::ThreadMetadataBuilder::new(
            thread_id,
            PathBuf::from(format!("/tmp/{thread_id}.jsonl")),
            Utc::now(),
            SessionSource::Cli,
        )
        .build("test-provider");
        metadata.title = "test session".to_string();
        metadata.preview = Some("test session".to_string());
        metadata.first_user_message = Some("test session".to_string());
        state_db
            .upsert_thread(&metadata)
            .await
            .expect("thread metadata should upsert");
    }

    fn test_thread_id() -> ThreadId {
        ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid thread id")
    }
}
