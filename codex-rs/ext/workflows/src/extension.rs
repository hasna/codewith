use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_protocol::ThreadId;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_state::StateRuntime;

use crate::manager_tool::ManageWorkflowTool;
use crate::tool::ValidateWorkflowYamlTool;

#[derive(Clone)]
struct WorkflowExtension<C> {
    workflows_enabled: Arc<dyn Fn(&C) -> bool + Send + Sync>,
    state_db: Option<Arc<StateRuntime>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WorkflowExtensionConfig {
    enabled: bool,
}

struct WorkflowExtensionState {
    enabled: Arc<AtomicBool>,
    tools_available_for_thread: AtomicBool,
    thread_id: Mutex<Option<ThreadId>>,
}

impl WorkflowExtensionState {
    fn new(enabled: bool, tools_available_for_thread: bool, thread_id: Option<ThreadId>) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            tools_available_for_thread: AtomicBool::new(tools_available_for_thread),
            thread_id: Mutex::new(thread_id),
        }
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn tools_available_for_thread(&self) -> bool {
        self.tools_available_for_thread.load(Ordering::Relaxed)
    }

    fn thread_id(&self) -> Option<ThreadId> {
        *self
            .thread_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_thread_context(&self, tools_available_for_thread: bool, thread_id: Option<ThreadId>) {
        self.tools_available_for_thread
            .store(tools_available_for_thread, Ordering::Relaxed);
        *self
            .thread_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = thread_id;
    }
}

impl<C> WorkflowExtension<C> {
    fn new(
        state_db: Option<Arc<StateRuntime>>,
        workflows_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            workflows_enabled: Arc::new(workflows_enabled),
            state_db,
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
        let tools_available_for_thread = input.persistent_thread_state_available
            && !matches!(
                input.session_source,
                SessionSource::SubAgent(SubAgentSource::Review)
            );
        let thread_id = ThreadId::from_string(input.thread_store.level_id()).ok();
        input.thread_store.insert(config);
        let state = input.thread_store.get_or_init(|| {
            WorkflowExtensionState::new(config.enabled, tools_available_for_thread, thread_id)
        });
        state.set_enabled(config.enabled);
        state.set_thread_context(tools_available_for_thread, thread_id);
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
            .get_or_init(|| WorkflowExtensionState::new(config.enabled, false, None))
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
        let mut tools: Vec<
            Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>,
        > = vec![Arc::new(ValidateWorkflowYamlTool::new(Arc::clone(
            &state.enabled,
        )))];
        if state.tools_available_for_thread()
            && let (Some(state_db), Some(thread_id)) = (&self.state_db, state.thread_id())
        {
            tools.push(Arc::new(ManageWorkflowTool::new(
                Arc::clone(&state.enabled),
                Arc::clone(state_db),
                thread_id,
            )));
        }
        tools
    }
}

pub fn install<C>(
    registry: &mut ExtensionRegistryBuilder<C>,
    state_db: Option<Arc<StateRuntime>>,
    workflows_enabled: impl Fn(&C) -> bool + Send + Sync + 'static,
) where
    C: Send + Sync + 'static,
{
    let extension = Arc::new(WorkflowExtension::new(state_db, workflows_enabled));
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
    use crate::manager_tool::MANAGE_WORKFLOW_TOOL_NAME;
    use crate::tool::VALIDATE_WORKFLOW_YAML_TOOL_NAME;

    #[test]
    fn installed_extension_hides_tool_when_disabled() {
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(&mut builder, None, |enabled| *enabled);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(WorkflowExtensionConfig { enabled: false });
        thread_store.insert(WorkflowExtensionState::new(false, false, None));

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
        install(&mut builder, None, |enabled| *enabled);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(WorkflowExtensionConfig { enabled: true });
        thread_store.insert(WorkflowExtensionState::new(true, false, None));

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

    #[tokio::test]
    async fn installed_extension_contributes_management_tool_when_state_is_available() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(
            tempdir.path().to_path_buf(),
            "test-provider".to_string(),
        )
        .await
        .expect("state runtime should initialize");
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(&mut builder, Some(state_db), |enabled| *enabled);
        let registry = builder.build();
        let thread_id = codex_protocol::ThreadId::new();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new(thread_id.to_string());
        thread_store.insert(WorkflowExtensionConfig { enabled: true });
        thread_store.insert(WorkflowExtensionState::new(true, true, Some(thread_id)));

        let tool_names = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>();

        assert_eq!(
            vec![
                ToolName::plain(VALIDATE_WORKFLOW_YAML_TOOL_NAME),
                ToolName::plain(MANAGE_WORKFLOW_TOOL_NAME),
            ],
            tool_names
        );
    }

    #[test]
    fn contributed_tool_observes_later_disable() {
        let mut builder = ExtensionRegistryBuilder::<bool>::new();
        install(&mut builder, None, |enabled| *enabled);
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        let state = WorkflowExtensionState::new(true, false, None);
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
