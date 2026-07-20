//! Shared contracts and harness helpers for external coding-agent runtimes.
//!
//! External agents are agent harnesses such as Cursor, Grok Build, or Claude
//! Code. They are not ordinary HTTP model providers: they may have their own
//! sessions, tools, prompts, permissions, artifacts, and process lifecycle.
//! This crate defines the narrow boundary Codewith uses to run those agents
//! while keeping transcript, approval, and audit ownership in Codewith.

mod acp;
mod acp_adapter;
mod claude;
mod contract;
mod platform_sandbox;
mod runtimes;
#[cfg(windows)]
mod windows_cmd_shim;

pub use acp::*;
pub use acp_adapter::*;
pub use claude::*;
pub use contract::*;
pub use platform_sandbox::*;
pub use runtimes::*;
#[cfg(windows)]
pub use windows_cmd_shim::WindowsBatchLaunchError;
#[cfg(windows)]
pub use windows_cmd_shim::WindowsNativeLaunch;
#[cfg(windows)]
pub use windows_cmd_shim::prepare_windows_batch_launch_from_source_env;
