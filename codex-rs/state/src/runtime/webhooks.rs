use super::*;
use crate::model::WebhookEventRow;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

pub const DEFAULT_WEBHOOK_EVENT_LIST_LIMIT: u32 = 50;
pub const MAX_WEBHOOK_EVENT_LIST_LIMIT: u32 = 200;
pub const WEBHOOK_EVENT_DEDUPE_CONFLICT_MESSAGE: &str = "webhook event dedupe keys conflict";
const WEBHOOK_PAYLOAD_PREVIEW_CHARS: usize = 320;

#[derive(Clone)]
pub struct WebhookEventStore {
    pool: Arc<SqlitePool>,
}

impl WebhookEventStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
pub struct WebhookEventIngestParams {
    pub source_app_id: String,
    pub source_app_name: Option<String>,
    pub subscription_id: Option<String>,
    pub event_type: String,
    pub external_delivery_id: Option<String>,
    pub idempotency_key: Option<String>,
    pub target_thread_id: Option<ThreadId>,
    pub payload_json: Value,
}

#[derive(Debug, Clone)]
pub struct WebhookEventListParams {
    pub source_app_id: Option<String>,
    pub target_thread_id: Option<ThreadId>,
    pub statuses: Option<Vec<crate::WebhookEventStatus>>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebhookEventListPage {
    pub data: Vec<crate::WebhookEvent>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebhookEventIngestOutcome {
    pub event: crate::WebhookEvent,
    pub created: bool,
}

impl WebhookEventStore {
    pub async fn ingest_webhook_event(
        &self,
        params: WebhookEventIngestParams,
    ) -> anyhow::Result<WebhookEventIngestOutcome> {
        if let Some(event) = self.find_existing_event(&params).await? {
            return Ok(WebhookEventIngestOutcome {
                event,
                created: false,
            });
        }

        let event_id = Uuid::new_v4().to_string();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let payload_sha256 = payload_sha256(&params.payload_json);
        let (redacted_payload, redactions) = redact_webhook_payload(params.payload_json.clone());
        let payload_json = serde_json::to_string(&redacted_payload)?;
        let redactions_json = serde_json::to_string(&redactions)?;
        let payload_preview = payload_preview(&redacted_payload);
        let idempotency_key = params.idempotency_key.as_deref().map(redact_state_string);
        let sql = webhook_event_returning(
            r#"
INSERT OR IGNORE INTO app_webhook_events (
    event_id,
    source_app_id,
    source_app_name,
    subscription_id,
    event_type,
    external_delivery_id,
    idempotency_key,
    target_thread_id,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    received_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'unread', ?, ?, ?, ?, ?, ?)
RETURNING
"#,
        );
        let target_thread_id = params
            .target_thread_id
            .as_ref()
            .map(std::string::ToString::to_string);
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(event_id)
            .bind(params.source_app_id.as_str())
            .bind(params.source_app_name.as_deref())
            .bind(params.subscription_id.as_deref())
            .bind(params.event_type.as_str())
            .bind(params.external_delivery_id.as_deref())
            .bind(idempotency_key.as_deref())
            .bind(target_thread_id.as_deref())
            .bind(payload_json)
            .bind(payload_sha256)
            .bind(payload_preview)
            .bind(redactions_json)
            .bind(now_ms)
            .bind(now_ms)
            .fetch_optional(self.pool.as_ref())
            .await?;
        let Some(row) = row else {
            let Some(event) = self.find_existing_event(&params).await? else {
                anyhow::bail!("webhook event insert was ignored but no existing row was found");
            };
            return Ok(WebhookEventIngestOutcome {
                event,
                created: false,
            });
        };
        Ok(WebhookEventIngestOutcome {
            event: webhook_event_from_row(&row)?,
            created: true,
        })
    }

    pub async fn list_webhook_events(
        &self,
        params: WebhookEventListParams,
    ) -> anyhow::Result<WebhookEventListPage> {
        let limit = params.limit.clamp(1, MAX_WEBHOOK_EVENT_LIST_LIMIT);
        let offset = params
            .cursor
            .as_deref()
            .map(str::parse::<u32>)
            .transpose()?
            .unwrap_or(0);
        let mut builder = QueryBuilder::<Sqlite>::new(webhook_event_select("SELECT "));
        builder.push(" WHERE 1 = 1");
        if let Some(source_app_id) = params.source_app_id.as_deref() {
            builder.push(" AND source_app_id = ");
            builder.push_bind(source_app_id);
        }
        if let Some(target_thread_id) = params.target_thread_id {
            builder.push(" AND target_thread_id = ");
            builder.push_bind(target_thread_id.to_string());
        }
        if let Some(statuses) = params.statuses.as_ref()
            && !statuses.is_empty()
        {
            builder.push(" AND status IN (");
            let mut separated = builder.separated(", ");
            for status in statuses {
                separated.push_bind(status.as_str());
            }
            separated.push_unseparated(")");
        }
        builder.push(" ORDER BY received_at_ms DESC, event_id DESC LIMIT ");
        builder.push_bind(i64::from(limit) + 1);
        builder.push(" OFFSET ");
        builder.push_bind(i64::from(offset));

        let rows = builder.build().fetch_all(self.pool.as_ref()).await?;
        let mut events = rows
            .iter()
            .map(webhook_event_from_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let has_more = events.len() > limit as usize;
        if has_more {
            events.truncate(limit as usize);
        }
        Ok(WebhookEventListPage {
            data: events,
            next_cursor: has_more.then(|| offset.saturating_add(limit).to_string()),
        })
    }

    pub async fn get_webhook_event(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<crate::WebhookEvent>> {
        let sql = webhook_event_select(
            r#"
SELECT
"#,
        );
        let query = format!("{sql} WHERE event_id = ?");
        let row = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(event_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| webhook_event_from_row(&row)).transpose()
    }

    pub async fn mark_webhook_event(
        &self,
        event_id: &str,
        status: crate::WebhookEventStatus,
    ) -> anyhow::Result<Option<crate::WebhookEvent>> {
        let sql = webhook_event_returning(
            r#"
UPDATE app_webhook_events
SET status = ?, updated_at_ms = ?
WHERE event_id = ?
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(status.as_str())
            .bind(datetime_to_epoch_millis(Utc::now()))
            .bind(event_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| webhook_event_from_row(&row)).transpose()
    }

    async fn find_existing_event(
        &self,
        params: &WebhookEventIngestParams,
    ) -> anyhow::Result<Option<crate::WebhookEvent>> {
        let idempotency_event = match params.idempotency_key.as_deref() {
            Some(idempotency_key) => {
                self.find_event_by_source_and_field(
                    params.source_app_id.as_str(),
                    "idempotency_key",
                    idempotency_key,
                )
                .await?
            }
            None => None,
        };
        let delivery_event = match params.external_delivery_id.as_deref() {
            Some(external_delivery_id) => {
                self.find_event_by_source_and_field(
                    params.source_app_id.as_str(),
                    "external_delivery_id",
                    external_delivery_id,
                )
                .await?
            }
            None => None,
        };

        match (idempotency_event, delivery_event) {
            (Some(idempotency_event), Some(delivery_event))
                if idempotency_event.event_id != delivery_event.event_id =>
            {
                anyhow::bail!(WEBHOOK_EVENT_DEDUPE_CONFLICT_MESSAGE);
            }
            (Some(event), _) | (_, Some(event)) => Ok(Some(event)),
            (None, None) => Ok(None),
        }
    }

    async fn find_event_by_source_and_field(
        &self,
        source_app_id: &str,
        field: &'static str,
        value: &str,
    ) -> anyhow::Result<Option<crate::WebhookEvent>> {
        let sql = webhook_event_select(
            r#"
SELECT
"#,
        );
        let query = format!("{sql} WHERE source_app_id = ? AND {field} = ?");
        let row = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(source_app_id)
            .bind(value)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| webhook_event_from_row(&row)).transpose()
    }
}

fn webhook_event_select(prefix: &str) -> String {
    format!(
        "{prefix}
    event_id,
    source_app_id,
    source_app_name,
    subscription_id,
    event_type,
    external_delivery_id,
    idempotency_key,
    target_thread_id,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    received_at_ms,
    updated_at_ms
FROM app_webhook_events"
    )
}

fn webhook_event_returning(prefix: &str) -> String {
    format!(
        "{prefix}
    event_id,
    source_app_id,
    source_app_name,
    subscription_id,
    event_type,
    external_delivery_id,
    idempotency_key,
    target_thread_id,
    status,
    payload_json,
    payload_sha256,
    payload_preview,
    redactions_json,
    received_at_ms,
    updated_at_ms"
    )
}

fn webhook_event_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<crate::WebhookEvent> {
    WebhookEventRow::try_from_row(row)?.try_into()
}

fn payload_sha256(value: &Value) -> String {
    let payload = serde_json::to_vec(value).unwrap_or_default();
    let digest = Sha256::digest(payload);
    format!("{digest:x}")
}

fn payload_preview(value: &Value) -> String {
    let rendered = serde_json::to_string(value).unwrap_or_else(|_| "<unrenderable>".to_string());
    truncate_chars(rendered.as_str(), WEBHOOK_PAYLOAD_PREVIEW_CHARS)
}

fn redact_webhook_payload(value: Value) -> (Value, Vec<crate::WebhookPayloadRedaction>) {
    let mut redactions = Vec::new();
    let redacted = redact_value(value, "$", &mut redactions);
    (redacted, redactions)
}

fn redact_value(
    value: Value,
    path: &str,
    redactions: &mut Vec<crate::WebhookPayloadRedaction>,
) -> Value {
    match value {
        Value::Object(map) => {
            let mut redacted = serde_json::Map::new();
            for (key, value) in map {
                let child_path = format!("{path}.{key}");
                if is_secret_key(key.as_str()) {
                    redactions.push(crate::WebhookPayloadRedaction {
                        path: child_path.clone(),
                        reason: "secret-like key".to_string(),
                    });
                    redacted.insert(key, Value::String("<redacted>".to_string()));
                } else {
                    redacted.insert(key, redact_value(value, child_path.as_str(), redactions));
                }
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(index, value)| {
                    redact_value(value, format!("{path}[{index}]").as_str(), redactions)
                })
                .collect(),
        ),
        Value::String(text) if is_secret_value(text.as_str()) => {
            redactions.push(crate::WebhookPayloadRedaction {
                path: path.to_string(),
                reason: "secret-like value".to_string(),
            });
            Value::String("<redacted>".to_string())
        }
        other => other,
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', '_'], "");
    normalized.contains("authorization")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.contains("token")
        || normalized.contains("apikey")
        || normalized == "cookie"
        || normalized == "setcookie"
}

fn is_secret_value(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("bearer ") || lower.starts_with("basic ") || value.starts_with("sk-") {
        return true;
    }
    value.len() >= 32
        && value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            .count()
            == value.len()
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        truncated.push_str("...");
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    #[tokio::test]
    async fn ingest_redacts_payload_and_deduplicates_delivery_id() {
        let runtime = test_runtime().await;
        let params = WebhookEventIngestParams {
            source_app_id: "github".to_string(),
            source_app_name: Some("GitHub".to_string()),
            subscription_id: Some("sub-1".to_string()),
            event_type: "pull_request".to_string(),
            external_delivery_id: Some("delivery-1".to_string()),
            idempotency_key: None,
            target_thread_id: None,
            payload_json: json!({
                "action": "opened",
                "authorization": "Bearer very-secret",
                "nested": { "api_key": "sk-test" }
            }),
        };

        let created = runtime
            .webhook_events()
            .ingest_webhook_event(params.clone())
            .await
            .unwrap();
        let duplicate = runtime
            .webhook_events()
            .ingest_webhook_event(params)
            .await
            .unwrap();

        assert!(created.created);
        assert!(!duplicate.created);
        assert_eq!(duplicate.event.event_id, created.event.event_id);
        assert_eq!(
            created.event.payload_json,
            json!({
                "action": "opened",
                "authorization": "<redacted>",
                "nested": { "api_key": "<redacted>" }
            })
        );
        assert_eq!(created.event.redactions.len(), 2);
    }

    #[tokio::test]
    async fn list_and_mark_webhook_events() {
        let runtime = test_runtime().await;
        let ingested = runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "linear".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "issue.created".to_string(),
                external_delivery_id: None,
                idempotency_key: Some("idem-1".to_string()),
                target_thread_id: Some(ThreadId::new()),
                payload_json: json!({ "title": "Fix bug" }),
            })
            .await
            .unwrap();

        let page = runtime
            .webhook_events()
            .list_webhook_events(WebhookEventListParams {
                source_app_id: Some("linear".to_string()),
                target_thread_id: None,
                statuses: Some(vec![crate::WebhookEventStatus::Unread]),
                cursor: None,
                limit: DEFAULT_WEBHOOK_EVENT_LIST_LIMIT,
            })
            .await
            .unwrap();
        assert_eq!(page.data, vec![ingested.event.clone()]);

        let marked = runtime
            .webhook_events()
            .mark_webhook_event(
                ingested.event.event_id.as_str(),
                crate::WebhookEventStatus::Queued,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(marked.status, crate::WebhookEventStatus::Queued);
    }

    #[tokio::test]
    async fn idempotency_key_is_scoped_to_source_app() {
        let runtime = test_runtime().await;
        let github = runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "github".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "pull_request".to_string(),
                external_delivery_id: None,
                idempotency_key: Some("shared-key".to_string()),
                target_thread_id: None,
                payload_json: json!({ "source": "github" }),
            })
            .await
            .unwrap();
        let duplicate = runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "github".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "pull_request".to_string(),
                external_delivery_id: None,
                idempotency_key: Some("shared-key".to_string()),
                target_thread_id: None,
                payload_json: json!({ "source": "duplicate" }),
            })
            .await
            .unwrap();
        let linear = runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "linear".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "issue.created".to_string(),
                external_delivery_id: None,
                idempotency_key: Some("shared-key".to_string()),
                target_thread_id: None,
                payload_json: json!({ "source": "linear" }),
            })
            .await
            .unwrap();

        assert!(!duplicate.created);
        assert!(linear.created);
        assert_eq!(duplicate.event.event_id, github.event.event_id);
        assert_ne!(linear.event.event_id, github.event.event_id);
    }

    #[tokio::test]
    async fn conflicting_dedupe_keys_are_rejected() {
        let runtime = test_runtime().await;
        runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "github".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "pull_request".to_string(),
                external_delivery_id: Some("delivery-1".to_string()),
                idempotency_key: Some("idempotency-1".to_string()),
                target_thread_id: None,
                payload_json: json!({ "delivery": 1 }),
            })
            .await
            .unwrap();
        runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "github".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "pull_request".to_string(),
                external_delivery_id: Some("delivery-2".to_string()),
                idempotency_key: Some("idempotency-2".to_string()),
                target_thread_id: None,
                payload_json: json!({ "delivery": 2 }),
            })
            .await
            .unwrap();

        let err = runtime
            .webhook_events()
            .ingest_webhook_event(WebhookEventIngestParams {
                source_app_id: "github".to_string(),
                source_app_name: None,
                subscription_id: None,
                event_type: "pull_request".to_string(),
                external_delivery_id: Some("delivery-2".to_string()),
                idempotency_key: Some("idempotency-1".to_string()),
                target_thread_id: None,
                payload_json: json!({ "delivery": "conflict" }),
            })
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), WEBHOOK_EVENT_DEDUPE_CONFLICT_MESSAGE);
    }
}
