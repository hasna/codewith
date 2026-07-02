use super::thread_mailbox_context::validate_mailbox_payload_context_size;
use super::*;
use codex_app_server_protocol::ThreadMailboxAckParams;
use codex_app_server_protocol::ThreadMailboxAckResponse;
use codex_app_server_protocol::ThreadMailboxClaimParams;
use codex_app_server_protocol::ThreadMailboxClaimResponse;
use codex_app_server_protocol::ThreadMailboxEnqueueParams;
use codex_app_server_protocol::ThreadMailboxEnqueueResponse;
use codex_app_server_protocol::ThreadMailboxFailDisposition;
use codex_app_server_protocol::ThreadMailboxFailParams;
use codex_app_server_protocol::ThreadMailboxFailResponse;
use codex_app_server_protocol::ThreadMailboxListParams;
use codex_app_server_protocol::ThreadMailboxListResponse;
use codex_app_server_protocol::ThreadMailboxReadParams;
use codex_app_server_protocol::ThreadMailboxReadResponse;
use codex_app_server_protocol::ThreadMailboxReceiptsListParams;
use codex_app_server_protocol::ThreadMailboxReceiptsListResponse;
use mapping::api_mailbox_claim;
use mapping::api_mailbox_detail;
use mapping::api_mailbox_kind_to_state;
use mapping::api_mailbox_receipt;
use mapping::api_mailbox_status_to_state;
use mapping::api_mailbox_summary;

pub(super) mod mapping;

const DEFAULT_MAILBOX_LEASE_SECONDS: u32 = 10 * 60;
const MAX_MAILBOX_LEASE_SECONDS: u32 = 60 * 60;
const DEFAULT_MAILBOX_MAX_ATTEMPTS: u32 = 10;
const MAX_MAILBOX_MAX_ATTEMPTS: u32 = 25;
const MAX_MAILBOX_PREVIEW_CHARS: usize = 240;
const MAX_MAILBOX_SENDER_LABEL_CHARS: usize = 120;

impl ThreadRequestProcessor {
    pub(crate) async fn thread_mailbox_enqueue(
        &self,
        params: ThreadMailboxEnqueueParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_enqueue_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_mailbox_list(
        &self,
        params: ThreadMailboxListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_mailbox_read(
        &self,
        params: ThreadMailboxReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_read_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_mailbox_claim(
        &self,
        params: ThreadMailboxClaimParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_claim_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_mailbox_ack(
        &self,
        params: ThreadMailboxAckParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_ack_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_mailbox_fail(
        &self,
        params: ThreadMailboxFailParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_fail_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn thread_mailbox_receipts_list(
        &self,
        params: ThreadMailboxReceiptsListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.thread_mailbox_receipts_list_inner(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(super) async fn thread_mailbox_enqueue_inner(
        &self,
        params: ThreadMailboxEnqueueParams,
    ) -> Result<ThreadMailboxEnqueueResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let sender_thread_id = params
            .sender_thread_id
            .as_deref()
            .map(parse_mailbox_thread_id)
            .transpose()?;
        validate_mailbox_payload_context_size(&params.message).map_err(invalid_request)?;
        let sender_label = normalize_optional_label(params.sender_label)?;
        let idempotency_key = normalize_optional_token("idempotencyKey", params.idempotency_key)?;
        let preview = normalize_mailbox_preview(params.preview, &params.message)?;
        let max_attempts = params.max_attempts.unwrap_or(DEFAULT_MAILBOX_MAX_ATTEMPTS);
        if max_attempts == 0 || max_attempts > MAX_MAILBOX_MAX_ATTEMPTS {
            return Err(invalid_request(format!(
                "mailbox maxAttempts must be between 1 and {MAX_MAILBOX_MAX_ATTEMPTS}"
            )));
        }
        let next_attempt_at = params
            .next_attempt_at
            .map(|timestamp| mailbox_timestamp_to_datetime(timestamp, "nextAttemptAt"))
            .transpose()?;
        let expires_at = params
            .expires_at
            .map(|timestamp| mailbox_timestamp_to_datetime(timestamp, "expiresAt"))
            .transpose()?;
        let outcome = state_db
            .mailbox_messages()
            .enqueue_message(codex_state::MailboxEnqueueParams {
                target_thread_id,
                sender_thread_id,
                sender_label,
                idempotency_key,
                kind: api_mailbox_kind_to_state(params.kind),
                payload_json: params.message,
                payload_preview: preview,
                priority: params.priority.unwrap_or(0),
                max_attempts: i64::from(max_attempts),
                next_attempt_at,
                expires_at,
            })
            .await
            .map_err(|err| internal_error(format!("failed to enqueue mailbox message: {err}")))?;
        Ok(ThreadMailboxEnqueueResponse {
            message: api_mailbox_summary(outcome.message),
            created: outcome.created,
        })
    }

    async fn thread_mailbox_list_inner(
        &self,
        params: ThreadMailboxListParams,
    ) -> Result<ThreadMailboxListResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let statuses = params
            .statuses
            .unwrap_or_default()
            .into_iter()
            .map(api_mailbox_status_to_state)
            .collect::<Vec<_>>();
        let limit = params
            .limit
            .unwrap_or(codex_state::DEFAULT_MAILBOX_MESSAGE_LIST_LIMIT);
        if limit == 0 || limit > codex_state::MAX_MAILBOX_MESSAGE_LIST_LIMIT {
            return Err(invalid_request(format!(
                "mailbox list limit must be between 1 and {}",
                codex_state::MAX_MAILBOX_MESSAGE_LIST_LIMIT
            )));
        }
        let page = state_db
            .mailbox_messages()
            .list_messages(codex_state::MailboxMessageStoreListParams {
                target_thread_id: Some(target_thread_id),
                statuses,
                cursor: params.cursor,
                limit,
            })
            .await
            .map_err(mailbox_store_error)?;
        Ok(ThreadMailboxListResponse {
            data: page.data.into_iter().map(api_mailbox_summary).collect(),
            next_cursor: page.next_cursor,
        })
    }

    async fn thread_mailbox_read_inner(
        &self,
        params: ThreadMailboxReadParams,
    ) -> Result<ThreadMailboxReadResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let message = read_target_mailbox_message(
            state_db.as_ref(),
            target_thread_id,
            params.message_id.as_str(),
        )
        .await?;
        Ok(ThreadMailboxReadResponse {
            message: api_mailbox_detail(message),
        })
    }

    async fn thread_mailbox_claim_inner(
        &self,
        params: ThreadMailboxClaimParams,
    ) -> Result<ThreadMailboxClaimResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let lease_owner =
            normalize_optional_label(params.lease_owner)?.unwrap_or_else(|| "app-server".into());
        let lease_seconds = params
            .lease_seconds
            .unwrap_or(DEFAULT_MAILBOX_LEASE_SECONDS);
        if lease_seconds == 0 || lease_seconds > MAX_MAILBOX_LEASE_SECONDS {
            return Err(invalid_request(format!(
                "mailbox leaseSeconds must be between 1 and {MAX_MAILBOX_LEASE_SECONDS}"
            )));
        }
        let claim = state_db
            .mailbox_messages()
            .claim_next_message(codex_state::MailboxClaimParams {
                target_thread_id,
                lease_owner,
                lease_duration: std::time::Duration::from_secs(u64::from(lease_seconds)),
                now: Utc::now(),
            })
            .await
            .map_err(|err| internal_error(format!("failed to claim mailbox message: {err}")))?
            .map(api_mailbox_claim);
        Ok(ThreadMailboxClaimResponse { claim })
    }

    async fn thread_mailbox_ack_inner(
        &self,
        params: ThreadMailboxAckParams,
    ) -> Result<ThreadMailboxAckResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let _existing =
            read_target_mailbox_message(state_db.as_ref(), target_thread_id, &params.message_id)
                .await?;
        let message = state_db
            .mailbox_messages()
            .ack_message(codex_state::MailboxAckParams {
                message_id: params.message_id,
                attempt_id: params.attempt_id,
                lease_id: params.lease_id,
                receipt_payload_json: params.receipt,
                now: Utc::now(),
            })
            .await
            .map_err(|err| internal_error(format!("failed to ack mailbox message: {err}")))?
            .ok_or_else(|| invalid_request("mailbox claim is not active"))?;
        Ok(ThreadMailboxAckResponse {
            message: api_mailbox_summary(message),
        })
    }

    async fn thread_mailbox_fail_inner(
        &self,
        params: ThreadMailboxFailParams,
    ) -> Result<ThreadMailboxFailResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let _existing =
            read_target_mailbox_message(state_db.as_ref(), target_thread_id, &params.message_id)
                .await?;
        let error = validate_required_text("mailbox fail error", params.error)?;
        let disposition = match params.disposition {
            ThreadMailboxFailDisposition::Retry => {
                let retry_at = params
                    .retry_at
                    .map(|timestamp| mailbox_timestamp_to_datetime(timestamp, "retryAt"))
                    .transpose()?
                    .unwrap_or_else(Utc::now);
                codex_state::MailboxFailDisposition::Retry {
                    next_attempt_at: retry_at,
                }
            }
            ThreadMailboxFailDisposition::Terminal => codex_state::MailboxFailDisposition::Terminal,
        };
        let message = state_db
            .mailbox_messages()
            .fail_message(codex_state::MailboxFailParams {
                message_id: params.message_id,
                attempt_id: params.attempt_id,
                lease_id: params.lease_id,
                error,
                disposition,
                now: Utc::now(),
            })
            .await
            .map_err(|err| internal_error(format!("failed to fail mailbox message: {err}")))?
            .ok_or_else(|| invalid_request("mailbox claim is not active"))?;
        Ok(ThreadMailboxFailResponse {
            message: api_mailbox_summary(message),
        })
    }

    pub(super) async fn thread_mailbox_receipts_list_inner(
        &self,
        params: ThreadMailboxReceiptsListParams,
    ) -> Result<ThreadMailboxReceiptsListResponse, JSONRPCErrorError> {
        let target_thread_id = parse_mailbox_thread_id(params.target_thread_id.as_str())?;
        let state_db = self.state_db_for_mailbox_thread(target_thread_id).await?;
        let _existing =
            read_target_mailbox_message(state_db.as_ref(), target_thread_id, &params.message_id)
                .await?;
        let receipts = state_db
            .mailbox_messages()
            .list_receipts(params.message_id.as_str())
            .await
            .map_err(|err| internal_error(format!("failed to list mailbox receipts: {err}")))?;
        Ok(ThreadMailboxReceiptsListResponse {
            data: receipts.into_iter().map(api_mailbox_receipt).collect(),
        })
    }

    async fn state_db_for_mailbox_thread(
        &self,
        thread_id: ThreadId,
    ) -> Result<StateDbHandle, JSONRPCErrorError> {
        if let Ok(thread) = self.thread_manager.get_thread(thread_id).await {
            if thread.rollout_path().is_none() {
                return Err(invalid_request(format!(
                    "ephemeral thread does not support mailbox: {thread_id}"
                )));
            }
            if let Some(state_db) = thread.state_db() {
                ensure_state_thread_exists(state_db.as_ref(), thread_id).await?;
                return Ok(state_db);
            }
        } else {
            codex_rollout::find_thread_path_by_id_str(
                &self.config.codex_home,
                &thread_id.to_string(),
                self.state_db.as_deref(),
            )
            .await
            .map_err(|err| {
                internal_error(format!("failed to locate thread id {thread_id}: {err}"))
            })?
            .ok_or_else(|| invalid_request(format!("thread not found: {thread_id}")))?;
        }

        let state_db = self
            .state_db
            .clone()
            .ok_or_else(|| internal_error("sqlite state db unavailable for thread mailbox"))?;
        ensure_state_thread_exists(state_db.as_ref(), thread_id).await?;
        Ok(state_db)
    }
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

async fn read_target_mailbox_message(
    state_db: &codex_state::StateRuntime,
    target_thread_id: ThreadId,
    message_id: &str,
) -> Result<codex_state::MailboxMessage, JSONRPCErrorError> {
    let message = state_db
        .mailbox_messages()
        .get_message(message_id)
        .await
        .map_err(|err| internal_error(format!("failed to read mailbox message: {err}")))?
        .ok_or_else(|| invalid_request(format!("mailbox message not found: {message_id}")))?;
    if message.target_thread_id != target_thread_id {
        return Err(invalid_request(format!(
            "mailbox message not found: {message_id}"
        )));
    }
    Ok(message)
}

fn parse_mailbox_thread_id(thread_id: &str) -> Result<ThreadId, JSONRPCErrorError> {
    ThreadId::from_string(thread_id)
        .map_err(|err| invalid_request(format!("invalid thread id: {err}")))
}

fn normalize_optional_token(
    field_name: &str,
    value: Option<String>,
) -> Result<Option<String>, JSONRPCErrorError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Err(invalid_request(format!("{field_name} must not be empty")));
            }
            Ok(value.to_string())
        })
        .transpose()
}

fn normalize_optional_label(value: Option<String>) -> Result<Option<String>, JSONRPCErrorError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                return Err(invalid_request("mailbox label must not be empty"));
            }
            if value.chars().count() > MAX_MAILBOX_SENDER_LABEL_CHARS {
                return Err(invalid_request(format!(
                    "mailbox label must be at most {MAX_MAILBOX_SENDER_LABEL_CHARS} characters"
                )));
            }
            Ok(value.to_string())
        })
        .transpose()
}

fn validate_required_text(field_name: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request(format!("{field_name} must not be empty")));
    }
    Ok(value.to_string())
}

fn normalize_mailbox_preview(
    preview: Option<String>,
    payload: &serde_json::Value,
) -> Result<String, JSONRPCErrorError> {
    if let Some(preview) = preview {
        let preview = preview.trim();
        if preview.is_empty() {
            return Err(invalid_request("mailbox preview must not be empty"));
        }
        return Ok(truncate_preview(preview));
    }
    let rendered = serde_json::to_string(payload).map_err(|err| {
        invalid_request(format!("mailbox message must be JSON serializable: {err}"))
    })?;
    Ok(truncate_preview(rendered.as_str()))
}

fn truncate_preview(value: &str) -> String {
    value.chars().take(MAX_MAILBOX_PREVIEW_CHARS).collect()
}

fn mailbox_timestamp_to_datetime(
    value: i64,
    field_name: &str,
) -> Result<DateTime<Utc>, JSONRPCErrorError> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| invalid_request(format!("{field_name} must be a valid Unix timestamp")))
}

fn mailbox_store_error(err: anyhow::Error) -> JSONRPCErrorError {
    let message = err.to_string();
    if message.contains("invalid mailbox cursor") {
        invalid_request(message)
    } else {
        internal_error(format!("failed to list mailbox messages: {err}"))
    }
}
