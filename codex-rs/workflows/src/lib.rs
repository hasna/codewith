//! Non-executing workflow specification parsing and validation.
//!
//! This crate owns the declarative workflow boundary. It validates YAML into a
//! typed spec, but does not grant permissions, spawn agents, run commands, or
//! persist runtime state.

mod ancient_names;
mod branch_prompt;
mod error;
mod parse;
mod spec;
mod validation;

pub use branch_prompt::WorkflowBranchPrompt;
pub use branch_prompt::render_workflow_branch_prompt;
pub use error::WorkflowSpecError;
pub use error::WorkflowSpecResult;
pub use parse::parse_workflow_yaml;
pub use spec::WorkflowAgent;
pub use spec::WorkflowApprovals;
pub use spec::WorkflowArtifacts;
pub use spec::WorkflowCleanup;
pub use spec::WorkflowCompletion;
pub use spec::WorkflowFixture;
pub use spec::WorkflowLimits;
pub use spec::WorkflowLoop;
pub use spec::WorkflowLoopIntervalUnit;
pub use spec::WorkflowLoopSchedule;
pub use spec::WorkflowModelRoute;
pub use spec::WorkflowModelRouter;
pub use spec::WorkflowModelRoutingCapability;
pub use spec::WorkflowModelRoutingConstraints;
pub use spec::WorkflowModelRoutingContext;
pub use spec::WorkflowModelRoutingContract;
pub use spec::WorkflowModelRoutingDecision;
pub use spec::WorkflowModelRoutingDecisionStatus;
pub use spec::WorkflowModelRoutingError;
pub use spec::WorkflowModelRoutingFallback;
pub use spec::WorkflowModelRoutingRequest;
pub use spec::WorkflowMonitorLink;
pub use spec::WorkflowSpec;
pub use spec::WorkflowStatus;
pub use spec::WorkflowStep;
pub use spec::WorkflowStopCondition;
pub use spec::WorkflowVerifier;
pub use spec::WorkflowVerifierRetryPolicy;
pub use spec::WorkflowWorkspace;

pub const MAX_WORKFLOW_PROMPT_FIELD_CHARS: usize = 240;
pub const MAX_WORKFLOW_YAML_BYTES: usize = 256 * 1024;
pub const SUPPORTED_SCHEMA_VERSION: &str = "workflow.codex.codewith/v0";

#[cfg(test)]
mod tests;
