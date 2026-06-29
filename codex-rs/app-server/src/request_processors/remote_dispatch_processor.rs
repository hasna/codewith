use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::MachineRegistryTrustState;
use codex_app_server_protocol::MissionControlDeliveryPolicy;
use codex_app_server_protocol::RemoteDispatchCapability;
use codex_app_server_protocol::RemoteDispatchDenial;
use codex_app_server_protocol::RemoteDispatchDenialReason;
use codex_app_server_protocol::RemoteDispatchDeniedOperationClass;
use codex_app_server_protocol::RemoteDispatchEnqueueInstructionParams;
use codex_app_server_protocol::RemoteDispatchEnqueueInstructionResult;
use codex_app_server_protocol::RemoteDispatchListPendingInteractionsParams;
use codex_app_server_protocol::RemoteDispatchListPendingInteractionsResult;
use codex_app_server_protocol::RemoteDispatchMailboxReceiptsParams;
use codex_app_server_protocol::RemoteDispatchMailboxReceiptsResult;
use codex_app_server_protocol::RemoteDispatchNegotiateParams;
use codex_app_server_protocol::RemoteDispatchNegotiateResponse;
use codex_app_server_protocol::RemoteDispatchOperation;
use codex_app_server_protocol::RemoteDispatchOperationKind;
use codex_app_server_protocol::RemoteDispatchOperationResult;
use codex_app_server_protocol::RemoteDispatchReadPendingInteractionParams;
use codex_app_server_protocol::RemoteDispatchReadPendingInteractionResult;
use codex_app_server_protocol::RemoteDispatchReceipt;
use codex_app_server_protocol::RemoteDispatchReceiptReadParams;
use codex_app_server_protocol::RemoteDispatchRedaction;
use codex_app_server_protocol::RemoteDispatchRequestStatus;
use codex_app_server_protocol::RemoteDispatchRespondInteractionParams;
use codex_app_server_protocol::RemoteDispatchRespondInteractionResult;
use codex_app_server_protocol::RemoteDispatchSubmitParams;
use codex_app_server_protocol::RemoteDispatchSubmitResponse;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_protocol::ThreadId;
use codex_rollout::StateDbHandle;
use serde_json::json;

use super::thread_mailbox_context::validate_mailbox_payload_context_size;
use super::thread_mailbox_processor::mapping::api_mailbox_receipt;
use super::thread_mailbox_processor::mapping::api_mailbox_summary;
use super::thread_pending_interaction_processor::api_pending_interaction;
use super::thread_pending_interaction_processor::api_pending_interaction_event;
use super::thread_pending_interaction_processor::api_pending_interaction_kind_to_state;
use super::thread_pending_interaction_processor::api_pending_interaction_status_to_state;
use super::thread_pending_interaction_processor::api_pending_interaction_terminal_status_to_state;
use super::thread_pending_interaction_processor::read_pending_interaction;
use super::thread_pending_interaction_processor::redacted_response_payload;
use super::thread_pending_interaction_processor::validate_response_matches_interaction;
use super::thread_pending_interaction_processor::validate_response_status_matches_payload;

const DEFAULT_REMOTE_DISPATCH_MAX_ATTEMPTS: u32 = 10;
const MAX_REMOTE_DISPATCH_MAX_ATTEMPTS: u32 = 25;
const REMOTE_DISPATCH_PREVIEW_CHARS: usize = 240;
const REMOTE_DISPATCH_CAPABILITY_VERSION: &str = "1";

#[derive(Clone)]
pub(crate) struct RemoteDispatchRequestProcessor {
    state_db: Option<StateDbHandle>,
}

impl RemoteDispatchRequestProcessor {
    pub(crate) fn new(state_db: Option<StateDbHandle>) -> Self {
        Self { state_db }
    }

    pub(crate) async fn negotiate(
        &self,
        params: RemoteDispatchNegotiateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let Some(state_db) = self.state_db.as_ref() else {
            return Ok(Some(
                negotiation_response(/*local_machine_id*/ None, /*allowed*/ false).into(),
            ));
        };
        let local_machine = local_machine(state_db).await?;
        let local_machine_id = local_machine
            .as_ref()
            .map(|machine| machine.machine_id.clone());
        let allowed = match (
            params.source_machine_id.as_deref(),
            params.target_machine_id.as_deref(),
            local_machine.as_ref(),
        ) {
            (Some(source_machine_id), Some(target_machine_id), Some(local_machine)) => {
                target_machine_id == local_machine.machine_id
                    && trusted_machine(state_db, source_machine_id)
                        .await?
                        .is_some()
            }
            _ => false,
        };
        Ok(Some(negotiation_response(local_machine_id, allowed).into()))
    }

    pub(crate) async fn submit(
        &self,
        params: RemoteDispatchSubmitParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let operation_kind = operation_kind(&params.operation);
        if let Some(denial) = self.gate_submit(&params).await? {
            return Ok(Some(
                RemoteDispatchSubmitResponse {
                    request_id: params.request_id.clone(),
                    status: RemoteDispatchRequestStatus::Denied,
                    receipt: remote_dispatch_receipt(
                        &params,
                        operation_kind,
                        RemoteDispatchRequestStatus::Denied,
                        Some(denial),
                        /*mailbox_message_id*/ None,
                    ),
                    result: None,
                }
                .into(),
            ));
        }
        match &params.operation {
            RemoteDispatchOperation::EnqueueInstruction { params: operation } => {
                self.submit_enqueue_instruction(&params, operation).await
            }
            RemoteDispatchOperation::ListPendingInteractions { params: operation } => {
                self.submit_list_pending_interactions(&params, operation)
                    .await
            }
            RemoteDispatchOperation::ReadPendingInteraction { params: operation } => {
                self.submit_read_pending_interaction(&params, operation)
                    .await
            }
            RemoteDispatchOperation::RespondInteraction { params: operation } => {
                self.submit_respond_interaction(&params, operation).await
            }
            RemoteDispatchOperation::MailboxReceipts { params: operation } => {
                self.submit_mailbox_receipts(&params, operation).await
            }
        }
    }

    pub(crate) async fn receipt_read(
        &self,
        _params: RemoteDispatchReceiptReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(invalid_request(
            "remote dispatch receipts are not enabled until receipt persistence is installed",
        ))
    }

    async fn gate_submit(
        &self,
        params: &RemoteDispatchSubmitParams,
    ) -> Result<Option<RemoteDispatchDenial>, JSONRPCErrorError> {
        if let Some(requested_at) = params.requested_at
            && chrono::DateTime::<chrono::Utc>::from_timestamp(requested_at, /*nsecs*/ 0).is_none()
        {
            return Err(invalid_request(
                "requestedAt must be a valid Unix timestamp",
            ));
        }
        if let Some(expires_at) = params.expires_at {
            if chrono::DateTime::<chrono::Utc>::from_timestamp(expires_at, /*nsecs*/ 0).is_none() {
                return Err(invalid_request("expiresAt must be a valid Unix timestamp"));
            }
            if expires_at <= chrono::Utc::now().timestamp() {
                return Ok(Some(denial(
                    RemoteDispatchDenialReason::ExpiredRequest,
                    "remote dispatch request has expired",
                )));
            }
        }
        let Some(state_db) = self.state_db.as_ref() else {
            return Ok(Some(denial(
                RemoteDispatchDenialReason::TransportUnavailable,
                "machine registry is unavailable for remote dispatch",
            )));
        };
        if let Some(capability_version) = params.capability_version.as_deref()
            && capability_version != REMOTE_DISPATCH_CAPABILITY_VERSION
        {
            return Ok(Some(denial(
                RemoteDispatchDenialReason::CapabilityMismatch,
                "remote dispatch capability version is not supported",
            )));
        }
        let Some(source_machine) = state_db
            .machine_registry()
            .get_machine(params.source_machine_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to read remote dispatch source machine: {err}"
                ))
            })?
        else {
            return Ok(Some(denial(
                RemoteDispatchDenialReason::UnknownMachine,
                "remote dispatch source machine is not registered",
            )));
        };
        if !matches!(
            source_machine.trust_state,
            codex_state::MachineTrustState::Local | codex_state::MachineTrustState::Trusted
        ) {
            return Ok(Some(denial(
                RemoteDispatchDenialReason::UntrustedMachine,
                "remote dispatch source machine is not trusted",
            )));
        }
        let Some(target_machine) = state_db
            .machine_registry()
            .get_machine(params.target_machine_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to read remote dispatch target machine: {err}"
                ))
            })?
        else {
            return Ok(Some(denial(
                RemoteDispatchDenialReason::UnknownMachine,
                "remote dispatch target machine is not registered",
            )));
        };
        match target_machine.trust_state {
            codex_state::MachineTrustState::Local => Ok(None),
            codex_state::MachineTrustState::Disabled | codex_state::MachineTrustState::Revoked => {
                Ok(Some(denial(
                    RemoteDispatchDenialReason::DisabledMachine,
                    "remote dispatch target machine is disabled",
                )))
            }
            codex_state::MachineTrustState::Trusted | codex_state::MachineTrustState::Untrusted => {
                Ok(Some(denial(
                    RemoteDispatchDenialReason::InvalidTarget,
                    "remote dispatch target machine is not the local machine",
                )))
            }
        }
    }

    async fn submit_enqueue_instruction(
        &self,
        submit_params: &RemoteDispatchSubmitParams,
        operation: &RemoteDispatchEnqueueInstructionParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let text = validate_required_text("remote dispatch message", operation.message.as_str())?;
        let delivery_policy = if operation.resume {
            MissionControlDeliveryPolicy::ResumeAndTrigger
        } else {
            MissionControlDeliveryPolicy::LiveOnly
        };
        validate_mailbox_payload_context_size(&json!({ "text": text.as_str() }))
            .map_err(invalid_request)?;
        let preview = truncate_preview(text.as_str());
        if submit_params.dry_run {
            return Ok(Some(
                RemoteDispatchSubmitResponse {
                    request_id: submit_params.request_id.clone(),
                    status: RemoteDispatchRequestStatus::DryRun,
                    receipt: remote_dispatch_receipt(
                        submit_params,
                        RemoteDispatchOperationKind::EnqueueInstruction,
                        RemoteDispatchRequestStatus::DryRun,
                        /*denial*/ None,
                        /*mailbox_message_id*/ None,
                    ),
                    result: Some(RemoteDispatchOperationResult::EnqueueInstruction {
                        result: RemoteDispatchEnqueueInstructionResult {
                            delivery_policy,
                            preview,
                            message: None,
                            created: None,
                        },
                    }),
                }
                .into(),
            ));
        }
        let state_db = self.state_db.as_ref().ok_or_else(|| {
            internal_error("remote dispatch state store disappeared after gate evaluation")
        })?;
        let target_thread_id = ThreadId::from_string(operation.target_thread_id.as_str())
            .map_err(|err| invalid_request(format!("invalid targetThreadId: {err}")))?;
        let sender_thread_id = operation
            .sender_thread_id
            .as_deref()
            .map(ThreadId::from_string)
            .transpose()
            .map_err(|err| invalid_request(format!("invalid senderThreadId: {err}")))?;
        let max_attempts = operation
            .max_attempts
            .unwrap_or(DEFAULT_REMOTE_DISPATCH_MAX_ATTEMPTS);
        if max_attempts == 0 || max_attempts > MAX_REMOTE_DISPATCH_MAX_ATTEMPTS {
            return Err(invalid_request(format!(
                "remote dispatch maxAttempts must be between 1 and {MAX_REMOTE_DISPATCH_MAX_ATTEMPTS}"
            )));
        }
        let expires_at = operation
            .expires_at
            .map(|timestamp| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp, /*nsecs*/ 0)
                    .ok_or_else(|| invalid_request("expiresAt must be a valid Unix timestamp"))
            })
            .transpose()?;
        let payload = remote_instruction_payload(
            text,
            delivery_policy,
            submit_params.request_id.as_str(),
            submit_params.source_machine_id.as_str(),
        );
        let sender_label = operation
            .sender_label
            .clone()
            .unwrap_or_else(|| format!("remote:{}", submit_params.source_machine_id));
        let outcome = state_db
            .mailbox_messages()
            .enqueue_message(codex_state::MailboxEnqueueParams {
                target_thread_id,
                sender_thread_id,
                sender_label: Some(sender_label),
                idempotency_key: Some(remote_mailbox_idempotency_key(submit_params)),
                kind: codex_state::MailboxMessageKind::UserInstruction,
                payload_json: payload,
                payload_preview: preview.clone(),
                priority: operation.priority.unwrap_or(0),
                max_attempts: i64::from(max_attempts),
                next_attempt_at: None,
                expires_at,
            })
            .await
            .map_err(|err| internal_error(format!("failed to enqueue remote dispatch: {err}")))?;
        let message = api_mailbox_summary(outcome.message);
        let mailbox_message_id = Some(message.message_id.clone());
        Ok(Some(
            RemoteDispatchSubmitResponse {
                request_id: submit_params.request_id.clone(),
                status: RemoteDispatchRequestStatus::Accepted,
                receipt: remote_dispatch_receipt(
                    submit_params,
                    RemoteDispatchOperationKind::EnqueueInstruction,
                    RemoteDispatchRequestStatus::Accepted,
                    /*denial*/ None,
                    mailbox_message_id,
                ),
                result: Some(RemoteDispatchOperationResult::EnqueueInstruction {
                    result: RemoteDispatchEnqueueInstructionResult {
                        delivery_policy,
                        preview,
                        message: Some(message),
                        created: Some(outcome.created),
                    },
                }),
            }
            .into(),
        ))
    }

    async fn submit_list_pending_interactions(
        &self,
        submit_params: &RemoteDispatchSubmitParams,
        operation: &RemoteDispatchListPendingInteractionsParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db_after_gate()?;
        let thread_id = operation
            .thread_id
            .as_deref()
            .map(parse_thread_id)
            .transpose()?;
        if let Some(thread_id) = thread_id {
            ensure_state_thread_exists(state_db.as_ref(), thread_id).await?;
        }
        let page = state_db
            .list_thread_pending_interactions(codex_state::PendingInteractionListParams {
                thread_id,
                statuses: operation
                    .statuses
                    .clone()
                    .unwrap_or_else(|| {
                        vec![
                            ThreadPendingInteractionStatus::Pending,
                            ThreadPendingInteractionStatus::Delivered,
                        ]
                    })
                    .into_iter()
                    .map(api_pending_interaction_status_to_state)
                    .collect(),
                kinds: operation
                    .kinds
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(api_pending_interaction_kind_to_state)
                    .collect(),
                cursor: operation.cursor.clone(),
                limit: operation
                    .limit
                    .unwrap_or(codex_state::DEFAULT_PENDING_INTERACTION_LIST_LIMIT),
            })
            .await
            .map_err(|err| internal_error(format!("failed to list pending interactions: {err}")))?;
        Ok(Some(
            RemoteDispatchSubmitResponse {
                request_id: submit_params.request_id.clone(),
                status: RemoteDispatchRequestStatus::Accepted,
                receipt: remote_dispatch_receipt(
                    submit_params,
                    RemoteDispatchOperationKind::ListPendingInteractions,
                    RemoteDispatchRequestStatus::Accepted,
                    /*denial*/ None,
                    /*mailbox_message_id*/ None,
                ),
                result: Some(RemoteDispatchOperationResult::ListPendingInteractions {
                    result: RemoteDispatchListPendingInteractionsResult {
                        data: page.data.into_iter().map(api_pending_interaction).collect(),
                        next_cursor: page.next_cursor,
                    },
                }),
            }
            .into(),
        ))
    }

    async fn submit_read_pending_interaction(
        &self,
        submit_params: &RemoteDispatchSubmitParams,
        operation: &RemoteDispatchReadPendingInteractionParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db_after_gate()?;
        let thread_id = operation
            .thread_id
            .as_deref()
            .map(parse_thread_id)
            .transpose()?;
        let interaction = read_pending_interaction(
            state_db.as_ref(),
            operation.interaction_id.as_str(),
            thread_id,
        )
        .await?;
        let events = state_db
            .list_thread_pending_interaction_events(interaction.interaction_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to read pending interaction events: {err}"))
            })?;
        Ok(Some(
            RemoteDispatchSubmitResponse {
                request_id: submit_params.request_id.clone(),
                status: RemoteDispatchRequestStatus::Accepted,
                receipt: remote_dispatch_receipt_with_links(
                    submit_params,
                    RemoteDispatchOperationKind::ReadPendingInteraction,
                    RemoteDispatchRequestStatus::Accepted,
                    /*denial*/ None,
                    /*mailbox_message_id*/ None,
                    Some(interaction.interaction_id.clone()),
                ),
                result: Some(RemoteDispatchOperationResult::ReadPendingInteraction {
                    result: RemoteDispatchReadPendingInteractionResult {
                        interaction: api_pending_interaction(interaction),
                        events: events
                            .into_iter()
                            .map(api_pending_interaction_event)
                            .collect(),
                    },
                }),
            }
            .into(),
        ))
    }

    async fn submit_respond_interaction(
        &self,
        submit_params: &RemoteDispatchSubmitParams,
        operation: &RemoteDispatchRespondInteractionParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db_after_gate()?;
        let thread_id = operation
            .thread_id
            .as_deref()
            .map(parse_thread_id)
            .transpose()?;
        let interaction = read_pending_interaction(
            state_db.as_ref(),
            operation.interaction_id.as_str(),
            thread_id,
        )
        .await?;
        validate_response_matches_interaction(interaction.kind, &operation.response)?;
        validate_response_status_matches_payload(&operation.response, operation.terminal_status)?;
        if interaction.server_request_id_json.is_some() {
            return Err(invalid_request(
                "pending interaction is tied to a live client request; use the app-server pending-interaction response path so the waiting client is notified",
            ));
        }
        if submit_params.dry_run {
            return Ok(Some(
                RemoteDispatchSubmitResponse {
                    request_id: submit_params.request_id.clone(),
                    status: RemoteDispatchRequestStatus::DryRun,
                    receipt: remote_dispatch_receipt_with_links(
                        submit_params,
                        RemoteDispatchOperationKind::RespondInteraction,
                        RemoteDispatchRequestStatus::DryRun,
                        /*denial*/ None,
                        /*mailbox_message_id*/ None,
                        Some(interaction.interaction_id.clone()),
                    ),
                    result: Some(RemoteDispatchOperationResult::RespondInteraction {
                        result: RemoteDispatchRespondInteractionResult {
                            updated: false,
                            interaction: Some(api_pending_interaction(interaction)),
                        },
                    }),
                }
                .into(),
            ));
        }
        let stored_response = redacted_response_payload(&operation.response);
        let updated = state_db
            .respond_thread_pending_interaction(&codex_state::PendingInteractionRespondParams {
                interaction_id: interaction.interaction_id.clone(),
                response_payload_json: stored_response.payload,
                response_payload_preview: stored_response.preview,
                response_redactions_json: json!(stored_response.redactions),
                terminal_status: api_pending_interaction_terminal_status_to_state(
                    operation.terminal_status,
                ),
            })
            .await
            .map_err(|err| {
                internal_error(format!("failed to respond to pending interaction: {err}"))
            })?;
        let interaction = state_db
            .get_thread_pending_interaction(interaction.interaction_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to reload pending interaction: {err}")))?
            .map(api_pending_interaction);
        Ok(Some(
            RemoteDispatchSubmitResponse {
                request_id: submit_params.request_id.clone(),
                status: RemoteDispatchRequestStatus::Accepted,
                receipt: remote_dispatch_receipt_with_links(
                    submit_params,
                    RemoteDispatchOperationKind::RespondInteraction,
                    RemoteDispatchRequestStatus::Accepted,
                    /*denial*/ None,
                    /*mailbox_message_id*/ None,
                    Some(operation.interaction_id.clone()),
                ),
                result: Some(RemoteDispatchOperationResult::RespondInteraction {
                    result: RemoteDispatchRespondInteractionResult {
                        updated,
                        interaction,
                    },
                }),
            }
            .into(),
        ))
    }

    async fn submit_mailbox_receipts(
        &self,
        submit_params: &RemoteDispatchSubmitParams,
        operation: &RemoteDispatchMailboxReceiptsParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db_after_gate()?;
        let target_thread_id = parse_thread_id(operation.target_thread_id.as_str())?;
        ensure_state_thread_exists(state_db.as_ref(), target_thread_id).await?;
        let message = state_db
            .mailbox_messages()
            .get_message(operation.message_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to read mailbox message: {err}")))?
            .ok_or_else(|| {
                invalid_request(format!(
                    "mailbox message not found: {}",
                    operation.message_id
                ))
            })?;
        if message.target_thread_id != target_thread_id {
            return Err(invalid_request(format!(
                "mailbox message not found: {}",
                operation.message_id
            )));
        }
        let receipts = state_db
            .mailbox_messages()
            .list_receipts(operation.message_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to list mailbox receipts: {err}")))?;
        Ok(Some(
            RemoteDispatchSubmitResponse {
                request_id: submit_params.request_id.clone(),
                status: RemoteDispatchRequestStatus::Accepted,
                receipt: remote_dispatch_receipt(
                    submit_params,
                    RemoteDispatchOperationKind::MailboxReceipts,
                    RemoteDispatchRequestStatus::Accepted,
                    /*denial*/ None,
                    Some(message.message_id),
                ),
                result: Some(RemoteDispatchOperationResult::MailboxReceipts {
                    result: RemoteDispatchMailboxReceiptsResult {
                        data: receipts.into_iter().map(api_mailbox_receipt).collect(),
                    },
                }),
            }
            .into(),
        ))
    }

    fn state_db_after_gate(&self) -> Result<&StateDbHandle, JSONRPCErrorError> {
        self.state_db.as_ref().ok_or_else(|| {
            internal_error("remote dispatch state store disappeared after gate evaluation")
        })
    }
}

fn negotiation_response(
    local_machine_id: Option<String>,
    allowed: bool,
) -> RemoteDispatchNegotiateResponse {
    RemoteDispatchNegotiateResponse {
        protocol_version: 1,
        required_trust_state: MachineRegistryTrustState::Trusted,
        supported_capabilities: allowed.then(supported_capabilities).unwrap_or_default(),
        supported_operations: allowed.then(supported_operations).unwrap_or_default(),
        denied_operation_classes: denied_operation_classes(),
        local_machine_id,
    }
}

fn supported_capabilities() -> Vec<RemoteDispatchCapability> {
    vec![
        RemoteDispatchCapability::ProtocolNegotiation,
        RemoteDispatchCapability::TrustAuthorization,
        RemoteDispatchCapability::DurableMailboxEnqueue,
        RemoteDispatchCapability::PendingInteractionList,
        RemoteDispatchCapability::PendingInteractionRead,
        RemoteDispatchCapability::PendingInteractionRespond,
        RemoteDispatchCapability::Idempotency,
    ]
}

fn supported_operations() -> Vec<RemoteDispatchOperationKind> {
    vec![
        RemoteDispatchOperationKind::EnqueueInstruction,
        RemoteDispatchOperationKind::ListPendingInteractions,
        RemoteDispatchOperationKind::ReadPendingInteraction,
        RemoteDispatchOperationKind::RespondInteraction,
        RemoteDispatchOperationKind::MailboxReceipts,
    ]
}

fn denied_operation_classes() -> Vec<RemoteDispatchDeniedOperationClass> {
    vec![
        RemoteDispatchDeniedOperationClass::Shell,
        RemoteDispatchDeniedOperationClass::CommandExec,
        RemoteDispatchDeniedOperationClass::Process,
        RemoteDispatchDeniedOperationClass::Filesystem,
        RemoteDispatchDeniedOperationClass::Mcp,
        RemoteDispatchDeniedOperationClass::Config,
        RemoteDispatchDeniedOperationClass::Auth,
        RemoteDispatchDeniedOperationClass::Plugin,
        RemoteDispatchDeniedOperationClass::WorkflowMutation,
        RemoteDispatchDeniedOperationClass::RawClientRequest,
        RemoteDispatchDeniedOperationClass::Unknown,
    ]
}

async fn local_machine(
    state_db: &StateDbHandle,
) -> Result<Option<codex_state::MachineRecord>, JSONRPCErrorError> {
    let page = state_db
        .machine_registry()
        .list_machines(codex_state::MachineRegistryListParams {
            include_disabled: false,
            include_forgotten: false,
            cursor: None,
            limit: codex_state::MAX_MACHINE_REGISTRY_LIST_LIMIT,
        })
        .await
        .map_err(|err| internal_error(format!("failed to list local machine registry: {err}")))?;
    Ok(page
        .data
        .into_iter()
        .find(|machine| machine.trust_state == codex_state::MachineTrustState::Local))
}

async fn trusted_machine(
    state_db: &StateDbHandle,
    machine_id: &str,
) -> Result<Option<codex_state::MachineRecord>, JSONRPCErrorError> {
    let machine = state_db
        .machine_registry()
        .get_machine(machine_id)
        .await
        .map_err(|err| internal_error(format!("failed to read trusted machine registry: {err}")))?;
    Ok(machine.filter(|machine| {
        matches!(
            machine.trust_state,
            codex_state::MachineTrustState::Local | codex_state::MachineTrustState::Trusted
        )
    }))
}

fn denial(reason: RemoteDispatchDenialReason, message: &str) -> RemoteDispatchDenial {
    RemoteDispatchDenial {
        reason,
        operation_class: None,
        message: message.to_string(),
    }
}

fn validate_required_text(field_name: &str, value: &str) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request(format!("{field_name} must not be empty")));
    }
    Ok(value.to_string())
}

fn truncate_preview(text: &str) -> String {
    if text.chars().count() <= REMOTE_DISPATCH_PREVIEW_CHARS {
        return text.to_string();
    }
    text.chars()
        .take(REMOTE_DISPATCH_PREVIEW_CHARS.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

fn remote_instruction_payload(
    text: String,
    delivery_policy: MissionControlDeliveryPolicy,
    request_id: &str,
    source_machine_id: &str,
) -> serde_json::Value {
    let remote_dispatch = json!({
        "requestId": request_id,
        "sourceMachineId": source_machine_id,
    });
    match delivery_policy {
        MissionControlDeliveryPolicy::LiveOnly => {
            json!({ "text": text, "remoteDispatch": remote_dispatch })
        }
        MissionControlDeliveryPolicy::ResumeAndTrigger => {
            json!({
                "text": text,
                "delivery": "resumeAndTrigger",
                "remoteDispatch": remote_dispatch
            })
        }
    }
}

fn remote_mailbox_idempotency_key(params: &RemoteDispatchSubmitParams) -> String {
    format!(
        "remote:{}:{}",
        params.source_machine_id, params.idempotency_key
    )
}

fn parse_thread_id(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}

async fn ensure_state_thread_exists(
    state_db: &codex_state::StateRuntime,
    thread_id: ThreadId,
) -> Result<(), JSONRPCErrorError> {
    state_db
        .get_thread(thread_id)
        .await
        .map_err(|err| internal_error(format!("failed to read thread metadata: {err}")))?
        .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?;
    Ok(())
}

fn operation_kind(operation: &RemoteDispatchOperation) -> RemoteDispatchOperationKind {
    match operation {
        RemoteDispatchOperation::EnqueueInstruction { .. } => {
            RemoteDispatchOperationKind::EnqueueInstruction
        }
        RemoteDispatchOperation::ListPendingInteractions { .. } => {
            RemoteDispatchOperationKind::ListPendingInteractions
        }
        RemoteDispatchOperation::ReadPendingInteraction { .. } => {
            RemoteDispatchOperationKind::ReadPendingInteraction
        }
        RemoteDispatchOperation::RespondInteraction { .. } => {
            RemoteDispatchOperationKind::RespondInteraction
        }
        RemoteDispatchOperation::MailboxReceipts { .. } => {
            RemoteDispatchOperationKind::MailboxReceipts
        }
    }
}

fn remote_dispatch_receipt(
    params: &RemoteDispatchSubmitParams,
    operation_kind: RemoteDispatchOperationKind,
    status: RemoteDispatchRequestStatus,
    denial: Option<RemoteDispatchDenial>,
    mailbox_message_id: Option<String>,
) -> RemoteDispatchReceipt {
    remote_dispatch_receipt_with_links(
        params,
        operation_kind,
        status,
        denial,
        mailbox_message_id,
        /*pending_interaction_id*/ None,
    )
}

fn remote_dispatch_receipt_with_links(
    params: &RemoteDispatchSubmitParams,
    operation_kind: RemoteDispatchOperationKind,
    status: RemoteDispatchRequestStatus,
    denial: Option<RemoteDispatchDenial>,
    mailbox_message_id: Option<String>,
    pending_interaction_id: Option<String>,
) -> RemoteDispatchReceipt {
    let now = chrono::Utc::now().timestamp();
    RemoteDispatchReceipt {
        receipt_id: uuid::Uuid::now_v7().to_string(),
        request_id: params.request_id.clone(),
        source_machine_id: params.source_machine_id.clone(),
        target_machine_id: params.target_machine_id.clone(),
        idempotency_key: params.idempotency_key.clone(),
        operation_kind,
        status,
        denial,
        mailbox_message_id,
        pending_interaction_id,
        payload_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        payload_preview: format!("{operation_kind:?}"),
        redactions: vec![RemoteDispatchRedaction::OperationPayload],
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ClientResponsePayload;
    use codex_app_server_protocol::RemoteDispatchEnqueueInstructionParams;
    use codex_app_server_protocol::RemoteDispatchListPendingInteractionsParams;
    use codex_app_server_protocol::RemoteDispatchMailboxReceiptsParams;
    use codex_app_server_protocol::RemoteDispatchOperation;
    use codex_app_server_protocol::RemoteDispatchOperationKind;
    use codex_app_server_protocol::RemoteDispatchOperationResult;
    use codex_app_server_protocol::RemoteDispatchReadPendingInteractionParams;
    use codex_app_server_protocol::RemoteDispatchRespondInteractionParams;
    use codex_app_server_protocol::RemoteDispatchSubmitParams;
    use codex_app_server_protocol::ThreadMailboxReceiptKind;
    use codex_app_server_protocol::ThreadPendingInteractionResponsePayload;
    use codex_app_server_protocol::ThreadPendingInteractionStatus;
    use codex_app_server_protocol::ThreadPendingInteractionTerminalStatus;
    use codex_app_server_protocol::ToolRequestUserInputAnswer;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;

    const TARGET_THREAD_ID: &str = "00000000-0000-0000-0000-000000000001";

    #[tokio::test]
    async fn negotiate_fails_closed_before_transport_gates_exist() {
        let response = RemoteDispatchRequestProcessor::new(/*state_db*/ None)
            .negotiate(RemoteDispatchNegotiateParams::default())
            .await
            .expect("negotiate should succeed")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchNegotiate(response) = response else {
            panic!("expected remote dispatch negotiate response");
        };

        assert_eq!(
            Vec::<RemoteDispatchOperationKind>::new(),
            response.supported_operations
        );
        assert!(response.supported_capabilities.is_empty());
        assert_eq!(
            denied_operation_classes(),
            response.denied_operation_classes
        );
    }

    #[tokio::test]
    async fn negotiate_advertises_gate_capabilities_for_trusted_source_to_local_target() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;

        let response = RemoteDispatchRequestProcessor::new(Some(state_db))
            .negotiate(RemoteDispatchNegotiateParams {
                source_machine_id: Some("trusted".to_string()),
                target_machine_id: Some("local".to_string()),
                protocol_version: Some(1),
                requested_capabilities: None,
            })
            .await
            .expect("negotiate should succeed")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchNegotiate(response) = response else {
            panic!("expected remote dispatch negotiate response");
        };

        assert_eq!(supported_capabilities(), response.supported_capabilities);
        assert!(
            !response
                .supported_capabilities
                .contains(&RemoteDispatchCapability::MailboxReceiptRead)
        );
        assert!(
            !response
                .supported_capabilities
                .contains(&RemoteDispatchCapability::AuditReceipts)
        );
        assert_eq!(supported_operations(), response.supported_operations);
        assert_eq!(Some("local".to_string()), response.local_machine_id);
    }

    #[tokio::test]
    async fn negotiate_reports_denied_unsafe_operation_classes() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;

        let response = RemoteDispatchRequestProcessor::new(Some(state_db))
            .negotiate(RemoteDispatchNegotiateParams {
                source_machine_id: Some("trusted".to_string()),
                target_machine_id: Some("local".to_string()),
                protocol_version: Some(1),
                requested_capabilities: None,
            })
            .await
            .expect("negotiate should succeed")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchNegotiate(response) = response else {
            panic!("expected remote dispatch negotiate response");
        };

        for operation_class in [
            RemoteDispatchDeniedOperationClass::Shell,
            RemoteDispatchDeniedOperationClass::CommandExec,
            RemoteDispatchDeniedOperationClass::Filesystem,
            RemoteDispatchDeniedOperationClass::Mcp,
            RemoteDispatchDeniedOperationClass::Config,
            RemoteDispatchDeniedOperationClass::Auth,
            RemoteDispatchDeniedOperationClass::Plugin,
            RemoteDispatchDeniedOperationClass::WorkflowMutation,
            RemoteDispatchDeniedOperationClass::RawClientRequest,
        ] {
            assert!(
                response.denied_operation_classes.contains(&operation_class),
                "missing denied remote operation class {operation_class:?}"
            );
        }
    }

    #[tokio::test]
    async fn submit_without_state_db_fails_as_transport_unavailable() {
        let response = RemoteDispatchRequestProcessor::new(/*state_db*/ None)
            .submit(test_submit_params("trusted", "local"))
            .await
            .expect("submit should return structured denial")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchSubmit(response) = response else {
            panic!("expected remote dispatch submit response");
        };

        assert_eq!(RemoteDispatchRequestStatus::Denied, response.status);
        assert_eq!(
            Some(RemoteDispatchDenialReason::TransportUnavailable),
            response.receipt.denial.map(|denial| denial.reason)
        );
    }

    #[tokio::test]
    async fn submit_denies_untrusted_source_with_structured_receipt() {
        let state_db = test_state_db().await;
        upsert_machine(
            &state_db,
            "local",
            codex_state::MachineTrustState::Local,
            codex_state::MachineEnrollmentState::Local,
        )
        .await;
        upsert_machine(
            &state_db,
            "untrusted",
            codex_state::MachineTrustState::Untrusted,
            codex_state::MachineEnrollmentState::Manual,
        )
        .await;

        let response = RemoteDispatchRequestProcessor::new(Some(state_db))
            .submit(test_submit_params("untrusted", "local"))
            .await
            .expect("submit should return structured denial")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchSubmit(response) = response else {
            panic!("expected remote dispatch submit response");
        };

        assert_eq!(RemoteDispatchRequestStatus::Denied, response.status);
        assert_eq!(
            Some(RemoteDispatchDenialReason::UntrustedMachine),
            response.receipt.denial.map(|denial| denial.reason)
        );
        assert_eq!(RemoteDispatchRequestStatus::Denied, response.receipt.status);
    }

    #[tokio::test]
    async fn submit_denies_non_local_target_with_structured_receipt() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_machine(
            &state_db,
            "other",
            codex_state::MachineTrustState::Trusted,
            codex_state::MachineEnrollmentState::Manual,
        )
        .await;

        let response = RemoteDispatchRequestProcessor::new(Some(state_db))
            .submit(test_submit_params("trusted", "other"))
            .await
            .expect("submit should return structured denial")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchSubmit(response) = response else {
            panic!("expected remote dispatch submit response");
        };

        assert_eq!(RemoteDispatchRequestStatus::Denied, response.status);
        assert_eq!(
            Some(RemoteDispatchDenialReason::InvalidTarget),
            response.receipt.denial.map(|denial| denial.reason)
        );
    }

    #[tokio::test]
    async fn submit_denies_stale_capability_version() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        let mut params = test_enqueue_submit_params("trusted", "local");
        params.capability_version = Some("0".to_string());

        let response = RemoteDispatchRequestProcessor::new(Some(state_db))
            .submit(params)
            .await
            .expect("submit should return structured denial")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchSubmit(response) = response else {
            panic!("expected remote dispatch submit response");
        };

        assert_eq!(RemoteDispatchRequestStatus::Denied, response.status);
        assert_eq!(
            Some(RemoteDispatchDenialReason::CapabilityMismatch),
            response.receipt.denial.map(|denial| denial.reason)
        );
    }

    #[tokio::test]
    async fn submit_denies_expired_request_with_structured_receipt() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        let mut params = test_enqueue_submit_params("trusted", "local");
        params.expires_at = Some(1);

        let response = RemoteDispatchRequestProcessor::new(Some(state_db))
            .submit(params)
            .await
            .expect("submit should return structured denial")
            .expect("response should be present");
        let ClientResponsePayload::RemoteDispatchSubmit(response) = response else {
            panic!("expected remote dispatch submit response");
        };

        assert_eq!(RemoteDispatchRequestStatus::Denied, response.status);
        assert_eq!(
            Some(RemoteDispatchDenialReason::ExpiredRequest),
            response.receipt.denial.map(|denial| denial.reason)
        );
        assert!(response.result.is_none());
    }

    #[tokio::test]
    async fn submit_mailbox_receipts_reads_durable_enqueue_receipt() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        let enqueue = expect_submit_response(
            RemoteDispatchRequestProcessor::new(Some(state_db.clone()))
                .submit(test_enqueue_submit_params("trusted", "local"))
                .await
                .expect("enqueue submit should succeed")
                .expect("response should be present"),
        );
        let message_id = enqueue
            .receipt
            .mailbox_message_id
            .expect("enqueue should link mailbox message");

        let response = expect_submit_response(
            RemoteDispatchRequestProcessor::new(Some(state_db))
                .submit(test_mailbox_receipts_submit_params(
                    "trusted",
                    "local",
                    message_id.as_str(),
                ))
                .await
                .expect("mailbox receipts submit should succeed")
                .expect("response should be present"),
        );

        assert_eq!(RemoteDispatchRequestStatus::Accepted, response.status);
        assert_eq!(Some(message_id), response.receipt.mailbox_message_id);
        let RemoteDispatchOperationResult::MailboxReceipts { result } = response
            .result
            .expect("mailbox receipts result should be present")
        else {
            panic!("expected mailbox receipts result");
        };
        assert_eq!(1, result.data.len());
        assert_eq!(ThreadMailboxReceiptKind::Enqueued, result.data[0].kind);
    }

    #[tokio::test]
    async fn submit_enqueue_instruction_writes_durable_mailbox_message() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        let response = expect_submit_response(
            RemoteDispatchRequestProcessor::new(Some(state_db.clone()))
                .submit(test_enqueue_submit_params("trusted", "local"))
                .await
                .expect("submit should succeed")
                .expect("response should be present"),
        );

        assert_eq!(RemoteDispatchRequestStatus::Accepted, response.status);
        assert_eq!(
            RemoteDispatchRequestStatus::Accepted,
            response.receipt.status
        );
        let RemoteDispatchOperationResult::EnqueueInstruction { result } =
            response.result.expect("enqueue result should be present")
        else {
            panic!("expected enqueue instruction result");
        };
        let message = result.message.expect("mailbox summary should be present");
        assert_eq!(Some(true), result.created);
        assert_eq!(
            Some(message.message_id.clone()),
            response.receipt.mailbox_message_id
        );
        assert_eq!("remote hello", message.payload_preview);
        assert_eq!(Some("remote:trusted".to_string()), message.sender_label);

        let target_thread_id =
            ThreadId::from_string(TARGET_THREAD_ID).expect("target thread id should parse");
        let page = state_db
            .mailbox_messages()
            .list_messages(codex_state::MailboxMessageStoreListParams {
                target_thread_id: Some(target_thread_id),
                statuses: Vec::new(),
                cursor: None,
                limit: 10,
            })
            .await
            .expect("mailbox messages should list");
        assert_eq!(1, page.data.len());
        assert_eq!(
            Some("remote hello"),
            page.data[0].payload_json["text"].as_str()
        );
        assert_eq!(
            Some("trusted"),
            page.data[0].payload_json["remoteDispatch"]["sourceMachineId"].as_str()
        );
    }

    #[tokio::test]
    async fn submit_enqueue_instruction_replays_remote_idempotency_key() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        let processor = RemoteDispatchRequestProcessor::new(Some(state_db));
        let first = expect_submit_response(
            processor
                .submit(test_enqueue_submit_params("trusted", "local"))
                .await
                .expect("first submit should succeed")
                .expect("response should be present"),
        );
        let second = expect_submit_response(
            processor
                .submit(test_enqueue_submit_params("trusted", "local"))
                .await
                .expect("second submit should succeed")
                .expect("response should be present"),
        );

        let first_message_id = first
            .receipt
            .mailbox_message_id
            .expect("first enqueue should link mailbox message");
        assert_eq!(RemoteDispatchRequestStatus::Accepted, second.status);
        assert_eq!(Some(first_message_id), second.receipt.mailbox_message_id);
        let RemoteDispatchOperationResult::EnqueueInstruction { result } =
            second.result.expect("enqueue result should be present")
        else {
            panic!("expected enqueue instruction result");
        };
        assert_eq!(Some(false), result.created);
    }

    #[tokio::test]
    async fn submit_list_pending_interactions_returns_remote_questions() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        create_pending_user_input_interaction(&state_db, "interaction-list").await;

        let response = expect_submit_response(
            RemoteDispatchRequestProcessor::new(Some(state_db))
                .submit(test_list_pending_submit_params("trusted", "local"))
                .await
                .expect("pending interaction list should succeed")
                .expect("response should be present"),
        );

        assert_eq!(RemoteDispatchRequestStatus::Accepted, response.status);
        let RemoteDispatchOperationResult::ListPendingInteractions { result } =
            response.result.expect("list result should be present")
        else {
            panic!("expected list pending interactions result");
        };
        assert_eq!(None, result.next_cursor);
        assert_eq!(1, result.data.len());
        assert_eq!("interaction-list", result.data[0].interaction_id);
        assert_eq!(
            ThreadPendingInteractionStatus::Pending,
            result.data[0].status
        );
    }

    #[tokio::test]
    async fn submit_read_pending_interaction_returns_events() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        create_pending_user_input_interaction(&state_db, "interaction-read").await;

        let response = expect_submit_response(
            RemoteDispatchRequestProcessor::new(Some(state_db))
                .submit(test_read_pending_submit_params(
                    "trusted",
                    "local",
                    "interaction-read",
                ))
                .await
                .expect("pending interaction read should succeed")
                .expect("response should be present"),
        );

        assert_eq!(RemoteDispatchRequestStatus::Accepted, response.status);
        assert_eq!(
            Some("interaction-read".to_string()),
            response.receipt.pending_interaction_id
        );
        let RemoteDispatchOperationResult::ReadPendingInteraction { result } =
            response.result.expect("read result should be present")
        else {
            panic!("expected read pending interaction result");
        };
        assert_eq!("interaction-read", result.interaction.interaction_id);
        assert_eq!(1, result.events.len());
    }

    #[tokio::test]
    async fn submit_respond_interaction_updates_pending_interaction_ledger() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        create_pending_user_input_interaction(&state_db, "interaction-respond").await;

        let response = expect_submit_response(
            RemoteDispatchRequestProcessor::new(Some(state_db.clone()))
                .submit(test_respond_pending_submit_params(
                    "trusted",
                    "local",
                    "interaction-respond",
                ))
                .await
                .expect("pending interaction response should succeed")
                .expect("response should be present"),
        );

        assert_eq!(RemoteDispatchRequestStatus::Accepted, response.status);
        assert_eq!(
            Some("interaction-respond".to_string()),
            response.receipt.pending_interaction_id
        );
        let RemoteDispatchOperationResult::RespondInteraction { result } =
            response.result.expect("respond result should be present")
        else {
            panic!("expected respond pending interaction result");
        };
        assert!(result.updated);
        let interaction = result.interaction.expect("interaction should reload");
        assert_eq!(
            ThreadPendingInteractionStatus::Responded,
            interaction.status
        );

        let stored = state_db
            .get_thread_pending_interaction("interaction-respond")
            .await
            .expect("pending interaction should load")
            .expect("pending interaction should exist");
        assert_eq!(
            Some(json!({
                "type": "requestUserInput",
                "answerCount": 1,
            })),
            stored.response_payload_json
        );
        assert_eq!(
            Some(json!(["responsePayload"])),
            stored.response_redactions_json
        );
    }

    #[tokio::test]
    async fn submit_respond_interaction_rejects_status_payload_mismatch() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        create_pending_user_input_interaction(&state_db, "interaction-status-mismatch").await;
        let mut params =
            test_respond_pending_submit_params("trusted", "local", "interaction-status-mismatch");
        let RemoteDispatchOperation::RespondInteraction {
            params: interaction_params,
        } = &mut params.operation
        else {
            panic!("expected respond interaction operation");
        };
        interaction_params.terminal_status = ThreadPendingInteractionTerminalStatus::Denied;

        let err = RemoteDispatchRequestProcessor::new(Some(state_db))
            .submit(params)
            .await
            .expect_err("status/payload mismatch should fail");

        assert!(
            err.message.contains("must be responded"),
            "unexpected error: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn submit_respond_interaction_rejects_live_client_requests() {
        let state_db = test_state_db().await;
        setup_trusted_local(&state_db).await;
        upsert_target_thread(&state_db).await;
        create_pending_user_input_interaction_with_server_request(
            &state_db,
            "interaction-live-client",
            /*server_request_id_json*/
            Some(json!(7)),
        )
        .await;

        let mut dry_run_params =
            test_respond_pending_submit_params("trusted", "local", "interaction-live-client");
        dry_run_params.dry_run = true;
        let err = RemoteDispatchRequestProcessor::new(Some(state_db.clone()))
            .submit(dry_run_params)
            .await
            .expect_err("live client-bound interaction dry-run should fail");

        assert!(
            err.message
                .contains("pending interaction is tied to a live client request"),
            "unexpected error: {}",
            err.message
        );

        let stored = state_db
            .get_thread_pending_interaction("interaction-live-client")
            .await
            .expect("pending interaction should reload")
            .expect("pending interaction should exist");
        assert_eq!(
            codex_state::PendingInteractionStatus::Pending,
            stored.status
        );

        let err = RemoteDispatchRequestProcessor::new(Some(state_db))
            .submit(test_respond_pending_submit_params(
                "trusted",
                "local",
                "interaction-live-client",
            ))
            .await
            .expect_err("live client-bound interaction should fail");

        assert!(
            err.message
                .contains("pending interaction is tied to a live client request"),
            "unexpected error: {}",
            err.message
        );
    }

    #[tokio::test]
    async fn receipt_read_is_denied_before_receipt_persistence_exists() {
        let err = RemoteDispatchRequestProcessor::new(/*state_db*/ None)
            .receipt_read(RemoteDispatchReceiptReadParams {
                request_id: Some("request-1".to_string()),
                idempotency_key: None,
                source_machine_id: "source".to_string(),
                target_machine_id: "target".to_string(),
            })
            .await
            .expect_err("receipt read should fail closed");
        assert!(err.message.contains("not enabled"));
    }

    async fn test_state_db() -> StateDbHandle {
        let tempdir = TempDir::new().expect("tempdir");
        codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    async fn setup_trusted_local(state_db: &StateDbHandle) {
        upsert_machine(
            state_db,
            "local",
            codex_state::MachineTrustState::Local,
            codex_state::MachineEnrollmentState::Local,
        )
        .await;
        upsert_machine(
            state_db,
            "trusted",
            codex_state::MachineTrustState::Trusted,
            codex_state::MachineEnrollmentState::Manual,
        )
        .await;
    }

    async fn upsert_target_thread(state_db: &StateDbHandle) {
        let thread_id = ThreadId::from_string(TARGET_THREAD_ID)
            .expect("target thread id should parse for fixture");
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
            .expect("timestamp should parse");
        let codex_home = state_db.codex_home();
        state_db
            .upsert_thread(&codex_state::ThreadMetadata {
                id: thread_id,
                rollout_path: codex_home.join(format!("rollout-{thread_id}.jsonl")),
                created_at: now,
                updated_at: now,
                source: "cli".to_string(),
                thread_source: None,
                agent_nickname: None,
                agent_role: None,
                agent_path: None,
                model_provider: "test-provider".to_string(),
                model: Some("gpt-5".to_string()),
                reasoning_effort: None,
                cwd: codex_home.join("workspace"),
                cli_version: "0.0.0".to_string(),
                title: String::new(),
                preview: Some("target thread".to_string()),
                sandbox_policy: "read-only".to_string(),
                approval_mode: "on-request".to_string(),
                tokens_used: 0,
                first_user_message: Some("target thread".to_string()),
                archived_at: None,
                git_sha: None,
                git_branch: None,
                git_origin_url: None,
            })
            .await
            .expect("target thread should upsert");
    }

    async fn create_pending_user_input_interaction(state_db: &StateDbHandle, interaction_id: &str) {
        create_pending_user_input_interaction_with_server_request(
            state_db,
            interaction_id,
            /*server_request_id_json*/ None,
        )
        .await;
    }

    async fn create_pending_user_input_interaction_with_server_request(
        state_db: &StateDbHandle,
        interaction_id: &str,
        server_request_id_json: Option<serde_json::Value>,
    ) {
        let thread_id = ThreadId::from_string(TARGET_THREAD_ID)
            .expect("target thread id should parse for fixture");
        state_db
            .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
                interaction_id: interaction_id.to_string(),
                thread_id,
                source_kind: codex_state::PendingInteractionSourceKind::Thread,
                source_id: None,
                turn_id: Some("turn-1".to_string()),
                worker_request_id: Some(interaction_id.to_string()),
                server_request_id_json,
                kind: codex_state::PendingInteractionKind::UserInput,
                request_payload_json: json!({
                    "type": "requestUserInput",
                    "questions": [{
                        "id": "decision",
                        "question": "Proceed?",
                    }],
                }),
                request_payload_preview: "Proceed?".to_string(),
                request_redactions_json: json!([]),
                no_client_policy: "record-and-wait-for-coordinator".to_string(),
                timeout_at: None,
            })
            .await
            .expect("pending interaction should create");
    }

    async fn upsert_machine(
        state_db: &StateDbHandle,
        machine_id: &str,
        trust_state: codex_state::MachineTrustState,
        enrollment_state: codex_state::MachineEnrollmentState,
    ) {
        state_db
            .machine_registry()
            .upsert_machine(codex_state::MachineRegistryUpsertParams {
                machine_id: Some(machine_id.to_string()),
                installation_id: None,
                display_name: Some(machine_id.to_string()),
                trust_state,
                enrollment_state,
                health_state: codex_state::MachineHealthState::Online,
                source_kind: codex_state::MachineSourceKind::Manual,
                adapter_name: None,
                capabilities_json: serde_json::json!({}),
                endpoints: Vec::new(),
                last_seen_at: None,
            })
            .await
            .expect("machine should upsert");
    }

    fn test_submit_params(
        source_machine_id: &str,
        target_machine_id: &str,
    ) -> RemoteDispatchSubmitParams {
        RemoteDispatchSubmitParams {
            request_id: "request-1".to_string(),
            source_machine_id: source_machine_id.to_string(),
            target_machine_id: target_machine_id.to_string(),
            idempotency_key: "idem-1".to_string(),
            operation: RemoteDispatchOperation::MailboxReceipts {
                params: RemoteDispatchMailboxReceiptsParams {
                    target_thread_id: TARGET_THREAD_ID.to_string(),
                    message_id: "message-1".to_string(),
                },
            },
            requested_at: None,
            expires_at: None,
            capability_version: None,
            dry_run: false,
        }
    }

    fn test_enqueue_submit_params(
        source_machine_id: &str,
        target_machine_id: &str,
    ) -> RemoteDispatchSubmitParams {
        RemoteDispatchSubmitParams {
            request_id: "request-1".to_string(),
            source_machine_id: source_machine_id.to_string(),
            target_machine_id: target_machine_id.to_string(),
            idempotency_key: "idem-1".to_string(),
            operation: RemoteDispatchOperation::EnqueueInstruction {
                params: RemoteDispatchEnqueueInstructionParams {
                    target_thread_id: TARGET_THREAD_ID.to_string(),
                    message: "remote hello".to_string(),
                    sender_thread_id: None,
                    sender_label: None,
                    priority: Some(10),
                    max_attempts: Some(3),
                    expires_at: None,
                    resume: false,
                },
            },
            requested_at: None,
            expires_at: None,
            capability_version: None,
            dry_run: false,
        }
    }

    fn test_mailbox_receipts_submit_params(
        source_machine_id: &str,
        target_machine_id: &str,
        message_id: &str,
    ) -> RemoteDispatchSubmitParams {
        RemoteDispatchSubmitParams {
            request_id: "request-mailbox-receipts".to_string(),
            source_machine_id: source_machine_id.to_string(),
            target_machine_id: target_machine_id.to_string(),
            idempotency_key: "idem-mailbox-receipts".to_string(),
            operation: RemoteDispatchOperation::MailboxReceipts {
                params: RemoteDispatchMailboxReceiptsParams {
                    target_thread_id: TARGET_THREAD_ID.to_string(),
                    message_id: message_id.to_string(),
                },
            },
            requested_at: None,
            expires_at: None,
            capability_version: None,
            dry_run: false,
        }
    }

    fn test_list_pending_submit_params(
        source_machine_id: &str,
        target_machine_id: &str,
    ) -> RemoteDispatchSubmitParams {
        RemoteDispatchSubmitParams {
            request_id: "request-list-pending".to_string(),
            source_machine_id: source_machine_id.to_string(),
            target_machine_id: target_machine_id.to_string(),
            idempotency_key: "idem-list-pending".to_string(),
            operation: RemoteDispatchOperation::ListPendingInteractions {
                params: RemoteDispatchListPendingInteractionsParams {
                    thread_id: Some(TARGET_THREAD_ID.to_string()),
                    statuses: None,
                    kinds: None,
                    cursor: None,
                    limit: Some(10),
                },
            },
            requested_at: None,
            expires_at: None,
            capability_version: None,
            dry_run: false,
        }
    }

    fn test_read_pending_submit_params(
        source_machine_id: &str,
        target_machine_id: &str,
        interaction_id: &str,
    ) -> RemoteDispatchSubmitParams {
        RemoteDispatchSubmitParams {
            request_id: "request-read-pending".to_string(),
            source_machine_id: source_machine_id.to_string(),
            target_machine_id: target_machine_id.to_string(),
            idempotency_key: "idem-read-pending".to_string(),
            operation: RemoteDispatchOperation::ReadPendingInteraction {
                params: RemoteDispatchReadPendingInteractionParams {
                    interaction_id: interaction_id.to_string(),
                    thread_id: Some(TARGET_THREAD_ID.to_string()),
                },
            },
            requested_at: None,
            expires_at: None,
            capability_version: None,
            dry_run: false,
        }
    }

    fn test_respond_pending_submit_params(
        source_machine_id: &str,
        target_machine_id: &str,
        interaction_id: &str,
    ) -> RemoteDispatchSubmitParams {
        RemoteDispatchSubmitParams {
            request_id: "request-respond-pending".to_string(),
            source_machine_id: source_machine_id.to_string(),
            target_machine_id: target_machine_id.to_string(),
            idempotency_key: "idem-respond-pending".to_string(),
            operation: RemoteDispatchOperation::RespondInteraction {
                params: RemoteDispatchRespondInteractionParams {
                    interaction_id: interaction_id.to_string(),
                    thread_id: Some(TARGET_THREAD_ID.to_string()),
                    terminal_status: ThreadPendingInteractionTerminalStatus::Responded,
                    response: ThreadPendingInteractionResponsePayload::RequestUserInput {
                        answers: HashMap::from([(
                            "decision".to_string(),
                            ToolRequestUserInputAnswer {
                                answers: vec!["ship it".to_string()],
                            },
                        )]),
                    },
                },
            },
            requested_at: None,
            expires_at: None,
            capability_version: None,
            dry_run: false,
        }
    }

    fn expect_submit_response(payload: ClientResponsePayload) -> RemoteDispatchSubmitResponse {
        let ClientResponsePayload::RemoteDispatchSubmit(response) = payload else {
            panic!("expected remote dispatch submit response");
        };
        response
    }
}
