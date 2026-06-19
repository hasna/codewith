use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;

use crate::tool::ValidateWorkflowYamlTool;

#[derive(Clone)]
struct WorkflowExtension<C> {
    workflows_enabled: Arc<dyn Fn(&C) -> bool + Send + Sync>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkflowExtensionConfig {
    enabled: bool,
}

struct WorkflowExtensionState {
    enabled: Arc<AtomicBool>,
}

impl WorkflowExtensionState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

impl<C> WorkflowExtension<C> {
    fn new(workflows_enabled: impl Fn(&C) -> bool + Send + Sync + 'static) -> Self {
        Self {
            workflows_enabled: Arc::new(workflows_enabled),
        }
    }

    fn config(&self, config: &C) -> WorkflowExtensionConfig {
        WorkflowExtensionConfig {
            enabled: (self.workflows_enabled)(config),
        }
    }
}

#[async_trait::async_trait]
impl<C> ThreadLifecycleContributor<C> for WorkflowExtension<C>
where
    C: Send + Sync + 'static,
{
    async fn on_thread_start(&self, input: ThreadStartInput<'_, C>) {
        let config = self.config(input.config);
        input.thread_store.insert(config);
        input
            .thread_store
            .get_or_init(|| WorkflowExtensionState::new(config.enabled))
            .set_enabled(config.enabled);
    }
}

impl<C> ConfigContributor<C> for WorkflowExtension<C>
where
    C: Send + Sync + 'static,
{
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &C,
        new_config: &C,
    ) {
        let config = self.config(new_config);
        thread_store.insert(config);
        thread_store
            .get_or_init(|| WorkflowExtensionState::new(config.enabled))
            .set_enabled(config.enabled);
    }
}

impl<C> ToolContributor for WorkflowExtension<C>
where
    C: Send + Sync + 'static,
{
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>> {
        let Some(state) = thread_store.get::<WorkflowExtensionState>() else {
            return Vec::new();
        };
        if !state.is_enabled() {
            return Vec::new();
        }
        vec![Arc::new(ValidateWorkflowYamlTool::new(Arc::clone(
            &state.enabled,
        )))]
    }
}

pub fn install<C>(
    registry: &mut ExtensionRegistryBuilder<C>,
    workflows_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
) where
    C: Send + Sync + 'static,
{
    let extension = Arc::new(WorkflowExtension::new(workflows_enabled));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use codex_extension_api::ExtensionData;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::ToolName;
    use pretty_assertions::assert_eq;

    use super::WorkflowExtensionConfig;
    use super::WorkflowExtensionState;
    use super::install;
    use crate::tool::VALIDATE_WORKFLOW_YAML_TOOL_NAME;

    #[test]
    fn installed_extension_hides_tool_when_disabled() {
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(&mut builder, |enabled| *enabled);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(WorkflowExtensionConfig { enabled: false });
        thread_store.insert(WorkflowExtensionState::new(false));

        let tool_names = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>();

        assert_eq!(Vec::<ToolName>::new(), tool_names);
    }

    #[test]
    fn installed_extension_contributes_validation_tool_when_enabled() {
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(&mut builder, |enabled| *enabled);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(WorkflowExtensionConfig { enabled: true });
        thread_store.insert(WorkflowExtensionState::new(true));

        let tool_names = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>();

        assert_eq!(
            vec![ToolName::plain(VALIDATE_WORKFLOW_YAML_TOOL_NAME)],
            tool_names
        );
    }

    #[test]
    fn contributed_tool_observes_later_disable() {
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(&mut builder, |enabled| *enabled);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        let state = WorkflowExtensionState::new(true);
        thread_store.insert(WorkflowExtensionConfig { enabled: true });
        thread_store.insert(state);

        let tools = registry.tool_contributors()[0].tools(&session_store, &thread_store);
        assert_eq!(tools.len(), 1);

        let state = thread_store
            .get::<WorkflowExtensionState>()
            .expect("workflow extension state");
        state.set_enabled(false);
        let tool_names = registry.tool_contributors()[0]
            .tools(&session_store, &thread_store)
            .into_iter()
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>();

        assert_eq!(Vec::<ToolName>::new(), tool_names);
    }
}
