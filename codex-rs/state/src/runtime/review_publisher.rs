use super::*;
use codex_protocol::protocol::ReviewEnvelope;
use codex_protocol::protocol::ReviewOutputEvent;
use codex_protocol::protocol::ReviewPublisherEvent;
use codex_protocol::protocol::ReviewPublisherEventKind;
use codex_protocol::protocol::ReviewPublisherVerdict;
use sha2::Digest;
use sha2::Sha256;

pub const REVIEW_ENVELOPE_SCHEMA_VERSION: &str = "codewith-review-envelope-v1";
pub const REVIEW_PUBLISHER_EVENT_SCHEMA_VERSION: &str = "codewith-review-publisher-event-v1";
pub const REVIEW_PUBLISHER_IMMUTABLE_CONFLICT: &str = "review publisher immutable payload conflict";

#[derive(Clone)]
pub struct ReviewPublisherStore {
    pool: Arc<SqlitePool>,
}

impl ReviewPublisherStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone)]
pub struct ReviewPublisherStartParams {
    pub thread_id: String,
    pub envelope: ReviewEnvelope,
}

#[derive(Debug, Clone)]
pub struct ReviewPublisherCompleteParams {
    pub review_run_id: String,
    pub envelope_sha256: String,
    pub review_output: Option<ReviewOutputEvent>,
    pub terminal_reason_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReviewPublisherClaimParams {
    pub lease_owner: String,
    pub lease_duration: Duration,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ReviewPublisherDeliveryAckParams {
    pub event_id: String,
    pub lease_owner: String,
    pub receipt_id: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPublisherFailureDisposition {
    Retry,
    DeadLetter,
}

#[derive(Debug, Clone)]
pub struct ReviewPublisherDeliveryFailParams {
    pub event_id: String,
    pub lease_owner: String,
    pub error_code: String,
    pub disposition: ReviewPublisherFailureDisposition,
    pub retry_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

impl ReviewPublisherStore {
    pub async fn start_review_run(
        &self,
        params: ReviewPublisherStartParams,
    ) -> anyhow::Result<crate::ReviewPublisherRunSnapshot> {
        verify_review_envelope(&params.envelope)?;
        let review_run_id = review_run_id(params.envelope.envelope_sha256.as_str());
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let envelope_json = params.envelope.canonical_json()?;
        let start_event = publisher_event(
            review_run_id.as_str(),
            ReviewPublisherEventKind::Started,
            params.envelope.clone(),
            /*verdict*/ None,
            /*overall_correctness*/ None,
            Vec::new(),
            /*terminal_reason*/ None,
        );
        let start_event_json = serde_json::to_string(&start_event)?;
        let start_payload_sha256 = sha256_hex(start_event_json.as_bytes());

        let mut tx = self.pool.begin().await?;
        if let Some(existing_sha) = sqlx::query_scalar::<_, String>(
            "SELECT envelope_sha256 FROM review_publisher_runs WHERE review_run_id = ?",
        )
        .bind(review_run_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing_sha != params.envelope.envelope_sha256 {
                anyhow::bail!(REVIEW_PUBLISHER_IMMUTABLE_CONFLICT);
            }
            tx.commit().await?;
            return self
                .get_review_run(review_run_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("existing review run disappeared"));
        }

        sqlx::query(
            r#"
INSERT INTO review_publisher_runs (
    review_run_id, thread_id, turn_id, envelope_json, envelope_sha256,
    status, verdict, terminal_reason, created_at_ms, completed_at_ms, updated_at_ms
) VALUES (?, ?, NULL, ?, ?, 'started', NULL, NULL, ?, NULL, ?)
"#,
        )
        .bind(review_run_id.as_str())
        .bind(params.thread_id.as_str())
        .bind(envelope_json)
        .bind(params.envelope.envelope_sha256.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
INSERT INTO review_publisher_outbox_events (
    event_id, review_run_id, event_kind, sequence, status, payload_json,
    payload_sha256, attempt_count, next_attempt_at_ms, lease_owner,
    lease_expires_at_ms, receipt_id, last_error_code, created_at_ms,
    delivered_at_ms, updated_at_ms
) VALUES (?, ?, 'started', 0, 'pending', ?, ?, 0, ?, NULL, NULL, NULL, NULL, ?, NULL, ?)
"#,
        )
        .bind(start_event.event_id.as_str())
        .bind(review_run_id.as_str())
        .bind(start_event_json)
        .bind(start_payload_sha256)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        self.get_review_run(review_run_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("created review run disappeared"))
    }

    pub async fn bind_review_turn(&self, review_run_id: &str, turn_id: &str) -> anyhow::Result<()> {
        let updated = sqlx::query(
            r#"
UPDATE review_publisher_runs
SET turn_id = COALESCE(turn_id, ?), updated_at_ms = ?
WHERE review_run_id = ? AND (turn_id IS NULL OR turn_id = ?)
"#,
        )
        .bind(turn_id)
        .bind(datetime_to_epoch_millis(Utc::now()))
        .bind(review_run_id)
        .bind(turn_id)
        .execute(self.pool.as_ref())
        .await?;
        if updated.rows_affected() != 1 {
            anyhow::bail!(REVIEW_PUBLISHER_IMMUTABLE_CONFLICT);
        }
        Ok(())
    }

    pub async fn complete_review_run(
        &self,
        params: ReviewPublisherCompleteParams,
    ) -> anyhow::Result<crate::ReviewPublisherRunSnapshot> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT envelope_json, envelope_sha256, status FROM review_publisher_runs WHERE review_run_id = ?",
        )
        .bind(params.review_run_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("review publisher run not found"))?;
        let envelope_sha256: String = row.try_get("envelope_sha256")?;
        if envelope_sha256 != params.envelope_sha256 {
            anyhow::bail!(REVIEW_PUBLISHER_IMMUTABLE_CONFLICT);
        }
        let envelope: ReviewEnvelope = serde_json::from_str(row.try_get("envelope_json")?)?;
        verify_review_envelope(&envelope)?;
        let (verdict, overall_correctness, finding_priorities, mapped_reason) =
            map_review_output(params.review_output.as_ref());
        let terminal_reason = params
            .terminal_reason_override
            .as_deref()
            .map(normalize_terminal_reason)
            .transpose()?
            .unwrap_or(mapped_reason);
        let terminal_event = publisher_event(
            params.review_run_id.as_str(),
            ReviewPublisherEventKind::Completed,
            envelope,
            Some(verdict),
            overall_correctness,
            finding_priorities,
            Some(terminal_reason.clone()),
        );
        let terminal_event_json = serde_json::to_string(&terminal_event)?;
        let terminal_payload_sha256 = sha256_hex(terminal_event_json.as_bytes());
        let status: String = row.try_get("status")?;

        if status == "completed" {
            let existing_sha = sqlx::query_scalar::<_, String>(
                "SELECT payload_sha256 FROM review_publisher_outbox_events WHERE review_run_id = ? AND sequence = 1",
            )
            .bind(params.review_run_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| anyhow::anyhow!("completed review run has no terminal event"))?;
            if existing_sha != terminal_payload_sha256 {
                anyhow::bail!(REVIEW_PUBLISHER_IMMUTABLE_CONFLICT);
            }
            tx.commit().await?;
            return self
                .get_review_run(params.review_run_id.as_str())
                .await?
                .ok_or_else(|| anyhow::anyhow!("completed review run disappeared"));
        }

        let start_exists = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM review_publisher_outbox_events WHERE review_run_id = ? AND sequence = 0",
        )
        .bind(params.review_run_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if start_exists != 1 {
            anyhow::bail!("review publisher terminal event requires one start event");
        }
        let now_ms = datetime_to_epoch_millis(Utc::now());
        sqlx::query(
            r#"
UPDATE review_publisher_runs
SET status = 'completed', verdict = ?, terminal_reason = ?, completed_at_ms = ?, updated_at_ms = ?
WHERE review_run_id = ? AND status = 'started'
"#,
        )
        .bind(verdict_str(verdict))
        .bind(terminal_reason.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .bind(params.review_run_id.as_str())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
INSERT INTO review_publisher_outbox_events (
    event_id, review_run_id, event_kind, sequence, status, payload_json,
    payload_sha256, attempt_count, next_attempt_at_ms, lease_owner,
    lease_expires_at_ms, receipt_id, last_error_code, created_at_ms,
    delivered_at_ms, updated_at_ms
) VALUES (?, ?, 'completed', 1, 'pending', ?, ?, 0, ?, NULL, NULL, NULL, NULL, ?, NULL, ?)
"#,
        )
        .bind(terminal_event.event_id.as_str())
        .bind(params.review_run_id.as_str())
        .bind(terminal_event_json)
        .bind(terminal_payload_sha256)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        self.get_review_run(params.review_run_id.as_str())
            .await?
            .ok_or_else(|| anyhow::anyhow!("completed review run disappeared"))
    }

    pub async fn get_review_run(
        &self,
        review_run_id: &str,
    ) -> anyhow::Result<Option<crate::ReviewPublisherRunSnapshot>> {
        let Some(row) = sqlx::query(
            r#"
SELECT review_run_id, thread_id, turn_id, envelope_json, envelope_sha256,
       status, verdict, terminal_reason, created_at_ms, completed_at_ms, updated_at_ms
FROM review_publisher_runs WHERE review_run_id = ?
"#,
        )
        .bind(review_run_id)
        .fetch_optional(self.pool.as_ref())
        .await?
        else {
            return Ok(None);
        };
        let run = review_run_from_row(&row)?;
        let query = format!(
            "{} WHERE review_run_id = ? ORDER BY sequence ASC",
            outbox_select("SELECT")
        );
        let event_rows = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(review_run_id)
            .fetch_all(self.pool.as_ref())
            .await?;
        let events = event_rows
            .iter()
            .map(review_outbox_from_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Some(crate::ReviewPublisherRunSnapshot { run, events }))
    }

    pub async fn claim_next_due_event(
        &self,
        params: ReviewPublisherClaimParams,
    ) -> anyhow::Result<Option<crate::ReviewPublisherOutboxClaim>> {
        let now_ms = datetime_to_epoch_millis(params.now);
        let lease_expires_at_ms = now_ms
            .saturating_add(i64::try_from(params.lease_duration.as_millis()).unwrap_or(i64::MAX));
        let mut tx = self.pool.begin().await?;
        let query = format!(
            r#"{}
WHERE (
    (status = 'pending' AND next_attempt_at_ms <= ?)
    OR (status = 'in_flight' AND lease_expires_at_ms <= ?)
)
AND (
    sequence = 0
    OR EXISTS (
        SELECT 1 FROM review_publisher_outbox_events start_event
        WHERE start_event.review_run_id = review_publisher_outbox_events.review_run_id
          AND start_event.sequence = 0
          AND start_event.status = 'delivered'
    )
)
ORDER BY sequence ASC, next_attempt_at_ms ASC, created_at_ms ASC
LIMIT 1
"#,
            outbox_select("SELECT")
        );
        let Some(row) = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(now_ms)
            .bind(now_ms)
            .fetch_optional(&mut *tx)
            .await?
        else {
            tx.commit().await?;
            return Ok(None);
        };
        let event_id: String = row.try_get("event_id")?;
        let updated = sqlx::query(
            r#"
UPDATE review_publisher_outbox_events
SET status = 'in_flight', attempt_count = attempt_count + 1,
    lease_owner = ?, lease_expires_at_ms = ?, updated_at_ms = ?
WHERE event_id = ?
  AND (
      (status = 'pending' AND next_attempt_at_ms <= ?)
      OR (status = 'in_flight' AND lease_expires_at_ms <= ?)
  )
"#,
        )
        .bind(params.lease_owner.as_str())
        .bind(lease_expires_at_ms)
        .bind(now_ms)
        .bind(event_id.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(None);
        }
        let query = format!("{} WHERE event_id = ?", outbox_select("SELECT"));
        let claimed_row = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(event_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
        let event = review_outbox_from_row(&claimed_row)?;
        tx.commit().await?;
        Ok(Some(crate::ReviewPublisherOutboxClaim { event }))
    }

    pub async fn acknowledge_delivery(
        &self,
        params: ReviewPublisherDeliveryAckParams,
    ) -> anyhow::Result<bool> {
        let now_ms = datetime_to_epoch_millis(params.now);
        let updated = sqlx::query(
            r#"
UPDATE review_publisher_outbox_events
SET status = 'delivered', lease_owner = NULL, lease_expires_at_ms = NULL,
    receipt_id = ?, last_error_code = NULL, delivered_at_ms = ?, updated_at_ms = ?
WHERE event_id = ? AND status = 'in_flight' AND lease_owner = ?
"#,
        )
        .bind(params.receipt_id.as_deref())
        .bind(now_ms)
        .bind(now_ms)
        .bind(params.event_id.as_str())
        .bind(params.lease_owner.as_str())
        .execute(self.pool.as_ref())
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn fail_delivery(
        &self,
        params: ReviewPublisherDeliveryFailParams,
    ) -> anyhow::Result<bool> {
        let error_code = normalize_error_code(params.error_code.as_str())?;
        let now_ms = datetime_to_epoch_millis(params.now);
        let mut tx = self.pool.begin().await?;
        let status = match params.disposition {
            ReviewPublisherFailureDisposition::Retry => "pending",
            ReviewPublisherFailureDisposition::DeadLetter => "dead_letter",
        };
        let updated = sqlx::query(
            r#"
UPDATE review_publisher_outbox_events
SET status = ?, next_attempt_at_ms = ?, lease_owner = NULL,
    lease_expires_at_ms = NULL, last_error_code = ?, updated_at_ms = ?
WHERE event_id = ? AND status = 'in_flight' AND lease_owner = ?
"#,
        )
        .bind(status)
        .bind(datetime_to_epoch_millis(params.retry_at))
        .bind(error_code.as_str())
        .bind(now_ms)
        .bind(params.event_id.as_str())
        .bind(params.lease_owner.as_str())
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        if params.disposition == ReviewPublisherFailureDisposition::DeadLetter {
            let sequence = sqlx::query_scalar::<_, i64>(
                "SELECT sequence FROM review_publisher_outbox_events WHERE event_id = ?",
            )
            .bind(params.event_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
            if sequence == 0 {
                sqlx::query(
                    r#"
UPDATE review_publisher_outbox_events
SET status = 'dead_letter', last_error_code = 'blocked_by_start', updated_at_ms = ?
WHERE review_run_id = (
    SELECT review_run_id FROM review_publisher_outbox_events WHERE event_id = ?
)
AND sequence > 0 AND status = 'pending'
"#,
                )
                .bind(now_ms)
                .bind(params.event_id.as_str())
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }

    pub async fn exact_replay(
        &self,
        event_id: &str,
        payload_sha256: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<crate::ReviewPublisherOutboxEvent>> {
        let now_ms = datetime_to_epoch_millis(now);
        let updated = sqlx::query(
            r#"
UPDATE review_publisher_outbox_events
SET status = 'pending', attempt_count = 0, next_attempt_at_ms = ?,
    lease_owner = NULL, lease_expires_at_ms = NULL, receipt_id = NULL,
    last_error_code = NULL, delivered_at_ms = NULL, updated_at_ms = ?
WHERE event_id = ? AND payload_sha256 = ? AND status != 'in_flight'
"#,
        )
        .bind(now_ms)
        .bind(now_ms)
        .bind(event_id)
        .bind(payload_sha256)
        .execute(self.pool.as_ref())
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.get_outbox_event(event_id).await
    }

    pub async fn get_outbox_event(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<crate::ReviewPublisherOutboxEvent>> {
        let query = format!("{} WHERE event_id = ?", outbox_select("SELECT"));
        let row = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(event_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.as_ref().map(review_outbox_from_row).transpose()
    }
}

pub fn review_envelope_sha256(envelope: &ReviewEnvelope) -> anyhow::Result<String> {
    let mut unsigned = envelope.clone();
    unsigned.envelope_sha256.clear();
    Ok(sha256_hex(unsigned.canonical_json()?.as_bytes()))
}

pub fn review_candidate_sha256(
    repository_origin: &str,
    pull_request_number: u64,
    reviewed_base_sha: &str,
    head_sha: &str,
    merge_result_tree_sha: &str,
) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ReviewCandidateIdentity<'a> {
        repository_origin: &'a str,
        pull_request_number: u64,
        reviewed_base_sha: &'a str,
        head_sha: &'a str,
        merge_result_tree_sha: &'a str,
    }

    let identity = ReviewCandidateIdentity {
        repository_origin,
        pull_request_number,
        reviewed_base_sha,
        head_sha,
        merge_result_tree_sha,
    };
    Ok(sha256_hex(serde_json::to_vec(&identity)?.as_slice()))
}

pub fn map_review_output(
    output: Option<&ReviewOutputEvent>,
) -> (ReviewPublisherVerdict, Option<String>, Vec<i32>, String) {
    let Some(output) = output else {
        return (
            ReviewPublisherVerdict::NoGo,
            None,
            Vec::new(),
            "missing_or_interrupted_review_output".to_string(),
        );
    };
    let correctness = match output.overall_correctness.as_str() {
        "patch is correct" => Some("patch is correct".to_string()),
        "patch is incorrect" => Some("patch is incorrect".to_string()),
        _ => None,
    };
    let priorities = output
        .findings
        .iter()
        .map(|finding| finding.priority)
        .collect::<Vec<_>>();
    if correctness.as_deref() != Some("patch is correct") {
        return (
            ReviewPublisherVerdict::NoGo,
            correctness,
            priorities,
            "overall_correctness_not_exact_go".to_string(),
        );
    }
    if output
        .findings
        .iter()
        .any(|finding| !(0..=3).contains(&finding.priority))
    {
        return (
            ReviewPublisherVerdict::NoGo,
            correctness,
            priorities,
            "unknown_finding_priority".to_string(),
        );
    }
    if output.findings.iter().any(|finding| {
        finding_priority_from_title(finding.title.as_str()) != Some(finding.priority)
    }) {
        return (
            ReviewPublisherVerdict::NoGo,
            correctness,
            priorities,
            "finding_priority_label_mismatch".to_string(),
        );
    }
    if priorities.iter().any(|priority| matches!(priority, 0 | 1)) {
        return (
            ReviewPublisherVerdict::NoGo,
            correctness,
            priorities,
            "blocking_finding_present".to_string(),
        );
    }
    (
        ReviewPublisherVerdict::Go,
        correctness,
        priorities,
        "exact_go".to_string(),
    )
}

fn verify_review_envelope(envelope: &ReviewEnvelope) -> anyhow::Result<()> {
    if envelope.schema_version != REVIEW_ENVELOPE_SCHEMA_VERSION
        || envelope.envelope_sha256.len() != 64
        || review_envelope_sha256(envelope)? != envelope.envelope_sha256
    {
        anyhow::bail!(REVIEW_PUBLISHER_IMMUTABLE_CONFLICT);
    }
    Ok(())
}

fn publisher_event(
    review_run_id: &str,
    event_kind: ReviewPublisherEventKind,
    envelope: ReviewEnvelope,
    verdict: Option<ReviewPublisherVerdict>,
    overall_correctness: Option<String>,
    finding_priorities: Vec<i32>,
    terminal_reason: Option<String>,
) -> ReviewPublisherEvent {
    let sequence = match event_kind {
        ReviewPublisherEventKind::Started => 0,
        ReviewPublisherEventKind::Completed => 1,
    };
    ReviewPublisherEvent {
        schema_version: REVIEW_PUBLISHER_EVENT_SCHEMA_VERSION.to_string(),
        event_id: format!("review-event-{}-{sequence}", envelope.envelope_sha256),
        event_kind,
        review_run_id: review_run_id.to_string(),
        sequence,
        envelope,
        verdict,
        overall_correctness,
        finding_priorities,
        terminal_reason,
    }
}

fn review_run_id(envelope_sha256: &str) -> String {
    format!("review-run-{envelope_sha256}")
}

pub fn review_run_id_from_envelope_sha256(envelope_sha256: &str) -> String {
    review_run_id(envelope_sha256)
}

fn finding_priority_from_title(title: &str) -> Option<i32> {
    let bytes = title.as_bytes();
    if bytes.len() < 4 || bytes[0] != b'[' || bytes[1] != b'P' || bytes[3] != b']' {
        return None;
    }
    char::from(bytes[2]).to_digit(10).map(|value| value as i32)
}

fn normalize_terminal_reason(reason: &str) -> anyhow::Result<String> {
    let reason = reason.trim();
    if reason.is_empty()
        || reason.len() > 128
        || !reason
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
    {
        anyhow::bail!("invalid review publisher terminal reason");
    }
    Ok(reason.to_string())
}

fn normalize_error_code(code: &str) -> anyhow::Result<String> {
    normalize_terminal_reason(code)
}

fn verdict_str(verdict: ReviewPublisherVerdict) -> &'static str {
    match verdict {
        ReviewPublisherVerdict::Go => "GO",
        ReviewPublisherVerdict::NoGo => "NO_GO",
    }
}

fn verdict_from_str(value: Option<&str>) -> anyhow::Result<Option<ReviewPublisherVerdict>> {
    value
        .map(|value| match value {
            "GO" => Ok(ReviewPublisherVerdict::Go),
            "NO_GO" => Ok(ReviewPublisherVerdict::NoGo),
            other => anyhow::bail!("unknown review publisher verdict `{other}`"),
        })
        .transpose()
}

fn event_kind_str(kind: ReviewPublisherEventKind) -> &'static str {
    match kind {
        ReviewPublisherEventKind::Started => "started",
        ReviewPublisherEventKind::Completed => "completed",
    }
}

fn event_kind_from_str(value: &str) -> anyhow::Result<ReviewPublisherEventKind> {
    match value {
        "started" => Ok(ReviewPublisherEventKind::Started),
        "completed" => Ok(ReviewPublisherEventKind::Completed),
        other => anyhow::bail!("unknown review publisher event kind `{other}`"),
    }
}

fn review_run_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<crate::ReviewPublisherRun> {
    let verdict: Option<String> = row.try_get("verdict")?;
    Ok(crate::ReviewPublisherRun {
        review_run_id: row.try_get("review_run_id")?,
        thread_id: row.try_get("thread_id")?,
        turn_id: row.try_get("turn_id")?,
        envelope: serde_json::from_str(row.try_get("envelope_json")?)?,
        envelope_sha256: row.try_get("envelope_sha256")?,
        status: crate::ReviewPublisherRunStatus::try_from(
            row.try_get::<String, _>("status")?.as_str(),
        )?,
        verdict: verdict_from_str(verdict.as_deref())?,
        terminal_reason: row.try_get("terminal_reason")?,
        created_at: epoch_millis_to_datetime(row.try_get("created_at_ms")?)?,
        completed_at: row
            .try_get::<Option<i64>, _>("completed_at_ms")?
            .map(epoch_millis_to_datetime)
            .transpose()?,
        updated_at: epoch_millis_to_datetime(row.try_get("updated_at_ms")?)?,
    })
}

fn outbox_select(prefix: &str) -> String {
    format!(
        "{prefix} event_id, review_run_id, event_kind, sequence, status, payload_json, \
         payload_sha256, attempt_count, next_attempt_at_ms, lease_owner, \
         lease_expires_at_ms, receipt_id, last_error_code, created_at_ms, \
         delivered_at_ms, updated_at_ms FROM review_publisher_outbox_events"
    )
}

fn review_outbox_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ReviewPublisherOutboxEvent> {
    let event_kind = event_kind_from_str(row.try_get::<String, _>("event_kind")?.as_str())?;
    let event_id: String = row.try_get("event_id")?;
    let review_run_id: String = row.try_get("review_run_id")?;
    let sequence: u8 = row.try_get::<i64, _>("sequence")?.try_into()?;
    let payload_json: String = row.try_get("payload_json")?;
    let payload_sha256: String = row.try_get("payload_sha256")?;
    let payload: ReviewPublisherEvent = serde_json::from_str(payload_json.as_str())?;
    if payload.event_kind != event_kind
        || event_kind_str(payload.event_kind) != event_kind_str(event_kind)
        || matches!(event_kind, ReviewPublisherEventKind::Started) != (sequence == 0)
        || payload.event_id != event_id
        || payload.review_run_id != review_run_id
        || payload.sequence != sequence
        || sha256_hex(payload_json.as_bytes()) != payload_sha256
        || review_envelope_sha256(&payload.envelope)? != payload.envelope.envelope_sha256
    {
        anyhow::bail!(REVIEW_PUBLISHER_IMMUTABLE_CONFLICT);
    }
    Ok(crate::ReviewPublisherOutboxEvent {
        event_id,
        review_run_id,
        event_kind,
        sequence,
        status: crate::ReviewPublisherOutboxStatus::try_from(
            row.try_get::<String, _>("status")?.as_str(),
        )?,
        payload,
        payload_sha256,
        attempt_count: row.try_get::<i64, _>("attempt_count")?.try_into()?,
        next_attempt_at: epoch_millis_to_datetime(row.try_get("next_attempt_at_ms")?)?,
        lease_owner: row.try_get("lease_owner")?,
        lease_expires_at: row
            .try_get::<Option<i64>, _>("lease_expires_at_ms")?
            .map(epoch_millis_to_datetime)
            .transpose()?,
        receipt_id: row.try_get("receipt_id")?,
        last_error_code: row.try_get("last_error_code")?,
        created_at: epoch_millis_to_datetime(row.try_get("created_at_ms")?)?,
        delivered_at: row
            .try_get::<Option<i64>, _>("delivered_at_ms")?
            .map(epoch_millis_to_datetime)
            .transpose()?,
        updated_at: epoch_millis_to_datetime(row.try_get("updated_at_ms")?)?,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::unique_temp_dir;
    use codex_protocol::protocol::ReviewCodeLocation;
    use codex_protocol::protocol::ReviewFinding;
    use codex_protocol::protocol::ReviewImplementerProvenance;
    use codex_protocol::protocol::ReviewImplementerProvenanceSource;
    use codex_protocol::protocol::ReviewLineRange;
    use pretty_assertions::assert_eq;

    #[test]
    fn candidate_digest_has_a_feature_independent_typed_order() {
        let digest = review_candidate_sha256(
            "github.com/hasna/codewith",
            475,
            "06ff792de7303ba5867c8a329e0aca80cc65cbf2",
            "f69adb218518d88f2192dccb670d01b2a5402239",
            "fd2de011b53698b4b16dc2f4fcb3a2661760228c",
        )
        .expect("candidate digest");

        assert_eq!(
            digest,
            "a77396eb10afeb21407736ad90e43ccc391751e27c0f4254a2826bd7996f6b6a"
        );
    }

    fn envelope() -> ReviewEnvelope {
        let mut envelope = ReviewEnvelope {
            schema_version: REVIEW_ENVELOPE_SCHEMA_VERSION.to_string(),
            repository_origin: "github.com/hasna/codewith".to_string(),
            pull_request_number: 1,
            base_ref: "refs/remotes/origin/main".to_string(),
            reviewed_base_sha: "a".repeat(40),
            head_sha: "b".repeat(40),
            merge_result_tree_sha: "c".repeat(40),
            candidate_sha256: "d".repeat(64),
            acceptance_scope_id: "codewith-review-envelope-v1".to_string(),
            acceptance_scope_sha256: "e".repeat(64),
            implementer: ReviewImplementerProvenance {
                source: ReviewImplementerProvenanceSource::GitAgentTrailer,
                agent: "Herminia".to_string(),
                commit_sha: "b".repeat(40),
            },
            envelope_sha256: String::new(),
        };
        envelope.envelope_sha256 = review_envelope_sha256(&envelope).expect("digest");
        envelope
    }

    fn finding(priority: i32, title_priority: i32) -> ReviewFinding {
        ReviewFinding {
            title: format!("[P{title_priority}] finding"),
            body: "body".to_string(),
            confidence_score: 1.0,
            priority,
            code_location: ReviewCodeLocation {
                absolute_file_path: "/tmp/file.rs".into(),
                line_range: ReviewLineRange { start: 1, end: 1 },
            },
        }
    }

    fn output(correctness: &str, findings: Vec<ReviewFinding>) -> ReviewOutputEvent {
        ReviewOutputEvent {
            findings,
            overall_correctness: correctness.to_string(),
            overall_explanation: "ignored untrusted prose".to_string(),
            overall_confidence_score: 1.0,
        }
    }

    async fn runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state runtime")
    }

    #[test]
    fn verdict_mapping_is_fail_closed() {
        assert_eq!(ReviewPublisherVerdict::NoGo, map_review_output(None).0);
        assert_eq!(
            ReviewPublisherVerdict::NoGo,
            map_review_output(Some(&output("patch is correct ", Vec::new()))).0
        );
        assert_eq!(
            ReviewPublisherVerdict::NoGo,
            map_review_output(Some(&output("patch is correct", vec![finding(4, 4)]))).0
        );
        assert_eq!(
            ReviewPublisherVerdict::NoGo,
            map_review_output(Some(&output("patch is correct", vec![finding(2, 3)]))).0
        );
        assert_eq!(
            ReviewPublisherVerdict::NoGo,
            map_review_output(Some(&output("patch is correct", vec![finding(1, 1)]))).0
        );
        assert_eq!(
            ReviewPublisherVerdict::Go,
            map_review_output(Some(&output("patch is correct", vec![finding(2, 2)]))).0
        );
    }

    #[tokio::test]
    async fn outbox_orders_deduplicates_reclaims_and_replays_exact_payload() {
        let runtime = runtime().await;
        let store = runtime.review_publisher();
        let envelope = envelope();
        let started = store
            .start_review_run(ReviewPublisherStartParams {
                thread_id: "thread".to_string(),
                envelope: envelope.clone(),
            })
            .await
            .expect("start");
        let duplicate = store
            .start_review_run(ReviewPublisherStartParams {
                thread_id: "thread".to_string(),
                envelope: envelope.clone(),
            })
            .await
            .expect("duplicate start");
        assert_eq!(started, duplicate);
        assert_eq!(started.events[0].payload.envelope, envelope);
        store
            .complete_review_run(ReviewPublisherCompleteParams {
                review_run_id: started.run.review_run_id.clone(),
                envelope_sha256: envelope.envelope_sha256.clone(),
                review_output: Some(output("patch is correct", Vec::new())),
                terminal_reason_override: None,
            })
            .await
            .expect("complete");

        let now = Utc::now();
        let first = store
            .claim_next_due_event(ReviewPublisherClaimParams {
                lease_owner: "owner-a".to_string(),
                lease_duration: Duration::from_millis(1),
                now,
            })
            .await
            .expect("claim")
            .expect("start claim");
        assert_eq!(0, first.event.sequence);
        assert!(
            store
                .claim_next_due_event(ReviewPublisherClaimParams {
                    lease_owner: "owner-b".to_string(),
                    lease_duration: Duration::from_secs(1),
                    now,
                })
                .await
                .expect("claim while leased")
                .is_none()
        );
        let reclaimed = store
            .claim_next_due_event(ReviewPublisherClaimParams {
                lease_owner: "owner-b".to_string(),
                lease_duration: Duration::from_secs(1),
                now: now + chrono::Duration::milliseconds(2),
            })
            .await
            .expect("reclaim")
            .expect("expired lease claim");
        assert_eq!(2, reclaimed.event.attempt_count);
        store
            .acknowledge_delivery(ReviewPublisherDeliveryAckParams {
                event_id: reclaimed.event.event_id.clone(),
                lease_owner: "owner-b".to_string(),
                receipt_id: Some("receipt".to_string()),
                now,
            })
            .await
            .expect("ack");
        let terminal = store
            .claim_next_due_event(ReviewPublisherClaimParams {
                lease_owner: "owner-b".to_string(),
                lease_duration: Duration::from_secs(1),
                now,
            })
            .await
            .expect("terminal claim")
            .expect("terminal");
        assert_eq!(1, terminal.event.sequence);
        assert!(
            !serde_json::to_string(&terminal.event.payload)
                .expect("event json")
                .contains("ignored untrusted prose")
        );
        store
            .acknowledge_delivery(ReviewPublisherDeliveryAckParams {
                event_id: terminal.event.event_id.clone(),
                lease_owner: "owner-b".to_string(),
                receipt_id: None,
                now,
            })
            .await
            .expect("terminal ack");
        let replayed = store
            .exact_replay(
                terminal.event.event_id.as_str(),
                terminal.event.payload_sha256.as_str(),
                now,
            )
            .await
            .expect("replay")
            .expect("exact event");
        assert_eq!(terminal.event.payload, replayed.payload);
        assert!(
            store
                .exact_replay(terminal.event.event_id.as_str(), "wrong-digest", now,)
                .await
                .expect("mismatch")
                .is_none()
        );
    }

    #[tokio::test]
    async fn dead_lettered_start_blocks_and_dead_letters_terminal_event() {
        let runtime = runtime().await;
        let store = runtime.review_publisher();
        let envelope = envelope();
        let started = store
            .start_review_run(ReviewPublisherStartParams {
                thread_id: "thread".to_string(),
                envelope: envelope.clone(),
            })
            .await
            .expect("start");
        store
            .complete_review_run(ReviewPublisherCompleteParams {
                review_run_id: started.run.review_run_id.clone(),
                envelope_sha256: envelope.envelope_sha256,
                review_output: None,
                terminal_reason_override: None,
            })
            .await
            .expect("complete");
        let now = Utc::now();
        let start = store
            .claim_next_due_event(ReviewPublisherClaimParams {
                lease_owner: "owner".to_string(),
                lease_duration: Duration::from_secs(30),
                now,
            })
            .await
            .expect("claim")
            .expect("start event");
        store
            .fail_delivery(ReviewPublisherDeliveryFailParams {
                event_id: start.event.event_id,
                lease_owner: "owner".to_string(),
                error_code: "http_401".to_string(),
                disposition: ReviewPublisherFailureDisposition::DeadLetter,
                retry_at: now,
                now,
            })
            .await
            .expect("dead letter start");

        let snapshot = store
            .get_review_run(started.run.review_run_id.as_str())
            .await
            .expect("read run")
            .expect("run");
        assert_eq!(snapshot.events.len(), 2);
        assert!(
            snapshot
                .events
                .iter()
                .all(|event| event.status == crate::ReviewPublisherOutboxStatus::DeadLetter)
        );
        assert!(
            store
                .claim_next_due_event(ReviewPublisherClaimParams {
                    lease_owner: "owner".to_string(),
                    lease_duration: Duration::from_secs(30),
                    now,
                })
                .await
                .expect("claim after dead letter")
                .is_none()
        );
    }
}
