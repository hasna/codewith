use super::*;
use crate::model::ManagedWorktreeMergeCandidateRow;
use crate::model::ManagedWorktreeRow;
use anyhow::Context;
use std::path::PathBuf;
use uuid::Uuid;

mod path_keys;
pub(crate) use path_keys::managed_worktree_path_key_from_display;
pub(crate) use path_keys::normalize_legacy_managed_worktree_paths;
pub(crate) use path_keys::path_to_db_string;
#[cfg(all(test, unix))]
use path_keys::path_to_string;

pub const DEFAULT_MANAGED_WORKTREE_LIST_LIMIT: u32 = 50;
pub const MAX_MANAGED_WORKTREE_LIST_LIMIT: u32 = 200;
const MANAGED_WORKTREE_LIST_SCAN_CHUNK_SIZE: u32 = DEFAULT_MANAGED_WORKTREE_LIST_LIMIT;

#[derive(Clone)]
pub struct ManagedWorktreeStore {
    pool: Arc<SqlitePool>,
}

impl ManagedWorktreeStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktreeCreateParams {
    pub worktree_id: Option<String>,
    pub identity: Option<String>,
    pub mode: crate::ManagedWorktreeMode,
    pub base_repo_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
    pub status_snapshot_json: serde_json::Value,
    pub dirty: bool,
    pub cleanup_policy: crate::ManagedWorktreeCleanupPolicy,
    pub owner_kind: crate::ManagedWorktreeOwnerKind,
    pub owner_thread_id: Option<ThreadId>,
    pub owner_agent_run_id: Option<String>,
    pub cleanup_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktreeStatusUpdateParams {
    pub worktree_id: String,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub status_snapshot_json: serde_json::Value,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktreeReleaseParams {
    pub worktree_id: String,
    pub cleanup_policy: crate::ManagedWorktreeCleanupPolicy,
    pub force_delete: bool,
    pub status_snapshot_json: serde_json::Value,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktreeCleanupFailureParams {
    pub worktree_id: String,
    pub reason: String,
    pub dirty: bool,
    pub status_snapshot_json: serde_json::Value,
    pub retry_after: Option<DateTime<Utc>>,
    pub force_delete_required: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktreeMergeCandidateRecordParams {
    pub candidate_id: Option<String>,
    pub worktree_id: String,
    pub target_ref: String,
    pub target_sha: Option<String>,
    pub base_sha: String,
    pub head_sha: String,
    pub status: crate::ManagedWorktreeMergeCandidateStatus,
    pub conflict_summary: Option<String>,
    pub test_summary_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedWorktreeAssignmentTarget {
    Thread(ThreadId),
    AgentRun(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktreeAttachParams {
    pub worktree_id: String,
    pub target: ManagedWorktreeAssignmentTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorktreeDetachParams {
    pub worktree_id: String,
    pub target: ManagedWorktreeAssignmentTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorktreeListPage {
    pub data: Vec<crate::ManagedWorktree>,
    pub next_cursor: Option<String>,
}

impl ManagedWorktreeStore {
    pub(crate) async fn detach_thread_assignments_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<u64> {
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE thread_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind(now_ms)
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
UPDATE managed_worktrees
SET
    owner_kind = ?,
    owner_thread_id = NULL,
    owner_agent_run_id = NULL,
    updated_at_ms = ?
WHERE owner_thread_id = ?
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
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    pub async fn create_managed_worktree(
        &self,
        params: ManagedWorktreeCreateParams,
    ) -> anyhow::Result<crate::ManagedWorktree> {
        validate_create_params(&params)?;
        let worktree_id = params
            .worktree_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let cleanup_after_ms = params.cleanup_after.map(datetime_to_epoch_millis);
        let status_snapshot_json = serde_json::to_string(&params.status_snapshot_json)?;
        let base_repo_path = path_to_db_string(&params.base_repo_path);
        let worktree_path = path_to_db_string(&params.worktree_path);
        // Store the admission key so create-time rows match startup backfill:
        // isolated worktrees are keyed by their worktree path, while shared
        // repositories are keyed by their base repository path. Keys
        // intentionally remain distinct from display paths on Windows.
        let worktree_path_key = managed_worktree_path_key_from_display(
            if params.mode == crate::ManagedWorktreeMode::SharedRepository {
                base_repo_path.as_str()
            } else {
                worktree_path.as_str()
            },
        );
        if params.mode == crate::ManagedWorktreeMode::IsolatedWorktree
            && base_repo_path == worktree_path
        {
            anyhow::bail!("isolated managed worktree path cannot match the base repo path");
        }
        let sql = format!(
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
    owner_thread_id,
    owner_agent_run_id,
    created_at_ms,
    updated_at_ms,
    released_at_ms,
    cleanup_after_ms,
    deleted_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING
{}
            "#,
            managed_worktree_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(worktree_id)
            .bind(params.identity)
            .bind(params.mode.as_str())
            .bind(base_repo_path)
            .bind(worktree_path)
            .bind(worktree_path_key)
            .bind(params.branch)
            .bind(params.base_sha)
            .bind(params.head_sha)
            .bind(crate::ManagedWorktreeLifecycleStatus::Active.as_str())
            .bind(status_snapshot_json)
            .bind(params.dirty)
            .bind(params.cleanup_policy.as_str())
            .bind(false)
            .bind(params.owner_kind.as_str())
            .bind(
                params
                    .owner_thread_id
                    .map(|thread_id| thread_id.to_string()),
            )
            .bind(params.owner_agent_run_id)
            .bind(now_ms)
            .bind(now_ms)
            .bind(Option::<i64>::None)
            .bind(cleanup_after_ms)
            .bind(Option::<i64>::None)
            .fetch_one(self.pool.as_ref())
            .await
            .context("managed worktree admission rejected")?;

        managed_worktree_from_row(&row)
    }

    pub async fn get_managed_worktree(
        &self,
        worktree_id: &str,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
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
            .bind(worktree_id)
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| managed_worktree_from_row(&row)).transpose()
    }

    pub async fn list_managed_worktrees_page(
        &self,
        base_repo_path: Option<&Path>,
        include_deleted: bool,
        cursor: Option<&str>,
        limit: u32,
    ) -> anyhow::Result<ManagedWorktreeListPage> {
        let offset = parse_managed_worktree_list_cursor(cursor)?;
        let limit = limit.clamp(1, MAX_MANAGED_WORKTREE_LIST_LIMIT);
        let normalized_base_repo_path = base_repo_path.map(path_to_db_string);

        if let Some(base_repo_path) = normalized_base_repo_path {
            let mut scanned_rows = 0_i64;
            let mut skipped_rows = 0;
            let mut matching_rows = Vec::with_capacity(limit as usize + 1);

            loop {
                let mut query = QueryBuilder::<Sqlite>::new(format!(
                    "SELECT {} FROM managed_worktrees WHERE (",
                    managed_worktree_select_columns()
                ));
                query.push_bind(include_deleted);
                query.push(" OR deleted_at_ms IS NULL)");
                query.push(
                    r#"
ORDER BY
    CASE lifecycle_status
        WHEN 'active' THEN 1
        WHEN 'cleanup_pending' THEN 2
        WHEN 'released' THEN 3
        WHEN 'deleted' THEN 4
    END,
    updated_at_ms DESC,
    worktree_id DESC
LIMIT
"#,
                );
                query.push_bind(i64::from(MANAGED_WORKTREE_LIST_SCAN_CHUNK_SIZE));
                query.push(" OFFSET ");
                query.push_bind(scanned_rows);

                let rows = query.build().fetch_all(self.pool.as_ref()).await?;
                let row_count = rows.len();
                for row in rows {
                    let worktree = managed_worktree_from_row(&row)?;
                    if path_to_db_string(worktree.base_repo_path.as_path()) != base_repo_path {
                        continue;
                    }
                    if skipped_rows < offset {
                        skipped_rows += 1;
                        continue;
                    }

                    matching_rows.push(worktree);
                    if matching_rows.len() > limit as usize {
                        break;
                    }
                }

                if matching_rows.len() > limit as usize
                    || row_count < MANAGED_WORKTREE_LIST_SCAN_CHUNK_SIZE as usize
                {
                    break;
                }
                scanned_rows = scanned_rows.saturating_add(row_count as i64);
            }

            let has_more = matching_rows.len() > limit as usize;
            let data = matching_rows
                .into_iter()
                .take(limit as usize)
                .collect::<Vec<_>>();
            let next_cursor = has_more.then(|| offset.saturating_add(limit).to_string());
            return Ok(ManagedWorktreeListPage { data, next_cursor });
        }

        let mut query = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {} FROM managed_worktrees WHERE (",
            managed_worktree_select_columns()
        ));
        query.push_bind(include_deleted);
        query.push(" OR deleted_at_ms IS NULL)");
        query.push(
            r#"
ORDER BY
    CASE lifecycle_status
        WHEN 'active' THEN 1
        WHEN 'cleanup_pending' THEN 2
        WHEN 'released' THEN 3
        WHEN 'deleted' THEN 4
    END,
    updated_at_ms DESC,
    worktree_id DESC
"#,
        );
        query.push(" LIMIT ");
        query.push_bind(i64::from(limit) + 1);
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));

        let rows = query.build().fetch_all(self.pool.as_ref()).await?;
        let rows = rows
            .into_iter()
            .map(|row| managed_worktree_from_row(&row))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let has_more = rows.len() > limit as usize;
        let data = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
        let next_cursor = has_more.then(|| offset.saturating_add(limit).to_string());
        Ok(ManagedWorktreeListPage { data, next_cursor })
    }

    pub async fn list_cleanup_candidates(
        &self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> anyhow::Result<Vec<crate::ManagedWorktree>> {
        let limit = limit.clamp(1, MAX_MANAGED_WORKTREE_LIST_LIMIT);
        let sql = format!(
            r#"
SELECT
{}
FROM managed_worktrees
WHERE {}
ORDER BY
    force_delete_requested DESC,
    COALESCE(cleanup_after_ms, updated_at_ms) ASC,
    updated_at_ms ASC,
    worktree_id ASC
LIMIT ?
            "#,
            managed_worktree_select_columns(),
            managed_worktree_cleanup_candidate_predicate()
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(datetime_to_epoch_millis(now))
            .bind(i64::from(limit))
            .fetch_all(self.pool.as_ref())
            .await?;
        rows.into_iter()
            .map(|row| managed_worktree_from_row(&row))
            .collect()
    }

    /// Rechecks that a known cleanup candidate remains eligible immediately
    /// before deleting its linked Git worktree.
    pub async fn get_cleanup_candidate_for_execution(
        &self,
        worktree_id: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        let sql = format!(
            r#"
SELECT
{}
FROM managed_worktrees
WHERE worktree_id = ?
  AND {}
            "#,
            managed_worktree_select_columns(),
            managed_worktree_cleanup_candidate_predicate()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(worktree_id)
            .bind(datetime_to_epoch_millis(now))
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| managed_worktree_from_row(&row)).transpose()
    }

    pub async fn active_thread_managed_worktree(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        let sql = format!(
            r#"
SELECT
{}
FROM managed_worktrees
WHERE lifecycle_status = 'active'
  AND deleted_at_ms IS NULL
  AND EXISTS (
    SELECT 1
    FROM managed_worktree_assignments AS assignment
    WHERE assignment.worktree_id = managed_worktrees.worktree_id
      AND assignment.thread_id = ?
      AND assignment.detached_at_ms IS NULL
  )
ORDER BY (
    SELECT MAX(assignment.attached_at_ms)
    FROM managed_worktree_assignments AS assignment
    WHERE assignment.worktree_id = managed_worktrees.worktree_id
      AND assignment.thread_id = ?
      AND assignment.detached_at_ms IS NULL
) DESC, worktree_id DESC
LIMIT 1
            "#,
            managed_worktree_select_columns()
        );
        let thread_id = thread_id.to_string();
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(thread_id.as_str())
            .bind(thread_id.as_str())
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| managed_worktree_from_row(&row)).transpose()
    }

    pub async fn attach_managed_worktree(
        &self,
        params: ManagedWorktreeAttachParams,
    ) -> anyhow::Result<crate::ManagedWorktree> {
        validate_attach_params(&params)?;
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let mut tx = self.pool.begin().await?;
        let owner: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
SELECT owner_kind, owner_thread_id, owner_agent_run_id
FROM managed_worktrees
WHERE worktree_id = ?
  AND lifecycle_status = 'active'
  AND deleted_at_ms IS NULL
            "#,
        )
        .bind(params.worktree_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((owner_kind, owner_thread_id, owner_agent_run_id)) = owner.as_ref() {
            match &params.target {
                ManagedWorktreeAssignmentTarget::Thread(target_thread_id) => {
                    let target_thread_id = target_thread_id.to_string();
                    if owner_agent_run_id.is_some()
                        || owner_thread_id.as_deref() != Some(target_thread_id.as_str())
                            && owner_thread_id.is_some()
                    {
                        tx.rollback().await?;
                        anyhow::bail!(
                            "managed worktree {} is owned by {} and cannot be assigned to thread {}",
                            params.worktree_id,
                            owner_kind,
                            target_thread_id
                        );
                    }
                }
                ManagedWorktreeAssignmentTarget::AgentRun(target_agent_run_id) => {
                    if owner_thread_id.is_some()
                        || owner_agent_run_id.as_deref() != Some(target_agent_run_id.as_str())
                            && owner_agent_run_id.is_some()
                    {
                        tx.rollback().await?;
                        anyhow::bail!(
                            "managed worktree {} is owned by {} and cannot be assigned to agent run {}",
                            params.worktree_id,
                            owner_kind,
                            target_agent_run_id
                        );
                    }
                }
            }
        }
        let existing_assignment: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            r#"
SELECT assignment_id, thread_id, agent_run_id
FROM managed_worktree_assignments
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
LIMIT 1
                "#,
        )
        .bind(params.worktree_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        let mut assignment_exists = false;
        if let Some((assignment_id, thread_id, agent_run_id)) = existing_assignment.as_ref() {
            let same_target = match &params.target {
                ManagedWorktreeAssignmentTarget::Thread(target_thread_id) => {
                    thread_id.as_deref() == Some(target_thread_id.to_string().as_str())
                }
                ManagedWorktreeAssignmentTarget::AgentRun(target_agent_run_id) => {
                    agent_run_id.as_deref() == Some(target_agent_run_id.as_str())
                }
            };
            if !same_target {
                tx.rollback().await?;
                anyhow::bail!(
                    "managed worktree {} is already assigned by {}",
                    params.worktree_id,
                    assignment_id
                );
            }
            assignment_exists = true;
        }

        match &params.target {
            ManagedWorktreeAssignmentTarget::Thread(thread_id) => {
                let target_thread_id = thread_id.to_string();
                sqlx::query(
                    r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE thread_id = ?
  AND detached_at_ms IS NULL
  AND worktree_id != ?
                    "#,
                )
                .bind(now_ms)
                .bind(target_thread_id.as_str())
                .bind(params.worktree_id.as_str())
                .execute(&mut *tx)
                .await?;
                clear_stale_thread_owner(
                    &mut tx,
                    now_ms,
                    target_thread_id.as_str(),
                    params.worktree_id.as_str(),
                )
                .await?;
            }
            ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) => {
                sqlx::query(
                    r#"
UPDATE managed_worktree_assignments
SET detached_at_ms = COALESCE(detached_at_ms, ?)
WHERE agent_run_id = ?
  AND detached_at_ms IS NULL
  AND worktree_id != ?
                    "#,
                )
                .bind(now_ms)
                .bind(agent_run_id.as_str())
                .bind(params.worktree_id.as_str())
                .execute(&mut *tx)
                .await?;
                clear_stale_agent_owner(
                    &mut tx,
                    now_ms,
                    agent_run_id.as_str(),
                    params.worktree_id.as_str(),
                )
                .await?;
            }
        }

        let inserted = match &params.target {
            ManagedWorktreeAssignmentTarget::Thread(thread_id) => {
                let assignment_id = Uuid::new_v4().to_string();
                let result = sqlx::query(
                    r#"
INSERT INTO managed_worktree_assignments (
    assignment_id,
    worktree_id,
    thread_id,
    agent_run_id,
    attached_at_ms,
    detached_at_ms
)
SELECT ?, worktree_id, ?, NULL, ?, NULL
FROM managed_worktrees
WHERE worktree_id = ?
  AND lifecycle_status = 'active'
  AND deleted_at_ms IS NULL
ON CONFLICT(worktree_id) WHERE detached_at_ms IS NULL DO NOTHING
                    "#,
                )
                .bind(assignment_id)
                .bind(thread_id.to_string())
                .bind(now_ms)
                .bind(params.worktree_id.as_str())
                .execute(&mut *tx)
                .await?;
                result.rows_affected() > 0
            }
            ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) => {
                let assignment_id = Uuid::new_v4().to_string();
                let result = sqlx::query(
                    r#"
INSERT INTO managed_worktree_assignments (
    assignment_id,
    worktree_id,
    thread_id,
    agent_run_id,
    attached_at_ms,
    detached_at_ms
)
SELECT ?, worktree_id, NULL, ?, ?, NULL
FROM managed_worktrees
WHERE worktree_id = ?
  AND lifecycle_status = 'active'
  AND deleted_at_ms IS NULL
ON CONFLICT(worktree_id) WHERE detached_at_ms IS NULL DO NOTHING
                    "#,
                )
                .bind(assignment_id)
                .bind(agent_run_id.as_str())
                .bind(now_ms)
                .bind(params.worktree_id.as_str())
                .execute(&mut *tx)
                .await?;
                result.rows_affected() > 0
            }
        };

        if !inserted && !assignment_exists {
            tx.rollback().await?;
            anyhow::bail!(
                "managed worktree {} could not be assigned",
                params.worktree_id
            );
        }
        match &params.target {
            ManagedWorktreeAssignmentTarget::Thread(thread_id) => {
                sqlx::query(
                    r#"
UPDATE managed_worktrees
SET
    owner_kind = ?,
    owner_thread_id = ?,
    owner_agent_run_id = NULL,
    updated_at_ms = ?
WHERE worktree_id = ?
                    "#,
                )
                .bind(crate::ManagedWorktreeOwnerKind::MainSession.as_str())
                .bind(thread_id.to_string())
                .bind(now_ms)
                .bind(params.worktree_id.as_str())
                .execute(&mut *tx)
                .await?;
            }
            ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) => {
                sqlx::query(
                    r#"
UPDATE managed_worktrees
SET
    owner_kind = ?,
    owner_thread_id = NULL,
    owner_agent_run_id = ?,
    updated_at_ms = ?
WHERE worktree_id = ?
                    "#,
                )
                .bind(crate::ManagedWorktreeOwnerKind::BackgroundAgent.as_str())
                .bind(agent_run_id.as_str())
                .bind(now_ms)
                .bind(params.worktree_id.as_str())
                .execute(&mut *tx)
                .await?;
            }
        }

        let sql = format!(
            r#"
SELECT
{}
FROM managed_worktrees
WHERE worktree_id = ?
  AND lifecycle_status = 'active'
  AND deleted_at_ms IS NULL
            "#,
            managed_worktree_select_columns()
        );
        let worktree_row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(params.worktree_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
        let Some(worktree_row) = worktree_row else {
            tx.rollback().await?;
            anyhow::bail!(
                "managed worktree {} is not active or does not exist",
                params.worktree_id
            );
        };
        let worktree = managed_worktree_from_row(&worktree_row)?;
        tx.commit().await?;
        Ok(worktree)
    }

    pub async fn detach_managed_worktree(
        &self,
        params: ManagedWorktreeDetachParams,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        validate_detach_params(&params)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin().await?;
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

    pub async fn record_merge_candidate(
        &self,
        params: ManagedWorktreeMergeCandidateRecordParams,
    ) -> anyhow::Result<crate::ManagedWorktreeMergeCandidate> {
        validate_merge_candidate_params(&params)?;
        let candidate_id = params
            .candidate_id
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let test_summary_json = params
            .test_summary_json
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            r#"
UPDATE managed_worktree_merge_candidates
SET
    status = ?,
    updated_at_ms = ?,
    dismissed_at_ms = COALESCE(dismissed_at_ms, ?)
WHERE worktree_id = ?
  AND status IN ('open', 'blocked')
  AND head_sha <> ?
            "#,
        )
        .bind(crate::ManagedWorktreeMergeCandidateStatus::Dismissed.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .bind(params.worktree_id.as_str())
        .bind(params.head_sha.as_str())
        .execute(&mut *tx)
        .await?;
        let sql = format!(
            r#"
INSERT INTO managed_worktree_merge_candidates (
    candidate_id,
    worktree_id,
    target_ref,
    target_sha,
    base_sha,
    head_sha,
    status,
    conflict_summary,
    test_summary_json,
    created_at_ms,
    updated_at_ms,
    applied_at_ms,
    dismissed_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL)
ON CONFLICT(worktree_id, head_sha, target_ref) WHERE status IN ('open', 'blocked')
DO UPDATE SET
    target_sha = excluded.target_sha,
    base_sha = excluded.base_sha,
    status = excluded.status,
    conflict_summary = excluded.conflict_summary,
    test_summary_json = excluded.test_summary_json,
    updated_at_ms = excluded.updated_at_ms,
    applied_at_ms = NULL,
    dismissed_at_ms = NULL
RETURNING
{}
            "#,
            managed_worktree_merge_candidate_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(candidate_id)
            .bind(params.worktree_id)
            .bind(params.target_ref)
            .bind(params.target_sha)
            .bind(params.base_sha)
            .bind(params.head_sha)
            .bind(params.status.as_str())
            .bind(params.conflict_summary)
            .bind(test_summary_json)
            .bind(now_ms)
            .bind(now_ms)
            .fetch_one(&mut *tx)
            .await?;
        let candidate = managed_worktree_merge_candidate_from_row(&row)?;
        tx.commit().await?;
        Ok(candidate)
    }

    pub async fn list_merge_candidates(
        &self,
        worktree_id: &str,
        status: Option<crate::ManagedWorktreeMergeCandidateStatus>,
        limit: u32,
    ) -> anyhow::Result<Vec<crate::ManagedWorktreeMergeCandidate>> {
        let limit = limit.clamp(1, MAX_MANAGED_WORKTREE_LIST_LIMIT);
        let mut query = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {} FROM managed_worktree_merge_candidates WHERE worktree_id = ",
            managed_worktree_merge_candidate_select_columns()
        ));
        query.push_bind(worktree_id);
        if let Some(status) = status {
            query.push(" AND status = ");
            query.push_bind(status.as_str());
        }
        query.push(" ORDER BY created_at_ms DESC, candidate_id DESC LIMIT ");
        query.push_bind(i64::from(limit));

        let rows = query.build().fetch_all(self.pool.as_ref()).await?;
        rows.into_iter()
            .map(|row| managed_worktree_merge_candidate_from_row(&row))
            .collect()
    }

    pub async fn get_merge_candidate(
        &self,
        candidate_id: &str,
    ) -> anyhow::Result<Option<crate::ManagedWorktreeMergeCandidate>> {
        if candidate_id.trim().is_empty() {
            anyhow::bail!("managed worktree merge candidate id cannot be empty");
        }
        let sql = format!(
            r#"
SELECT
{}
FROM managed_worktree_merge_candidates
WHERE candidate_id = ?
            "#,
            managed_worktree_merge_candidate_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(candidate_id)
            .fetch_optional(self.pool.as_ref())
            .await?;

        row.map(|row| managed_worktree_merge_candidate_from_row(&row))
            .transpose()
    }

    pub async fn mark_merge_candidate_status(
        &self,
        candidate_id: &str,
        status: crate::ManagedWorktreeMergeCandidateStatus,
    ) -> anyhow::Result<Option<crate::ManagedWorktreeMergeCandidate>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let sql = format!(
            r#"
UPDATE managed_worktree_merge_candidates
SET
    status = ?,
    updated_at_ms = ?,
    applied_at_ms = CASE WHEN ? THEN COALESCE(applied_at_ms, ?) ELSE applied_at_ms END,
    dismissed_at_ms = CASE WHEN ? THEN COALESCE(dismissed_at_ms, ?) ELSE dismissed_at_ms END
WHERE candidate_id = ?
RETURNING
{}
            "#,
            managed_worktree_merge_candidate_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(status.as_str())
            .bind(now_ms)
            .bind(status == crate::ManagedWorktreeMergeCandidateStatus::Applied)
            .bind(now_ms)
            .bind(status == crate::ManagedWorktreeMergeCandidateStatus::Dismissed)
            .bind(now_ms)
            .bind(candidate_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| managed_worktree_merge_candidate_from_row(&row))
            .transpose()
    }

    pub async fn dismiss_merge_candidate(
        &self,
        candidate_id: &str,
    ) -> anyhow::Result<Option<crate::ManagedWorktreeMergeCandidate>> {
        if candidate_id.trim().is_empty() {
            anyhow::bail!("managed worktree merge candidate id cannot be empty");
        }
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let sql = format!(
            r#"
UPDATE managed_worktree_merge_candidates
SET
    status = ?,
    updated_at_ms = ?,
    dismissed_at_ms = COALESCE(dismissed_at_ms, ?)
WHERE candidate_id = ?
  AND status IN ('open', 'blocked')
RETURNING
{}
            "#,
            managed_worktree_merge_candidate_select_columns()
        );
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(crate::ManagedWorktreeMergeCandidateStatus::Dismissed.as_str())
            .bind(now_ms)
            .bind(now_ms)
            .bind(candidate_id)
            .fetch_optional(self.pool.as_ref())
            .await?;
        row.map(|row| managed_worktree_merge_candidate_from_row(&row))
            .transpose()
    }
}

impl StateRuntime {
    pub async fn mark_managed_worktree_cleanup_succeeded(
        &self,
        worktree_id: &str,
    ) -> anyhow::Result<Option<crate::ManagedWorktree>> {
        let now = Utc::now();
        let now_ms = datetime_to_epoch_millis(now);
        let now_seconds = datetime_to_epoch_seconds(now);
        let mut tx = self.pool.begin().await?;
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

fn validate_create_params(params: &ManagedWorktreeCreateParams) -> anyhow::Result<()> {
    if let Some(worktree_id) = params.worktree_id.as_deref()
        && worktree_id.trim().is_empty()
    {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    if !params.base_repo_path.is_absolute() {
        anyhow::bail!("managed worktree base repo path must be absolute");
    }
    if !params.worktree_path.is_absolute() {
        anyhow::bail!("managed worktree path must be absolute");
    }
    if let Some(branch) = params.branch.as_deref()
        && branch.trim().is_empty()
    {
        anyhow::bail!("managed worktree branch cannot be empty");
    }
    if let Some(identity) = params.identity.as_deref()
        && identity.trim().is_empty()
    {
        anyhow::bail!("managed worktree identity cannot be empty");
    }
    Ok(())
}

fn validate_cleanup_failure_params(
    params: &ManagedWorktreeCleanupFailureParams,
) -> anyhow::Result<()> {
    if params.worktree_id.trim().is_empty() {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    if params.reason.trim().is_empty() {
        anyhow::bail!("managed worktree cleanup failure reason cannot be empty");
    }
    Ok(())
}

fn validate_status_update_params(params: &ManagedWorktreeStatusUpdateParams) -> anyhow::Result<()> {
    if params.worktree_id.trim().is_empty() {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    if let Some(branch) = params.branch.as_deref()
        && branch.trim().is_empty()
    {
        anyhow::bail!("managed worktree branch cannot be empty");
    }
    Ok(())
}

fn validate_release_params(params: &ManagedWorktreeReleaseParams) -> anyhow::Result<()> {
    if params.worktree_id.trim().is_empty() {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    Ok(())
}

async fn ensure_not_active_background_agent_worktree_lease(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    worktree_id: &str,
) -> anyhow::Result<()> {
    let active_lease: Option<(String,)> = sqlx::query_as(
        r#"
SELECT run_id
FROM background_agent_worktree_leases
WHERE id = ? AND released_at IS NULL AND deleted_at IS NULL
        "#,
    )
    .bind(worktree_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((run_id,)) = active_lease {
        anyhow::bail!(
            "managed worktree {worktree_id} is owned by active background agent worktree lease for run {run_id}; release the background agent worktree lease first"
        );
    }
    Ok(())
}

fn validate_attach_params(params: &ManagedWorktreeAttachParams) -> anyhow::Result<()> {
    if params.worktree_id.trim().is_empty() {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    if let ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) = &params.target
        && agent_run_id.trim().is_empty()
    {
        anyhow::bail!("managed worktree assignment agent run id cannot be empty");
    }
    Ok(())
}

fn validate_detach_params(params: &ManagedWorktreeDetachParams) -> anyhow::Result<()> {
    if params.worktree_id.trim().is_empty() {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    if let ManagedWorktreeAssignmentTarget::AgentRun(agent_run_id) = &params.target
        && agent_run_id.trim().is_empty()
    {
        anyhow::bail!("managed worktree assignment agent run id cannot be empty");
    }
    Ok(())
}

fn validate_merge_candidate_params(
    params: &ManagedWorktreeMergeCandidateRecordParams,
) -> anyhow::Result<()> {
    if let Some(candidate_id) = params.candidate_id.as_deref()
        && candidate_id.trim().is_empty()
    {
        anyhow::bail!("managed worktree merge candidate id cannot be empty");
    }
    if params.worktree_id.trim().is_empty() {
        anyhow::bail!("managed worktree id cannot be empty");
    }
    if params.target_ref.trim().is_empty() {
        anyhow::bail!("managed worktree merge target ref cannot be empty");
    }
    if params.base_sha.trim().is_empty() {
        anyhow::bail!("managed worktree merge base sha cannot be empty");
    }
    if params.head_sha.trim().is_empty() {
        anyhow::bail!("managed worktree merge head sha cannot be empty");
    }
    Ok(())
}

async fn clear_stale_thread_owner(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    now_ms: i64,
    thread_id: &str,
    current_worktree_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
UPDATE managed_worktrees
SET
    owner_kind = ?,
    owner_thread_id = NULL,
    owner_agent_run_id = NULL,
    updated_at_ms = ?
WHERE owner_thread_id = ?
  AND worktree_id != ?
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
    .bind(thread_id)
    .bind(current_worktree_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn clear_stale_agent_owner(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    now_ms: i64,
    agent_run_id: &str,
    current_worktree_id: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
UPDATE managed_worktrees
SET
    owner_kind = ?,
    owner_thread_id = NULL,
    owner_agent_run_id = NULL,
    updated_at_ms = ?
WHERE owner_agent_run_id = ?
  AND worktree_id != ?
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
    .bind(agent_run_id)
    .bind(current_worktree_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn managed_worktree_cleanup_candidate_predicate() -> &'static str {
    r#"
mode = 'isolated_worktree'
  AND lifecycle_status = 'cleanup_pending'
  AND released_at_ms IS NOT NULL
  AND deleted_at_ms IS NULL
  AND worktree_path_key IS NOT NULL
  AND NOT EXISTS (
    SELECT 1
    FROM managed_worktree_assignments AS assignment
    WHERE assignment.worktree_id = managed_worktrees.worktree_id
      AND assignment.detached_at_ms IS NULL
  )
  AND NOT EXISTS (
    SELECT 1
    FROM managed_worktrees AS sibling
    WHERE sibling.worktree_id != managed_worktrees.worktree_id
      AND sibling.mode = 'isolated_worktree'
      AND sibling.deleted_at_ms IS NULL
      AND sibling.worktree_path_key = managed_worktrees.worktree_path_key
      AND (
        sibling.lifecycle_status = 'active'
        OR EXISTS (
          SELECT 1
          FROM managed_worktree_assignments AS sibling_assignment
          WHERE sibling_assignment.worktree_id = sibling.worktree_id
            AND sibling_assignment.detached_at_ms IS NULL
        )
      )
  )
  AND (
    cleanup_after_ms IS NULL
    OR cleanup_after_ms <= ?
    OR force_delete_requested = 1
  )
"#
}

pub(crate) fn managed_worktree_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ManagedWorktree> {
    ManagedWorktreeRow::try_from_row(row)?.try_into()
}

pub(crate) fn managed_worktree_merge_candidate_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::ManagedWorktreeMergeCandidate> {
    ManagedWorktreeMergeCandidateRow::try_from_row(row)?.try_into()
}

pub(crate) fn managed_worktree_select_columns() -> &'static str {
    r#"
    worktree_id,
    identity,
    mode,
    base_repo_path,
    worktree_path,
    branch,
    base_sha,
    head_sha,
    lifecycle_status,
    status_snapshot_json,
    dirty,
    cleanup_policy,
    force_delete_requested,
    owner_kind,
    owner_thread_id,
    owner_agent_run_id,
    created_at_ms,
    updated_at_ms,
    released_at_ms,
    cleanup_after_ms,
    deleted_at_ms
"#
}

fn managed_worktree_merge_candidate_select_columns() -> &'static str {
    r#"
    candidate_id,
    worktree_id,
    target_ref,
    target_sha,
    base_sha,
    head_sha,
    status,
    conflict_summary,
    test_summary_json,
    created_at_ms,
    updated_at_ms,
    applied_at_ms,
    dismissed_at_ms
"#
}

fn parse_managed_worktree_list_cursor(cursor: Option<&str>) -> anyhow::Result<u32> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let cursor = cursor.trim();
    if cursor.is_empty() {
        return Ok(0);
    }
    cursor
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("invalid managed worktree list cursor `{cursor}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::test_support::test_thread_metadata;
    use crate::runtime::test_support::unique_temp_dir;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    #[cfg(unix)]
    use std::collections::BTreeSet;

    async fn test_runtime() -> Arc<StateRuntime> {
        StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_temp_dir() -> anyhow::Result<PathBuf> {
        let path = unique_temp_dir();
        std::fs::create_dir_all(&path)?;
        Ok(path)
    }

    fn repo_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "codewith-managed-worktrees-{}",
            name.trim_start_matches('/').replace('/', "-")
        ))
    }

    fn create_params(worktree_id: &str, base_repo_path: &str) -> ManagedWorktreeCreateParams {
        let base_repo_path = repo_path(base_repo_path);
        let worktree_path = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join(worktree_id);
        create_params_for_paths(worktree_id, base_repo_path, worktree_path)
    }

    fn create_params_for_paths(
        worktree_id: &str,
        base_repo_path: PathBuf,
        worktree_path: PathBuf,
    ) -> ManagedWorktreeCreateParams {
        ManagedWorktreeCreateParams {
            worktree_id: Some(worktree_id.to_string()),
            identity: Some(format!("session:{worktree_id}")),
            mode: crate::ManagedWorktreeMode::IsolatedWorktree,
            base_repo_path,
            worktree_path,
            branch: Some(format!("codewith/{worktree_id}")),
            base_sha: Some("base-sha".to_string()),
            head_sha: Some("head-sha".to_string()),
            status_snapshot_json: json!({}),
            dirty: false,
            cleanup_policy: crate::ManagedWorktreeCleanupPolicy::DeleteIfClean,
            owner_kind: crate::ManagedWorktreeOwnerKind::MainSession,
            owner_thread_id: None,
            owner_agent_run_id: None,
            cleanup_after: None,
        }
    }

    async fn worktree_path_key(
        runtime: &StateRuntime,
        worktree_id: &str,
    ) -> anyhow::Result<Option<String>> {
        sqlx::query_scalar("SELECT worktree_path_key FROM managed_worktrees WHERE worktree_id = ?")
            .bind(worktree_id)
            .fetch_one(runtime.pool.as_ref())
            .await
            .map_err(Into::into)
    }

    #[tokio::test]
    async fn stores_normalized_worktree_path_key_on_create() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let base_repo_path = test_temp_dir()?.join("repo");
        let canonical_worktree_path = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("wt-key");
        let requested_worktree_path = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("unused")
            .join("..")
            .join("wt-key");
        std::fs::create_dir_all(&base_repo_path)?;

        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-key",
                base_repo_path,
                requested_worktree_path,
            ))
            .await?;

        assert_eq!(
            Some(path_to_db_string(&canonical_worktree_path)),
            worktree_path_key(runtime.as_ref(), "wt-key").await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn deleted_isolated_worktree_path_can_be_reused() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let base_repo_path = test_temp_dir()?.join("repo");
        let worktree_path = base_repo_path
            .join(".codewith")
            .join("worktrees")
            .join("reused");
        std::fs::create_dir_all(&base_repo_path)?;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params_for_paths(
                "wt-deleted",
                base_repo_path.clone(),
                worktree_path.clone(),
            ))
            .await?;
        store
            .mark_managed_worktree_deleted("wt-deleted")
            .await?
            .expect("worktree should be marked deleted");

        assert_eq!(
            "wt-reused",
            store
                .create_managed_worktree(create_params_for_paths(
                    "wt-reused",
                    base_repo_path,
                    worktree_path,
                ))
                .await?
                .worktree_id
        );
        Ok(())
    }

    #[tokio::test]
    async fn shared_repository_base_path_key_preserves_display_paths_and_does_not_block_isolated_admission()
    -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let temp = test_temp_dir()?;
        let shared_base_repo_path = temp.join("shared-repo");
        let isolated_base_repo_path = temp.join("isolated-repo");
        let shared_worktree_path = temp.join("shared-worktree-path");
        std::fs::create_dir_all(&shared_base_repo_path)?;
        std::fs::create_dir_all(&isolated_base_repo_path)?;
        let mut shared_params = create_params_for_paths(
            "shared",
            shared_base_repo_path.clone(),
            shared_worktree_path.clone(),
        );
        shared_params.mode = crate::ManagedWorktreeMode::SharedRepository;
        let store = runtime.managed_worktrees();
        let shared = store.create_managed_worktree(shared_params).await?;

        assert_eq!(
            path_to_db_string(&shared_base_repo_path),
            shared.base_repo_path.to_string_lossy()
        );
        assert_eq!(
            path_to_db_string(&shared_worktree_path),
            shared.worktree_path.to_string_lossy()
        );
        assert_eq!(
            Some(managed_worktree_path_key_from_display(
                path_to_db_string(&shared_base_repo_path).as_str()
            )),
            worktree_path_key(runtime.as_ref(), "shared").await?
        );
        assert_eq!(
            "isolated",
            store
                .create_managed_worktree(create_params_for_paths(
                    "isolated",
                    isolated_base_repo_path,
                    shared_worktree_path,
                ))
                .await?
                .worktree_id
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_isolated_worktree_path_equal_to_normalized_base_repo_path()
    -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let base_repo_path = test_temp_dir()?.join("repo");
        let child = base_repo_path.join("child");
        std::fs::create_dir_all(&child)?;

        let error = runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-normalized-base",
                base_repo_path,
                child.join(".."),
            ))
            .await
            .expect_err("a normalized base-repository path cannot be an isolated worktree");
        assert!(
            error
                .to_string()
                .contains("isolated managed worktree path cannot match the base repo path"),
            "unexpected admission error: {error:#}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_live_legacy_alias_blocks_new_canonical_admission() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let codex_home = temp.join("codewith-home");
        let repo = temp.join("repo");
        let other_repo = temp.join("other-repo");
        let legacy_repo = temp.join("repo-legacy");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(&other_repo)?;
        symlink(&repo, &legacy_repo)?;
        let worktree_path = repo.join(".codewith").join("worktrees").join("wt-a");
        let legacy_worktree_path = legacy_repo.join(".codewith").join("worktrees").join("wt-a");

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-a",
                repo.clone(),
                worktree_path.clone(),
            ))
            .await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-legacy",
                other_repo.clone(),
                other_repo
                    .join(".codewith")
                    .join("worktrees")
                    .join("wt-legacy"),
            ))
            .await?;
        sqlx::query(
            "UPDATE managed_worktrees SET base_repo_path = ?, worktree_path = ? WHERE worktree_id = ?",
        )
        .bind(legacy_repo.to_string_lossy().as_ref())
        .bind(legacy_worktree_path.to_string_lossy().as_ref())
        .bind("wt-legacy")
        .execute(runtime.pool.as_ref())
        .await?;
        drop(runtime);

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
        let retained_legacy = runtime
            .managed_worktrees()
            .get_managed_worktree("wt-legacy")
            .await?
            .expect("colliding legacy worktree should remain readable");
        assert_eq!(
            legacy_worktree_path.to_string_lossy().as_ref(),
            path_to_string(&retained_legacy.worktree_path)
        );
        runtime
            .managed_worktrees()
            .mark_managed_worktree_deleted("wt-a")
            .await?
            .expect("canonical worktree should be marked deleted");

        let error = runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths("wt-new", repo, worktree_path))
            .await
            .expect_err("retained live legacy alias must block a new canonical worktree");
        assert!(
            format!("{error:#}").contains("normalized isolated worktree path is already live"),
            "unexpected admission error: {error:#}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn retained_active_shared_repository_alias_blocks_new_canonical_admission()
    -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let codex_home = temp.join("codewith-home");
        let repo = temp.join("repo");
        let other_repo = temp.join("other-repo");
        let legacy_repo = temp.join("repo-legacy");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(&other_repo)?;
        symlink(&repo, &legacy_repo)?;

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let mut canonical_params =
            create_params_for_paths("shared-canonical", repo.clone(), repo.clone());
        canonical_params.mode = crate::ManagedWorktreeMode::SharedRepository;
        runtime
            .managed_worktrees()
            .create_managed_worktree(canonical_params)
            .await?;
        let mut legacy_params =
            create_params_for_paths("shared-legacy", other_repo.clone(), other_repo.clone());
        legacy_params.mode = crate::ManagedWorktreeMode::SharedRepository;
        runtime
            .managed_worktrees()
            .create_managed_worktree(legacy_params)
            .await?;
        sqlx::query(
            "UPDATE managed_worktrees SET base_repo_path = ?, worktree_path = ? WHERE worktree_id = ?",
        )
        .bind(legacy_repo.to_string_lossy().as_ref())
        .bind(legacy_repo.to_string_lossy().as_ref())
        .bind("shared-legacy")
        .execute(runtime.pool.as_ref())
        .await?;
        drop(runtime);

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
        let retained_legacy = runtime
            .managed_worktrees()
            .get_managed_worktree("shared-legacy")
            .await?
            .expect("colliding legacy shared repository should remain readable");
        assert_eq!(
            legacy_repo.to_string_lossy().as_ref(),
            path_to_string(&retained_legacy.base_repo_path)
        );
        runtime
            .managed_worktrees()
            .mark_managed_worktree_deleted("shared-canonical")
            .await?
            .expect("canonical shared repository should be marked deleted");

        let mut new_params = create_params_for_paths("shared-new", repo.clone(), repo);
        new_params.mode = crate::ManagedWorktreeMode::SharedRepository;
        let error = runtime
            .managed_worktrees()
            .create_managed_worktree(new_params)
            .await
            .expect_err("retained active shared repository alias must block a new canonical row");
        assert!(
            format!("{error:#}").contains("normalized shared repository path is already active"),
            "unexpected admission error: {error:#}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_alias_admission_is_rejected_at_the_sqlite_boundary() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let runtime = test_runtime().await;
        let temp = test_temp_dir()?;
        let repo = temp.join("repo");
        let repo_alias = temp.join("repo-alias");
        std::fs::create_dir_all(&repo)?;
        symlink(&repo, &repo_alias)?;
        let worktree_path = repo.join(".codewith").join("worktrees").join("wt-race");
        let alias_worktree_path = repo_alias
            .join(".codewith")
            .join("worktrees")
            .join("wt-race");
        let store = runtime.managed_worktrees();
        let (canonical_result, alias_result) = tokio::join!(
            store.create_managed_worktree(create_params_for_paths(
                "wt-canonical",
                repo,
                worktree_path,
            )),
            store.create_managed_worktree(create_params_for_paths(
                "wt-alias",
                repo_alias,
                alias_worktree_path,
            )),
        );

        assert_eq!(
            1,
            usize::from(canonical_result.is_ok()) + usize::from(alias_result.is_ok())
        );
        let error = canonical_result
            .err()
            .or_else(|| alias_result.err())
            .expect("one concurrent admission must be rejected");
        assert!(
            format!("{error:#}").contains("normalized isolated worktree path is already live"),
            "unexpected admission error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn normalizes_ordinary_parent_components() -> anyhow::Result<()> {
        let temp = test_temp_dir()?;
        let parent = temp.join("parent");
        let child = parent.join("child");
        std::fs::create_dir_all(&child)?;

        assert_eq!(
            path_to_db_string(&parent),
            path_to_db_string(&child.join(".."))
        );
        assert_eq!(
            path_to_db_string(&parent.join("missing")),
            path_to_db_string(&child.join("..").join("missing"))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn resolves_existing_symlink_parent_components_in_filesystem_order() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let physical_parent = temp.join("physical-parent");
        let target = physical_parent.join("target");
        let alias = temp.join("alias");
        std::fs::create_dir_all(&target)?;
        symlink(&target, &alias)?;

        let expected = std::fs::canonicalize(&physical_parent)?;
        let symlink_parent = alias.join("..");

        assert_eq!(
            path_to_db_string(&expected),
            path_to_db_string(&symlink_parent)
        );
        assert_ne!(path_to_db_string(&temp), path_to_db_string(&symlink_parent));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn normalizes_missing_descendants_after_symlink_parent_components() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let physical_parent = temp.join("physical-parent");
        let target = physical_parent.join("target");
        let alias = temp.join("alias");
        std::fs::create_dir_all(&target)?;
        symlink(&target, &alias)?;

        let missing_leaf = alias.join("..").join("missing").join("leaf");
        let expected = std::fs::canonicalize(&physical_parent)?
            .join("missing")
            .join("leaf");

        assert_eq!(
            path_to_db_string(&expected),
            path_to_db_string(&missing_leaf)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn normalizes_missing_suffix_parent_components_after_resolving_symlink() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let physical_parent = temp.join("physical-parent");
        let target = physical_parent.join("target");
        let alias = temp.join("alias");
        std::fs::create_dir_all(&target)?;
        symlink(&target, &alias)?;

        assert_eq!(
            path_to_db_string(&target.join("leaf")),
            path_to_db_string(&alias.join("missing").join("..").join("leaf"))
        );
        assert_eq!(
            path_to_db_string(&physical_parent.join("leaf")),
            path_to_db_string(&alias.join("missing").join("..").join("..").join("leaf"))
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn preserves_windows_verbatim_and_unc_paths_when_leaves_are_missing() {
        assert_eq!(
            r"C:\managed-worktrees\missing",
            path_to_db_string(Path::new(r"\\?\C:\managed-worktrees\missing"))
        );
        assert_eq!(
            r"\\server\share\managed-worktrees\missing",
            path_to_db_string(Path::new(r"\\?\UNC\server\share\managed-worktrees\missing"))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_normalizes_noncolliding_legacy_managed_worktree_paths() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let codex_home = temp.join("codewith-home");
        let repo = temp.join("repo");
        let symlink_target = repo.join("symlink-target");
        let legacy_alias = temp.join("repo-legacy");
        std::fs::create_dir_all(&symlink_target)?;
        symlink(&symlink_target, &legacy_alias)?;
        let legacy_repo = legacy_alias.join("..");
        let worktree = repo.join(".codewith").join("worktrees").join("wt-legacy");
        let legacy_worktree = legacy_repo
            .join(".codewith")
            .join("worktrees")
            .join("wt-legacy");

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-legacy",
                repo.clone(),
                worktree.clone(),
            ))
            .await?;
        sqlx::query(
            "UPDATE managed_worktrees SET base_repo_path = ?, worktree_path = ? WHERE worktree_id = ?",
        )
        .bind(legacy_repo.to_string_lossy().as_ref())
        .bind(legacy_worktree.to_string_lossy().as_ref())
        .bind("wt-legacy")
        .execute(runtime.pool.as_ref())
        .await?;
        drop(runtime);

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        let normalized = runtime
            .managed_worktrees()
            .get_managed_worktree("wt-legacy")
            .await?
            .expect("legacy managed worktree should remain readable");
        assert_eq!(
            path_to_db_string(&repo),
            path_to_string(&normalized.base_repo_path)
        );
        assert_eq!(
            path_to_db_string(&worktree),
            path_to_string(&normalized.worktree_path)
        );
        assert_eq!(
            vec![normalized.clone()],
            runtime
                .managed_worktrees()
                .list_managed_worktrees_page(
                    Some(&repo),
                    /*include_deleted*/ false,
                    /*cursor*/ None,
                    /*limit*/ 10,
                )
                .await?
                .data
        );
        drop(runtime);

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
        assert_eq!(
            normalized,
            runtime
                .managed_worktrees()
                .get_managed_worktree("wt-legacy")
                .await?
                .expect("a second startup should retain the normalized row")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn startup_retains_colliding_legacy_paths_and_blocks_alias_cleanup_while_owned()
    -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = test_temp_dir()?;
        let codex_home = temp.join("codewith-home");
        let repo = temp.join("repo");
        let other_repo = temp.join("other-repo");
        let legacy_repo = temp.join("repo-legacy");
        std::fs::create_dir_all(&repo)?;
        std::fs::create_dir_all(&other_repo)?;
        symlink(&repo, &legacy_repo)?;
        let worktree = repo.join(".codewith").join("worktrees").join("wt-a");
        let legacy_worktree = legacy_repo.join(".codewith").join("worktrees").join("wt-a");

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths("wt-a", repo.clone(), worktree))
            .await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-b",
                other_repo.clone(),
                other_repo.join(".codewith").join("worktrees").join("wt-b"),
            ))
            .await?;
        sqlx::query(
            "UPDATE managed_worktrees SET base_repo_path = ?, worktree_path = ? WHERE worktree_id = ?",
        )
        .bind(legacy_repo.to_string_lossy().as_ref())
        .bind(legacy_worktree.to_string_lossy().as_ref())
        .bind("wt-b")
        .execute(runtime.pool.as_ref())
        .await?;
        drop(runtime);

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
        let legacy = runtime
            .managed_worktrees()
            .get_managed_worktree("wt-b")
            .await?
            .expect("colliding legacy managed worktree should remain readable");
        assert_eq!(
            legacy_repo.to_string_lossy().as_ref(),
            path_to_string(&legacy.base_repo_path)
        );
        assert_eq!(
            legacy_worktree.to_string_lossy().as_ref(),
            path_to_string(&legacy.worktree_path)
        );
        let page = runtime
            .managed_worktrees()
            .list_managed_worktrees_page(
                Some(&repo),
                /*include_deleted*/ false,
                /*cursor*/ None,
                /*limit*/ 10,
            )
            .await?;
        let ids = page
            .data
            .iter()
            .map(|worktree| worktree.worktree_id.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(BTreeSet::from(["wt-a", "wt-b"]), ids);

        let store = runtime.managed_worktrees();
        let thread_id = ThreadId::new();
        runtime
            .upsert_thread(&test_thread_metadata(&temp, thread_id, repo.clone()))
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-b".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;
        let stale_force_candidate = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "wt-a".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::ForceDelete,
                force_delete: true,
                status_snapshot_json: json!({"dirty": true}),
                dirty: true,
            })
            .await?
            .expect("stale worktree should be released for cleanup");
        let unique = store
            .create_managed_worktree(create_params_for_paths(
                "wt-unique",
                repo.clone(),
                repo.join(".codewith").join("worktrees").join("wt-unique"),
            ))
            .await?;
        let unique_force_candidate = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: unique.worktree_id,
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::ForceDelete,
                force_delete: true,
                status_snapshot_json: json!({"dirty": true}),
                dirty: true,
            })
            .await?
            .expect("unique worktree should be released for cleanup");

        assert_eq!(
            vec![unique_force_candidate.clone()],
            store
                .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
                .await?
        );
        assert_eq!(
            None,
            store
                .get_cleanup_candidate_for_execution(
                    stale_force_candidate.worktree_id.as_str(),
                    chrono::Utc::now(),
                )
                .await?
        );
        assert_eq!(
            Some(unique_force_candidate.clone()),
            store
                .get_cleanup_candidate_for_execution(
                    unique_force_candidate.worktree_id.as_str(),
                    chrono::Utc::now(),
                )
                .await?
        );

        store
            .detach_managed_worktree(ManagedWorktreeDetachParams {
                worktree_id: "wt-b".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;
        store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "wt-b".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::Retain,
                force_delete: false,
                status_snapshot_json: json!({"dirty": false}),
                dirty: false,
            })
            .await?;
        let candidate_ids = store
            .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
            .await?
            .into_iter()
            .map(|worktree| worktree.worktree_id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            BTreeSet::from(["wt-a".to_string(), "wt-unique".to_string()]),
            candidate_ids
        );
        assert_eq!(
            Some(stale_force_candidate),
            store
                .get_cleanup_candidate_for_execution("wt-a", chrono::Utc::now())
                .await?
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn startup_normalizes_macos_var_alias_rows() -> anyhow::Result<()> {
        let temp = test_temp_dir()?;
        let codex_home = temp.join("codewith-home");
        let canonical_repo = std::fs::canonicalize(&temp)?;
        let legacy_repo = Path::new("/var").join(
            canonical_repo
                .strip_prefix("/private/var")
                .expect("macOS temporary directory should use the /private/var alias"),
        );
        let worktree = canonical_repo.join("missing-worktree");
        let legacy_worktree = legacy_repo.join("missing-worktree");

        let runtime = StateRuntime::init(codex_home.clone(), "test-provider".to_string()).await?;
        runtime
            .managed_worktrees()
            .create_managed_worktree(create_params_for_paths(
                "wt-var-alias",
                canonical_repo.clone(),
                worktree.clone(),
            ))
            .await?;
        sqlx::query(
            "UPDATE managed_worktrees SET base_repo_path = ?, worktree_path = ? WHERE worktree_id = ?",
        )
        .bind(legacy_repo.to_string_lossy().as_ref())
        .bind(legacy_worktree.to_string_lossy().as_ref())
        .bind("wt-var-alias")
        .execute(runtime.pool.as_ref())
        .await?;
        drop(runtime);

        let runtime = StateRuntime::init(codex_home, "test-provider".to_string()).await?;
        let normalized = runtime
            .managed_worktrees()
            .get_managed_worktree("wt-var-alias")
            .await?
            .expect("legacy /var row should remain readable");
        assert_eq!(
            path_to_db_string(&canonical_repo),
            path_to_string(&normalized.base_repo_path)
        );
        assert_eq!(
            path_to_db_string(&worktree),
            path_to_string(&normalized.worktree_path)
        );
        Ok(())
    }

    #[tokio::test]
    async fn lists_managed_worktrees_with_pagination() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        let first = store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        let second = store
            .create_managed_worktree(create_params("wt-b", "/repo-a"))
            .await?;

        let first_page = store
            .list_managed_worktrees_page(
                Some(repo_path("/repo-a").as_path()),
                /*include_deleted*/ false,
                /*cursor*/ None,
                /*limit*/ 1,
            )
            .await?;
        assert_eq!(vec![second], first_page.data);
        assert_eq!(Some("1".to_string()), first_page.next_cursor);

        let second_page = store
            .list_managed_worktrees_page(
                Some(repo_path("/repo-a").as_path()),
                /*include_deleted*/ false,
                first_page.next_cursor.as_deref(),
                /*limit*/ 1,
            )
            .await?;
        assert_eq!(vec![first], second_page.data);
        assert_eq!(None, second_page.next_cursor);

        Ok(())
    }

    #[tokio::test]
    async fn filters_managed_worktrees_by_base_repo_path() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        let expected = store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        store
            .create_managed_worktree(create_params("wt-b", "/repo-b"))
            .await?;

        let page = store
            .list_managed_worktrees_page(
                Some(repo_path("/repo-a").as_path()),
                /*include_deleted*/ false,
                /*cursor*/ None,
                DEFAULT_MANAGED_WORKTREE_LIST_LIMIT,
            )
            .await?;
        assert_eq!(vec![expected], page.data);
        assert_eq!(None, page.next_cursor);

        Ok(())
    }

    #[tokio::test]
    async fn paginates_normalized_managed_worktrees_across_sql_chunks() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-target-a", "/target-repo"))
            .await?;
        store
            .create_managed_worktree(create_params("wt-target-b", "/target-repo"))
            .await?;
        for index in 0..=MANAGED_WORKTREE_LIST_SCAN_CHUNK_SIZE {
            let worktree_id = format!("wt-nonmatching-{index:03}");
            store
                .create_managed_worktree(create_params(&worktree_id, "/other-repo"))
                .await?;
        }
        sqlx::query("UPDATE managed_worktrees SET updated_at_ms = ? WHERE worktree_id LIKE ?")
            .bind(2_000_000_000_000_i64)
            .bind("wt-nonmatching-%")
            .execute(runtime.pool.as_ref())
            .await?;
        sqlx::query("UPDATE managed_worktrees SET updated_at_ms = ? WHERE worktree_id IN (?, ?)")
            .bind(1_000_000_000_000_i64)
            .bind("wt-target-a")
            .bind("wt-target-b")
            .execute(runtime.pool.as_ref())
            .await?;
        let target_a = store
            .get_managed_worktree("wt-target-a")
            .await?
            .expect("target worktree should exist");
        let target_b = store
            .get_managed_worktree("wt-target-b")
            .await?
            .expect("target worktree should exist");

        let first_page = store
            .list_managed_worktrees_page(
                Some(repo_path("/target-repo").as_path()),
                /*include_deleted*/ false,
                /*cursor*/ None,
                /*limit*/ 1,
            )
            .await?;
        assert_eq!(vec![target_b], first_page.data);
        assert_eq!(Some("1".to_string()), first_page.next_cursor);

        let second_page = store
            .list_managed_worktrees_page(
                Some(repo_path("/target-repo").as_path()),
                /*include_deleted*/ false,
                first_page.next_cursor.as_deref(),
                /*limit*/ 1,
            )
            .await?;
        assert_eq!(vec![target_a], second_page.data);
        assert_eq!(None, second_page.next_cursor);

        Ok(())
    }

    #[tokio::test]
    async fn cleanup_failure_queues_due_isolated_candidate_until_success() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;

        let queued = runtime
            .record_managed_worktree_cleanup_failure(ManagedWorktreeCleanupFailureParams {
                worktree_id: "wt-a".to_string(),
                reason: "git worktree remove failed".to_string(),
                dirty: false,
                status_snapshot_json: json!({"dirty": false}),
                retry_after: Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
                force_delete_required: false,
            })
            .await?
            .expect("worktree should be queued for cleanup");
        assert_eq!(
            queued.lifecycle_status,
            crate::ManagedWorktreeLifecycleStatus::CleanupPending
        );
        assert!(!queued.force_delete_requested);
        assert_eq!(
            store
                .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
                .await?,
            vec![queued]
        );

        let deleted = runtime
            .mark_managed_worktree_cleanup_succeeded("wt-a")
            .await?
            .expect("cleanup success should mark worktree deleted");
        assert_eq!(
            deleted.lifecycle_status,
            crate::ManagedWorktreeLifecycleStatus::Deleted
        );
        assert!(deleted.deleted_at.is_some());
        assert_eq!(
            store
                .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
                .await?,
            Vec::<crate::ManagedWorktree>::new()
        );

        Ok(())
    }

    #[tokio::test]
    async fn active_assignment_blocks_cleanup_candidate() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        let worktree = store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        let thread_id = ThreadId::new();
        let codex_home = unique_temp_dir();
        let metadata = test_thread_metadata(&codex_home, thread_id, repo_path("/repo-a"));
        runtime.upsert_thread(&metadata).await?;
        let attached = store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: worktree.worktree_id.clone(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;
        assert_eq!(worktree.worktree_id, attached.worktree_id);
        assert_eq!(Some(thread_id), attached.owner_thread_id);
        assert_eq!(None, attached.owner_agent_run_id);

        runtime
            .record_managed_worktree_cleanup_failure(ManagedWorktreeCleanupFailureParams {
                worktree_id: "wt-a".to_string(),
                reason: "cleanup requested while active".to_string(),
                dirty: false,
                status_snapshot_json: json!({"dirty": false}),
                retry_after: None,
                force_delete_required: false,
            })
            .await?;
        assert_eq!(
            store
                .list_cleanup_candidates(chrono::Utc::now(), /*limit*/ 10)
                .await?,
            Vec::<crate::ManagedWorktree>::new()
        );

        Ok(())
    }

    #[tokio::test]
    async fn reads_active_thread_managed_worktree() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        let expected = store
            .create_managed_worktree(create_params("wt-b", "/repo-a"))
            .await?;
        let thread_id = ThreadId::new();
        let codex_home = unique_temp_dir();
        runtime
            .upsert_thread(&test_thread_metadata(
                &codex_home,
                thread_id,
                PathBuf::from("/repo-a"),
            ))
            .await?;

        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: expected.worktree_id.clone(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;

        assert_eq!(
            Some(expected.worktree_id.clone()),
            store
                .active_thread_managed_worktree(thread_id)
                .await?
                .map(|worktree| worktree.worktree_id)
        );

        store
            .detach_managed_worktree(ManagedWorktreeDetachParams {
                worktree_id: expected.worktree_id,
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;
        assert_eq!(None, store.active_thread_managed_worktree(thread_id).await?);

        Ok(())
    }

    #[tokio::test]
    async fn thread_reassignment_clears_previous_worktree_owner() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        store
            .create_managed_worktree(create_params("wt-b", "/repo-a"))
            .await?;

        let codex_home = unique_temp_dir();
        let first_thread_id = ThreadId::new();
        let second_thread_id = ThreadId::new();
        runtime
            .upsert_thread(&test_thread_metadata(
                &codex_home,
                first_thread_id,
                repo_path("/repo-a"),
            ))
            .await?;
        runtime
            .upsert_thread(&test_thread_metadata(
                &codex_home,
                second_thread_id,
                repo_path("/repo-a"),
            ))
            .await?;

        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(first_thread_id),
            })
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-b".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(first_thread_id),
            })
            .await?;
        let released = store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(second_thread_id),
            })
            .await?;

        assert_eq!(Some(second_thread_id), released.owner_thread_id);
        assert_eq!(
            store
                .get_managed_worktree("wt-b")
                .await?
                .expect("wt-b exists")
                .owner_thread_id,
            Some(first_thread_id)
        );

        Ok(())
    }

    #[tokio::test]
    async fn detach_clears_owner_when_no_active_assignment_remains() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        let thread_id = ThreadId::new();
        let codex_home = unique_temp_dir();
        runtime
            .upsert_thread(&test_thread_metadata(
                &codex_home,
                thread_id,
                repo_path("/repo-a"),
            ))
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;

        let detached = store
            .detach_managed_worktree(ManagedWorktreeDetachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?
            .expect("worktree exists");

        assert_eq!(crate::ManagedWorktreeOwnerKind::Manual, detached.owner_kind);
        assert_eq!(None, detached.owner_thread_id);
        assert_eq!(None, detached.owner_agent_run_id);

        Ok(())
    }

    #[tokio::test]
    async fn release_isolated_worktree_sets_cleanup_pending() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;

        let released = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "wt-a".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::ForceDelete,
                force_delete: true,
                status_snapshot_json: json!({"dirty": true}),
                dirty: true,
            })
            .await?
            .expect("worktree should exist");

        assert_eq!(
            crate::ManagedWorktreeLifecycleStatus::CleanupPending,
            released.lifecycle_status
        );
        assert_eq!(
            crate::ManagedWorktreeCleanupPolicy::ForceDelete,
            released.cleanup_policy
        );
        assert!(released.force_delete_requested);
        assert!(released.dirty);
        assert_eq!(json!({"dirty": true}), released.status_snapshot_json);
        assert!(released.released_at.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn force_delete_release_overrides_retain_cleanup_policy() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;

        let released = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "wt-a".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::Retain,
                force_delete: true,
                status_snapshot_json: json!({"dirty": false}),
                dirty: false,
            })
            .await?
            .expect("worktree should exist");

        assert_eq!(
            crate::ManagedWorktreeLifecycleStatus::CleanupPending,
            released.lifecycle_status
        );
        assert_eq!(
            crate::ManagedWorktreeCleanupPolicy::Retain,
            released.cleanup_policy
        );
        assert!(released.force_delete_requested);

        Ok(())
    }

    #[tokio::test]
    async fn release_rejects_active_assignment() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        let thread_id = ThreadId::new();
        let codex_home = unique_temp_dir();
        runtime
            .upsert_thread(&test_thread_metadata(
                &codex_home,
                thread_id,
                repo_path("/repo-a"),
            ))
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;

        let err = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "wt-a".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::DeleteIfClean,
                force_delete: false,
                status_snapshot_json: json!({"dirty": false}),
                dirty: false,
            })
            .await
            .expect_err("active assignments must block release");

        assert!(
            err.to_string().contains("has an active assignment"),
            "unexpected error: {err}"
        );
        let worktree = store
            .get_managed_worktree("wt-a")
            .await?
            .expect("worktree should remain active");
        assert_eq!(
            crate::ManagedWorktreeLifecycleStatus::Active,
            worktree.lifecycle_status
        );
        assert_eq!(Some(thread_id), worktree.owner_thread_id);
        let active_assignment_count: (i64,) = sqlx::query_as(
            r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind("wt-a")
        .fetch_one(runtime.pool.as_ref())
        .await?;
        assert_eq!((1,), active_assignment_count);

        Ok(())
    }

    #[tokio::test]
    async fn active_background_agent_lease_blocks_generic_detach_and_release() -> anyhow::Result<()>
    {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        runtime
            .create_background_agent_run(&crate::BackgroundAgentRunCreateParams {
                id: "run-1".to_string(),
                idempotency_key: Some("idem-1".to_string()),
                request_id: Some("req-1".to_string()),
                source: "cli".to_string(),
                prompt_snapshot_ref: "prompt://run-1".to_string(),
                input_snapshot_ref: None,
                thread_id: None,
                thread_store_kind: "background-agent".to_string(),
                thread_store_id: None,
                rollout_path: None,
                parent_thread_id: None,
                parent_agent_run_id: None,
                spawn_linkage_json: None,
                auth_profile_ref: None,
                status_reason: Some("worktree owner".to_string()),
                config_fingerprint: None,
                version_fingerprint: None,
            })
            .await?;
        let repo = repo_path("/repo");
        let worktree = repo.join(".git").join("worktrees").join("run-1");
        runtime
            .create_background_agent_worktree_lease(
                &crate::BackgroundAgentWorktreeLeaseCreateParams {
                    id: "lease-1".to_string(),
                    run_id: "run-1".to_string(),
                    identity: "bg-run-1".to_string(),
                    mode: crate::BackgroundAgentWorkspaceMode::IsolatedWorktree,
                    base_repo_path: path_to_db_string(&repo),
                    worktree_path: path_to_db_string(&worktree),
                    branch: Some("codewith/bg-run-1".to_string()),
                    head_sha: Some("abc123".to_string()),
                    status_snapshot_json: json!({"dirty": false}),
                    dirty: false,
                    cleanup_after: None,
                },
            )
            .await?;

        let detach_err = store
            .detach_managed_worktree(ManagedWorktreeDetachParams {
                worktree_id: "lease-1".to_string(),
                target: ManagedWorktreeAssignmentTarget::AgentRun("run-1".to_string()),
            })
            .await
            .expect_err("generic detach should not clear an active background-agent lease");
        assert!(
            detach_err
                .to_string()
                .contains("active background agent worktree lease"),
            "unexpected detach error: {detach_err}"
        );
        let release_err = store
            .release_managed_worktree(ManagedWorktreeReleaseParams {
                worktree_id: "lease-1".to_string(),
                cleanup_policy: crate::ManagedWorktreeCleanupPolicy::DeleteIfClean,
                force_delete: false,
                status_snapshot_json: json!({"dirty": false}),
                dirty: false,
            })
            .await
            .expect_err("generic release should not bypass background-agent lease release");
        assert!(
            release_err
                .to_string()
                .contains("active background agent worktree lease"),
            "unexpected release error: {release_err}"
        );

        let lease = runtime
            .get_background_agent_worktree_lease("lease-1")
            .await?
            .expect("lease should remain active");
        assert_eq!(lease.released_at, None);
        let worktree = store
            .get_managed_worktree("lease-1")
            .await?
            .expect("managed worktree should still exist");
        assert_eq!(
            crate::ManagedWorktreeOwnerKind::BackgroundAgent,
            worktree.owner_kind
        );
        assert_eq!(Some("run-1"), worktree.owner_agent_run_id.as_deref());
        let active_assignment_count: (i64,) = sqlx::query_as(
            r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ? AND agent_run_id = ? AND detached_at_ms IS NULL
            "#,
        )
        .bind("lease-1")
        .bind("run-1")
        .fetch_one(runtime.pool.as_ref())
        .await?;
        assert_eq!((1,), active_assignment_count);

        let deleted = store
            .mark_managed_worktree_deleted("lease-1")
            .await?
            .expect("background-agent worktree should be marked deleted");
        assert_eq!(
            crate::ManagedWorktreeLifecycleStatus::Deleted,
            deleted.lifecycle_status
        );
        let lease = runtime
            .get_background_agent_worktree_lease("lease-1")
            .await?
            .expect("lease should remain readable");
        assert!(lease.released_at.is_some());
        assert!(lease.deleted_at.is_some());
        let active_assignment_count: (i64,) = sqlx::query_as(
            r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ? AND agent_run_id = ? AND detached_at_ms IS NULL
            "#,
        )
        .bind("lease-1")
        .bind("run-1")
        .fetch_one(runtime.pool.as_ref())
        .await?;
        assert_eq!((0,), active_assignment_count);

        Ok(())
    }

    #[tokio::test]
    async fn mark_deleted_detaches_active_assignment() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        let thread_id = ThreadId::new();
        let codex_home = unique_temp_dir();
        runtime
            .upsert_thread(&test_thread_metadata(
                &codex_home,
                thread_id,
                repo_path("/repo-a"),
            ))
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::Thread(thread_id),
            })
            .await?;

        let deleted = store
            .mark_managed_worktree_deleted("wt-a")
            .await?
            .expect("worktree should exist");

        assert_eq!(
            crate::ManagedWorktreeLifecycleStatus::Deleted,
            deleted.lifecycle_status
        );
        assert!(deleted.released_at.is_some());
        assert!(deleted.deleted_at.is_some());
        let active_assignment_count: (i64,) = sqlx::query_as(
            r#"
SELECT COUNT(*)
FROM managed_worktree_assignments
WHERE worktree_id = ?
  AND detached_at_ms IS NULL
            "#,
        )
        .bind("wt-a")
        .fetch_one(runtime.pool.as_ref())
        .await?;
        assert_eq!((0,), active_assignment_count);

        Ok(())
    }

    #[tokio::test]
    async fn agent_reassignment_clears_previous_worktree_owner() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        store
            .create_managed_worktree(create_params("wt-b", "/repo-a"))
            .await?;

        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::AgentRun("agent-a".to_string()),
            })
            .await?;
        store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-b".to_string(),
                target: ManagedWorktreeAssignmentTarget::AgentRun("agent-a".to_string()),
            })
            .await?;
        let released = store
            .attach_managed_worktree(ManagedWorktreeAttachParams {
                worktree_id: "wt-a".to_string(),
                target: ManagedWorktreeAssignmentTarget::AgentRun("agent-b".to_string()),
            })
            .await?;

        assert_eq!(Some("agent-b"), released.owner_agent_run_id.as_deref());
        assert_eq!(
            store
                .get_managed_worktree("wt-b")
                .await?
                .expect("wt-b exists")
                .owner_agent_run_id
                .as_deref(),
            Some("agent-a")
        );

        Ok(())
    }

    #[tokio::test]
    async fn merge_candidates_are_unique_per_active_worktree_head_and_target() -> anyhow::Result<()>
    {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;

        let open = store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-1".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "refs/heads/main".to_string(),
                target_sha: Some("target-sha".to_string()),
                base_sha: "base-sha".to_string(),
                head_sha: "head-sha".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Open,
                conflict_summary: None,
                test_summary_json: Some(json!({"status": "passed"})),
            })
            .await?;
        assert_eq!(open.candidate_id, "candidate-1");

        let blocked = store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-2".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "refs/heads/main".to_string(),
                target_sha: Some("target-sha-2".to_string()),
                base_sha: "base-sha-2".to_string(),
                head_sha: "head-sha".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Blocked,
                conflict_summary: Some("src/lib.rs".to_string()),
                test_summary_json: Some(json!({"status": "conflicts"})),
            })
            .await?;
        assert_eq!(blocked.candidate_id, "candidate-1");
        assert_eq!(
            blocked.status,
            crate::ManagedWorktreeMergeCandidateStatus::Blocked
        );
        assert_eq!(blocked.conflict_summary, Some("src/lib.rs".to_string()));
        assert_eq!(
            store
                .list_merge_candidates(
                    "wt-a",
                    Some(crate::ManagedWorktreeMergeCandidateStatus::Blocked),
                    /*limit*/ 10,
                )
                .await?,
            vec![blocked.clone()]
        );

        let dismissed = store
            .mark_merge_candidate_status(
                "candidate-1",
                crate::ManagedWorktreeMergeCandidateStatus::Dismissed,
            )
            .await?
            .expect("candidate should be dismissed");
        assert_eq!(
            dismissed.status,
            crate::ManagedWorktreeMergeCandidateStatus::Dismissed
        );
        assert!(dismissed.dismissed_at.is_some());

        let reopened = store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-3".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "refs/heads/main".to_string(),
                target_sha: Some("target-sha-3".to_string()),
                base_sha: "base-sha-3".to_string(),
                head_sha: "head-sha".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Open,
                conflict_summary: None,
                test_summary_json: None,
            })
            .await?;
        assert_eq!(reopened.candidate_id, "candidate-3");

        Ok(())
    }

    #[tokio::test]
    async fn refreshed_merge_candidate_supersedes_stale_open_candidate() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;

        let stale = store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-stale".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "HEAD".to_string(),
                target_sha: Some("target-sha".to_string()),
                base_sha: "base-sha".to_string(),
                head_sha: "head-sha-old".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Open,
                conflict_summary: None,
                test_summary_json: None,
            })
            .await?;
        assert_eq!("candidate-stale", stale.candidate_id);
        let stale_other_target = store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-stale-other-target".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "refs/heads/release".to_string(),
                target_sha: Some("release-target-sha".to_string()),
                base_sha: "base-sha".to_string(),
                head_sha: "head-sha-old".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Open,
                conflict_summary: None,
                test_summary_json: None,
            })
            .await?;
        assert_eq!(
            "candidate-stale-other-target",
            stale_other_target.candidate_id
        );

        let current = store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-current".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "HEAD".to_string(),
                target_sha: Some("target-sha".to_string()),
                base_sha: "base-sha".to_string(),
                head_sha: "head-sha-new".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Open,
                conflict_summary: None,
                test_summary_json: None,
            })
            .await?;
        assert_eq!("candidate-current", current.candidate_id);

        assert_eq!(
            vec![current],
            store
                .list_merge_candidates(
                    "wt-a",
                    Some(crate::ManagedWorktreeMergeCandidateStatus::Open),
                    /*limit*/ 10,
                )
                .await?
        );
        let dismissed = store
            .list_merge_candidates(
                "wt-a",
                Some(crate::ManagedWorktreeMergeCandidateStatus::Dismissed),
                /*limit*/ 10,
            )
            .await?;
        let dismissed_ids = merge_candidate_ids(&dismissed);
        assert!(dismissed_ids.contains(&"candidate-stale"));
        assert!(dismissed_ids.contains(&"candidate-stale-other-target"));

        Ok(())
    }

    #[tokio::test]
    async fn dismiss_merge_candidate_does_not_overwrite_applied_candidate() -> anyhow::Result<()> {
        let runtime = test_runtime().await;
        let store = runtime.managed_worktrees();
        store
            .create_managed_worktree(create_params("wt-a", "/repo-a"))
            .await?;
        store
            .record_merge_candidate(ManagedWorktreeMergeCandidateRecordParams {
                candidate_id: Some("candidate-applied".to_string()),
                worktree_id: "wt-a".to_string(),
                target_ref: "HEAD".to_string(),
                target_sha: Some("target-sha".to_string()),
                base_sha: "base-sha".to_string(),
                head_sha: "head-sha".to_string(),
                status: crate::ManagedWorktreeMergeCandidateStatus::Open,
                conflict_summary: None,
                test_summary_json: None,
            })
            .await?;
        let applied = store
            .mark_merge_candidate_status(
                "candidate-applied",
                crate::ManagedWorktreeMergeCandidateStatus::Applied,
            )
            .await?
            .expect("candidate should be applied");

        assert_eq!(
            None,
            store.dismiss_merge_candidate("candidate-applied").await?
        );
        assert_eq!(
            applied,
            store
                .get_merge_candidate("candidate-applied")
                .await?
                .expect("candidate should remain")
        );

        Ok(())
    }

    fn merge_candidate_ids(candidates: &[crate::ManagedWorktreeMergeCandidate]) -> Vec<&str> {
        candidates
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect()
    }
}
