use super::path_keys::managed_worktree_path_key_from_db_string;
use sqlx::SqlitePool;
use tracing::info;
use tracing::warn;

const MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE: i64 = 100;

#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct ManagedWorktreePathKeyBackfillOutcome {
    scanned_rows: u32,
    updated_rows: u32,
    unkeyable_paths: u32,
    worktree_path_collisions: u32,
}

struct LegacyManagedWorktreePathRow {
    worktree_id: String,
    base_repo_path: String,
    worktree_path: String,
    base_repo_path_key: Option<Vec<u8>>,
    worktree_path_key: Option<Vec<u8>>,
}

pub(crate) async fn backfill_legacy_managed_worktree_path_keys(
    pool: &SqlitePool,
) -> anyhow::Result<ManagedWorktreePathKeyBackfillOutcome> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query_as::<_, (String, String, String, Option<Vec<u8>>, Option<Vec<u8>>)>(
        r#"
SELECT
    worktree_id,
    base_repo_path,
    worktree_path,
    base_repo_path_key,
    worktree_path_key
FROM managed_worktrees
WHERE base_repo_path_key IS NULL OR worktree_path_key IS NULL
ORDER BY
    CASE
        WHEN base_repo_path_key IS NULL AND (LENGTH(TRIM(base_repo_path)) = 0 OR INSTR(base_repo_path, CHAR(0)) > 0)
            THEN 1
        WHEN worktree_path_key IS NULL AND (LENGTH(TRIM(worktree_path)) = 0 OR INSTR(worktree_path, CHAR(0)) > 0)
            THEN 1
        ELSE 0
    END ASC,
    worktree_id ASC
LIMIT ?
        "#,
    )
    .bind(MANAGED_WORKTREE_PATH_KEY_BACKFILL_BATCH_SIZE)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(
        |(worktree_id, base_repo_path, worktree_path, base_repo_path_key, worktree_path_key)| {
            LegacyManagedWorktreePathRow {
                worktree_id,
                base_repo_path,
                worktree_path,
                base_repo_path_key,
                worktree_path_key,
            }
        },
    )
    .collect::<Vec<_>>();
    let mut outcome = ManagedWorktreePathKeyBackfillOutcome {
        scanned_rows: rows.len() as u32,
        ..Default::default()
    };

    for row in rows {
        let base_repo_path_key = row
            .base_repo_path_key
            .is_none()
            .then(|| managed_worktree_path_key_from_db_string(&row.base_repo_path))
            .flatten();
        let worktree_path_key = row
            .worktree_path_key
            .is_none()
            .then(|| managed_worktree_path_key_from_db_string(&row.worktree_path))
            .flatten();
        let unkeyable_paths =
            u32::from(row.base_repo_path_key.is_none() && base_repo_path_key.is_none())
                + u32::from(row.worktree_path_key.is_none() && worktree_path_key.is_none());
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
ORDER BY worktree_id ASC
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
            .bind(row.worktree_id)
            .execute(&mut *tx)
            .await?;
            outcome.updated_rows += updated.rows_affected() as u32;
        }
    }
    tx.commit().await?;
    if outcome.scanned_rows > 0 {
        info!(
            scanned_rows = outcome.scanned_rows,
            updated_rows = outcome.updated_rows,
            unkeyable_paths = outcome.unkeyable_paths,
            worktree_path_collisions = outcome.worktree_path_collisions,
            "bounded managed worktree path-key backfill finished"
        );
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::backfill_legacy_managed_worktree_path_keys;
    use crate::StateRuntime;
    use pretty_assertions::assert_eq;
    use sqlx::Row;
    use std::path::PathBuf;
    use uuid::Uuid;

    async fn test_runtime() -> (std::sync::Arc<StateRuntime>, PathBuf) {
        let codex_home = std::env::temp_dir().join(format!(
            "codex-state-managed-worktree-path-backfill-{}",
            Uuid::new_v4()
        ));
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
    worktree_id,
    mode,
    base_repo_path,
    worktree_path,
    lifecycle_status,
    status_snapshot_json,
    dirty,
    cleanup_policy,
    force_delete_requested,
    owner_kind,
    created_at_ms,
    updated_at_ms
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
    async fn clean_migration_adds_non_enforcing_path_key_columns_and_indexes() {
        let (runtime, codex_home) = test_runtime().await;
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
        assert_eq!(1, second.scanned_rows);
        assert_eq!(0, second.updated_rows);
        assert_eq!(1, second.unkeyable_paths);

        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }

    #[tokio::test]
    async fn startup_backfill_is_bounded_and_resumes_on_the_next_startup() {
        let (runtime, codex_home) = test_runtime().await;
        for index in 0..101 {
            insert_legacy_worktree(
                &runtime,
                format!("worktree-{index:03}").as_str(),
                "/repos/project",
                format!("/repos/project/worktree-{index:03}").as_str(),
            )
            .await;
        }
        drop(runtime);

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("first restarted runtime should initialize");
        let first_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktrees WHERE worktree_path_key IS NOT NULL",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count first startup backfill");
        assert_eq!(100, first_count);
        drop(runtime);

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string())
            .await
            .expect("second restarted runtime should initialize");
        let second_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM managed_worktrees WHERE worktree_path_key IS NOT NULL",
        )
        .fetch_one(runtime.pool.as_ref())
        .await
        .expect("count second startup backfill");
        assert_eq!(101, second_count);

        drop(runtime);
        let _ = tokio::fs::remove_dir_all(codex_home).await;
    }
}
