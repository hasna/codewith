//! Responses API tool definition for auth profile management.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const MANAGE_AUTH_PROFILES_TOOL_NAME: &str = "manage_auth_profiles";

pub fn create_manage_auth_profiles_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::string_enum(
                vec![json!("list"), json!("current"), json!("switch")],
                Some(
                    "Required. Use list to inspect available auth profiles, current to inspect the active auth profile, and switch to request a profile switch through the session settings path."
                        .to_string(),
                ),
            ),
        ),
        (
            "profile".to_string(),
            JsonSchema::string(Some(
                "Profile name for switch. Omit or pass null to switch back to the default root auth profile."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: MANAGE_AUTH_PROFILES_TOOL_NAME.to_string(),
        description: "List Codewith auth profiles, inspect the current auth profile, or request a safe auth profile switch for the current session. Profile switches use the same session settings path as the /profile picker."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["action".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manage_auth_profiles_tool_requires_action() {
        let ToolSpec::Function(tool) = create_manage_auth_profiles_tool() else {
            panic!("manage_auth_profiles should be a function tool");
        };

        assert_eq!(tool.name, MANAGE_AUTH_PROFILES_TOOL_NAME);
        assert_eq!(tool.parameters.required, Some(vec!["action".to_string()]));
        assert!(
            tool.parameters
                .properties
                .as_ref()
                .is_some_and(|properties| properties.contains_key("profile"))
        );
        let action = tool
            .parameters
            .properties
            .as_ref()
            .and_then(|properties| properties.get("action"))
            .expect("action property");
        assert_eq!(
            action.enum_values.as_ref(),
            Some(&vec![json!("list"), json!("current"), json!("switch")])
        );
    }
}
