use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use std::collections::BTreeMap;

pub const RENAME_SESSION_TOOL_NAME: &str = "rename_session";

pub fn create_rename_session_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "name".to_string(),
            JsonSchema::string(Some(
                "Required. The new user-facing name for the current session/thread. Must be at most 120 characters."
                    .to_string(),
            )),
        ),
        (
            "overwrite_existing".to_string(),
            JsonSchema::boolean(Some(
                "Optional. Defaults to false. Set to true only when the user explicitly asked to rename an already named session."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: RENAME_SESSION_TOOL_NAME.to_string(),
        description: "Rename the current Codewith session/thread. Use this only when the user asks you to rename the current session, or when explicit product instructions ask you to set a session name. This affects only the current session. Do not casually rename a session, and do not overwrite an existing user-chosen name unless the user explicitly asked for a rename."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            /*required*/ Some(vec!["name".to_string()]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_session_tool_requires_name() {
        let ToolSpec::Function(tool) = create_rename_session_tool() else {
            panic!("rename_session should be a function tool");
        };

        assert_eq!(tool.name, RENAME_SESSION_TOOL_NAME);
        assert_eq!(tool.parameters.required, Some(vec!["name".to_string()]));
        assert!(
            tool.parameters
                .properties
                .as_ref()
                .is_some_and(|properties| properties.contains_key("overwrite_existing"))
        );
    }
}
