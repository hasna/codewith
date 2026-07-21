//! Cross-provider integration coverage for the built-in model providers.
//!
//! These tests exercise the provider stack through its public surface only and
//! tie together the three crates that own provider behavior:
//!
//!   * `codex-model-provider-info` — provider registration and request shaping
//!     (`to_api_provider`: base URL, retry policy, static headers, wire API).
//!   * `codex-model-provider` — runtime auth resolution and error mapping
//!     (`ModelProvider::api_auth`).
//!   * `codex-known-provider-models` + `codex-models-manager` — model metadata
//!     and context-window resolution.
//!
//! They are deliberately hermetic: no network calls, no reliance on ambient
//! provider credentials, and no dependency on the external `secrets` CLI. Tests
//! that need a "missing" credential use a synthetic env key whose suffix is not
//! a recognized credential suffix, so the secret backend is never consulted and
//! the result is deterministic regardless of the host environment.

use std::collections::BTreeSet;

use codex_known_provider_models::fallback_models_for_provider;
use codex_known_provider_models::metadata_for_local_fallback;
use codex_model_provider::create_model_provider_with_id;
use codex_model_provider_info::LMSTUDIO_OSS_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID;
use codex_model_provider_info::OPENAI_API_BASE_URL;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::built_in_model_provider_ids;
use codex_model_provider_info::built_in_model_providers;
use codex_model_provider_info::default_wire_api_for_builtin_provider_override;
use codex_models_manager::model_info::model_info_from_slug;
use codex_protocol::error::CodexErr;

/// Built-in providers that do not resolve model metadata from the
/// `known-provider-models` fallback catalog: OpenAI serves a hosted bundled
/// catalog, and the two local OSS providers discover models at runtime.
const NON_FALLBACK_CATALOG_PROVIDERS: &[&str] = &[
    OPENAI_PROVIDER_ID,
    OLLAMA_OSS_PROVIDER_ID,
    LMSTUDIO_OSS_PROVIDER_ID,
];

/// Local OSS providers that must resolve to unauthenticated requests.
const LOCAL_OSS_PROVIDERS: &[&str] = &[OLLAMA_OSS_PROVIDER_ID, LMSTUDIO_OSS_PROVIDER_ID];

/// Returns a guaranteed-unset environment key for `provider_id`.
///
/// The `_SECRET` suffix is intentionally not one of the recognized credential
/// suffixes (`_API_KEY`, `_ACCESS_TOKEN`, `_AUTH_TOKEN`, `_BEARER_TOKEN`,
/// `_TOKEN`), so credential resolution never derives a secret name and never
/// spawns the `secrets` CLI. That keeps these tests hermetic and fast.
fn missing_env_key(provider_id: &str) -> String {
    let normalized = provider_id
        .to_ascii_uppercase()
        .replace(['-', '.', '/'], "_");
    format!("CODEWITH_TEST_PROVIDER_INTEGRATION_{normalized}_MISSING_SECRET")
}

/// The built-in external providers that authenticate with a provider-owned
/// environment key (everything except OpenAI, which uses ambient Codewith auth,
/// and the local OSS providers, which are unauthenticated).
fn built_in_env_key_providers() -> Vec<(String, ModelProviderInfo)> {
    built_in_model_providers(/*openai_base_url*/ None)
        .into_iter()
        .filter(|(_, info)| info.env_key.is_some() && !info.requires_openai_auth)
        .collect()
}

// -------------------------------------------------------------------------
// Request shaping
// -------------------------------------------------------------------------

#[test]
fn built_in_providers_shape_api_requests_consistently() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    // The registry surface must be exactly the advertised built-in ids.
    let expected_ids: BTreeSet<String> =
        built_in_model_provider_ids().map(str::to_string).collect();
    let actual_ids: BTreeSet<String> = providers.keys().cloned().collect();
    assert_eq!(actual_ids, expected_ids);

    for (id, info) in &providers {
        let api = info
            .to_api_provider(/*auth_mode*/ None)
            .unwrap_or_else(|err| panic!("{id} should build an API provider: {err:?}"));

        // Every provider resolves a concrete base URL.
        assert!(!api.base_url.is_empty(), "{id} must expose a base URL");
        if id == OPENAI_PROVIDER_ID {
            // OpenAI has no configured base URL and falls back to the default.
            assert_eq!(api.base_url, OPENAI_API_BASE_URL, "{id} base URL");
        } else if let Some(base_url) = info.base_url.as_deref() {
            assert_eq!(api.base_url, base_url, "{id} base URL should round-trip");
        }

        // Retry policy mirrors the provider's effective config and stays capped.
        assert_eq!(
            api.retry.max_attempts,
            info.request_max_retries(),
            "{id} retry attempts should match the effective config"
        );
        assert!(
            api.retry.max_attempts <= 100,
            "{id} retry attempts must be capped"
        );
        assert!(api.retry.retry_5xx, "{id} should retry 5xx responses");
        assert!(
            api.retry.retry_transport,
            "{id} should retry transport errors"
        );
        assert!(!api.retry.retry_429, "{id} should not retry 429 by default");
        assert_eq!(
            api.stream_idle_timeout,
            info.stream_idle_timeout(),
            "{id} stream idle timeout should match the effective config"
        );

        // Static headers configured on the provider survive into the request.
        if let Some(static_headers) = info.http_headers.as_ref() {
            for (name, value) in static_headers {
                assert_eq!(
                    api.headers.get(name).and_then(|value| value.to_str().ok()),
                    Some(value.as_str()),
                    "{id} should forward static header {name}"
                );
            }
        }
    }
}

#[test]
fn built_in_provider_defaults_match_override_wire_api() {
    // The default provider definition and the wire API used when a user override
    // omits `wire_api` must not drift apart.
    for (id, info) in built_in_model_providers(/*openai_base_url*/ None) {
        assert_eq!(
            info.wire_api,
            default_wire_api_for_builtin_provider_override(&id),
            "{id} default wire API should match its override default"
        );
    }
}

// -------------------------------------------------------------------------
// Auth resolution + error mapping
// -------------------------------------------------------------------------

#[tokio::test]
async fn env_key_providers_map_missing_credentials_to_env_var_error() {
    for (id, info) in built_in_env_key_providers() {
        let env_key = missing_env_key(&id);
        let instructions = format!("Set {env_key}.");
        let provider_info = ModelProviderInfo {
            env_key: Some(env_key.clone()),
            env_key_instructions: Some(instructions.clone()),
            experimental_bearer_token: None,
            ..info.clone()
        };
        let provider =
            create_model_provider_with_id(id.clone(), provider_info, /*auth_manager*/ None);

        match provider.api_auth().await {
            Err(CodexErr::EnvVar(err)) => {
                assert_eq!(err.var, env_key, "{id} should report the missing env key");
                assert_eq!(
                    err.instructions,
                    Some(instructions),
                    "{id} should surface the env key instructions"
                );
            }
            Ok(_) => panic!("{id} must not resolve auth without a provider credential"),
            Err(other) => {
                panic!("{id} should map a missing credential to EnvVar, got {other:?}")
            }
        }
    }
}

#[tokio::test]
async fn env_key_providers_attach_bearer_from_experimental_token() {
    for (id, info) in built_in_env_key_providers() {
        let token = format!("integration-token-{id}");
        let provider_info = ModelProviderInfo {
            // Force the resolved provider key to be absent so the deterministic
            // experimental token is the credential under test.
            env_key: Some(missing_env_key(&id)),
            experimental_bearer_token: Some(token.clone()),
            ..info.clone()
        };
        let provider =
            create_model_provider_with_id(id.clone(), provider_info, /*auth_manager*/ None);

        let auth = provider
            .api_auth()
            .await
            .unwrap_or_else(|err| panic!("{id} should resolve auth from the token: {err:?}"));
        let headers = auth.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some(format!("Bearer {token}").as_str()),
            "{id} should attach the experimental bearer token"
        );
    }
}

#[tokio::test]
async fn local_oss_providers_resolve_unauthenticated() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);
    for id in LOCAL_OSS_PROVIDERS {
        let info = providers
            .get(*id)
            .unwrap_or_else(|| panic!("{id} should be a built-in provider"))
            .clone();
        assert!(
            info.env_key.is_none(),
            "{id} should not use a provider env key"
        );

        let provider = create_model_provider_with_id(*id, info, /*auth_manager*/ None);
        let auth = provider
            .api_auth()
            .await
            .unwrap_or_else(|err| panic!("{id} auth should resolve: {err:?}"));
        assert!(
            auth.to_auth_headers().is_empty(),
            "{id} should issue unauthenticated requests"
        );
    }
}

// -------------------------------------------------------------------------
// Model metadata + context windows
// -------------------------------------------------------------------------

#[test]
fn every_registered_external_provider_has_fallback_catalog_metadata() {
    for id in built_in_model_provider_ids() {
        if NON_FALLBACK_CATALOG_PROVIDERS.contains(&id) {
            continue;
        }

        let models = fallback_models_for_provider(id);
        assert!(!models.is_empty(), "{id} should expose a fallback catalog");

        let default_model = models
            .iter()
            .find(|model| model.is_default)
            .unwrap_or_else(|| panic!("{id} should mark a default fallback model"));
        assert_eq!(
            models[0].id, default_model.id,
            "{id} should list its default fallback model first"
        );

        // Every advertised fallback model must resolve coherent metadata with a
        // positive context window. This guards against orphaned catalog entries
        // and the class of context-window drift corrected in PR #281.
        for model in models {
            let metadata = metadata_for_local_fallback(Some(id), model.id).unwrap_or_else(|| {
                panic!("{id} fallback model {} should resolve metadata", model.id)
            });
            assert!(
                metadata.context_window > 0,
                "{id} fallback model {} should have a positive context window",
                model.id
            );
            assert!(
                !metadata.display_name.is_empty(),
                "{id} fallback model {} should have a display name",
                model.id
            );
            assert!(
                !metadata.input_modalities.is_empty(),
                "{id} fallback model {} should advertise input modalities",
                model.id
            );
        }
    }
}

#[test]
fn models_manager_applies_known_provider_context_window_for_known_slug() {
    // `gpt-oss-120b` resolves through the unqualified (Cerebras) metadata path,
    // so the models-manager view must match the known-provider-models source.
    let metadata = metadata_for_local_fallback(/*provider_id*/ None, "gpt-oss-120b")
        .expect("gpt-oss-120b should resolve known metadata");

    let info = model_info_from_slug("gpt-oss-120b");

    assert!(
        !info.used_fallback_model_metadata,
        "a known slug should not fall back to generic metadata"
    );
    assert_eq!(info.display_name.as_str(), metadata.display_name);
    assert_eq!(info.context_window, Some(metadata.context_window));
    assert_eq!(info.max_context_window, Some(metadata.context_window));
}

#[test]
fn models_manager_uses_generic_fallback_for_unknown_slug() {
    let info = model_info_from_slug("codewith-provider-integration-unknown-model");

    assert!(
        info.used_fallback_model_metadata,
        "an unknown slug should fall back to generic metadata"
    );
    assert!(
        info.supported_in_api,
        "the generic fallback stays API-usable"
    );
    assert_eq!(
        info.context_window,
        Some(272_000),
        "the generic fallback context window should be stable"
    );
    assert_eq!(info.max_context_window, Some(272_000));
}
