use crate::session::turn_context::TurnContext;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExposure;
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
    pub(super) fn tool_name(self) -> ToolName {
        match self {
            Self::WebSearch => ToolName::namespaced("web", "run"),
            Self::ImageGeneration => ToolName::namespaced(IMAGE_GEN_NAMESPACE, IMAGEGEN_TOOL_NAME),
        }
    }

    pub(super) fn matches_spec(self, spec: &ToolSpec) -> bool {
        match self {
            Self::WebSearch => matches!(spec, ToolSpec::BuiltInWebSearch(_)),
            Self::ImageGeneration => extension_spec_matches_tool_name(spec, &self.tool_name()),
        }
    }
}

/// Defense-in-depth guard against a first-party namespace tool being assembled
/// under a Responses-API-reserved built-in namespace.
///
/// This is the durable regression guard for the `image_gen.imagegen` 400: the
/// standalone image tool must live under the non-reserved `images` namespace,
/// never `image_gen`. If a future refactor (or a second registration path)
/// reintroduces a reserved namespace, this fails loudly in debug/test builds
/// and, in release builds, drops the offending tool so the request still ships
/// (minus one tool) instead of the API rejecting the entire turn.
///
/// The standalone web-search extension uses `ToolSpec::BuiltInWebSearch`, so
/// generic namespace specs never need a name-based reserved-tool exception.
pub(super) fn namespace_spec_is_safe_for_wire(spec: &ToolSpec) -> bool {
    let ToolSpec::Namespace(namespace) = spec else {
        return true;
    };
    let forbidden_tool_name = namespace.forbidden_reserved_tool_name();
    debug_assert!(
        forbidden_tool_name.is_none(),
        "first-party tool assembled under Responses-API-reserved namespace `{namespace}` \
         (wire name `{namespace}.{tool_name}`); rename it to a non-reserved namespace, or use \
         the owning built-in ToolSpec representation",
        namespace = namespace.name,
        tool_name = forbidden_tool_name.unwrap_or("*"),
    );
    if let Some(tool_name) = forbidden_tool_name {
        warn!(
            namespace = %namespace.name,
            tool_name,
            "dropping first-party tool under reserved Responses namespace to avoid a runtime 400",
        );
        return false;
    }
    true
}

pub(super) fn namespace_spec_is_safe_for_runtime(spec: &ToolSpec) -> bool {
    let ToolSpec::Namespace(namespace) = spec else {
        return true;
    };
    let Some(tool_name) = namespace.forbidden_reserved_tool_name() else {
        return true;
    };
    warn!(
        namespace = %namespace.name,
        tool_name,
        "dropping runtime under reserved Responses namespace",
    );
    false
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
        ToolSpec::BuiltInWebSearch(_) => tool_name == &ToolName::namespaced("web", "run"),
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

pub(super) fn extension_spec_is_accepted(spec: &ToolSpec, tool_name: &ToolName) -> bool {
    if !namespace_spec_is_safe_for_runtime(spec) {
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
