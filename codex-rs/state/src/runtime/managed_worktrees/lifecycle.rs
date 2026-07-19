use super::*;

impl ManagedWorktreeStore {
    pub async fn detach_managed_worktree(
        &self,
        params: ManagedWorktreeDetachParams,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        validate_detach_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
        fences::ensure_no_active_background_agent_run_assignment(
            &mut tx,
            params.worktree_id.as_str(),
        )
        .await?;
        ensure_not_active_background_agent_worktree_lease(&mut tx, params.worktree_id.as_str())
            .await?;
        match &params.target {
            ManagedWorktreeAssignmentTarget::Thread(thread_id) => {
                sqlx::query(
                    r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE worktree_id = ?
  AND thread_id = ?
  AND detached_at_ms IS NULL
                    "#,
                )
                .bind(now_ms)
                .bind(params.worktree_id.as_str())
                .bind(thread_id.to_string())
                .execute(&mut *tx)
                .await?;
            }
            ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) => {
                sqlx::query(
                    r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE worktree_id = ?
  AND agent_run_id = ?
  AND detached_at_ms IS NULL
                    "#,
                )
                .bind(now_ms)
                .bind(params.worktree_id.as_str())
                .bind(agent_run_id.as_str())
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query(
            r#"
UPDATE managed_worktrees
SET
    owner_kind = ?,
    owner_thread_id = NULL,
    owner_agent_run_id = NULL,
    updated_at_ms = ?
WHERE worktree_id = ?
  AND NOT EXISTS (
    SELECT 1
    FROM managed_worktree_assignments AS assignment
    WHERE assignment.worktree_id = managed_worktrees.worktree_id
      AND assignment.detached_at_ms IS NULL
  )
            "#,
        )
        .bind(crate::ManagedWorktreeOwnerKind::Manual.as_str())
        .bind(now_ms)
        .bind(params.worktree_id.as_str())
        .execute(&mut *tx)
        .await?;
        let sql = format!(
            r#"
SELECT
{}
FROM managed_worktrees
WHERE worktree_id = ?
            "#,
            managed_worktree_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(params.worktree_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
        let worktree = row.map(|row| managed_worktree_from_row(&row)).transpose()?;
        tx.commit().await?;
        Ok(worktree)
    }

    pub async fn update_managed_worktree_status(
        &self,
        params: ManagedWorktreeStatusUpdateParams,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        validate_status_update_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let status_snapshot_json = serde_json::to_string(&params.status_snapshot_json)?;
        let sql = format!(
            r#"
UPDATE managed_worktrees
SET
    branch = ?,
    head_sha = ?,
    status_snapshot_json = ?,
    dirty = ?,
    updated_at_ms = ?
WHERE worktree_id = ?
  AND deleted_at_ms IS NULL
RETURNING
{}
            "#,
            managed_worktree_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(params.branch)
            .bind(params.head_sha)
            .bind(status_snapshot_json)
            .bind(params.dirty)
            .bind(now_ms)
            .bind(params.worktree_id)
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| managed_worktree_from_row(&row)).transpose()
    }

    pub async fn release_managed_worktree(
        &self,
        params: ManagedWorktreeReleaseParams,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        validate_release_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let force_delete_requested = params.force_delete
            || params.cleanup_policy == crate::ManagedWorktreeCleanupPolicy::ForceDelete;
        let status_snapshot_json = serde_json::to_string(&params.status_snapshot_json)?;
        let mut tx = self.pool.begin().await?;
        let mode: Option<(String, Option<i64>)> = sqlx::query_as(
            r#"
SELECT mode, deleted_at_ms
FROM managed_worktrees
WHERE worktree_id = ?
            "#,
        )
        .bind(params.worktree_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((mode, deleted_at_ms)) = mode else {
            tx.commit().await?;
            return Ok(None);
        };
        fences::ensure_no_active_background_agent_run_assignment(
            &mut tx,
            params.worktree_id.as_str(),
        )
        .await?;
        ensure_not_active_background_agent_worktree_lease(&mut tx, params.worktree_id.as_str())
            .await?;
        let active_assignment_count: i64 = sqlx::query_scalar(
            r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind(params.worktree_id.as_str())
        .fetch_one(&mut *tx)
        .await?;
        if active_assignment_count > 0 {
            anyhow::bail!(
                "managed worktree {} has an active assignment; detach it before release",
                params.worktree_id
            );
        }
        let mode = crate::ManagedWorktreeMode::try_from(mode.as_str())?;
        let lifecycle_status = if deleted_at_ms.is_some() {
            crate::ManagedWorktreeLifecycleStatus::Deleted
        } else if mode == crate::ManagedWorktreeMode::IsolatedWorktree
            && (force_delete_requested
                || params.cleanup_policy != crate::ManagedWorktreeCleanupPolicy::Retain)
        {
            crate::ManagedWorktreeLifecycleStatus::CleanupPending
        } else {
            crate::ManagedWorktreeLifecycleStatus::Released
        };
        let sql = format!(
            r#"
UPDATE managed_worktrees
SET
    lifecycle_status = ?,
    status_snapshot_json = ?,
    dirty = ?,
    cleanup_policy = ?,
    force_delete_requested = CASE WHEN ? THEN 1 ELSE force_delete_requested END,
    released_at_ms = COALESCE(released_at_ms, ?),
    updated_at_ms = ?
WHERE worktree_id = ?
RETURNING
{}
            "#,
            managed_worktree_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(lifecycle_status.as_str())
            .bind(status_snapshot_json.as_str())
            .bind(params.dirty)
            .bind(params.cleanup_policy.as_str())
            .bind(force_delete_requested)
            .bind(now_ms)
            .bind(now_ms)
            .bind(params.worktree_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
        sqlx::query(
            r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind(now_ms)
        .bind(params.worktree_id.as_str())
        .execute(&mut *tx)
        .await?;
        let worktree = row.map(|row| managed_worktree_from_row(&row)).transpose()?;
        tx.commit().await?;
        Ok(worktree)
    }

    pub async fn mark_managed_worktree_deleted(
        &self,
        worktree_id: &str,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        if worktree_id.trim().is_empty() {
            anyhow::bail!("managed worktree id cannot be empty");
        }
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
        sqlx::query(
            r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind(now_ms)
        .bind(worktree_id)
        .execute(&mut *tx)
        .await?;
        let worktree = row.map(|row| managed_worktree_from_row(&row)).transpose()?;
        if worktree
            .as_ref()
            .and_then(|worktree| worktree.owner_agent_run_id.as_ref())
            .is_some()
        {
            sqlx::query(
                r#"
UPDATE background_agent_worktree_leases
SET
    released_at = COALESCE(released_at, ?),
    deleted_at = COALESCE(deleted_at, ?),
    updated_at = ?
WHERE id = ?
  AND mode = 'isolated_worktree'
  AND deleted_at IS NULL
                "#,
            )
            .bind(now_seconds)
            .bind(now_seconds)
            .bind(now_seconds)
            .bind(worktree_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(worktree)
    }
}
