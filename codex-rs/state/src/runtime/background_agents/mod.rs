mod admission;
mod events;
mod interactions;
mod runs;
mod snapshots;
mod worktrees;

#[cfg(test)]
mod admission_tests;
#[cfg(test)]
mod tests;

pub use admission::BackgroundAgentAdmissionError;
pub use admission::BackgroundAgentRunAdmission;
pub use admission::BackgroundAgentRunAdmissionParams;
pub(in crate::runtime) use events::append_background_agent_event_in_tx;

use super::*;
