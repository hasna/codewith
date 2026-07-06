//! Local-only readiness checks and static metadata for the optional
//! OpenAI Privacy Filter secret-redaction layer.
//!
//! This crate is deliberately narrow: it never performs any network I/O, never
//! downloads a model, and never touches secret material. It only inspects the
//! host machine (RAM, disk, CPU, best-effort accelerator support) so the TUI
//! can tell the user whether their machine could run the Privacy Filter
//! classifier *before* anything is installed.
//!
//! The actual on-device classifier install and inference runtime are tracked as
//! follow-up work and intentionally not wired here. Any inference-related state
//! therefore reports [`InstallStatus::NotInstalled`].

mod model;
mod readiness;

pub use model::ModelSummary;
pub use model::PRIVACY_FILTER_MODEL;
pub use model::PrivacyFilterModel;
pub use readiness::HardwareSupport;
pub use readiness::InstallStatus;
pub use readiness::ReadinessReport;
pub use readiness::collect_readiness;
