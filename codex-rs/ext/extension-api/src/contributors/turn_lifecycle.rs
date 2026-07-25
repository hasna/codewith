use codex_protocol::config_types::CollaborationMode;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnAbortReason;
use std::path::Path;

use crate::ExtensionData;

/// Input supplied when the host starts a turn.
pub struct TurnStartInput<'a> {
    /// Stable host-owned turn identifier.
    pub turn_id: &'a str,
    /// Effective collaboration mode for this turn.
    pub collaboration_mode: &'a CollaborationMode,
    /// Total token usage snapshot captured when the turn started.
    pub token_usage_at_turn_start: &'a TokenUsage,
    /// Absolute working directory of this turn when it runs against exactly one
    /// local environment. `None` for remote or multi-environment turns, where
    /// the host filesystem does not hold the turn's working tree.
    pub local_cwd: Option<&'a Path>,
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

/// Input supplied when the host completes a turn.
pub struct TurnStopInput<'a> {
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

/// Input supplied when the host aborts a turn.
pub struct TurnAbortInput<'a> {
    /// Reason the host aborted the turn.
    pub reason: TurnAbortReason,
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

/// Input supplied when the host observes an error for a turn.
pub struct TurnErrorInput<'a> {
    /// Stable host-owned turn identifier.
    pub turn_id: &'a str,
    /// Error surfaced by the host for this turn.
    pub error: CodexErrorInfo,
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}
