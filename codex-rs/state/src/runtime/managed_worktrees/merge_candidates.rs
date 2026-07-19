use super::*;

impl ManagedWorktreeStore {
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
