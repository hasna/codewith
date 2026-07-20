//! Live discovery of Cursor Composer models.
//!
//! Codewith advertises which Composer models a Cursor run may target. Historic
//! builds hard-coded that list through a static `cursor_composer_seed_models()`
//! seed; this module replaces the seed with live discovery while keeping the
//! same safe default.
//!
//! [`discover_cursor_composer_models`] queries the Cursor CLI/SDK (the
//! equivalent of `Cursor.models.list()`) for the Composer models available to
//! the current login — the default model plus any alternates — parses the
//! result, caches it for the lifetime of the process, and gracefully falls back
//! to [`CURSOR_DEFAULT_COMPOSER_MODEL_ID`] (`composer-2.5`) whenever discovery
//! is impossible (the CLI is missing, offline, unauthenticated, times out, or
//! returns output Codewith cannot parse).
//!
//! [`CursorComposerModel`] is the model type shared with the rest of the
//! external-agent crate (Comp1's `cursor.rs` imports it from here) and with the
//! app-server RPC that surfaces the discovered list to clients.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use tokio::process::Command;
use tokio::time::timeout;

/// Canonical identifier of the default Cursor Composer model.
///
/// This is both the static fallback returned when live discovery is
/// unavailable and the id [`CursorAcpAdapter`](crate::CursorAcpAdapter)
/// advertises through its `_meta` model, so the advertised default and the
/// discovery fallback can never drift apart.
pub const CURSOR_DEFAULT_COMPOSER_MODEL_ID: &str = "composer-2.5";

/// Human-facing label paired with [`CURSOR_DEFAULT_COMPOSER_MODEL_ID`].
pub const CURSOR_DEFAULT_COMPOSER_MODEL_DISPLAY_NAME: &str = "Composer 2.5";

/// Program names to resolve on `PATH` when querying Cursor for its model list,
/// most-canonical first. Mirrors the launch candidates the Cursor ACP adapter
/// uses so discovery and launch resolve the same binary family.
const CURSOR_MODEL_PROGRAM_CANDIDATES: &[&str] = &["cursor-agent", "agent"];

/// Arguments passed to the resolved Cursor program to list models as JSON.
const CURSOR_MODELS_LIST_ARGS: &[&str] = &["models", "list", "--output-format", "json"];

/// How long discovery waits for the Cursor CLI before falling back. A slow CLI
/// must never block a model picker, so a timeout is treated as "unavailable".
const CURSOR_MODEL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-process cache of a successful discovery result. Populated only by a
/// genuine, non-empty discovery; a fallback is never cached, so a process that
/// starts offline can still pick up real models once the CLI becomes reachable.
static CURSOR_MODEL_CACHE: OnceLock<Vec<CursorComposerModel>> = OnceLock::new();

/// A single Composer model Codewith may target for a Cursor run.
///
/// This is the shared model type for the external-agent crate: live discovery,
/// the static fallback, and the app-server RPC all speak in terms of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorComposerModel {
    /// Stable model identifier, for example `composer-2.5`.
    pub id: String,
    /// Human-facing label, for example `Composer 2.5`.
    pub display_name: String,
    /// Whether Cursor reports this as the default Composer model. Exactly one
    /// model in a normalized list is the default.
    pub is_default: bool,
}

impl CursorComposerModel {
    /// Build a model from its parts.
    pub fn new(id: impl Into<String>, display_name: impl Into<String>, is_default: bool) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            is_default,
        }
    }
}

/// The default Composer model Codewith advertises when nothing else is known.
pub fn cursor_default_composer_model() -> CursorComposerModel {
    CursorComposerModel::new(
        CURSOR_DEFAULT_COMPOSER_MODEL_ID,
        CURSOR_DEFAULT_COMPOSER_MODEL_DISPLAY_NAME,
        true,
    )
}

/// The Composer model list Codewith advertises when live discovery is
/// unavailable: just the single default model.
pub fn cursor_composer_fallback_models() -> Vec<CursorComposerModel> {
    vec![cursor_default_composer_model()]
}

/// Reason a live Cursor model discovery attempt did not yield a usable list.
///
/// Every variant is non-fatal: callers translate any of these into the
/// [`cursor_composer_fallback_models`] list rather than surfacing an error.
#[derive(Debug, thiserror::Error)]
pub enum CursorModelDiscoveryError {
    /// No Cursor program resolved on `PATH`.
    #[error("cursor CLI not found on PATH")]
    CliNotFound,
    /// The Cursor CLI did not respond within [`CURSOR_MODEL_DISCOVERY_TIMEOUT`].
    #[error("cursor model discovery timed out")]
    Timeout,
    /// The Cursor CLI could not be spawned or exited unsuccessfully.
    #[error("cursor model discovery process failed: {0}")]
    Process(String),
    /// The Cursor CLI produced output Codewith could not parse.
    #[error("failed to parse cursor model list: {0}")]
    Parse(String),
    /// Discovery ran but yielded no usable models.
    #[error("cursor model discovery returned no models")]
    Empty,
}

/// Discover the Composer models available to the current Cursor login.
///
/// Returns a normalized list (deduplicated, exactly one default, default first)
/// on success and caches it for the lifetime of the process. On any failure it
/// returns [`cursor_composer_fallback_models`] without caching, so a later call
/// can still succeed once the CLI becomes reachable.
pub async fn discover_cursor_composer_models() -> Vec<CursorComposerModel> {
    if let Some(cached) = CURSOR_MODEL_CACHE.get() {
        return cached.clone();
    }
    match discover_cursor_composer_models_uncached().await {
        Ok(models) => {
            // First writer wins; a racing writer's identical list is discarded.
            let _ = CURSOR_MODEL_CACHE.set(models.clone());
            models
        }
        Err(_) => cursor_composer_fallback_models(),
    }
}

/// Run a single live discovery attempt without consulting or updating the cache.
pub async fn discover_cursor_composer_models_uncached()
-> Result<Vec<CursorComposerModel>, CursorModelDiscoveryError> {
    let raw = run_cursor_models_command().await?;
    parse_cursor_models_list(&raw)
}

/// Discover Composer models from a caller-supplied raw-output producer.
///
/// This is the transport-agnostic core of discovery: it applies the same
/// parsing, normalization, and fallback rules as
/// [`discover_cursor_composer_models`] but lets a caller (or a test) provide the
/// raw `Cursor.models.list()` payload instead of shelling out to the CLI. Any
/// producer error or unparseable payload yields the fallback list.
pub async fn discover_cursor_composer_models_from<F, Fut>(fetch: F) -> Vec<CursorComposerModel>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, CursorModelDiscoveryError>>,
{
    match fetch().await {
        Ok(raw) => parse_cursor_models_list(&raw).unwrap_or_else(|_| cursor_composer_fallback_models()),
        Err(_) => cursor_composer_fallback_models(),
    }
}

/// Parse a `Cursor.models.list()` payload into a normalized Composer model list.
///
/// Accepts a bare JSON array or an object wrapping the array under a `models`,
/// `data`, `items`, or `composerModels` key. Each entry may be a bare id string
/// or an object with an id (`id`/`model`/`slug`/`name`), an optional display
/// name (`displayName`/`display_name`/`label`/`title`/`name`), and an optional
/// default flag (`default`/`isDefault`/`is_default`/`recommended`). Entries
/// whose id mentions "composer" are kept; if none do, all entries are kept so a
/// Cursor rename cannot silently drop the whole list.
pub fn parse_cursor_models_list(
    raw: &str,
) -> Result<Vec<CursorComposerModel>, CursorModelDiscoveryError> {
    let value: JsonValue = serde_json::from_str(raw.trim())
        .map_err(|err| CursorModelDiscoveryError::Parse(err.to_string()))?;
    let items = extract_model_array(&value).ok_or_else(|| {
        CursorModelDiscoveryError::Parse("no model array found in cursor output".to_string())
    })?;

    let mut models = Vec::new();
    for item in items {
        if let Some(model) = model_from_value(item) {
            models.push(model);
        }
    }

    let models = normalize_composer_models(retain_composer_models(models));
    if models.is_empty() {
        return Err(CursorModelDiscoveryError::Empty);
    }
    Ok(models)
}

fn extract_model_array(value: &JsonValue) -> Option<&Vec<JsonValue>> {
    if let Some(array) = value.as_array() {
        return Some(array);
    }
    ["models", "data", "items", "composerModels"]
        .iter()
        .find_map(|key| value.get(*key).and_then(JsonValue::as_array))
}

fn model_from_value(item: &JsonValue) -> Option<CursorComposerModel> {
    if let Some(raw_id) = item.as_str() {
        let id = raw_id.trim();
        if id.is_empty() {
            return None;
        }
        return Some(CursorComposerModel::new(id, default_display_name(id), false));
    }

    let object = item.as_object()?;
    let id = ["id", "model", "slug", "name"]
        .iter()
        .find_map(|key| object.get(*key).and_then(JsonValue::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let display_name = ["displayName", "display_name", "label", "title", "name"]
        .iter()
        .find_map(|key| object.get(*key).and_then(JsonValue::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_display_name(id));
    let is_default = ["default", "isDefault", "is_default", "recommended"]
        .iter()
        .find_map(|key| object.get(*key).and_then(JsonValue::as_bool))
        .unwrap_or(false);
    Some(CursorComposerModel::new(id, display_name, is_default))
}

fn is_composer_model(model: &CursorComposerModel) -> bool {
    model.id.to_ascii_lowercase().contains("composer")
}

fn retain_composer_models(models: Vec<CursorComposerModel>) -> Vec<CursorComposerModel> {
    let composer: Vec<CursorComposerModel> =
        models.iter().filter(|model| is_composer_model(model)).cloned().collect();
    if composer.is_empty() { models } else { composer }
}

fn normalize_composer_models(models: Vec<CursorComposerModel>) -> Vec<CursorComposerModel> {
    let mut seen = BTreeSet::new();
    let mut deduped: Vec<CursorComposerModel> = Vec::new();
    for model in models {
        if seen.insert(model.id.clone()) {
            deduped.push(model);
        }
    }
    if deduped.is_empty() {
        return deduped;
    }

    // Prefer an explicitly-flagged default, then the canonical default id, then
    // the first model. `unwrap_or(0)` keeps this total without a panic path.
    let default_index = deduped
        .iter()
        .position(|model| model.is_default)
        .or_else(|| {
            deduped
                .iter()
                .position(|model| model.id == CURSOR_DEFAULT_COMPOSER_MODEL_ID)
        })
        .unwrap_or(0);

    for (index, model) in deduped.iter_mut().enumerate() {
        model.is_default = index == default_index;
    }

    let default_model = deduped.remove(default_index);
    let mut ordered = Vec::with_capacity(deduped.len() + 1);
    ordered.push(default_model);
    ordered.extend(deduped);
    ordered
}

fn default_display_name(id: &str) -> String {
    let name = id
        .split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(capitalize_word)
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() { id.to_string() } else { name }
}

fn capitalize_word(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {
            let mut out = String::with_capacity(part.len());
            out.push(first.to_ascii_uppercase());
            out.push_str(chars.as_str());
            out
        }
        _ => part.to_string(),
    }
}

fn resolve_cursor_program() -> Option<PathBuf> {
    CURSOR_MODEL_PROGRAM_CANDIDATES
        .iter()
        .find_map(|candidate| which::which(candidate).ok())
}

async fn run_cursor_models_command() -> Result<String, CursorModelDiscoveryError> {
    let program = resolve_cursor_program().ok_or(CursorModelDiscoveryError::CliNotFound)?;
    let mut command = Command::new(program);
    command
        .args(CURSOR_MODELS_LIST_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match timeout(CURSOR_MODEL_DISCOVERY_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(CursorModelDiscoveryError::Process(err.to_string())),
        Err(_) => return Err(CursorModelDiscoveryError::Timeout),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let detail = if detail.is_empty() {
            format!("cursor models list exited with {}", output.status)
        } else {
            detail.to_string()
        };
        return Err(CursorModelDiscoveryError::Process(detail));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn ids(models: &[CursorComposerModel]) -> Vec<&str> {
        models.iter().map(|model| model.id.as_str()).collect()
    }

    fn default_count(models: &[CursorComposerModel]) -> usize {
        models.iter().filter(|model| model.is_default).count()
    }

    #[test]
    fn fallback_is_a_single_default_composer_2_5() {
        let fallback = cursor_composer_fallback_models();
        assert_eq!(
            fallback,
            vec![CursorComposerModel::new("composer-2.5", "Composer 2.5", true)]
        );
        assert_eq!(fallback[0].id, CURSOR_DEFAULT_COMPOSER_MODEL_ID);
    }

    #[test]
    fn parses_object_with_models_array_and_default_flag() {
        let raw = r#"{
            "models": [
                {"id": "composer-1", "displayName": "Composer 1"},
                {"id": "composer-2.5", "displayName": "Composer 2.5", "default": true}
            ]
        }"#;

        let models =
            parse_cursor_models_list(raw).unwrap_or_else(|err| panic!("parse models: {err}"));

        // The default is normalized to the front of the list.
        assert_eq!(ids(&models), vec!["composer-2.5", "composer-1"]);
        assert_eq!(default_count(&models), 1);
        assert!(models[0].is_default);
        assert!(!models[1].is_default);
    }

    #[test]
    fn parses_bare_array_of_ids_and_assigns_canonical_default() {
        let raw = r#"["composer-1", "composer-2.5"]"#;

        let models =
            parse_cursor_models_list(raw).unwrap_or_else(|err| panic!("parse models: {err}"));

        assert_eq!(ids(&models), vec!["composer-2.5", "composer-1"]);
        assert_eq!(default_count(&models), 1);
        assert!(models[0].is_default);
        // Display names are derived from the id when none is supplied.
        assert_eq!(models[0].display_name, "Composer 2.5");
        assert_eq!(models[1].display_name, "Composer 1");
    }

    #[test]
    fn parses_data_key_and_falls_back_to_first_when_no_default() {
        let raw = r#"{"data": [{"model": "composer-9"}, {"model": "composer-4"}]}"#;

        let models =
            parse_cursor_models_list(raw).unwrap_or_else(|err| panic!("parse models: {err}"));

        // No explicit default and no canonical id present, so the first wins.
        assert_eq!(ids(&models), vec!["composer-9", "composer-4"]);
        assert_eq!(default_count(&models), 1);
        assert!(models[0].is_default);
    }

    #[test]
    fn keeps_only_composer_models_when_mixed() {
        let raw = r#"{"models": [
            {"id": "gpt-5", "displayName": "GPT-5"},
            {"id": "composer-2.5", "displayName": "Composer 2.5", "isDefault": true}
        ]}"#;

        let models =
            parse_cursor_models_list(raw).unwrap_or_else(|err| panic!("parse models: {err}"));

        assert_eq!(ids(&models), vec!["composer-2.5"]);
    }

    #[test]
    fn keeps_all_models_when_none_look_like_composer() {
        let raw = r#"["fast", "smart"]"#;

        let models =
            parse_cursor_models_list(raw).unwrap_or_else(|err| panic!("parse models: {err}"));

        assert_eq!(ids(&models), vec!["fast", "smart"]);
        assert_eq!(default_count(&models), 1);
    }

    #[test]
    fn collapses_duplicate_ids_and_multiple_defaults() {
        let raw = r#"{"models": [
            {"id": "composer-2.5", "default": true},
            {"id": "composer-1", "default": true},
            {"id": "composer-1"}
        ]}"#;

        let models =
            parse_cursor_models_list(raw).unwrap_or_else(|err| panic!("parse models: {err}"));

        assert_eq!(ids(&models), vec!["composer-2.5", "composer-1"]);
        assert_eq!(default_count(&models), 1);
        assert!(models[0].is_default);
    }

    #[test]
    fn empty_array_is_an_empty_error() {
        let err = parse_cursor_models_list("[]")
            .err()
            .unwrap_or_else(|| panic!("empty array should error"));
        assert!(matches!(err, CursorModelDiscoveryError::Empty));
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        let err = parse_cursor_models_list("not json")
            .err()
            .unwrap_or_else(|| panic!("malformed json should error"));
        assert!(matches!(err, CursorModelDiscoveryError::Parse(_)));
    }

    #[test]
    fn missing_model_array_is_a_parse_error() {
        let err = parse_cursor_models_list(r#"{"unexpected": true}"#)
            .err()
            .unwrap_or_else(|| panic!("missing array should error"));
        assert!(matches!(err, CursorModelDiscoveryError::Parse(_)));
    }

    #[tokio::test]
    async fn discovery_from_valid_payload_returns_parsed_models() {
        let models = discover_cursor_composer_models_from(|| async {
            Ok(r#"{"models": [
                {"id": "composer-2.5", "default": true},
                {"id": "composer-1"}
            ]}"#
            .to_string())
        })
        .await;

        assert_eq!(ids(&models), vec!["composer-2.5", "composer-1"]);
        assert_eq!(default_count(&models), 1);
    }

    #[tokio::test]
    async fn discovery_from_unparseable_payload_falls_back() {
        let models = discover_cursor_composer_models_from(|| async { Ok("garbage".to_string()) }).await;
        assert_eq!(models, cursor_composer_fallback_models());
    }

    #[tokio::test]
    async fn discovery_from_error_falls_back() {
        let models = discover_cursor_composer_models_from(|| async {
            Err(CursorModelDiscoveryError::CliNotFound)
        })
        .await;
        assert_eq!(models, cursor_composer_fallback_models());
    }

    #[test]
    fn model_serializes_as_camel_case() {
        let value = serde_json::to_value(CursorComposerModel::new(
            "composer-2.5",
            "Composer 2.5",
            true,
        ))
        .unwrap_or_else(|err| panic!("serialize model: {err}"));
        assert_eq!(
            value,
            serde_json::json!({
                "id": "composer-2.5",
                "displayName": "Composer 2.5",
                "isDefault": true
            })
        );
    }
}
