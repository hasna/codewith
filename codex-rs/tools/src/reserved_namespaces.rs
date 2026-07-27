//! Namespaces the OpenAI Responses API reserves for its own built-in tools.
//!
//! The Responses API refuses any request whose `tools` array declares a
//! namespaced function under one of these names unless it exactly matches the
//! built-in's configured schema. A custom/first-party tool that collides is
//! rejected at runtime with, e.g.:
//!
//! ```text
//! 400 Function 'image_gen.imagegen' is reserved for use by this model
//!     and must match the configured schema.
//! ```
//!
//! Because that error rejects the *whole* request, a single mis-namespaced tool
//! breaks every turn in the session. To prevent this we (1) reject
//! dynamic/user-supplied tools that try to use a reserved namespace and (2)
//! validate canonical reserved schemas at the Responses request boundary.
//! Chat transports may safely flatten these names.

use crate::ToolSpec;
use codex_tool_contracts::WEB_RUN_DESCRIPTION;
use codex_tool_contracts::WEB_RUN_SCHEMA_JSON;
use serde_json::Value;

/// Namespaces reserved by the OpenAI Responses API for its built-in tools.
///
/// Keep this list sorted. It is the single source of truth consumed by the
/// dynamic-tool validator, the `multi_agent_v2` namespace validator, and the
/// final namespace serializer.
pub const RESERVED_RESPONSES_NAMESPACES: &[&str] = &[
    "api_tool",
    "browser",
    "computer",
    "container",
    "file_search",
    "functions",
    "image_gen",
    "multi_tool_use",
    "python",
    "python_user_visible",
    "submodel_delegator",
    "terminal",
    "tool_search",
    "web",
];

/// Reserved namespaces that retain a canonical first-party representation.
pub const FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES: &[&str] = &["web"];

/// True if `namespace` is reserved by the Responses API for a built-in tool.
pub fn is_reserved_responses_namespace(namespace: &str) -> bool {
    RESERVED_RESPONSES_NAMESPACES.contains(&namespace)
}

/// True if a custom namespace tool must not be serialized under `namespace`
/// because it is Responses-API-reserved.
///
/// This is the regression guard for the `image_gen.imagegen` 400: a
/// first-party / extension / code-mode tool must never be assembled under
/// `image_gen` (or any other reserved namespace without a canonical exception).
pub fn is_forbidden_first_party_namespace(namespace: &str) -> bool {
    is_reserved_responses_namespace(namespace)
        && !FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES.contains(&namespace)
}

/// True if a typed tool declaration cannot cross a Responses API boundary.
pub fn is_forbidden_reserved_namespace_spec(spec: &ToolSpec) -> bool {
    let ToolSpec::Namespace(namespace) = spec else {
        return false;
    };
    is_reserved_responses_namespace(&namespace.name)
        && !crate::is_canonical_web_search_namespace(namespace)
}

/// True if a serialized generic namespace declaration uses a reserved name.
///
/// This handles persisted and server-originated tool-search output, where the
/// declaration is already JSON and cannot pass through the typed namespace
/// serializer again.
pub fn is_forbidden_reserved_namespace_value(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("namespace") {
        return false;
    }
    let Some(namespace) = value.get("name").and_then(Value::as_str) else {
        return false;
    };
    if !is_reserved_responses_namespace(namespace) {
        return false;
    }
    namespace != "web"
        || !is_bounded_canonical_web_candidate(value)
        || !crate::is_canonical_web_search_namespace_value(value)
}

const MAX_CANONICAL_WEB_VALUE_WORK_BYTES: usize =
    WEB_RUN_SCHEMA_JSON.len() + WEB_RUN_DESCRIPTION.len() + 4_096;

fn is_bounded_canonical_web_candidate(value: &Value) -> bool {
    crate::bounded_json_serialized_len(value, MAX_CANONICAL_WEB_VALUE_WORK_BYTES).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_gen_is_reserved_and_forbidden_for_first_party() {
        assert!(is_reserved_responses_namespace("image_gen"));
        assert!(is_forbidden_first_party_namespace("image_gen"));
    }

    #[test]
    fn web_retains_its_canonical_first_party_exception() {
        assert!(is_reserved_responses_namespace("web"));
        assert!(!is_forbidden_first_party_namespace("web"));
    }

    #[test]
    fn non_reserved_namespaces_are_allowed() {
        for namespace in [
            "images",
            "memory",
            "agents",
            "multi_agent_v1",
            "mcp__server",
        ] {
            assert!(!is_reserved_responses_namespace(namespace));
            assert!(!is_forbidden_first_party_namespace(namespace));
        }
    }

    #[test]
    fn reserved_list_is_sorted_and_unique() {
        let mut sorted = RESERVED_RESPONSES_NAMESPACES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.as_slice(), RESERVED_RESPONSES_NAMESPACES);
    }

    #[test]
    fn serialized_guard_rejects_reserved_namespace_tools() {
        let canonical_web = serde_json::to_value(ToolSpec::built_in_web_search())
            .expect("canonical web.run should serialize");
        assert!(!is_forbidden_reserved_namespace_value(&canonical_web));

        for value in [
            serde_json::json!({"type": "namespace", "name": "image_gen", "tools": []}),
            serde_json::json!({
                "type": "namespace",
                "name": "image_gen",
                "tools": [{"type": "function", "name": "imagegen"}],
            }),
            serde_json::json!({
                "type": "namespace",
                "name": "web",
                "tools": [{"type": "function", "name": "imagegen"}],
            }),
            serde_json::json!({
                "type": "namespace",
                "name": "web",
                "description": "Tools in the web namespace.",
                "tools": [{
                    "type": "function",
                    "name": "run",
                    "description": "Untrusted web tool.",
                    "strict": false,
                    "parameters": {
                        "type": "object",
                        "properties": {},
                    },
                }],
            }),
        ] {
            assert!(is_forbidden_reserved_namespace_value(&value), "{value}");
        }

        for value in [
            serde_json::json!({
                "type": "namespace",
                "name": "images",
                "tools": [{"type": "function", "name": "imagegen"}],
            }),
            serde_json::json!({
                "type": "function",
                "name": "image_gen",
            }),
        ] {
            assert!(!is_forbidden_reserved_namespace_value(&value), "{value}");
        }
    }

    #[test]
    fn serialized_guard_bounds_malformed_web_history_before_canonical_comparison() {
        let value = serde_json::json!({
            "type": "namespace",
            "name": "web",
            "padding": "x".repeat(MAX_CANONICAL_WEB_VALUE_WORK_BYTES),
        });

        assert!(is_forbidden_reserved_namespace_value(&value));
    }

    #[test]
    fn serialized_guard_rejects_deep_and_wide_malformed_web_history() {
        let mut deep = Value::Null;
        for _ in 0..=64 {
            deep = Value::Array(vec![deep]);
        }
        let deep = serde_json::json!({
            "type": "namespace",
            "name": "web",
            "padding": deep,
        });
        let wide = serde_json::json!({
            "type": "namespace",
            "name": "web",
            "padding": vec![Value::Null; MAX_CANONICAL_WEB_VALUE_WORK_BYTES],
        });

        assert!(is_forbidden_reserved_namespace_value(&deep));
        assert!(is_forbidden_reserved_namespace_value(&wide));
    }
}
