use std::sync::Arc;

use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::HostToolCapability;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;

struct EmptyToolContributor;

impl ToolContributor for EmptyToolContributor {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        _thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        Vec::new()
    }
}

#[test]
fn host_capability_is_bound_to_the_exact_registered_contributor() {
    let registered: Arc<dyn ToolContributor> = Arc::new(EmptyToolContributor);
    let lookalike: Arc<dyn ToolContributor> = Arc::new(EmptyToolContributor);
    let mut builder = ExtensionRegistryBuilder::<()>::new();

    assert!(
        !builder.assign_host_tool_capability(&registered, HostToolCapability::WebSearch),
        "an unregistered contributor must not receive host authority"
    );
    builder.tool_contributor(Arc::clone(&registered));
    assert!(builder.assign_host_tool_capability(&registered, HostToolCapability::WebSearch));
    assert!(
        !builder.assign_host_tool_capability(&registered, HostToolCapability::WebSearch),
        "the same authority assignment must not report success twice"
    );
    assert!(builder.assign_host_tool_capability(&registered, HostToolCapability::ImageGeneration));

    let registry = builder.build();
    assert_eq!(
        registry.host_tool_capability(&registered),
        Some(HostToolCapability::WebSearch)
    );
    assert_eq!(
        registry.host_tool_capabilities(&registered),
        vec![
            HostToolCapability::WebSearch,
            HostToolCapability::ImageGeneration,
        ]
    );
    assert_eq!(registry.host_tool_capability(&lookalike), None);
    assert!(registry.host_tool_capabilities(&lookalike).is_empty());
}
