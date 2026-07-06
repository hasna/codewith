use super::*;
use crate::model::ThreadMonitorEventRow;
use crate::model::ThreadMonitorRow;
use uuid::Uuid;

#[derive(Clone)]
pub struct MonitorStore {
    pool: Arc<SqlitePool>,
}

impl MonitorStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

pub struct ThreadMonitorCreateParams {
    pub thread_id: ThreadId,
    pub name: String,
    pub prompt: String,
    pub command: String,
    pub cwd: Option<String>,
    pub routing: crate::ThreadMonitorRouting,
    pub output_file: Option<String>,
    pub status: crate::ThreadMonitorStatus,
}

pub struct ThreadMonitorUpdate {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<Option<String>>,
    pub routing: Option<crate::ThreadMonitorRouting>,
    pub output_file: Option<Option<String>>,
    pub status: Option<crate::ThreadMonitorStatus>,
    pub generation: Option<i64>,
    pub process_id: Option<Option<i64>>,
    pub last_event_at: Option<Option<DateTime<Utc>>>,
    pub last_error: Option<Option<String>>,
}

pub struct ThreadMonitorEventCreateParams {
    pub thread_id: ThreadId,
    pub monitor_id: String,
    pub stream: crate::ThreadMonitorEventStream,
    pub text: String,
}

impl MonitorStore {
    pub async fn create_thread_monitor(
        &self,
        params: ThreadMonitorCreateParams,
    ) -> anyhow::Result<crate::ThreadMonitor> {
        let monitor_id = Uuid::new_v4().to_string();
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let sql = thread_monitor_returning(
            r#"
INSERT INTO thread_monitors (
    monitor_id,
    thread_id,
    name,
    prompt,
    command,
    cwd,
    routing,
    output_file,
    status,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(monitor_id)
            .bind(params.thread_id.to_string())
            .bind(redact_state_string(params.name))
            .bind(redact_state_string(params.prompt))
            .bind(redact_state_string(params.command))
            .bind(redact_state_optional_string(params.cwd))
            .bind(params.routing.as_str())
            .bind(redact_state_optional_string(params.output_file))
            .bind(params.status.as_str())
            .bind(now_ms)
            .bind(now_ms)
            .fetch_one(self.pool.as_ref())
            .await?;
        thread_monitor_from_row(&row)
    }

    pub async fn get_thread_monitor(
        &self,
        monitor_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadMonitor>> {
        let sql = thread_monitor_select_by_id(
            r#"
SELECT
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(monitor_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| thread_monitor_from_row(&row)).transpose()
    }

    pub async fn list_thread_monitors(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Vec<crate::ThreadMonitor>> {
        let rows = sqlx::query(
            r#"
SELECT
    monitor_id,
    thread_id,
    name,
    prompt,
    command,
    cwd,
    routing,
    output_file,
    status,
    generation,
    process_id,
    last_event_at_ms,
    last_error,
    created_at_ms,
    updated_at_ms
FROM thread_monitors
WHERE thread_id = ?
ORDER BY status, updated_at_ms DESC, created_at_ms DESC
            "#,
        )
        .bind(thread_id.to_string())
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.iter().map(thread_monitor_from_row).collect()
    }

    pub async fn list_running_thread_monitors(&self) -> anyhow::Result<Vec<crate::ThreadMonitor>> {
        let rows = sqlx::query(
            r#"
SELECT
    monitor_id,
    thread_id,
    name,
    prompt,
    command,
    cwd,
    routing,
    output_file,
    status,
    generation,
    process_id,
    last_event_at_ms,
    last_error,
    created_at_ms,
    updated_at_ms
FROM thread_monitors
WHERE status = 'running'
ORDER BY updated_at_ms, monitor_id
            "#,
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.iter().map(thread_monitor_from_row).collect()
    }

    pub async fn update_thread_monitor(
        &self,
        monitor_id: &str,
        update: ThreadMonitorUpdate,
    ) -> anyhow::Result<Option<crate::ThreadMonitor>> {
        let Some(existing) = self.get_thread_monitor(monitor_id).await? else {
            return Ok(None);
        };
        let name = update.name.unwrap_or(existing.name);
        let prompt = update.prompt.unwrap_or(existing.prompt);
        let command = update.command.unwrap_or(existing.command);
        let cwd = update.cwd.unwrap_or(existing.cwd);
        let routing = update.routing.unwrap_or(existing.routing);
        let output_file = update.output_file.unwrap_or(existing.output_file);
        let status = update.status.unwrap_or(existing.status);
        let generation = update.generation.unwrap_or(existing.generation);
        let process_id = update.process_id.unwrap_or(existing.process_id);
        let last_event_at = update.last_event_at.unwrap_or(existing.last_event_at);
        let last_error = update.last_error.unwrap_or(existing.last_error);
        let name = redact_state_string(name);
        let prompt = redact_state_string(prompt);
        let command = redact_state_string(command);
        let cwd = redact_state_optional_string(cwd);
        let output_file = redact_state_optional_string(output_file);
        let last_error = redact_state_optional_string(last_error);
        let sql = thread_monitor_returning(
            r#"
UPDATE thread_monitors
SET
    name = ?,
    prompt = ?,
    command = ?,
    cwd = ?,
    routing = ?,
    output_file = ?,
    status = ?,
    generation = ?,
    process_id = ?,
    last_event_at_ms = ?,
    last_error = ?,
    updated_at_ms = ?
WHERE monitor_id = ?
RETURNING
"#,
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(name)
            .bind(prompt)
            .bind(command)
            .bind(cwd)
            .bind(routing.as_str())
            .bind(output_file)
            .bind(status.as_str())
            .bind(generation)
            .bind(process_id)
            .bind(last_event_at.map(datetime_to_epoch_millis))
            .bind(last_error)
            .bind(datetime_to_epoch_millis(Utc::now()))
            .bind(monitor_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| thread_monitor_from_row(&row)).transpose()
    }

    pub async fn set_thread_monitor_status(
        &self,
        monitor_id: &str,
        status: crate::ThreadMonitorStatus,
        last_error: Option<String>,
    ) -> anyhow::Result<Option<crate::ThreadMonitor>> {
        self.update_thread_monitor(
            monitor_id,
            ThreadMonitorUpdate {
                name: None,
                prompt: None,
                command: None,
                cwd: None,
                routing: None,
                output_file: None,
                status: Some(status),
                generation: None,
                process_id: Some(None),
                last_event_at: None,
                last_error: Some(last_error),
            },
        )
        .await
    }

    pub async fn restart_thread_monitor(
        &self,
        monitor_id: &str,
    ) -> anyhow::Result<Option<crate::ThreadMonitor>> {
        let Some(existing) = self.get_thread_monitor(monitor_id).await? else {
            return Ok(None);
        };
        self.update_thread_monitor(
            monitor_id,
            ThreadMonitorUpdate {
                name: None,
                prompt: None,
                command: None,
                cwd: None,
                routing: None,
                output_file: None,
                status: Some(crate::ThreadMonitorStatus::Running),
                generation: Some(existing.generation + 1),
                process_id: Some(None),
                last_event_at: None,
                last_error: Some(None),
            },
        )
        .await
    }

    pub async fn mark_thread_monitor_started(
        &self,
        monitor_id: &str,
        generation: i64,
        process_id: Option<i64>,
    ) -> anyhow::Result<Option<crate::ThreadMonitor>> {
        let Some(existing) = self.get_thread_monitor(monitor_id).await? else {
            return Ok(None);
        };
        if existing.generation != generation {
            return Ok(Some(existing));
        }
        self.update_thread_monitor(
            monitor_id,
            ThreadMonitorUpdate {
                name: None,
                prompt: None,
                command: None,
                cwd: None,
                routing: None,
                output_file: None,
                status: Some(crate::ThreadMonitorStatus::Running),
                generation: None,
                process_id: Some(process_id),
                last_event_at: None,
                last_error: Some(None),
            },
        )
        .await
    }

    pub async fn delete_thread_monitor(&self, monitor_id: &str) -> anyhow::Result<bool> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM thread_monitor_events WHERE monitor_id = ?")
            .bind(monitor_id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM thread_monitors WHERE monitor_id = ?")
            .bind(monitor_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_thread_monitors_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<u64> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM thread_monitor_events WHERE thread_id = ?")
            .bind(thread_id.to_string())
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM thread_monitors WHERE thread_id = ?")
            .bind(thread_id.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn create_thread_monitor_event(
        &self,
        params: ThreadMonitorEventCreateParams,
    ) -> anyhow::Result<crate::ThreadMonitorEvent> {
        let event_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let text = redact_state_string(params.text);
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            r#"
INSERT INTO thread_monitor_events (
    event_id,
    monitor_id,
    thread_id,
    stream,
    text,
    created_at_ms
) VALUES (?, ?, ?, ?, ?, ?)
RETURNING
    event_id,
    monitor_id,
    thread_id,
    stream,
    text,
    created_at_ms
            "#,
        )
        .bind(event_id)
        .bind(params.monitor_id)
        .bind(params.thread_id.to_string())
        .bind(params.stream.as_str())
        .bind(text)
        .bind(now_ms)
        .fetch_one(&mut *tx)
        .await?;
        let last_error: Option<String> = if params.stream == crate::ThreadMonitorEventStream::Stderr
        {
            Some(row.try_get::<String, _>("text")?)
        } else {
            None
        };
        let monitor_id: String = row.try_get("monitor_id")?;
        sqlx::query(
            r#"
UPDATE thread_monitors
SET
    last_event_at_ms = ?,
    last_error = COALESCE(?, last_error),
    updated_at_ms = ?
WHERE monitor_id = ?
            "#,
        )
        .bind(now_ms)
        .bind(last_error)
        .bind(now_ms)
        .bind(monitor_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        thread_monitor_event_from_row(&row)
    }

    pub async fn list_thread_monitor_events(
        &self,
        monitor_id: &str,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<crate::ThreadMonitorEvent>> {
        let rows = sqlx::query(
            r#"
SELECT
    event_id,
    monitor_id,
    thread_id,
    stream,
    text,
    created_at_ms
FROM thread_monitor_events
WHERE monitor_id = ?
ORDER BY created_at_ms, event_id
LIMIT ? OFFSET ?
            "#,
        )
        .bind(monitor_id)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.iter().map(thread_monitor_event_from_row).collect()
    }

    pub async fn count_thread_monitor_events(&self, monitor_id: &str) -> anyhow::Result<usize> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM thread_monitor_events WHERE monitor_id = ?")
                .bind(monitor_id)
                .fetch_one(self.pool.as_ref())
                .await?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }
}

fn thread_monitor_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<crate::ThreadMonitor> {
    ThreadMonitorRow::try_from_row(row)?.try_into()
}

fn thread_monitor_event_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ThreadMonitorEvent> {
    ThreadMonitorEventRow::try_from_row(row)?.try_into()
}

fn thread_monitor_select_columns() -> &'static str {
    r#"
    monitor_id,
    thread_id,
    name,
    prompt,
    command,
    cwd,
    routing,
    output_file,
    status,
    generation,
    process_id,
    last_event_at_ms,
    last_error,
    created_at_ms,
    updated_at_ms
"#
}

fn thread_monitor_returning(prefix: &'static str) -> String {
    format!("{}{}", prefix, thread_monitor_select_columns())
}

fn thread_monitor_select_by_id(prefix: &'static str) -> String {
    format!(
        "{}{}FROM thread_monitors WHERE monitor_id = ?",
        prefix,
        thread_monitor_select_columns()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_thread_id(id: u32) -> ThreadId {
        ThreadId::from_string(&format!("00000000-0000-0000-0000-{id:012}"))
            .expect("valid thread id")
    }

    fn synthetic_secret() -> String {
        format!("{}{}", "sk-proj-", "a".repeat(32))
    }

    async fn upsert_test_thread(runtime: &StateRuntime, thread_id: ThreadId) {
        let metadata = test_thread_metadata(
            runtime.codex_home(),
            thread_id,
            runtime.codex_home().join("workspace"),
        );
        runtime
            .upsert_thread(&metadata)
            .await
            .expect("test thread should be upserted");
    }

    async fn create_test_monitor(
        runtime: &StateRuntime,
        thread_id: ThreadId,
    ) -> crate::ThreadMonitor {
        runtime
            .thread_monitors()
            .create_thread_monitor(ThreadMonitorCreateParams {
                thread_id,
                name: "CI watcher".to_string(),
                prompt: "watch CI".to_string(),
                command: "while true; do echo ok; sleep 60; done".to_string(),
                cwd: None,
                routing: crate::ThreadMonitorRouting::Stream,
                output_file: None,
                status: crate::ThreadMonitorStatus::Running,
            })
            .await
            .expect("monitor should be created")
    }

    #[tokio::test]
    async fn create_event_restart_and_delete_thread_monitor() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 1);
        upsert_test_thread(&runtime, thread_id).await;

        let created = create_test_monitor(&runtime, thread_id).await;
        assert_eq!(thread_id, created.thread_id);
        assert_eq!("CI watcher", created.name);
        assert_eq!(crate::ThreadMonitorStatus::Running, created.status);
        assert_eq!(0, created.generation);

        let event = runtime
            .thread_monitors()
            .create_thread_monitor_event(ThreadMonitorEventCreateParams {
                thread_id,
                monitor_id: created.monitor_id.clone(),
                stream: crate::ThreadMonitorEventStream::Stdout,
                text: "CI is green".to_string(),
            })
            .await
            .expect("event should be created");
        assert_eq!(created.monitor_id, event.monitor_id);

        let events = runtime
            .thread_monitors()
            .list_thread_monitor_events(
                created.monitor_id.as_str(),
                /*offset*/ 0,
                /*limit*/ 10,
            )
            .await
            .expect("events should list");
        assert_eq!(vec![event], events);

        let restarted = runtime
            .thread_monitors()
            .restart_thread_monitor(created.monitor_id.as_str())
            .await
            .expect("restart should succeed")
            .expect("monitor should exist");
        assert_eq!(crate::ThreadMonitorStatus::Running, restarted.status);
        assert_eq!(1, restarted.generation);

        let deleted = runtime
            .thread_monitors()
            .delete_thread_monitor(created.monitor_id.as_str())
            .await
            .expect("delete should succeed");
        assert!(deleted);
        assert!(
            runtime
                .thread_monitors()
                .get_thread_monitor(created.monitor_id.as_str())
                .await
                .expect("read should succeed")
                .is_none()
        );
    }

    #[tokio::test]
    async fn monitor_fields_and_events_are_redacted_before_write() {
        let runtime = test_runtime().await;
        let thread_id = test_thread_id(/*id*/ 9);
        upsert_test_thread(&runtime, thread_id).await;
        let secret = synthetic_secret();

        let created = runtime
            .thread_monitors()
            .create_thread_monitor(ThreadMonitorCreateParams {
                thread_id,
                name: format!("CI {secret}"),
                prompt: format!("watch {secret}"),
                command: format!("printf '%s' {secret}"),
                cwd: None,
                routing: crate::ThreadMonitorRouting::Stream,
                output_file: None,
                status: crate::ThreadMonitorStatus::Running,
            })
            .await
            .expect("monitor should be created");
        assert!(
            created
                .prompt
                .contains(crate::local_state_redaction_marker())
        );
        assert!(!created.prompt.contains(secret.as_str()));
        assert!(!created.command.contains(secret.as_str()));

        let event = runtime
            .thread_monitors()
            .create_thread_monitor_event(ThreadMonitorEventCreateParams {
                thread_id,
                monitor_id: created.monitor_id.clone(),
                stream: crate::ThreadMonitorEventStream::Stderr,
                text: format!("failed with {secret}"),
            })
            .await
            .expect("event should be created");
        assert!(event.text.contains(crate::local_state_redaction_marker()));
        assert!(!event.text.contains(secret.as_str()));

        let reloaded = runtime
            .thread_monitors()
            .get_thread_monitor(created.monitor_id.as_str())
            .await
            .expect("monitor query should work")
            .expect("monitor should exist");
        assert_eq!(reloaded.last_error.as_deref(), Some(event.text.as_str()));
        assert!(
            !reloaded
                .last_error
                .unwrap_or_default()
                .contains(secret.as_str())
        );
    }
}
