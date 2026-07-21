pub(crate) mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod model_presets;
pub mod test_support;

pub use codex_app_server_protocol::AuthMode;
pub use config::ModelsManagerConfig;

/// Load the bundled model catalog shipped with `codex-models-manager`.
pub fn bundled_models_response()
-> std::result::Result<codex_protocol::openai_models::ModelsResponse, serde_json::Error> {
    let mut response: codex_protocol::openai_models::ModelsResponse =
        serde_json::from_str(include_str!("../models.json"))?;
    model_info::ensure_required_local_models(&mut response.models);
    Ok(response)
}

/// Convert the advertised Codex API compatibility version to a whole version
/// string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    version_without_build_metadata(&codex_protocol::client_version::codex_api_version()).to_string()
}

fn version_without_build_metadata(version: &str) -> &str {
    version.split(['-', '+']).next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_version_to_whole_uses_codex_api_compat_version() {
        if std::env::var(codex_protocol::client_version::CODEX_API_VERSION_ENV_VAR).is_err() {
            assert_eq!(
                client_version_to_whole(),
                codex_protocol::client_version::CODEX_API_COMPAT_VERSION
            );
            assert_ne!(client_version_to_whole(), env!("CARGO_PKG_VERSION"));
        }
    }

    #[test]
    fn version_without_build_metadata_strips_prerelease_and_build_metadata() {
        assert_eq!(version_without_build_metadata("0.144.4-alpha.9"), "0.144.4");
        assert_eq!(version_without_build_metadata("0.144.4+build.1"), "0.144.4");
        assert_eq!(
            version_without_build_metadata("0.144.4-alpha.9+build.1"),
            "0.144.4"
        );
    }
}
