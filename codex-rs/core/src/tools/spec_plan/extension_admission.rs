use crate::session::turn_context::TurnContext;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExposure;
use codex_extension_api::HostToolCapability;
use codex_model_provider_info::WireApi;
use codex_protocol::openai_models::ToolMode;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use tracing::warn;

use super::IMAGE_GEN_NAMESPACE;
use super::IMAGEGEN_TOOL_NAME;
use super::is_excluded_from_code_mode;
use super::is_hidden_by_code_mode;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum HostedToolReplacement {
    WebSearch,
    ImageGeneration,
}

impl HostedToolReplacement {
    pub(super) fn from_host_capability(capability: HostToolCapability) -> Self {
        match capability {
            HostToolCapability::WebSearch => Self::WebSearch,
            HostToolCapability::ImageGeneration => Self::ImageGeneration,
            _ => unreachable!("unknown host tool capability"),
        }
    }

    pub(super) fn tool_name(self) -> ToolName {
        match self {
            Self::WebSearch => ToolName::namespaced("web", "run"),
            Self::ImageGeneration => ToolName::namespaced(IMAGE_GEN_NAMESPACE, IMAGEGEN_TOOL_NAME),
        }
    }

    pub(super) fn matches_spec(self, spec: &ToolSpec) -> bool {
        match self {
            Self::WebSearch => matches!(
                spec,
                ToolSpec::Namespace(namespace)
                    if codex_tools::is_canonical_web_search_namespace(namespace)
            ),
            Self::ImageGeneration => matches!(
                spec,
                ToolSpec::Namespace(namespace)
                    if codex_tools::is_canonical_image_generation_namespace(namespace)
            ),
        }
    }
}

/// Defense-in-depth guard against a namespace tool being assembled
/// under a Responses-API-reserved built-in namespace.
///
/// This is the durable regression guard for the `image_gen.imagegen` 400: the
/// standalone image tool must live under the non-reserved `images` namespace,
/// never `image_gen`. If a future refactor (or a second registration path)
/// reintroduces a reserved namespace, this drops the offending tool so the
/// request still ships (minus one tool) instead of the API rejecting the
/// entire turn.
///
pub(super) fn namespace_spec_is_safe_for_wire(turn_context: &TurnContext, spec: &ToolSpec) -> bool {
    if turn_context.provider.info().wire_api == WireApi::Chat {
        return true;
    }
    let ToolSpec::Namespace(namespace) = spec else {
        return true;
    };
    let forbidden = codex_tools::is_reserved_responses_namespace(&namespace.name)
        && !codex_tools::is_canonical_web_search_namespace(namespace);
    if forbidden {
        let tool_name = namespace.tools.first().map_or("*", |tool| match tool {
            ResponsesApiNamespaceTool::Function(tool) => tool.name.as_str(),
        });
        warn!(
            namespace = %namespace.name,
            tool_name,
            "dropping tool under reserved Responses namespace to avoid a runtime 400",
        );
        return false;
    }
    true
}

pub(super) fn namespace_spec_is_safe_for_untrusted_runtime(
    turn_context: &TurnContext,
    spec: &ToolSpec,
) -> bool {
    if turn_context.provider.info().wire_api == WireApi::Chat {
        return true;
    }
    let ToolSpec::Namespace(namespace) = spec else {
        return true;
    };
    !codex_tools::is_reserved_responses_namespace(&namespace.name)
}

pub(super) fn extension_spec_matches_tool_name(spec: &ToolSpec, tool_name: &ToolName) -> bool {
    match spec {
        ToolSpec::Function(tool) => tool_name.namespace.is_none() && tool.name == tool_name.name,
        ToolSpec::Namespace(namespace) => {
            let [ResponsesApiNamespaceTool::Function(tool)] = namespace.tools.as_slice() else {
                return false;
            };
            tool_name.namespace.as_deref() == Some(namespace.name.as_str())
                && tool.name == tool_name.name
        }
        ToolSpec::Freeform(tool) => tool_name.namespace.is_none() && tool.name == tool_name.name,
        ToolSpec::ToolSearch { .. }
        | ToolSpec::ImageGeneration { .. }
        | ToolSpec::WebSearch { .. }
        | ToolSpec::AnthropicWebSearch { .. }
        | ToolSpec::OpenRouterWebSearch { .. }
        | ToolSpec::XaiWebSearch { .. }
        | ToolSpec::XiaomiWebSearch { .. }
        | ToolSpec::QwenWebSearch { .. }
        | ToolSpec::ZaiWebSearch { .. } => false,
    }
}

pub(super) fn extension_spec_is_accepted(
    turn_context: &TurnContext,
    spec: &ToolSpec,
    tool_name: &ToolName,
) -> bool {
    if !namespace_spec_is_safe_for_wire(turn_context, spec) {
        return false;
    }

    if !extension_spec_matches_tool_name(spec, tool_name) {
        warn!(
            %tool_name,
            spec_name = spec.name(),
            "dropping extension tool whose ToolSpec does not match its declared tool name",
        );
        return false;
    }
    true
}

pub(super) fn runtime_replaces_hosted_tool(
    turn_context: &TurnContext,
    runtime: &dyn CoreToolRuntime,
    replacement: HostedToolReplacement,
) -> bool {
    let expected_name = replacement.tool_name();
    if runtime.tool_name() != expected_name || !runtime.exposure().is_direct() {
        return false;
    }

    let exposure = runtime.exposure();
    let spec = runtime.spec();
    if !replacement.matches_spec(&spec) {
        return false;
    }
    let direct_visible = !is_hidden_by_code_mode(turn_context, &expected_name, exposure);
    let nested_visible = matches!(
        turn_context.tool_mode,
        ToolMode::CodeMode | ToolMode::CodeModeOnly
    ) && exposure != ToolExposure::DirectModelOnly
        && !is_excluded_from_code_mode(turn_context, &expected_name)
        && codex_code_mode::is_code_mode_nested_tool(spec.name());

    direct_visible || nested_visible
}
