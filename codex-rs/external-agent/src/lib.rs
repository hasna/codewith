//! Shared contracts and harness helpers for external coding-agent runtimes.
//!
//! External agents are agent harnesses such as Cursor, Grok Build, or Claude
//! Code. They are not ordinary HTTP model providers: they may have their own
//! sessions, tools, prompts, permissions, artifacts, and process lifecycle.
//! This crate defines the narrow boundary Codewith uses to run those agents
//! while keeping transcript, approval, and audit ownership in Codewith.

mod acp;
mod claude;
mod claude_auth;
mod contract;
mod platform_sandbox;
mod runtimes;

pub use acp::*;
pub use claude::*;
pub use claude_auth::claude_provider_credential_read_roots;
pub use contract::*;
pub use platform_sandbox::*;
pub use runtimes::*;
