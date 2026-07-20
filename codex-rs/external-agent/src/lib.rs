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
mod cursor;
mod cursor_cloud;
mod cursor_models;
pub(crate) mod cursor_sdk;
mod platform_sandbox;
mod runtimes;
mod safety;
#[cfg(windows)]
mod windows_cmd_shim;

pub use acp::*;
pub use acp_adapter::*;
pub use claude::*;
pub use contract::*;
pub use cursor::*;
pub use cursor_cloud::*;
pub use cursor_models::*;
pub use cursor_sdk::*;
pub use platform_sandbox::*;
pub use runtimes::*;
pub use safety::*;
#[cfg(windows)]
pub use windows_cmd_shim::WindowsBatchLaunchError;
#[cfg(windows)]
pub use windows_cmd_shim::WindowsNativeLaunch;
#[cfg(windows)]
pub use windows_cmd_shim::prepare_windows_batch_launch_from_source_env;
