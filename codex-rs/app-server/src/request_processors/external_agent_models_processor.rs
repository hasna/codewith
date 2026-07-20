//! App-server RPC handler for external-agent model discovery.
//!
//! This processor answers "which models can this external-agent runtime target
//! right now?" for a given runtime id. For Cursor it delegates to
//! [`codex_external_agent::discover_cursor_composer_models`], which performs
//! live discovery of the available Composer models (default plus alternates),
//! caches the result per process, and falls back to the static `composer-2.5`
//! default when Cursor is offline, unauthenticated, or otherwise unreachable.
//! Runtimes without a model catalog return an empty list.
//!
//! # Wiring (owned by the RPC/protocol component)
//!
//! The request/response types below are the handler's contract shape. The
//! protocol component owns the canonical `codex-app-server-protocol` types, the
//! `ClientRequest` variant, and the `mod` + dispatch lines in
//! `request_processors.rs` / `message_processor.rs` that route a request to
//! [`ExternalAgentModelsRequestProcessor::list_models`]. Those hub edits, and
//! any relocation of these types into the protocol crate, are intentionally not
//! made here so parallel components do not collide on shared files.

use codex_app_server_protocol::JSONRPCErrorError;
use codex_external_agent::CursorComposerModel;
use codex_external_agent::ExternalAgentRuntimeId;
use codex_external_agent::discover_cursor_composer_models;
use serde::Deserialize;
use serde::Serialize;

use crate::error_code::invalid_params;

/// Parameters for an external-agent model list request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalAgentModelsListParams {
    /// Runtime id whose models to discover, for example `cursor`.
    pub runtime: String,
}

/// A single model advertised for an external-agent runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalAgentModel {
    /// Stable model identifier, for example `composer-2.5`.
    pub id: String,
    /// Human-facing label, for example `Composer 2.5`.
    pub display_name: String,
    /// Whether this is the runtime's default model.
    pub is_default: bool,
}

impl From<CursorComposerModel> for ExternalAgentModel {
    fn from(model: CursorComposerModel) -> Self {
        Self {
            id: model.id,
            display_name: model.display_name,
            is_default: model.is_default,
        }
    }
}

/// Response for an external-agent model list request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalAgentModelsListResponse {
    /// Runtime id the models belong to.
    pub runtime: String,
    /// Discovered models, default first. Empty when the runtime exposes none.
    pub models: Vec<ExternalAgentModel>,
    /// Convenience id of the default model, if any.
    pub default_model_id: Option<String>,
}

impl ExternalAgentModelsListResponse {
    fn from_models(runtime: String, models: Vec<CursorComposerModel>) -> Self {
        let default_model_id = models
            .iter()
            .find(|model| model.is_default)
            .map(|model| model.id.clone());
        Self {
            runtime,
            models: models.into_iter().map(ExternalAgentModel::from).collect(),
            default_model_id,
        }
    }
}

/// Handles external-agent model discovery requests.
#[derive(Clone, Default)]
pub(crate) struct ExternalAgentModelsRequestProcessor;

impl ExternalAgentModelsRequestProcessor {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Return the models available for the requested runtime id.
    pub(crate) async fn list_models(
        &self,
        params: ExternalAgentModelsListParams,
    ) -> Result<ExternalAgentModelsListResponse, JSONRPCErrorError> {
        let runtime = params.runtime.trim().to_string();
        if runtime.is_empty() {
            return Err(invalid_params(
                "external agent runtime id must not be empty",
            ));
        }

        let models = if runtime == ExternalAgentRuntimeId::CURSOR {
            discover_cursor_composer_models().await
        } else {
            // Runtimes without a Composer-style model catalog expose no models.
            Vec::new()
        };

        Ok(ExternalAgentModelsListResponse::from_models(runtime, models))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn cursor_models() -> Vec<CursorComposerModel> {
        vec![
            CursorComposerModel::new("composer-2.5", "Composer 2.5", true),
            CursorComposerModel::new("composer-1", "Composer 1", false),
        ]
    }

    #[test]
    fn from_models_promotes_default_and_maps_items() {
        let response =
            ExternalAgentModelsListResponse::from_models("cursor".to_string(), cursor_models());

        assert_eq!(response.runtime, "cursor");
        assert_eq!(response.default_model_id.as_deref(), Some("composer-2.5"));
        assert_eq!(
            response.models,
            vec![
                ExternalAgentModel {
                    id: "composer-2.5".to_string(),
                    display_name: "Composer 2.5".to_string(),
                    is_default: true,
                },
                ExternalAgentModel {
                    id: "composer-1".to_string(),
                    display_name: "Composer 1".to_string(),
                    is_default: false,
                },
            ]
        );
    }

    #[test]
    fn from_models_without_default_reports_none() {
        let models = vec![CursorComposerModel::new("composer-1", "Composer 1", false)];
        let response = ExternalAgentModelsListResponse::from_models("cursor".to_string(), models);
        assert_eq!(response.default_model_id, None);
    }

    #[test]
    fn response_serializes_as_camel_case() {
        let response =
            ExternalAgentModelsListResponse::from_models("cursor".to_string(), cursor_models());
        let value = serde_json::to_value(&response)
            .unwrap_or_else(|err| panic!("serialize response: {err}"));
        assert_eq!(value["defaultModelId"], serde_json::json!("composer-2.5"));
        assert_eq!(value["models"][0]["displayName"], serde_json::json!("Composer 2.5"));
        assert_eq!(value["models"][0]["isDefault"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn list_models_rejects_empty_runtime() {
        let processor = ExternalAgentModelsRequestProcessor::new();
        let err = processor
            .list_models(ExternalAgentModelsListParams {
                runtime: "   ".to_string(),
            })
            .await
            .err()
            .unwrap_or_else(|| panic!("empty runtime should be rejected"));
        assert_eq!(err.code, crate::error_code::INVALID_PARAMS_ERROR_CODE);
    }

    #[tokio::test]
    async fn list_models_for_unknown_runtime_is_empty() {
        let processor = ExternalAgentModelsRequestProcessor::new();
        let response = processor
            .list_models(ExternalAgentModelsListParams {
                runtime: "grok-build".to_string(),
            })
            .await
            .unwrap_or_else(|err| panic!("unknown runtime should succeed: {}", err.message));
        assert!(response.models.is_empty());
        assert_eq!(response.default_model_id, None);
        assert_eq!(response.runtime, "grok-build");
    }

    #[tokio::test]
    async fn list_models_for_cursor_always_has_a_single_default() {
        // Offline (no Cursor CLI on PATH) this resolves to the static fallback;
        // when a live catalog is reachable it resolves to real models. Either
        // way the invariant holds: a non-empty list with exactly one default.
        let processor = ExternalAgentModelsRequestProcessor::new();
        let response = processor
            .list_models(ExternalAgentModelsListParams {
                runtime: "cursor".to_string(),
            })
            .await
            .unwrap_or_else(|err| panic!("cursor discovery should succeed: {}", err.message));

        assert!(!response.models.is_empty());
        assert_eq!(
            response.models.iter().filter(|model| model.is_default).count(),
            1
        );
        assert!(response.default_model_id.is_some());
    }
}
