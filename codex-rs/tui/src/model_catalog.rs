use codex_protocol::openai_models::ModelPreset;
use std::convert::Infallible;

#[derive(Debug, Clone)]
pub(crate) struct ModelCatalog {
    provider_id: Option<String>,
    models: Vec<ModelPreset>,
}

impl ModelCatalog {
    #[cfg(test)]
    pub(crate) fn new(models: Vec<ModelPreset>) -> Self {
        Self {
            provider_id: None,
            models,
        }
    }

    pub(crate) fn new_for_provider(provider_id: String, models: Vec<ModelPreset>) -> Self {
        Self {
            provider_id: Some(provider_id),
            models,
        }
    }

    pub(crate) fn provider_id(&self) -> Option<&str> {
        self.provider_id.as_deref()
    }

    pub(crate) fn try_list_models(&self) -> Result<Vec<ModelPreset>, Infallible> {
        Ok(self.models.clone())
    }
}
