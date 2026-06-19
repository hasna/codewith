use super::MachineRegistryTrustState;
use super::MissionControlDeliveryPolicy;
use super::ThreadMailboxMessageSummary;
use super::ThreadMailboxReceipt;
use super::ThreadPendingInteraction;
use super::ThreadPendingInteractionEvent;
use super::ThreadPendingInteractionKind;
use super::ThreadPendingInteractionResponsePayload;
use super::ThreadPendingInteractionStatus;
use super::ThreadPendingInteractionTerminalStatus;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchNegotiateParams {
    #[ts(optional = nullable)]
    pub source_machine_id: Option<String>,
    #[ts(optional = nullable)]
    pub target_machine_id: Option<String>,
    #[ts(optional = nullable)]
    pub protocol_version: Option<u32>,
    #[ts(optional = nullable)]
    pub requested_capabilities: Option<Vec<RemoteDispatchCapability>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchNegotiateResponse {
    pub protocol_version: u32,
    pub required_trust_state: MachineRegistryTrustState,
    pub supported_capabilities: Vec<RemoteDispatchCapability>,
    pub supported_operations: Vec<RemoteDispatchOperationKind>,
    pub denied_operation_classes: Vec<RemoteDispatchDeniedOperationClass>,
    pub local_machine_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchSubmitParams {
    pub request_id: String,
    pub source_machine_id: String,
    pub target_machine_id: String,
    pub idempotency_key: String,
    pub operation: RemoteDispatchOperation,
    #[ts(type = "number | null", optional = nullable)]
    pub requested_at: Option<i64>,
    #[ts(type = "number | null", optional = nullable)]
    pub expires_at: Option<i64>,
    #[ts(optional = nullable)]
    pub capability_version: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchSubmitResponse {
    pub request_id: String,
    pub status: RemoteDispatchRequestStatus,
    pub receipt: RemoteDispatchReceipt,
    pub result: Option<RemoteDispatchOperationResult>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchReceiptReadParams {
    #[ts(optional = nullable)]
    pub request_id: Option<String>,
    #[ts(optional = nullable)]
    pub idempotency_key: Option<String>,
    pub source_machine_id: String,
    pub target_machine_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchReceiptReadResponse {
    pub receipt: Option<RemoteDispatchReceipt>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchEnqueueInstructionParams {
    pub target_thread_id: String,
    pub message: String,
    #[ts(optional = nullable)]
    pub sender_thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub sender_label: Option<String>,
    #[ts(optional = nullable)]
    pub priority: Option<i64>,
    #[ts(optional = nullable)]
    pub max_attempts: Option<u32>,
    #[ts(type = "number | null", optional = nullable)]
    pub expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchListPendingInteractionsParams {
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    #[ts(optional = nullable)]
    pub statuses: Option<Vec<ThreadPendingInteractionStatus>>,
    #[ts(optional = nullable)]
    pub kinds: Option<Vec<ThreadPendingInteractionKind>>,
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchReadPendingInteractionParams {
    pub interaction_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchRespondInteractionParams {
    pub interaction_id: String,
    #[ts(optional = nullable)]
    pub thread_id: Option<String>,
    pub terminal_status: ThreadPendingInteractionTerminalStatus,
    pub response: ThreadPendingInteractionResponsePayload,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchMailboxReceiptsParams {
    pub target_thread_id: String,
    pub message_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchEnqueueInstructionResult {
    pub delivery_policy: MissionControlDeliveryPolicy,
    pub preview: String,
    pub message: Option<ThreadMailboxMessageSummary>,
    pub created: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchListPendingInteractionsResult {
    pub data: Vec<ThreadPendingInteraction>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchReadPendingInteractionResult {
    pub interaction: ThreadPendingInteraction,
    pub events: Vec<ThreadPendingInteractionEvent>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchRespondInteractionResult {
    pub updated: bool,
    pub interaction: Option<ThreadPendingInteraction>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchMailboxReceiptsResult {
    pub data: Vec<ThreadMailboxReceipt>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", export_to = "v2/")]
pub enum RemoteDispatchOperation {
    #[serde(rename_all = "camelCase")]
    EnqueueInstruction {
        params: RemoteDispatchEnqueueInstructionParams,
    },
    #[serde(rename_all = "camelCase")]
    ListPendingInteractions {
        params: RemoteDispatchListPendingInteractionsParams,
    },
    #[serde(rename_all = "camelCase")]
    ReadPendingInteraction {
        params: RemoteDispatchReadPendingInteractionParams,
    },
    #[serde(rename_all = "camelCase")]
    RespondInteraction {
        params: RemoteDispatchRespondInteractionParams,
    },
    #[serde(rename_all = "camelCase")]
    MailboxReceipts {
        params: RemoteDispatchMailboxReceiptsParams,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", export_to = "v2/")]
pub enum RemoteDispatchOperationResult {
    #[serde(rename_all = "camelCase")]
    EnqueueInstruction {
        result: RemoteDispatchEnqueueInstructionResult,
    },
    #[serde(rename_all = "camelCase")]
    ListPendingInteractions {
        result: RemoteDispatchListPendingInteractionsResult,
    },
    #[serde(rename_all = "camelCase")]
    ReadPendingInteraction {
        result: RemoteDispatchReadPendingInteractionResult,
    },
    #[serde(rename_all = "camelCase")]
    RespondInteraction {
        result: RemoteDispatchRespondInteractionResult,
    },
    #[serde(rename_all = "camelCase")]
    MailboxReceipts {
        result: RemoteDispatchMailboxReceiptsResult,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchReceipt {
    pub receipt_id: String,
    pub request_id: String,
    pub source_machine_id: String,
    pub target_machine_id: String,
    pub idempotency_key: String,
    pub operation_kind: RemoteDispatchOperationKind,
    pub status: RemoteDispatchRequestStatus,
    pub denial: Option<RemoteDispatchDenial>,
    pub mailbox_message_id: Option<String>,
    pub pending_interaction_id: Option<String>,
    pub payload_sha256: String,
    pub payload_preview: String,
    pub redactions: Vec<RemoteDispatchRedaction>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct RemoteDispatchDenial {
    pub reason: RemoteDispatchDenialReason,
    pub operation_class: Option<RemoteDispatchDeniedOperationClass>,
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum RemoteDispatchCapability {
    ProtocolNegotiation,
    TrustAuthorization,
    DurableMailboxEnqueue,
    MailboxReceiptRead,
    PendingInteractionList,
    PendingInteractionRead,
    PendingInteractionRespond,
    Idempotency,
    AuditReceipts,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum RemoteDispatchOperationKind {
    EnqueueInstruction,
    ListPendingInteractions,
    ReadPendingInteraction,
    RespondInteraction,
    MailboxReceipts,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum RemoteDispatchRequestStatus {
    Accepted,
    Duplicate,
    Denied,
    Unsupported,
    Failed,
    DryRun,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum RemoteDispatchDenialReason {
    UnknownMachine,
    UntrustedMachine,
    DisabledMachine,
    CapabilityMismatch,
    OperationNotAllowed,
    OperationClassDenied,
    StaleIdempotencyKey,
    ExpiredRequest,
    InvalidTarget,
    TransportUnavailable,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum RemoteDispatchDeniedOperationClass {
    Shell,
    CommandExec,
    Process,
    Filesystem,
    Mcp,
    Config,
    Auth,
    Plugin,
    WorkflowMutation,
    RawClientRequest,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum RemoteDispatchRedaction {
    Credential,
    EndpointAddress,
    OperationPayload,
    MachineAddress,
}
