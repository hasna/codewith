use super::path_keys::managed_worktree_path_key_from_db_string;
use sqlx::QueryBuilder;
use sqlx::Sqlite;
use sqlx::SqlitePool;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
#[cfg(test)]
use std::sync::atomic::Ordering;
use tracing::info;
use tracing::warn;

const MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE: i64 = 100;

const LEGACY_MANAGED_WORKTREE_PATH_SELECT: &str = r#"
SELECT
    m.worktree_id, m.base_repo_path, m.worktree_path,
    m.base_repo_path_key, m.worktree_path_key,
    COALESCE(t.base_repo_path_terminal, 0) AS base_repo_path_terminal,
    COALESCE(t.worktree_path_terminal, 0) AS worktree_path_terminal
FROM managed_worktrees AS m
LEFT JOIN managed_worktree_path_key_backfill_terminal AS t ON t.worktree_id = m.worktree_id
"#;
const LEGACY_MANAGED_WORKTREE_PATH_ORDER: &str = " ORDER BY m.worktree_id ASC LIMIT ";

#[cfg(test)]
struct BackfillReadHook {
    attempts: AtomicUsize,
    race: tokio::sync::Barrier,
}

#[cfg(test)]
impl BackfillReadHook {
    fn new() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
            race: tokio::sync::Barrier::new(2),
        }
    }

    async fn after_read(&self) {
        if self.attempts.fetch_add(1, Ordering::SeqCst) != 0 {
            return;
        }
        self.race.wait().await;
        self.race.wait().await;
    }
}

#[cfg(not(test))]
type BackfillReadHook = ();

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct ManagedWorktreePathKeyBackfillOutcome {
    scanned_rows: u32,
    updated_rows: u32,
    unkeyable_paths: u32,
    worktree_path_collisions: u32,
    completed: bool,
}

#[derive(sqlx::FromRow)]
struct LegacyManagedWorktreePathRow {
    worktree_id: String,
    base_repo_path: String,
    worktree_path: String,
    base_repo_path_key: Option<Vec<u8>>,
    worktree_path_key: Option<Vec<u8>>,
    base_repo_path_terminal: bool,
    worktree_path_terminal: bool,
}

pub(crate) async fn backfill_legacy_managed_worktree_path_keys(
    pool: &SqlitePool,
) -> anyhow::Result<ManagedWorktreePathKeyBackfillOutcome> {
    backfill_legacy_managed_worktree_path_keys_with_hook(pool, None).await
}

async fn backfill_legacy_managed_worktree_path_keys_with_hook(
    pool: &SqlitePool,
    read_hook: Option<&BackfillReadHook>,
) -> anyhow::Result<ManagedWorktreePathKeyBackfillOutcome> {
    let outcome = crate::busy_retry::retry_on_busy("backfill managed worktree path keys", || {
        backfill_legacy_managed_worktree_path_keys_once(pool, read_hook)
    })
    .await?;
    if outcome.scanned_rows > 0 {
        info!(
            scanned_rows = outcome.scanned_rows,
            updated_rows = outcome.updated_rows,
            unkeyable_paths = outcome.unkeyable_paths,
            worktree_path_collisions = outcome.worktree_path_collisions,
            completed = outcome.completed,
            "bounded managed worktree path-key backfill finished"
        );
    }
    Ok(outcome)
}

async fn backfill_legacy_managed_worktree_path_keys_once(
    pool: &SqlitePool,
    _read_hook: Option<&BackfillReadHook>,
) -> anyhow::Result<ManagedWorktreePathKeyBackfillOutcome> {
    let mut tx = pool.begin().await?;
    let last_scanned_worktree_id: Option<String> = sqlx::query_scalar(
        "SELECT last_scanned_worktree_id FROM managed_worktree_path_key_backfill_state WHERE id = 1",
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let mut query = QueryBuilder::<Sqlite>::new(LEGACY_MANAGED_WORKTREE_PATH_SELECT);
    if let Some(last_scanned_worktree_id) = last_scanned_worktree_id.as_deref() {
        query
            .push(" WHERE m.worktree_id > ")
            .push_bind(last_scanned_worktree_id);
    }
    let mut rows = query
        .push(LEGACY_MANAGED_WORKTREE_PATH_ORDER)
        .push_bind(MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE)
        .build_query_as::<LegacyManagedWorktreePathRow>()
        .fetch_all(&mut *tx)
        .await?;
    #[cfg(test)]
    if let Some(read_hook) = _read_hook {
        read_hook.after_read().await;
    }
    let window_rows = rows.len();
    let next_last_scanned_worktree_id = (window_rows
        == MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE as usize)
        .then(|| rows.last().map(|row| row.worktree_id.clone()))
        .flatten();
    rows.retain(|row| {
        (row.base_repo_path_key.is_none() && !row.base_repo_path_terminal)
            || (row.worktree_path_key.is_none() && !row.worktree_path_terminal)
    });
    let mut outcome = ManagedWorktreePathKeyBackfillOutcome {
        scanned_rows: rows.len() as u32,
        completed: window_rows < MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE as usize,
        ..Default::default()
    };

    for row in rows {
        let base_repo_path_key = match (
            row.base_repo_path_key.is_none(),
            row.base_repo_path_terminal,
        ) {
            (true, false) => managed_worktree_path_key_from_db_string(&row.base_repo_path),
            _ => None,
        };
        let worktree_path_key = match (row.worktree_path_key.is_none(), row.worktree_path_terminal)
        {
            (true, false) => managed_worktree_path_key_from_db_string(&row.worktree_path),
            _ => None,
        };
        let base_repo_path_terminal = row.base_repo_path_key.is_none()
            && !row.base_repo_path_terminal
            && base_repo_path_key.is_none();
        let worktree_path_terminal = row.worktree_path_key.is_none()
            && !row.worktree_path_terminal
            && worktree_path_key.is_none();
        let unkeyable_paths =
            u32::from(base_repo_path_terminal) + u32::from(worktree_path_terminal);
        if unkeyable_paths > 0 {
            outcome.unkeyable_paths += unkeyable_paths;
            warn!(
                worktree_id = row.worktree_id,
                unkeyable_paths, "retaining legacy managed worktree row with unkeyable path"
            );
        }
        if let Some(worktree_path_key) = worktree_path_key.as_ref() {
            let collision: Option<String> = sqlx::query_scalar(
                r#"
SELECT worktree_id
FROM managed_worktrees
WHERE worktree_id != ? AND worktree_path_key = ?
LIMIT 1
                "#,
            )
            .bind(row.worktree_id.as_str())
            .bind(worktree_path_key)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(existing_worktree_id) = collision {
                outcome.worktree_path_collisions += 1;
                warn!(
                    worktree_id = row.worktree_id,
                    existing_worktree_id,
                    "retaining managed worktree path-key collision without selecting a winner"
                );
            }
        }
        if base_repo_path_key.is_some() || worktree_path_key.is_some() {
            let updated = sqlx::query(
                r#"
UPDATE managed_worktrees
SET
    base_repo_path_key = COALESCE(base_repo_path_key, ?),
    worktree_path_key = COALESCE(worktree_path_key, ?)
WHERE worktree_id = ?
  AND (base_repo_path_key IS NULL OR worktree_path_key IS NULL)
                "#,
            )
            .bind(base_repo_path_key)
            .bind(worktree_path_key)
            .bind(row.worktree_id.as_str())
            .execute(&mut *tx)
            .await?;
            outcome.updated_rows += updated.rows_affected() as u32;
        }
        if base_repo_path_terminal || worktree_path_terminal {
            sqlx::query(
                r#"
INSERT INTO managed_worktree_path_key_backfill_terminal (
    worktree_id,
    base_repo_path_terminal,
    worktree_path_terminal
) VALUES (?, ?, ?)
ON CONFLICT(worktree_id) DO UPDATE SET
    base_repo_path_terminal = MAX(
        managed_worktree_path_key_backfill_terminal.base_repo_path_terminal,
        excluded.base_repo_path_terminal
    ),
    worktree_path_terminal = MAX(
        managed_worktree_path_key_backfill_terminal.worktree_path_terminal,
        excluded.worktree_path_terminal
    )
                "#,
            )
            .bind(row.worktree_id.as_str())
            .bind(base_repo_path_terminal)
            .bind(worktree_path_terminal)
            .execute(&mut *tx)
            .await?;
        }
    }
    sqlx::query(
        r#"
INSERT INTO managed_worktree_path_key_backfill_state (id, last_scanned_worktree_id)
VALUES (1, ?)
ON CONFLICT(id) DO UPDATE SET
    last_scanned_worktree_id = excluded.last_scanned_worktree_id
        "#,
    )
    .bind(next_last_scanned_worktree_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateRuntime;
    use pretty_assertions::assert_eq;
    use sqlx::Row;
    use std::path::PathBuf;

    async fn test_runtime() -> (std::sync::Arc<StateRuntime>, PathBuf) {
        let codex_home = crate::runtime::test_support::unique_temp_dir();
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("state runtime should initialize");
        (runtime, codex_home)
    }

    async fn insert_legacy_worktree(
        runtime: &StateRuntime,
        worktree_id: &str,
        base_repo_path: &str,
        worktree_path: &str,
    ) {
        sqlx::query(
            r#"
INSERT INTO managed_worktrees (
    worktree_id, mode, base_repo_path, worktree_path, lifecycle_status, status_snapshot_json,
    dirty, cleanup_policy, force_delete_requested, owner_kind, created_at_ms, updated_at_ms
) VALUES (?, 'isolated_worktree', ?, ?, 'active', '{}', 0, 'retain', 0, 'manual', 1, 1)
            "#,
        )
        .bind(worktree_id)
        .bind(base_repo_path)
        .bind(worktree_path)
        .execute(runtime.pool.as_ref())
        .await
        .expect("insert legacy managed worktree");
    }

    #[tokio::test]
    async fn retries_the_whole_batch_after_a_concurrent_writer_advances_the_snapshot()
    -> anyhow::Result<()> {
        let (runtime, codex_home) = test_runtime().await;
        let writer = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(codex_home.join(crate::STATE_DB_FILENAME)),
        )
        .await?;
        insert_legacy_worktree(
            &runtime,
            "raced",
            "/repos/project",
            "/repos/project/before-race",
        )
        .await;
        let hook = std::sync::Arc::new(BackfillReadHook::new());
        let backfill_runtime = std::sync::Arc::clone(&runtime);
        let backfill_hook = std::sync::Arc::clone(&hook);
        let backfill = tokio::spawn(async move {
            backfill_legacy_managed_worktree_path_keys_with_hook(
                backfill_runtime.pool.as_ref(),
                Some(&backfill_hook),
            )
            .await
        });
        hook.race.wait().await;
        sqlx::query(
            "UPDATE managed_worktrees SET worktree_path = '/repos/project/after-race' WHERE worktree_id = 'raced'",
        )
        .execute(&writer)
        .await?;
        hook.race.wait().await;
        backfill.await??;
        let key: Vec<u8> = sqlx::query_scalar(
            "SELECT worktree_path_key FROM managed_worktrees WHERE worktree_id = 'raced'",
        )
        .fetch_one(runtime.pool.as_ref())
        .await?;
        assert_eq!(
            managed_worktree_path_key_from_db_string("/repos/project/after-race")
                .expect("updated path should be keyable"),
            key
        );
        assert_eq!(2, hook.attempts.load(Ordering::SeqCst));
        writer.close().await;
        drop(runtime);
        tokio::fs::remove_dir_all(codex_home).await?;
        Ok(())
    }

    #[tokio::test]
    async fn clean_migration_adds_non_enforcing_path_key_columns_and_indexes() {
        let (runtime, codex_home) = test_runtime().await;
        let plan = sqlx::query(sqlx::AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {LEGACY_MANAGED_WORKTREE_PATH_SELECT} WHERE m.worktree_id > ? {LEGACY_MANAGED_WORKTREE_PATH_ORDER} ?"
        )))
        .bind("cursor")
        .bind(MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE)
        .fetch_all(runtime.pool.as_ref())
        .await
        .expect("explain backfill candidate query")
        .into_iter()
        .map(|row| row.get::<String, _>(3))
        .collect::<Vec<_>>();
        assert!(
            plan.iter()
                .any(|detail| detail.contains("SEARCH m") && detail.contains("worktree_id>?"))
                && plan.iter().any(|detail| {
                    detail.contains("SEARCH t") && detail.contains("worktree_id=?)")
                }),
            "expected indexed candidate and terminal-marker searches, got {plan:?}"
        );
        let columns = sqlx::query("PRAGMA table_info(managed_worktrees)")
            .fetch_all(runtime.pool.as_ref())
            .await
            .expect("query managed worktree columns")
            .into_iter()
            .map(|row| row.get::<String, _>(1))
            .collect::<Vec<_>>();
        assert!(columns.contains(&"base_repo_path_key".to_string()));
        assert!(columns.contains(&"worktree_path_key".to_string()));
        let indexes = sqlx::query("PRAGMA index_list(managed_worktrees)")
            .fetch_all(runtime.pool.as_ref())
            .await
            .expect("query managed worktree indexes")
            .into_iter()
            .map(|row| row.get::<String, _>(1))
            .collect::<Vec<_>>();
        assert!(indexes.contains(&"idx_managed_worktrees_base_repo_path_key".to_string()));
        assert!(indexes.contains(&"idx_managed_worktrees_worktree_path_key".to_string()));
        let terminal_table_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'managed_worktree_path_key_backfill_terminal'",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("query terminal marker table");
        assert_eq!(1, terminal_table_count);
        let uniqueness: Vec<i64> = sqlx::query_scalar(
            "SELECT [unique] FROM pragma_index_list('managed_worktrees') WHERE name LIKE 'idx_managed_worktrees_%_path_key' ORDER BY name",
        )
        .fetch_all(runtime.pool.as_ref())
        .await
        .expect("query path-key index uniqueness");
        assert_eq!(vec![0, 0], uniqueness);
        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn legacy_paths_backfill_without_merging_collisions_or_malformed_rows() {
        let (runtime, codex_home) = test_runtime().await;
        insert_legacy_worktree(
            &runtime,
            "first",
            "/repos/project",
            "/repos/project/./worktree",
        )
        .await;
        insert_legacy_worktree(
            &runtime,
            "second",
            "/repos/project",
            "/repos/project//worktree",
        )
        .await;
        insert_legacy_worktree(
            &runtime,
            "malformed",
            "/repos/project",
            "/repos/project/malformed",
        )
        .await;
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(runtime.pool.as_ref())
            .await
            .expect("relax legacy check constraint");
        sqlx::query(
            "UPDATE managed_worktrees SET worktree_path = '' WHERE worktree_id = 'malformed'",
        )
        .execute(runtime.pool.as_ref())
        .await
        .expect("seed malformed legacy row");
        sqlx::query("PRAGMA ignore_check_constraints = OFF")
            .execute(runtime.pool.as_ref())
            .await
            .expect("restore check constraint enforcement");
        let first = backfill_legacy_managed_worktree_path_keys(runtime.pool.as_ref())
            .await
            .expect("backfill should retain every legacy row");
        assert_eq!(3, first.scanned_rows);
        assert_eq!(3, first.updated_rows);
        assert_eq!(1, first.unkeyable_paths);
        assert_eq!(1, first.worktree_path_collisions);
        let rows = sqlx::query(
            "SELECT worktree_id, worktree_path, base_repo_path_key, worktree_path_key FROM managed_worktrees ORDER BY worktree_id",
        )
        .fetch_all(runtime.pool.as_ref())
        .await
        .expect("query backfilled rows");
        assert_eq!(3, rows.len());
        assert_eq!("/repos/project/./worktree", rows[0].get::<String, _>(1));
        assert_eq!("/repos/project//worktree", rows[2].get::<String, _>(1));
        assert_eq!("", rows[1].get::<String, _>(1));
        assert_eq!(
            rows[0].get::<Option<Vec<u8>>, _>(3),
            rows[2].get::<Option<Vec<u8>>, _>(3)
        );
        assert!(rows[1].get::<Option<Vec<u8>>, _>(2).is_some());
        assert_eq!(None, rows[1].get::<Option<Vec<u8>>, _>(3));
        let second = backfill_legacy_managed_worktree_path_keys(runtime.pool.as_ref())
            .await
            .expect("repeat backfill should be safe");
        assert_eq!(0, second.scanned_rows);
        assert_eq!(0, second.updated_rows);
        assert_eq!(0, second.unkeyable_paths);
        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn startup_backfill_is_bounded_and_marks_terminal_fields_without_starving_rows() {
        let (runtime, codex_home) = test_runtime().await;
        sqlx::query("PRAGMA ignore_check_constraints = ON")
            .execute(runtime.pool.as_ref())
            .await
            .expect("relax legacy check constraint");
        for index in 0..101 {
            insert_legacy_worktree(
                &runtime,
                format!("malformed-{index:03}").as_str(),
                "",
                format!("/repos/project/malformed-{index:03}").as_str(),
            )
            .await;
        }
        insert_legacy_worktree(
            &runtime,
            "zzz-later-keyable",
            "/repos/project",
            "/repos/project/zzz-later-keyable",
        )
        .await;
        sqlx::query("PRAGMA ignore_check_constraints = OFF")
            .execute(runtime.pool.as_ref())
            .await
            .expect("restore check constraint enforcement");
        drop(runtime);
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("first restarted runtime should initialize");
        let first_keyed_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktrees WHERE worktree_path_key IS NOT NULL",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count first startup keys");
        assert_eq!(100, first_keyed_rows);
        drop(runtime);
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("second restarted runtime should initialize");
        let terminal_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktree_path_key_backfill_terminal WHERE base_repo_path_terminal = 1",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count terminal malformed fields");
        assert_eq!(101, terminal_rows);
        let later_keys: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktrees WHERE worktree_id = 'zzz-later-keyable' AND base_repo_path_key IS NOT NULL AND worktree_path_key IS NOT NULL",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count later keyable row keys");
        assert_eq!(1, later_keys);
        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn startup_backfill_rediscovers_late_legacy_rows_on_both_sides_of_old_cursor() {
        let (runtime, codex_home) = test_runtime().await;
        insert_legacy_worktree(
            &runtime,
            "middle-before-completion",
            "/repos/project",
            "/repos/project/middle-before-completion",
        )
        .await;
        drop(runtime);
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("initial startup should complete the existing legacy row");
        insert_legacy_worktree(
            &runtime,
            "aaa-inserted-after-completion",
            "/repos/project",
            "/repos/project/aaa-inserted-after-completion",
        )
        .await;
        insert_legacy_worktree(
            &runtime,
            "zzz-inserted-after-completion",
            "/repos/project",
            "/repos/project/zzz-inserted-after-completion",
        )
        .await;
        sqlx::query(
            "UPDATE managed_worktree_path_key_backfill_state SET last_scanned_worktree_id = 'middle-before-completion' WHERE id = 1",
        )
        .execute(runtime.pool.as_ref())
        .await
        .expect("restore an active cursor between late legacy writers");
        drop(runtime);
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("first later startup should scan above the cursor");
        drop(runtime);
        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("second later startup should wrap below the cursor");
        let keyed_rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktrees WHERE worktree_id IN ('aaa-inserted-after-completion', 'zzz-inserted-after-completion') AND base_repo_path_key IS NOT NULL AND worktree_path_key IS NOT NULL",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count rediscovered late legacy rows");
        assert_eq!(2, keyed_rows);
        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }
}
