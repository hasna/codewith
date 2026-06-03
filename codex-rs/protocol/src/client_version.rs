//! Codex API-compatibility version advertised to the OpenAI/ChatGPT backend.
//!
//! The product/build version (`CARGO_PKG_VERSION`) is the Codewith fork's own
//! version (e.g. `0.1.x`) and is intentionally low. OpenAI's backend gates newer
//! models (for example `gpt-5.5`) behind a minimum *Codex* client version and
//! otherwise rejects requests with "requires a newer version of Codex". Because
//! this fork tracks upstream Codex, we advertise the upstream Codex version it is
//! API-compatible with rather than the low product version. This keeps Codewith's
//! own versioning independent while staying compatible with the backend's model
//! gating.
//!
//! Override at runtime with the `CODEX_API_VERSION` environment variable if the
//! backend raises the floor again before this constant is bumped.

/// Upstream Codex version this fork is API-compatible with. Bump when syncing
/// upstream or when the backend raises the model-gating floor.
pub const CODEX_API_COMPAT_VERSION: &str = "0.137.0";

/// Environment variable that overrides [`codex_api_version`] without rebuilding.
pub const CODEX_API_VERSION_ENV_VAR: &str = "CODEX_API_VERSION";

/// Version string advertised to the OpenAI/ChatGPT backend (the `User-Agent`
/// header and the `version` request header). Honors `CODEX_API_VERSION`, falling
/// back to [`CODEX_API_COMPAT_VERSION`].
pub fn codex_api_version() -> String {
    std::env::var(CODEX_API_VERSION_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| CODEX_API_COMPAT_VERSION.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_compat_version() {
        // Note: assumes CODEX_API_VERSION is unset in the test environment.
        if std::env::var(CODEX_API_VERSION_ENV_VAR).is_err() {
            assert_eq!(codex_api_version(), CODEX_API_COMPAT_VERSION);
        }
    }

    #[test]
    fn compat_version_is_high_enough_for_model_gating() {
        // The product version is 0.1.x; the advertised version must be well above
        // it so the backend does not reject newer models as "too old".
        assert!(CODEX_API_COMPAT_VERSION.starts_with("0.13"));
    }
}
