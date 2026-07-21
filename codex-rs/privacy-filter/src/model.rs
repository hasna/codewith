//! Static metadata for the Hugging Face `openai/privacy-filter` model.
//!
//! Numbers here are approximate, conservative planning estimates used purely to
//! render readiness guidance. They are not downloaded from anywhere and must
//! never be treated as authoritative once the real install path (a follow-up
//! slice) can report exact on-disk sizes.

use serde::Deserialize;
use serde::Serialize;

const GIB: u64 = 1024 * 1024 * 1024;

/// Approximate, human-facing metadata about the Privacy Filter classifier.
///
/// This is the static, borrowed form used by the [`PRIVACY_FILTER_MODEL`]
/// constant. For a serializable/deserializable snapshot (e.g. inside a
/// [`crate::ReadinessReport`]) convert into [`ModelSummary`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PrivacyFilterModel {
    /// Hugging Face repository id.
    pub repo_id: &'static str,
    /// Friendly display name.
    pub display_name: &'static str,
    /// Human-readable parameter description (total / active).
    pub approx_parameters: &'static str,
    /// Model license identifier.
    pub license: &'static str,
    /// Maximum context window in tokens.
    pub context_window_tokens: u32,
    /// Approximate on-disk footprint once installed, in bytes. Conservative
    /// planning estimate only.
    pub approx_on_disk_bytes: u64,
    /// Minimum recommended total system RAM to run the classifier, in bytes.
    pub min_recommended_ram_bytes: u64,
}

/// Best-known metadata for `openai/privacy-filter`.
pub const PRIVACY_FILTER_MODEL: PrivacyFilterModel = PrivacyFilterModel {
    repo_id: "openai/privacy-filter",
    display_name: "OpenAI Privacy Filter",
    approx_parameters: "~1.5B total (~50M active per token)",
    license: "Apache-2.0",
    context_window_tokens: 128_000,
    // A ~1.5B parameter classifier in half precision lands around 3 GiB on
    // disk; keep a little headroom for tokenizer/config assets.
    approx_on_disk_bytes: 3 * GIB,
    // Leave room for weights plus runtime working set.
    min_recommended_ram_bytes: 4 * GIB,
};

/// Owned, (de)serializable snapshot of [`PrivacyFilterModel`].
///
/// Used inside readiness reports so they can round-trip through JSON without the
/// `'static` borrow of the constant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSummary {
    pub repo_id: String,
    pub display_name: String,
    pub approx_parameters: String,
    pub license: String,
    pub context_window_tokens: u32,
    pub approx_on_disk_bytes: u64,
    pub min_recommended_ram_bytes: u64,
}

impl From<PrivacyFilterModel> for ModelSummary {
    fn from(model: PrivacyFilterModel) -> Self {
        Self {
            repo_id: model.repo_id.to_string(),
            display_name: model.display_name.to_string(),
            approx_parameters: model.approx_parameters.to_string(),
            license: model.license.to_string(),
            context_window_tokens: model.context_window_tokens,
            approx_on_disk_bytes: model.approx_on_disk_bytes,
            min_recommended_ram_bytes: model.min_recommended_ram_bytes,
        }
    }
}
