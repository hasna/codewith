//! Responses API tool definition for managing thread monitors.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const MANAGE_MONITOR_TOOL_NAME: &str = "manage_monitor";

pub fn create_manage_monitor_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::string_enum(
                vec![
                    json!("create"),
                    json!("list"),
                    json!("read"),
                    json!("stop"),
                    json!("restart"),
                    json!("delete"),
                ],
                Some(
                    "Required. Use create to start a new monitor, list to inspect monitors, read to inspect recent output, stop to halt a monitor, restart to rerun its command, and delete to remove it."
                        .to_string(),
                ),
            ),
        ),
        (
            "monitor_id".to_string(),
            JsonSchema::string(Some(
                "The monitor id for read, stop, restart, or delete. Omit only when exactly one monitor exists in this thread."
                    .to_string(),
            )),
        ),
        (
            "name".to_string(),
            JsonSchema::string(Some(
                "Short human-readable monitor name. Required for create.".to_string(),
            )),
        ),
        (
            "prompt".to_string(),
            JsonSchema::string(Some(
                "The user's natural-language monitor request or the purpose this monitor serves. Required for create."
                    .to_string(),
            )),
        ),
        (
            "command".to_string(),
            JsonSchema::string(Some(
                "Shell command or script to run as the monitor. Required for create. Design this dynamically from the user's request; do not choose from predefined monitor categories. Prefer commands that emit concise one-line stdout updates."
                    .to_string(),
            )),
        ),
        (
            "cwd".to_string(),
            JsonSchema::string(Some(
                "Optional working directory for the command. Relative output_file paths are resolved from this directory."
                    .to_string(),
            )),
        ),
        (
            "routing".to_string(),
            JsonSchema::string_enum(
                vec![json!("stream"), json!("file"), json!("both")],
                Some(
                    "Output routing. `stream` steers stdout into the active turn when one exists; `file` appends monitor output to output_file; `both` does both. Defaults to stream."
                        .to_string(),
                ),
            ),
        ),
        (
            "output_file".to_string(),
            JsonSchema::string(Some(
                "Required when routing is file or both. Absolute or cwd-relative file path where monitor output should be appended."
                    .to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(
                "Maximum number of events to return when action is read. Defaults to 20."
                    .to_string(),
            )),
        ),
        (
            "verbose".to_string(),
            JsonSchema::boolean(Some(
                "Optional. Defaults to false. When true, return full prompts, commands, paths, errors, and event text instead of compact previews."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: MANAGE_MONITOR_TOOL_NAME.to_string(),
        description: r#"Manage generic monitors for the current thread.
Use `create` after dynamically designing a monitor from the user's natural-language request. Do not use predefined monitor categories or hardcoded source types.
The monitor command is a model-designed shell command or script. It should emit concise single-line stdout updates when something relevant happens.
List and read responses are compact by default: they include ids, status, command/event previews, and counts. Set `verbose=true` only when full command text or full event output is needed.
`stream` routing attempts to steer stdout updates into the active turn and always records events. `file` routing appends output to `output_file`. `both` does both.
Use `list` before mutating when the user did not name a specific monitor.
`stop` halts the monitor process. `restart` reruns the monitor command. `delete` removes the selected monitor.
The tool is scoped to the current thread and rejects monitor ids from other threads."#
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
    fn manage_monitor_tool_exposes_expected_actions() {
        let ToolSpec::Function(tool) = create_manage_monitor_tool() else {
            panic!("manage_monitor should be a function tool");
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
                json!("create"),
                json!("list"),
                json!("read"),
                json!("stop"),
                json!("restart"),
                json!("delete"),
            ])
        );
        assert_eq!(tool.parameters.required, Some(vec!["action".to_string()]));
    }
}
