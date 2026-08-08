//! App-server execution and terminal replay for one scheduled occurrence.

use super::*;

mod execution;
mod terminal;

pub(super) use terminal::PersistedScheduledTurnTerminal;
#[cfg(test)]
pub(super) use terminal::ScheduledTurnFinish;
pub(in super::super) use terminal::default_thread_schedule_expires_at;
pub(in super::super) use terminal::finish_scheduled_run_after_turn;
pub(super) use terminal::next_thread_schedule_run_after_completion;
pub(in super::super) use terminal::next_thread_schedule_run_at;
pub(in super::super) use terminal::normalize_schedule_timezone;
pub(super) use terminal::persisted_scheduled_turn_terminal;
pub(in super::super) use terminal::recover_scheduled_run_for_terminal_turn;
#[cfg(test)]
pub(super) use terminal::scheduled_turn_finish;

impl ThreadScheduleRuntime {
    pub(super) async fn execute_claim(
        &self,
        state_db: StateDbHandle,
        claim: codex_state::ThreadScheduleClaim,
    ) {
        self.execute_occurrence_claim(state_db, claim).await;
    }
}

struct ScheduleSubmitError {
    error: anyhow::Error,
    goal_id: Option<String>,
}
