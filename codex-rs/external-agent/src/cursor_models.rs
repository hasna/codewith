//! Cursor Composer model catalog and discovery (Comp3).
//!
//! # Parallel-integration note
//!
//! This module is the model-discovery seam owned by the Cursor **model
//! discovery** component (Comp3). The cursor-composer-runtime component depends
//! on the public surface here — [`CursorComposerModel`],
//! [`CursorComposerModelCatalog`], [`cursor_composer_seed_models`], and
//! [`resolve_cursor_composer_model`] — for model selection in both the local
//! `@cursor/sdk` and the cloud (`bc-`) backends. When Comp3 lands its own
//! `cursor_models.rs`, prefer that version as long as it keeps this surface
//! stable.
//!
//! The shipping design sources models from the SDK's `Cursor.models.list()` at
//! runtime; [`CursorComposerModelCatalog::from_discovery_json`] parses that
//! response, and [`cursor_composer_seed_models`] pins a minimal offline catalog
//! (default `composer-2.5`) so the rest of Codewith can be wired and tested
//! before a live Cursor account is available.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;

/// Default model advertised by the Cursor Composer SDK (`@cursor/sdk`).
pub const CURSOR_COMPOSER_DEFAULT_MODEL: &str = "composer-2.5";

/// A Cursor Composer model as surfaced by model discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorComposerModel {
    /// Stable model id passed back to the SDK (for example `composer-2.5`).
    pub id: String,
    /// Human-readable label for pickers.
    pub display_name: String,
    /// Whether this is the runtime's default model.
    #[serde(default)]
    pub default: bool,
}

impl CursorComposerModel {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>, default: bool) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            default,
        }
    }
}

/// An ordered catalog of Cursor Composer models with a guaranteed default.
///
/// The catalog is never empty: construction always folds in the seed default so
/// [`CursorComposerModelCatalog::default_model`] and
/// [`CursorComposerModelCatalog::resolve`] cannot fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorComposerModelCatalog {
    models: Vec<CursorComposerModel>,
}

impl CursorComposerModelCatalog {
    /// The offline seed catalog used until live discovery lands.
    pub fn seed() -> Self {
        Self::from_models(cursor_composer_seed_models())
    }

    /// Build a catalog from discovered models, guaranteeing a non-empty list
    /// with exactly one default. Unknown/empty ids are dropped; duplicates keep
    /// their first occurrence. If no model is marked default the first entry is
    /// promoted, and if the list is empty it falls back to the seed catalog.
    pub fn from_models(models: Vec<CursorComposerModel>) -> Self {
        let mut deduped: Vec<CursorComposerModel> = Vec::with_capacity(models.len());
        for model in models {
            if model.id.trim().is_empty() {
                continue;
            }
            if deduped.iter().any(|existing| existing.id == model.id) {
                continue;
            }
            deduped.push(model);
        }
        if deduped.is_empty() {
            deduped = cursor_composer_seed_models();
        }
        if !deduped.iter().any(|model| model.default) {
            if let Some(first) = deduped.first_mut() {
                first.default = true;
            }
        } else {
            // Keep exactly one default (the first one wins).
            let mut seen_default = false;
            for model in &mut deduped {
                if model.default {
                    if seen_default {
                        model.default = false;
                    } else {
                        seen_default = true;
                    }
                }
            }
        }
        Self { models: deduped }
    }

    /// Parse a `Cursor.models.list()` response into a catalog.
    ///
    /// Accepts either a bare JSON array of model objects or an object with a
    /// `models`/`data` array. Returns `None` when no model objects are found so
    /// callers can fall back to the seed catalog.
    pub fn from_discovery_json(value: &JsonValue) -> Option<Self> {
        let array = value
            .as_array()
            .or_else(|| value.get("models").and_then(JsonValue::as_array))
            .or_else(|| value.get("data").and_then(JsonValue::as_array))?;
        let mut models = Vec::new();
        for entry in array {
            let Some(id) = entry
                .get("id")
                .or_else(|| entry.get("model"))
                .and_then(JsonValue::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let display_name = entry
                .get("displayName")
                .or_else(|| entry.get("display_name"))
                .or_else(|| entry.get("name"))
                .and_then(JsonValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| id.to_string());
            let default = entry
                .get("default")
                .or_else(|| entry.get("isDefault"))
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            models.push(CursorComposerModel::new(id, display_name, default));
        }
        if models.is_empty() {
            return None;
        }
        Some(Self::from_models(models))
    }

    /// All models in discovery order.
    pub fn models(&self) -> &[CursorComposerModel] {
        &self.models
    }

    /// The default model (always present).
    pub fn default_model(&self) -> &CursorComposerModel {
        self.models
            .iter()
            .find(|model| model.default)
            .or_else(|| self.models.first())
            .unwrap_or_else(|| unreachable!("catalog is never empty"))
    }

    /// Whether the catalog contains a model with `id`.
    pub fn contains(&self, id: &str) -> bool {
        self.models.iter().any(|model| model.id == id)
    }

    /// Resolve a requested model id against the catalog.
    ///
    /// Returns the requested id when it is known, otherwise the default model
    /// id. This is the single selection entry point shared by both Cursor
    /// backends.
    pub fn resolve(&self, requested: Option<&str>) -> String {
        match requested.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) if self.contains(id) => id.to_string(),
            _ => self.default_model().id.clone(),
        }
    }
}

impl Default for CursorComposerModelCatalog {
    fn default() -> Self {
        Self::seed()
    }
}

/// Seed model list used until live discovery via `Cursor.models.list()` lands.
pub fn cursor_composer_seed_models() -> Vec<CursorComposerModel> {
    vec![
        CursorComposerModel::new(CURSOR_COMPOSER_DEFAULT_MODEL, "Composer 2.5", true),
        CursorComposerModel::new("composer-2", "Composer 2", false),
    ]
}

/// Resolve a requested model id against the seed catalog.
///
/// Convenience wrapper over [`CursorComposerModelCatalog::seed`] +
/// [`CursorComposerModelCatalog::resolve`] for callers that do not carry a live
/// catalog.
pub fn resolve_cursor_composer_model(requested: Option<&str>) -> String {
    CursorComposerModelCatalog::seed().resolve(requested)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn seed_catalog_defaults_to_composer_2_5() {
        let catalog = CursorComposerModelCatalog::seed();
        assert_eq!(catalog.default_model().id, CURSOR_COMPOSER_DEFAULT_MODEL);
        assert_eq!(CURSOR_COMPOSER_DEFAULT_MODEL, "composer-2.5");
        assert!(catalog.contains("composer-2"));
    }

    #[test]
    fn resolve_prefers_known_requested_model() {
        let catalog = CursorComposerModelCatalog::seed();
        assert_eq!(catalog.resolve(Some("composer-2")), "composer-2");
        assert_eq!(catalog.resolve(Some("  composer-2  ")), "composer-2");
    }

    #[test]
    fn resolve_falls_back_to_default_for_unknown_or_empty() {
        let catalog = CursorComposerModelCatalog::seed();
        assert_eq!(catalog.resolve(Some("gpt-9")), CURSOR_COMPOSER_DEFAULT_MODEL);
        assert_eq!(catalog.resolve(Some("")), CURSOR_COMPOSER_DEFAULT_MODEL);
        assert_eq!(catalog.resolve(None), CURSOR_COMPOSER_DEFAULT_MODEL);
    }

    #[test]
    fn resolve_free_function_uses_seed_catalog() {
        assert_eq!(resolve_cursor_composer_model(Some("composer-2")), "composer-2");
        assert_eq!(
            resolve_cursor_composer_model(Some("nope")),
            CURSOR_COMPOSER_DEFAULT_MODEL
        );
    }

    #[test]
    fn discovery_parses_bare_array_and_promotes_default() {
        let value = serde_json::json!([
            {"id": "composer-3", "name": "Composer 3"},
            {"id": "composer-2.5", "displayName": "Composer 2.5"},
        ]);
        let catalog = CursorComposerModelCatalog::from_discovery_json(&value).expect("catalog");
        assert_eq!(catalog.models().len(), 2);
        // No explicit default -> first entry is promoted.
        assert_eq!(catalog.default_model().id, "composer-3");
        assert_eq!(catalog.resolve(Some("composer-2.5")), "composer-2.5");
    }

    #[test]
    fn discovery_parses_wrapped_object_and_honors_explicit_default() {
        let value = serde_json::json!({
            "models": [
                {"id": "composer-2", "name": "Composer 2"},
                {"id": "composer-2.5", "name": "Composer 2.5", "default": true},
            ]
        });
        let catalog = CursorComposerModelCatalog::from_discovery_json(&value).expect("catalog");
        assert_eq!(catalog.default_model().id, "composer-2.5");
    }

    #[test]
    fn discovery_dedupes_and_keeps_single_default() {
        let value = serde_json::json!([
            {"id": "a", "default": true},
            {"id": "a", "default": true},
            {"id": "b", "default": true},
            {"id": "", "default": true},
        ]);
        let catalog = CursorComposerModelCatalog::from_discovery_json(&value).expect("catalog");
        assert_eq!(catalog.models().len(), 2);
        assert_eq!(
            catalog
                .models()
                .iter()
                .filter(|model| model.default)
                .count(),
            1
        );
        assert_eq!(catalog.default_model().id, "a");
    }

    #[test]
    fn discovery_returns_none_without_models() {
        assert!(CursorComposerModelCatalog::from_discovery_json(&serde_json::json!({})).is_none());
        assert!(CursorComposerModelCatalog::from_discovery_json(&serde_json::json!([])).is_none());
    }

    #[test]
    fn from_models_empty_falls_back_to_seed() {
        let catalog = CursorComposerModelCatalog::from_models(Vec::new());
        assert_eq!(catalog.default_model().id, CURSOR_COMPOSER_DEFAULT_MODEL);
    }
}
