use std::sync::Arc;

use codex_core::config::Config;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::HostToolCapability;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_login::AuthManager;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::backend::CodexImagesBackend;
use crate::tool::ImageGenerationTool;

#[derive(Clone)]
struct ImageGenerationExtension {
    auth_manager: Arc<AuthManager>,
}

#[derive(Clone)]
struct ImageGenerationExtensionConfig {
    available: bool,
    provider: ModelProviderInfo,
    codex_home: AbsolutePathBuf,
}

impl From<&Config> for ImageGenerationExtensionConfig {
    /// Resolves whether standalone image generation should be available for a thread.
    fn from(config: &Config) -> Self {
        Self {
            // Core selects this executor per turn using the feature flag or model metadata.
            available: config.model_provider.is_openai(),
            provider: config.model_provider.clone(),
            codex_home: config.codex_home.clone(),
        }
    }
}

#[async_trait::async_trait]
impl ThreadLifecycleContributor<Config> for ImageGenerationExtension {
    /// Seeds image-generation availability when a thread begins.
    async fn on_thread_start(&self, input: ThreadStartInput<'_, Config>) {
        input
            .thread_store
            .insert(ImageGenerationExtensionConfig::from(input.config));
    }
}

impl ConfigContributor<Config> for ImageGenerationExtension {
    /// Refreshes image-generation availability after thread configuration changes.
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        thread_store.insert(ImageGenerationExtensionConfig::from(new_config));
    }
}

impl ToolContributor for ImageGenerationExtension {
    /// Creates the image-generation tool exposed by this installed extension.
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(config) = thread_store.get::<ImageGenerationExtensionConfig>() else {
            return Vec::new();
        };
        if !config.available || !self.auth_manager.current_auth_uses_codex_backend() {
            return Vec::new();
        }

        vec![Arc::new(ImageGenerationTool::new(
            CodexImagesBackend::new(create_model_provider(
                config.provider.clone(),
                Some(self.auth_manager.clone()),
            )),
            config.codex_home.clone(),
            thread_store.level_id().to_string(),
        ))]
    }
}

/// Installs the standalone image-generation extension contributors.
pub fn install(registry: &mut ExtensionRegistryBuilder<Config>, auth_manager: Arc<AuthManager>) {
    let contributor = install_with_handle(registry, auth_manager);
    assert!(
        registry.assign_host_tool_capability(&contributor, HostToolCapability::ImageGeneration,),
        "the installed image-generation contributor must be registered"
    );
}

/// Installs image generation and returns the contributor handle for host policy.
pub fn install_with_handle(
    registry: &mut ExtensionRegistryBuilder<Config>,
    auth_manager: Arc<AuthManager>,
) -> Arc<dyn ToolContributor> {
    let extension = Arc::new(ImageGenerationExtension { auth_manager });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    let contributor: Arc<dyn ToolContributor> = extension;
    registry.tool_contributor(Arc::clone(&contributor));
    contributor
}

#[cfg(test)]
mod tests {
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::HostToolCapability;
    use codex_login::CodexAuth;
    use pretty_assertions::assert_eq;

    use super::AuthManager;
    use super::Config;
    use super::install;

    #[test]
    fn legacy_install_preserves_image_generation_host_capability() {
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        install(
            &mut builder,
            AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing()),
        );
        let registry = builder.build();
        let contributor = registry
            .tool_contributors()
            .first()
            .expect("legacy install should register its tool contributor");

        assert_eq!(
            registry.host_tool_capability(contributor),
            Some(HostToolCapability::ImageGeneration),
            "legacy install must preserve hosted replacement behavior"
        );
    }
}
