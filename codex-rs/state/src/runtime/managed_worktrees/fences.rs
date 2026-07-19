use super::*;

/// Rejects lifecycle changes while an active background-agent run owns the
/// worktree through a managed-worktree assignment.
pub(super) async fn ensure_no_active_background_agent_run_assignment(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    worktree_id: &str,
) -> anyhow::Result<()> {
    let active_run: Option<(String, String)> = sqlx::query_as(
        r#"
SELECT run.id, run.status
FROM managed_worktree_assignments AS assignment
JOIN background_agent_runs AS run ON run.id = assignment.agent_run_id
WHERE assignment.worktree_id = ?
  AND assignment.detached_at_ms IS NULL
  AND run.status NOT IN ('completed', 'failed', 'cancelled')
LIMIT 1
        "#,
    )
    .bind(worktree_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((run_id, status)) = active_run {
        anyhow::bail!(
            "managed worktree {worktree_id} is assigned to active background agent run {run_id} ({status}); stop or wait for the run to finish before changing the worktree lifecycle"
        );
    }
    Ok(())
}
