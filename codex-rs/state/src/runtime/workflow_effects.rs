use super::*;
use crate::runtime::workflow_orchestrator::validate_owner_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRunVerifierEffectPublishParams {
    pub run_id: String,
    pub owner_id: String,
    pub generation: i64,
    pub verifier_run_id: String,
    pub effect_key: String,
}

impl StateRuntime {
    /// Publishes one logical verifier effect while its workflow generation is fenced.
    ///
    /// The caller must reuse `effect_key` for the same logical verifier effect across
    /// retries and ownership takeovers. The closure must perform a bounded effect
    /// directly and must not call back into this [`StateRuntime`]. The state writer
    /// transaction prevents a takeover from committing between the generation check and
    /// the effect, while the durable key prevents another generation from publishing the
    /// same logical effect.
    pub async fn publish_workflow_run_verifier_effect<T, F>(
        &self,
        params: WorkflowRunVerifierEffectPublishParams,
        publish: F,
    ) -> anyhow::Result<Option<T>>
    where
        F: FnOnce() -> anyhow::Result<T>,
    {
        validate_owner_id(&params.owner_id)?;
        if params.effect_key.trim().is_empty() {
            anyhow::bail!("workflow run effect_key must not be empty");
        }
        if params.verifier_run_id.trim().is_empty() {
            anyhow::bail!("workflow run verifier_run_id must not be empty");
        }

        let now_ms = datetime_to_epoch_millis(Utc::now());
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let claimed_generation: Option<i64> = sqlx::query_scalar(
            r#"
INSERT INTO workflow_run_verifier_effect_publications (
    run_id,
    verifier_run_id,
    effect_key,
    generation,
    owner_id,
    owner_instance_id,
    published_at_ms
)
SELECT
    run.run_id,
    verifier.verifier_run_id,
    ?,
    run.generation,
    run.owner_id,
    run.owner_instance_id,
    ?
FROM workflow_runs run
JOIN workflow_run_step_verifiers verifier
  ON verifier.run_id = run.run_id
WHERE run.run_id = ?
  AND run.owner_id = ?
  AND run.owner_instance_id = ?
  AND run.generation = ?
  AND run.lease_expires_at_ms > ?
  AND run.status NOT IN ('completed', 'failed', 'cancelled', 'cancel_requested', 'paused')
  AND verifier.verifier_run_id = ?
  AND verifier.status = 'running'
ON CONFLICT(verifier_run_id, effect_key) DO NOTHING
RETURNING generation
            "#,
        )
        .bind(params.effect_key.as_str())
        .bind(now_ms)
        .bind(params.run_id.as_str())
        .bind(params.owner_id.as_str())
        .bind(self.workflow_owner_instance_id.as_str())
        .bind(params.generation)
        .bind(now_ms)
        .bind(params.verifier_run_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if claimed_generation.is_none() {
            tx.commit().await?;
            return Ok(None);
        }

        let published = match publish() {
            Ok(published) => published,
            Err(err) => {
                tx.rollback().await?;
                return Err(err);
            }
        };
        tx.commit().await?;
        Ok(Some(published))
    }
}
