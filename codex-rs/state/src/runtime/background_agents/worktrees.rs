use super::*;
use crate::BackgroundAgentWorkspaceMode;
use crate::runtime::managed_worktrees::path_to_db_string;
use std::path::Path;

impl StateRuntime {
    pub async fn list_background_agent_worktree_leases(
        &self,
        base_repo_path: Option<&str>,
        include_deleted: bool,
        offset: usize,
        limit: usize,
    ) -> anyhow::Result<Vec<BackgroundAgentWorktreeLease>> {
        let base_repo_path = base_repo_path.map(|path| path_to_db_string(Path::new(path)));
        let rows = sqlx::query_as::<_, BackgroundAgentWorktreeLeaseRow>(
            r#"
SELECT
    id,
    run_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    head_sha,
    status_snapshot_json,
    dirty,
    cleanup_after,
    force_delete_requested,
    created_at,
    updated_at,
    released_at,
    deleted_at
FROM background_agent_worktree_leases
WHERE (? IS NULL OR base_repo_path = ?)
  AND (? OR deleted_at IS NULL)
ORDER BY
    CASE
        WHEN deleted_at IS NOT NULL THEN 3
        WHEN released_at IS NOT NULL THEN 2
        ELSE 1
    END,
    updated_at DESC,
    id ASC
LIMIT ? OFFSET ?
            "#,
        )
        .bind(base_repo_path.as_deref())
        .bind(base_repo_path.as_deref())
        .bind(include_deleted)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.into_iter()
            .map(BackgroundAgentWorktreeLease::try_from)
            .collect()
    }

    pub async fn create_background_agent_worktree_lease(
        &self,
        params: &BackgroundAgentWorktreeLeaseCreateParams,
    ) -> anyhow::Result<BackgroundAgentWorktreeLease> {
        let now = Utc::now().timestamp();
        let base_repo_path = path_to_db_string(Path::new(params.base_repo_path.as_str()));
        let worktree_path = path_to_db_string(Path::new(params.worktree_path.as_str()));
        let cleanup_after = params.cleanup_after.map(|timestamp| timestamp.timestamp());
        let status_snapshot_json = serde_json::to_string(&params.status_snapshot_json)?;
        let mut tx = self.pool.begin().await?;
        if params.mode == BackgroundAgentWorkspaceMode::SharedRepository {
            let active_shared_repo_lease: Option<(String,)> = sqlx::query_as(
                r#"
SELECT id
FROM background_agent_worktree_leases
WHERE mode = 'shared_repository'
  AND base_repo_path = ?
  AND released_at IS NULL
  AND deleted_at IS NULL
LIMIT 1
            "#,
            )
            .bind(base_repo_path.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((lease_id,)) = active_shared_repo_lease {
                tx.rollback().await?;
                anyhow::bail!(
                    "shared repository {base_repo_path} is already leased by background agent worktree lease {lease_id}"
                );
            }
        }
        if params.mode == BackgroundAgentWorkspaceMode::IsolatedWorktree {
            let active_path_lease: Option<(String,)> = sqlx::query_as(
                r#"
SELECT id
FROM background_agent_worktree_leases
WHERE mode = 'isolated_worktree'
  AND worktree_path = ?
  AND deleted_at IS NULL
LIMIT 1
                "#,
            )
            .bind(worktree_path.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((lease_id,)) = active_path_lease {
                tx.rollback().await?;
                anyhow::bail!(
                    "isolated worktree path {worktree_path} is already leased by background agent worktree lease {lease_id}"
                );
            }
        }
        sqlx::query(
            r#"
INSERT INTO background_agent_worktree_leases (
    id,
    run_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    head_sha,
    status_snapshot_json,
    dirty,
    cleanup_after,
    created_at,
    updated_at
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(params.id.as_str())
        .bind(params.run_id.as_str())
        .bind(params.identity.as_str())
        .bind(params.mode.as_str())
        .bind(base_repo_path.as_str())
        .bind(worktree_path.as_str())
        .bind(params.branch.as_deref())
        .bind(params.head_sha.as_deref())
        .bind(status_snapshot_json.as_str())
        .bind(if params.dirty { 1 } else { 0 })
        .bind(cleanup_after)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
INSERT INTO managed_worktrees (
    worktree_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    worktree_path_key,
    branch,
    base_sha,
    head_sha,
    lifecycle_status,
    status_snapshot_json,
    dirty,
    cleanup_policy,
    force_delete_requested,
    owner_kind,
    owner_agent_run_id,
    created_at_ms,
    updated_at_ms,
    cleanup_after_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(params.id.as_str())
        .bind(params.identity.as_str())
        .bind(params.mode.as_str())
        .bind(base_repo_path.as_str())
        .bind(worktree_path.as_str())
        .bind(worktree_path.as_str())
        .bind(params.branch.as_deref())
        .bind(params.head_sha.as_deref())
        .bind(params.head_sha.as_deref())
        .bind(ManagedWorktreeLifecycleStatus::Active.as_str())
        .bind(status_snapshot_json.as_str())
        .bind(params.dirty)
        .bind(
            if params.cleanup_after.is_some() {
                ManagedWorktreeCleanupPolicy::DeleteIfClean
            } else {
                ManagedWorktreeCleanupPolicy::Retain
            }
            .as_str(),
        )
        .bind(false)
        .bind(ManagedWorktreeOwnerKind::BackgroundAgent.as_str())
        .bind(params.run_id.as_str())
        .bind(now * 1000)
        .bind(now * 1000)
        .bind(cleanup_after.map(|timestamp| timestamp * 1000))
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
INSERT INTO managed_worktree_assignments (
    assignment_id,
    worktree_id,
    thread_id,
    agent_run_id,
    attached_at_ms,
    detached_at_ms
) VALUES (?, ?, NULL, ?, ?, NULL)
            "#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(params.id.as_str())
        .bind(params.run_id.as_str())
        .bind(now * 1000)
        .execute(&mut *tx)
        .await?;

        let run_update = sqlx::query(
            r#"
UPDATE background_agent_runs
SET worktree_lease_id = ?, updated_at = ?
WHERE id = ? AND (worktree_lease_id IS NULL OR worktree_lease_id = ?)
            "#,
        )
        .bind(params.id.as_str())
        .bind(now)
        .bind(params.run_id.as_str())
        .bind(params.id.as_str())
        .execute(&mut *tx)
        .await?;
        if run_update.rows_affected() == 0 {
            tx.rollback().await?;
            anyhow::bail!(
                "background agent run {} already has a different worktree lease",
                params.run_id
            );
        }

        tx.commit().await?;
        self.get_background_agent_worktree_lease(params.id.as_str())
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "failed to load background agent worktree lease {}",
                    params.id
                )
            })
    }

    pub async fn get_background_agent_worktree_lease(
        &self,
        lease_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentWorktreeLease>> {
        let row = sqlx::query_as::<_, BackgroundAgentWorktreeLeaseRow>(
            r#"
SELECT
    id,
    run_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    head_sha,
    status_snapshot_json,
    dirty,
    cleanup_after,
    force_delete_requested,
    created_at,
    updated_at,
    released_at,
    deleted_at
FROM background_agent_worktree_leases
WHERE id = ?
            "#,
        )
        .bind(lease_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentWorktreeLease::try_from).transpose()
    }

    pub async fn get_background_agent_worktree_lease_for_run(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<BackgroundAgentWorktreeLease>> {
        let row = sqlx::query_as::<_, BackgroundAgentWorktreeLeaseRow>(
            r#"
SELECT
    lease.id,
    lease.run_id,
    lease.identity,
    lease.mode,
    lease.base_repo_path,
    lease.worktree_path,
    lease.branch,
    lease.head_sha,
    lease.status_snapshot_json,
    lease.dirty,
    lease.cleanup_after,
    lease.force_delete_requested,
    lease.created_at,
    lease.updated_at,
    lease.released_at,
    lease.deleted_at
FROM background_agent_runs AS run
JOIN background_agent_worktree_leases AS lease
  ON lease.id = run.worktree_lease_id
WHERE run.id = ?
            "#,
        )
        .bind(run_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        row.map(BackgroundAgentWorktreeLease::try_from).transpose()
    }

    pub async fn update_background_agent_worktree_lease_status(
        &self,
        lease_id: &str,
        dirty: bool,
        status_snapshot_json: &serde_json::Value,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp();
        let status_snapshot_json = serde_json::to_string(status_snapshot_json)?;
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE background_agent_worktree_leases
SET dirty = ?, status_snapshot_json = ?, updated_at = ?
WHERE id = ? AND deleted_at IS NULL
            "#,
        )
        .bind(if dirty { 1 } else { 0 })
        .bind(status_snapshot_json.as_str())
        .bind(now)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() > 0 {
            sqlx::query(
                r#"
UPDATE managed_worktrees
SET dirty = ?, status_snapshot_json = ?, updated_at_ms = ?
WHERE worktree_id = ? AND deleted_at_ms IS NULL
                "#,
            )
            .bind(dirty)
            .bind(status_snapshot_json.as_str())
            .bind(now * 1000)
            .bind(lease_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn release_background_agent_worktree_lease(
        &self,
        lease_id: &str,
        cleanup: BackgroundAgentWorkspaceCleanup,
    ) -> anyhow::Result<Option<BackgroundAgentWorktreeLease>> {
        let now = Utc::now().timestamp();
        let force_delete = matches!(cleanup, BackgroundAgentWorkspaceCleanup::ForceDelete);
        let delete_if_clean = matches!(cleanup, BackgroundAgentWorkspaceCleanup::DeleteIfClean);
        let cleanup_policy = match cleanup {
            BackgroundAgentWorkspaceCleanup::Retain => ManagedWorktreeCleanupPolicy::Retain,
            BackgroundAgentWorkspaceCleanup::DeleteIfClean => {
                ManagedWorktreeCleanupPolicy::DeleteIfClean
            }
            BackgroundAgentWorkspaceCleanup::ForceDelete => {
                ManagedWorktreeCleanupPolicy::ForceDelete
            }
        };
        let mut tx = self.pool.begin().await?;
        let lease_row = sqlx::query_as::<_, BackgroundAgentWorktreeLeaseRow>(
            r#"
UPDATE background_agent_worktree_leases
SET
    released_at = COALESCE(released_at, ?),
    force_delete_requested = CASE WHEN ? THEN 1 ELSE force_delete_requested END,
    updated_at = ?
WHERE id = ?
RETURNING
    id,
    run_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    head_sha,
    status_snapshot_json,
    dirty,
    cleanup_after,
    force_delete_requested,
    created_at,
    updated_at,
    released_at,
    deleted_at
            "#,
        )
        .bind(now)
        .bind(force_delete)
        .bind(now)
        .bind(lease_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(lease_row) = lease_row else {
            tx.commit().await?;
            return Ok(None);
        };
        let lease = BackgroundAgentWorktreeLease::try_from(lease_row)?;
        let isolated_cleanup_requested = lease.mode
            == BackgroundAgentWorkspaceMode::IsolatedWorktree
            && (force_delete || delete_if_clean);
        let lifecycle_status = if lease.deleted_at.is_some() {
            ManagedWorktreeLifecycleStatus::Deleted
        } else if isolated_cleanup_requested {
            ManagedWorktreeLifecycleStatus::CleanupPending
        } else {
            ManagedWorktreeLifecycleStatus::Released
        };
        let status_snapshot_json = serde_json::to_string(&lease.status_snapshot_json)?;

        sqlx::query(
            r#"
UPDATE managed_worktrees
SET
    lifecycle_status = ?,
    status_snapshot_json = ?,
    dirty = ?,
    released_at_ms = COALESCE(released_at_ms, ?),
    cleanup_policy = ?,
    force_delete_requested = ?,
    updated_at_ms = ?
WHERE worktree_id = ?
            "#,
        )
        .bind(lifecycle_status.as_str())
        .bind(status_snapshot_json.as_str())
        .bind(lease.dirty)
        .bind(now * 1000)
        .bind(cleanup_policy.as_str())
        .bind(lease.force_delete_requested)
        .bind(now * 1000)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind(now * 1000)
        .bind(lease_id)
        .execute(&mut *tx)
        .await?;

        if lifecycle_status == ManagedWorktreeLifecycleStatus::CleanupPending
            || (lease.mode == BackgroundAgentWorkspaceMode::SharedRepository
                && delete_if_clean
                && lease.dirty)
        {
            let payload_json = serde_json::json!({
                "cleanup": cleanup_policy.as_str(),
                "forceDeleteRequired": force_delete || lease.dirty,
                "statusSnapshot": lease.status_snapshot_json,
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
            .bind(lease.run_id.as_str())
            .bind("worktree cleanup pending")
            .bind(lease.worktree_path.as_str())
            .bind(if lease.dirty { 1 } else { 0 })
            .bind(lease.cleanup_after.map(|timestamp| timestamp.timestamp()))
            .bind(serde_json::to_string(&payload_json)?)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        self.get_background_agent_worktree_lease(lease_id).await
    }
}
