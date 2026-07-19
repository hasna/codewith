use super::*;

impl StateRuntime {
    pub async fn mark_managed_worktree_cleanup_succeeded(
        &self,
        worktree_id: &str,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let now_seconds = datetime_to_epoch_seconds(now);
        let mut tx = self.pool.begin().await?;
        fences::ensure_no_active_background_agent_run_assignment(&mut tx, worktree_id).await?;
        let sql = format!(
            r#"
UPDATE managed_worktrees
SET
    lifecycle_status = 'deleted',
    released_at_ms = COALESCE(released_at_ms, ?),
    deleted_at_ms = COALESCE(deleted_at_ms, ?),
    updated_at_ms = ?
WHERE worktree_id = ?
  AND mode = 'isolated_worktree'
  AND lifecycle_status = 'cleanup_pending'
  AND deleted_at_ms IS NULL
RETURNING
{}
            "#,
            managed_worktree_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(now_ms)
            .bind(now_ms)
            .bind(now_ms)
            .bind(worktree_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let worktree = managed_worktree_from_row(&row)?;
        if worktree.owner_agent_run_id.is_some() {
            sqlx::query(
                r#"
UPDATE background_agent_worktree_leases
SET
    deleted_at = COALESCE(deleted_at, ?),
    updated_at = ?
WHERE id = ?
  AND mode = 'isolated_worktree'
  AND deleted_at IS NULL
            "#,
            )
            .bind(now_seconds)
            .bind(now_seconds)
            .bind(worktree_id)
            .execute(&mut *tx)
            .await?;
        }
        if let Some(run_id) = worktree.owner_agent_run_id.as_deref() {
            sqlx::query(
                r#"
UPDATE background_agent_cleanup_tombstones
SET deleted_at = COALESCE(deleted_at, ?)
WHERE run_id = ?
            "#,
            )
            .bind(now_seconds)
            .bind(run_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Some(worktree))
    }

    pub async fn record_managed_worktree_cleanup_failure(
        &self,
        params: ManagedWorktreeCleanupFailureParams,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        validate_cleanup_failure_params(&params)?;
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let now_seconds = datetime_to_epoch_seconds(now);
        let retry_after_ms = params.retry_after.map(datetime_to_epoch_millis);
        let retry_after_seconds = params.retry_after.map(datetime_to_epoch_seconds);
        let status_snapshot_json = serde_json::to_string(&params.status_snapshot_json)?;
        let mut tx = self.pool.begin().await?;
        let sql = format!(
            r#"
UPDATE managed_worktrees
SET
    lifecycle_status = 'cleanup_pending',
    status_snapshot_json = ?,
    dirty = ?,
    released_at_ms = COALESCE(released_at_ms, ?),
    cleanup_after_ms = ?,
    updated_at_ms = ?
WHERE worktree_id = ?
  AND deleted_at_ms IS NULL
RETURNING
{}
            "#,
            managed_worktree_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(status_snapshot_json.as_str())
            .bind(params.dirty)
            .bind(now_ms)
            .bind(retry_after_ms)
            .bind(now_ms)
            .bind(params.worktree_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;

        let Some(row) = row else {
            tx.commit().await?;
            return Ok(None);
        };
        let worktree = managed_worktree_from_row(&row)?;
        if worktree.owner_agent_run_id.is_some() {
            sqlx::query(
                r#"
UPDATE background_agent_worktree_leases
SET
    dirty = ?,
    status_snapshot_json = ?,
    cleanup_after = ?,
    updated_at = ?
WHERE id = ?
  AND deleted_at IS NULL
            "#,
            )
            .bind(if params.dirty { 1 } else { 0 })
            .bind(status_snapshot_json.as_str())
            .bind(retry_after_seconds)
            .bind(now_seconds)
            .bind(params.worktree_id.as_str())
            .execute(&mut *tx)
            .await?;
        }
        if let Some(run_id) = worktree.owner_agent_run_id.as_deref() {
            let payload_json = serde_json::json!({
                "cleanup": "failure",
                "forceDeleteRequired": params.force_delete_required,
                "statusSnapshot": params.status_snapshot_json,
            });
            sqlx::query(
                r#"
INSERT INTO background_agent_cleanup_tombstones (
    run_id,
    reason,
    worktree_path,
    dirty_worktree,
    retained_until,
    payload_json,
    created_at
) VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(run_id) DO UPDATE SET
    reason = excluded.reason,
    worktree_path = excluded.worktree_path,
    dirty_worktree = excluded.dirty_worktree,
    retained_until = excluded.retained_until,
    payload_json = excluded.payload_json,
    created_at = excluded.created_at,
    deleted_at = NULL
                "#,
            )
            .bind(run_id)
            .bind(params.reason.as_str())
            .bind(path_to_db_string(&worktree.worktree_path))
            .bind(if params.dirty { 1 } else { 0 })
            .bind(retry_after_seconds)
            .bind(serde_json::to_string(&payload_json)?)
            .bind(now_seconds)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Some(worktree))
    }
}
