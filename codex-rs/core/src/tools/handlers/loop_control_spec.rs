//! Responses API tool definition for managing `/loop` schedules.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const MANAGE_LOOP_TOOL_NAME: &str = "manage_loop";

pub fn create_manage_loop_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::string_enum(
                vec![json!("list"), json!("stop"), json!("resume"), json!("clear")],
                Some(
                    "Required. Use list to inspect current /loop schedules, stop to pause future runs, resume to reactivate a paused loop, and clear to delete a loop."
                        .to_string(),
                ),
            ),
        ),
        (
            "schedule_id".to_string(),
            JsonSchema::string(Some(
                "The loop schedule id to stop, resume, or clear. Omit only when exactly one non-expired loop exists in this thread."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: MANAGE_LOOP_TOOL_NAME.to_string(),
        description: r#"Manage recurring `/loop` schedules for the current thread.
Use `list` before mutating when the user did not name a specific loop.
`stop` pauses future scheduled runs; it does not abort an already running turn.
`clear` deletes the selected loop schedule.
The tool is scoped to the current thread and rejects schedule ids from other threads."#
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
    use pretty_assertions::assert_eq;

    #[test]
    fn manage_loop_tool_exposes_expected_actions() {
        let ToolSpec::Function(tool) = create_manage_loop_tool() else {
            panic!("manage_loop should be a function tool");
        };
        let action = tool
            .parameters
            .properties
            .as_ref()
            .and_then(|properties| properties.get("action"))
            .expect("action property should exist");

        assert_eq!(
            action.enum_values,
            Some(vec![
                json!("list"),
                json!("stop"),
                json!("resume"),
                json!("clear"),
            ])
        );
        assert_eq!(tool.parameters.required, Some(vec!["action".to_string()]));
    }
}
