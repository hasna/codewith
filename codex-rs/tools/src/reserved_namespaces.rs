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
//! guard the first-party assembled tool list so a Codewith tool can never be
//! shipped under a reserved namespace it is not schema-compatible with.

/// Namespaces reserved by the OpenAI Responses API for its built-in tools.
///
/// Keep this list sorted. It is the single source of truth consumed by the
/// dynamic-tool validator, the `multi_agent_v2` namespace validator, and the
/// first-party tool-assembly guard.
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

/// True if `namespace` is reserved by the Responses API for a built-in tool.
pub fn is_reserved_responses_namespace(namespace: &str) -> bool {
    RESERVED_RESPONSES_NAMESPACES.contains(&namespace)
}

/// True if a *first-party* namespace tool must not be assembled under
/// `namespace` because it is Responses-API-reserved and not on Codewith's
/// vetted allowlist.
///
/// This is the regression guard for the `image_gen.imagegen` 400: a
/// first-party / extension / code-mode tool must never be assembled under
/// `image_gen` (or any other reserved namespace except the allowlisted ones).
pub fn is_forbidden_first_party_namespace(namespace: &str) -> bool {
    is_reserved_responses_namespace(namespace)
        && !FIRST_PARTY_ALLOWED_RESERVED_NAMESPACES.contains(&namespace)
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
}
