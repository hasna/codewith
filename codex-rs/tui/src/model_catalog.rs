use codex_model_provider_info::model_gateway_for_provider;
use codex_model_provider_info::model_gateway_name;
use codex_protocol::openai_models::ModelPreset;
use std::convert::Infallible;

#[derive(Debug, Clone)]
pub(crate) struct ModelCatalog {
    gateway_name: String,
    provider_id: Option<String>,
    models: Vec<ModelPreset>,
}

impl ModelCatalog {
    #[cfg(test)]
    pub(crate) fn new(models: Vec<ModelPreset>) -> Self {
        Self {
            gateway_name: codex_model_provider_info::HASNA_GATEWAY_NAME.to_string(),
            provider_id: None,
            models,
        }
    }

    pub(crate) fn new_for_provider(provider_id: String, models: Vec<ModelPreset>) -> Self {
        let gateway_id = model_gateway_for_provider(&provider_id).to_string();
        let gateway_name = model_gateway_name(&gateway_id)
            .unwrap_or(codex_model_provider_info::HASNA_GATEWAY_NAME)
            .to_string();
        Self {
            gateway_name,
            provider_id: Some(provider_id),
            models,
        }
    }

    pub(crate) fn gateway_name(&self) -> &str {
        &self.gateway_name
    }

    pub(crate) fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }

    pub(crate) fn try_list_models(&self) -> Result<Vec<ModelPreset>, Infallible> {
        Ok(self.models.clone())
    }
}
