//! Responses API tool definition for session and auth-profile usage checks.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const GET_USAGE_TOOL_NAME: &str = "get_usage";

pub fn create_get_usage_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "scope".to_string(),
            JsonSchema::string_enum(
                vec![json!("session"), json!("account"), json!("both")],
                Some(
                    "Required. Use session for current conversation token usage, account for the selected auth profile's Codex account usage, and both for both surfaces."
                        .to_string(),
                ),
            ),
        ),
        (
            "auth_profile".to_string(),
            JsonSchema::string(Some(
                "Optional auth profile name for account usage. Omit to inspect the current session profile; pass an empty string to inspect the default root auth without switching."
                    .to_string(),
            )),
        ),
        (
            "include_token_profile".to_string(),
            JsonSchema::boolean(Some(
                "Set true to include backend token profile data when the selected account supports it. Usage checks never switch profiles by themselves."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: GET_USAGE_TOOL_NAME.to_string(),
        description: "Read current session usage and scoped Codewith auth-profile account usage. This tool is read-only, does not enumerate every saved profile, does not switch profiles, and never returns auth tokens or raw auth storage details."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["scope".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_usage_tool_requires_scope() {
        let ToolSpec::Function(tool) = create_get_usage_tool() else {
            panic!("get_usage should be a function tool");
        };

        assert_eq!(tool.name, GET_USAGE_TOOL_NAME);
        assert_eq!(tool.parameters.required, Some(vec!["scope".to_string()]));
        let properties = tool.parameters.properties.as_ref().expect("properties");
        assert!(properties.contains_key("auth_profile"));
        assert!(properties.contains_key("include_token_profile"));
        assert_eq!(
            properties
                .get("scope")
                .and_then(|scope| scope.enum_values.as_ref()),
            Some(&vec![json!("session"), json!("account"), json!("both")])
        );
    }
}
