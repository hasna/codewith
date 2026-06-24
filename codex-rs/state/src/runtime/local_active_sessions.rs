use super::*;
use sqlx::sqlite::SqliteRow;

#[derive(Clone)]
pub struct LocalActiveSessionStore {
    pool: Arc<SqlitePool>,
}

impl LocalActiveSessionStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalActiveSessionRecord {
    pub thread_id: ThreadId,
    pub owner_id: String,
    pub session_id: String,
    pub pid: Option<u32>,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalActiveSessionHeartbeatParams {
    pub thread_id: ThreadId,
    pub owner_id: String,
    pub session_id: String,
    pub pid: Option<u32>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalActiveSessionPruneOwnerParams {
    pub owner_id: String,
    pub active_thread_ids: Vec<ThreadId>,
    pub observed_at: DateTime<Utc>,
}

impl LocalActiveSessionStore {
    pub async fn heartbeat_session(
        &self,
        params: LocalActiveSessionHeartbeatParams,
    ) -> anyhow::Result<LocalActiveSessionRecord> {
        let LocalActiveSessionHeartbeatParams {
            thread_id,
            owner_id,
            session_id,
            pid,
            now,
        } = params;
        let now_ms = datetime_to_epoch_millis(now);
        let pid = pid.map(i64::from);
        let row = sqlx::query(
            r#"
INSERT INTO local_active_sessions (
    thread_id,
    owner_id,
    session_id,
    pid,
    last_seen_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(thread_id) DO UPDATE SET
    owner_id = excluded.owner_id,
    session_id = excluded.session_id,
    pid = excluded.pid,
    last_seen_at_ms = excluded.last_seen_at_ms,
    updated_at_ms = excluded.updated_at_ms
WHERE
    excluded.last_seen_at_ms >= local_active_sessions.last_seen_at_ms
RETURNING
    thread_id,
    owner_id,
    session_id,
    pid,
    last_seen_at_ms,
    created_at_ms,
    updated_at_ms
            "#,
        )
        .bind(thread_id.to_string())
        .bind(owner_id)
        .bind(session_id)
        .bind(pid)
        .bind(now_ms)
        .bind(now_ms)
        .bind(now_ms)
        .fetch_optional(self.pool.as_ref())
        .await?;
        if let Some(row) = row {
            return local_active_session_from_row(&row);
        }
        self.get_session(thread_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("local active session missing after heartbeat"))
    }

    pub async fn prune_owner_sessions(
        &self,
        params: LocalActiveSessionPruneOwnerParams,
    ) -> anyhow::Result<u64> {
        let mut builder =
            QueryBuilder::<Sqlite>::new("DELETE FROM local_active_sessions WHERE owner_id = ");
        builder.push_bind(params.owner_id);
        builder.push(" AND last_seen_at_ms < ");
        builder.push_bind(datetime_to_epoch_millis(params.observed_at));
        if !params.active_thread_ids.is_empty() {
            builder.push(" AND thread_id NOT IN (");
            let mut separated = builder.separated(", ");
            for thread_id in params.active_thread_ids {
                separated.push_bind(thread_id.to_string());
            }
            separated.push_unseparated(")");
        }
        let result = builder.build().execute(self.pool.as_ref()).await?;
        Ok(result.rows_affected())
    }

    pub async fn prune_stale_sessions(&self, stale_before: DateTime<Utc>) -> anyhow::Result<u64> {
        let result = sqlx::query("DELETE FROM local_active_sessions WHERE last_seen_at_ms < ?")
            .bind(datetime_to_epoch_millis(stale_before))
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn get_session(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<LocalActiveSessionRecord>> {
        let row = sqlx::query(
            r#"
SELECT
    thread_id,
    owner_id,
    session_id,
    pid,
    last_seen_at_ms,
    created_at_ms,
    updated_at_ms
FROM local_active_sessions
WHERE thread_id = ?
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(|row| local_active_session_from_row(&row))
            .transpose()
    }

    pub async fn get_fresh_session(
        &self,
        thread_id: ThreadId,
        fresh_after: DateTime<Utc>,
    ) -> anyhow::Result<Option<LocalActiveSessionRecord>> {
        let row = sqlx::query(
            r#"
SELECT
    thread_id,
    owner_id,
    session_id,
    pid,
    last_seen_at_ms,
    created_at_ms,
    updated_at_ms
FROM local_active_sessions
WHERE thread_id = ? AND last_seen_at_ms >= ?
            "#,
        )
        .bind(thread_id.to_string())
        .bind(datetime_to_epoch_millis(fresh_after))
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(|row| local_active_session_from_row(&row))
            .transpose()
    }
}

fn local_active_session_from_row(row: &SqliteRow) -> anyhow::Result<LocalActiveSessionRecord> {
    let thread_id = ThreadId::try_from(row.try_get::<String, _>("thread_id")?)?;
    let pid = row
        .try_get::<Option<i64>, _>("pid")?
        .map(u32::try_from)
        .transpose()?;
    Ok(LocalActiveSessionRecord {
        thread_id,
        owner_id: row.try_get("owner_id")?,
        session_id: row.try_get("session_id")?,
        pid,
        last_seen_at: epoch_millis_to_datetime(row.try_get("last_seen_at_ms")?)?,
        created_at: epoch_millis_to_datetime(row.try_get("created_at_ms")?)?,
        updated_at: epoch_millis_to_datetime(row.try_get("updated_at_ms")?)?,
    })
}
