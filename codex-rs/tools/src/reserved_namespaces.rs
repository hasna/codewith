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
//! fail serialization for any first-party or deferred namespace tool under a
//! reserved namespace it is not schema-compatible with.

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

/// Reserved namespaces that Codewith's own first-party tools are permitted to
/// reuse because the tool is deliberately schema-compatible with the built-in.
///
/// Currently this is only `web` (standalone web search advertises the built-in
/// `web.run` schema). Every other reserved namespace must never appear as a
/// first-party namespace tool — notably `image_gen`, which the standalone image
/// tool must not use (it lives under the non-reserved `images` namespace).
///
/// Invariant: if a new first-party tool must legitimately live under a reserved
/// namespace, add that namespace here in the same change that introduces it.
pub const FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES: &[&str] = &["web"];

/// Exact first-party tool names permitted under reserved Responses namespaces.
///
/// A namespace allowlist alone is insufficient because it would permit an
/// unrelated custom schema such as `web.imagegen`. Keep this list limited to
/// tool names whose owning extension advertises the provider's canonical
/// schema.
pub const FIRST_PARTY_ALLOWED_RESERVED_TOOLS: &[(&str, &str)] = &[("web", "run")];

/// True if `namespace` is reserved by the Responses API for a built-in tool.
pub fn is_reserved_responses_namespace(namespace: &str) -> bool {
    RESERVED_RESPONSES_NAMESPACES.contains(&namespace)
}

/// True if a custom namespace tool must not be serialized under `namespace`
/// because it is Responses-API-reserved and not on Codewith's vetted
/// allowlist.
///
/// This is the regression guard for the `image_gen.imagegen` 400: a
/// first-party / extension / code-mode tool must never be assembled under
/// `image_gen` (or any other reserved namespace except the allowlisted ones).
pub fn is_forbidden_first_party_namespace(namespace: &str) -> bool {
    is_reserved_responses_namespace(namespace)
        && !FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES.contains(&namespace)
}

/// True if a custom tool name is not explicitly vetted for a reserved
/// Responses namespace.
pub fn is_forbidden_first_party_namespace_tool(namespace: &str, tool_name: &str) -> bool {
    is_reserved_responses_namespace(namespace)
        && !FIRST_PARTY_ALLOWED_RESERVED_TOOLS.contains(&(namespace, tool_name))
}

/// True if a serialized namespace declaration contains an unvetted reserved
/// tool name.
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
    let Some(tools) = value.get("tools").and_then(Value::as_array) else {
        return true;
    };
    tools.is_empty()
        || tools.iter().any(|tool| {
            tool.get("type").and_then(Value::as_str) != Some("function")
                || tool
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none_or(|tool_name| {
                        is_forbidden_first_party_namespace_tool(namespace, tool_name)
                    })
        })
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
    fn web_is_reserved_but_allowed_for_first_party() {
        assert!(is_reserved_responses_namespace("web"));
        assert!(!is_forbidden_first_party_namespace("web"));
        assert!(!is_forbidden_first_party_namespace_tool("web", "run"));
        assert!(is_forbidden_first_party_namespace_tool("web", "imagegen"));
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
    fn every_allowlisted_namespace_is_reserved() {
        for namespace in FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES {
            assert!(
                is_reserved_responses_namespace(namespace),
                "allowlisting a non-reserved namespace `{namespace}` is meaningless"
            );
        }
    }

    #[test]
    fn every_allowlisted_tool_uses_an_allowlisted_reserved_namespace() {
        for (namespace, tool_name) in FIRST_PARTY_ALLOWED_RESERVED_TOOLS {
            assert!(
                FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES.contains(namespace),
                "allowlisted tool `{namespace}.{tool_name}` must use an allowlisted namespace"
            );
        }
    }

    #[test]
    fn serialized_guard_rejects_only_unvetted_reserved_namespace_tools() {
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
                "type": "namespace",
                "name": "web",
                "tools": [{"type": "function", "name": "run"}],
            }),
            serde_json::json!({"type": "function", "name": "image_gen"}),
        ] {
            assert!(!is_forbidden_reserved_namespace_value(&value), "{value}");
        }
    }
}
