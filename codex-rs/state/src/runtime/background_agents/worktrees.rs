use super::*;
use crate::BackgroundAgentWorkspaceMode;

impl StateRuntime {
    pub async fn create_background_agent_worktree_lease(
        &self,
        params: &BackgroundAgentWorktreeLeaseCreateParams,
    ) -> anyhow::Result<BackgroundAgentWorktreeLease> {
        let now = Utc::now().timestamp();
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
            .bind(params.base_repo_path.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((lease_id,)) = active_shared_repo_lease {
                tx.rollback().await?;
                anyhow::bail!(
                    "shared repository {} is already leased by background agent worktree lease {}",
                    params.base_repo_path,
                    lease_id
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
            .bind(params.worktree_path.as_str())
            .fetch_optional(&mut *tx)
            .await?;
            if let Some((lease_id,)) = active_path_lease {
                tx.rollback().await?;
                anyhow::bail!(
                    "isolated worktree path {} is already leased by background agent worktree lease {}",
                    params.worktree_path,
                    lease_id
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
        .bind(params.base_repo_path.as_str())
        .bind(params.worktree_path.as_str())
        .bind(params.branch.as_deref())
        .bind(params.head_sha.as_deref())
        .bind(status_snapshot_json.as_str())
        .bind(if params.dirty { 1 } else { 0 })
        .bind(cleanup_after)
        .bind(now)
        .bind(now)
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
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn release_background_agent_worktree_lease(
        &self,
        lease_id: &str,
        cleanup: BackgroundAgentWorkspaceCleanup,
    ) -> anyhow::Result<Option<BackgroundAgentWorktreeLease>> {
        let Some(lease) = self.get_background_agent_worktree_lease(lease_id).await? else {
            return Ok(None);
        };
        let now = Utc::now().timestamp();
        let should_delete = match cleanup {
            BackgroundAgentWorkspaceCleanup::Retain => false,
            BackgroundAgentWorkspaceCleanup::DeleteIfClean => !lease.dirty,
            BackgroundAgentWorkspaceCleanup::ForceDelete => true,
        };
        let force_delete_requested =
            match cleanup {
                BackgroundAgentWorkspaceCleanup::ForceDelete => 1,
                BackgroundAgentWorkspaceCleanup::Retain
                | BackgroundAgentWorkspaceCleanup::DeleteIfClean => {
                    if lease.force_delete_requested { 1 } else { 0 }
                }
            };
        sqlx::query(
            r#"
UPDATE background_agent_worktree_leases
SET
    released_at = COALESCE(released_at, ?),
    deleted_at = CASE WHEN ? THEN COALESCE(deleted_at, ?) ELSE deleted_at END,
    force_delete_requested = ?,
    updated_at = ?
WHERE id = ?
            "#,
        )
        .bind(now)
        .bind(should_delete)
        .bind(now)
        .bind(force_delete_requested)
        .bind(now)
        .bind(lease_id)
        .execute(self.pool.as_ref())
        .await?;

        if matches!(cleanup, BackgroundAgentWorkspaceCleanup::DeleteIfClean) && lease.dirty {
            let payload_json = serde_json::json!({
                "cleanup": "delete_if_clean",
                "forceDeleteRequired": true,
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
) VALUES (?, ?, ?, 1, ?, ?, ?)
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
            .bind("dirty worktree retained")
            .bind(lease.worktree_path.as_str())
            .bind(lease.cleanup_after.map(|timestamp| timestamp.timestamp()))
            .bind(serde_json::to_string(&payload_json)?)
            .bind(now)
            .execute(self.pool.as_ref())
            .await?;
        }

        self.get_background_agent_worktree_lease(lease_id).await
    }
}
