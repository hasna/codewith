mod event;
mod interaction;
mod run;
mod snapshot;
mod worktree;

pub use event::BACKGROUND_AGENT_EVENT_CURSOR_COMPACTED;
pub use event::BackgroundAgentEvent;
pub use event::BackgroundAgentEventRow;
pub use interaction::BackgroundAgentPendingInteraction;
pub use interaction::BackgroundAgentPendingInteractionCreateParams;
pub use interaction::BackgroundAgentPendingInteractionKind;
pub use interaction::BackgroundAgentPendingInteractionRow;
pub use interaction::BackgroundAgentPendingInteractionStatus;
pub use run::BackgroundAgentDesiredState;
pub use run::BackgroundAgentExecutionHandleParams;
pub use run::BackgroundAgentProcessHandleRecord;
pub use run::BackgroundAgentRetentionState;
pub use run::BackgroundAgentRun;
pub use run::BackgroundAgentRunCreateParams;
pub use run::BackgroundAgentRunRow;
pub use run::BackgroundAgentRunStatus;
pub use run::BackgroundAgentStatusEventForSupervisorParams;
pub use run::BackgroundAgentThreadBindingParams;
pub(crate) use run::encode_background_agent_opaque_identity;
pub use snapshot::BackgroundAgentExecutionSnapshot;
pub use snapshot::BackgroundAgentExecutionSnapshotParams;
pub use snapshot::BackgroundAgentExecutionSnapshotRow;
pub use snapshot::BackgroundAgentStatusSnapshot;
pub use snapshot::BackgroundAgentStatusSnapshotParams;
pub use snapshot::BackgroundAgentStatusSnapshotRow;
pub use worktree::BackgroundAgentWorkspaceCleanup;
pub use worktree::BackgroundAgentWorkspaceMode;
pub use worktree::BackgroundAgentWorktreeLease;
pub use worktree::BackgroundAgentWorktreeLeaseCreateParams;
pub use worktree::BackgroundAgentWorktreeLeaseRow;

use anyhow::Result;
use chrono::DateTime;
use chrono::Utc;

fn epoch_seconds_to_datetime(secs: i64) -> Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(secs, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid unix timestamp: {secs}"))
}
