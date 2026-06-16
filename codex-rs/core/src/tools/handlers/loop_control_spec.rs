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
                vec![
                    json!("create"),
                    json!("list"),
                    json!("stop"),
                    json!("resume"),
                    json!("clear"),
                ],
                Some(
                    "Required. Use create to add a new /loop schedule, list to inspect current /loop schedules and run stats, stop to pause future runs, resume to reactivate a paused loop, and clear to delete a loop."
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
        (
            "prompt".to_string(),
            JsonSchema::string(Some(
                "Prompt to run. Required for create.".to_string(),
            )),
        ),
        ("schedule".to_string(), loop_schedule_spec_schema()),
        (
            "timezone".to_string(),
            JsonSchema::string(Some(
                "IANA timezone name such as UTC or America/Los_Angeles. Defaults to the local timezone."
                    .to_string(),
            )),
        ),
        (
            "next_run_at".to_string(),
            JsonSchema::integer(Some(
                "Optional Unix timestamp in seconds for the next run. Omit to calculate from the schedule."
                    .to_string(),
            )),
        ),
        (
            "expires_at".to_string(),
            JsonSchema::integer(Some(
                "Optional Unix timestamp in seconds when the loop expires. Defaults to seven days from creation."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: MANAGE_LOOP_TOOL_NAME.to_string(),
        description: r#"Manage recurring `/loop` schedules for the current thread.
Use `list` before mutating when the user did not name a specific loop, or when exact run counts are needed.
`create` adds a recurring prompt with an interval, cron expression, or dynamic one-minute cadence.
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

fn loop_schedule_spec_schema() -> JsonSchema {
    JsonSchema::any_of(
        vec![
            JsonSchema::object(
                BTreeMap::from([(
                    "type".to_string(),
                    JsonSchema::string_enum(
                        vec![json!("dynamic")],
                        Some("Use the default dynamic cadence.".to_string()),
                    ),
                )]),
                Some(vec!["type".to_string()]),
                Some(false.into()),
            ),
            JsonSchema::object(
                BTreeMap::from([
                    (
                        "type".to_string(),
                        JsonSchema::string_enum(
                            vec![json!("interval")],
                            Some("Use a fixed interval.".to_string()),
                        ),
                    ),
                    (
                        "amount".to_string(),
                        JsonSchema::integer(Some("Positive interval amount.".to_string())),
                    ),
                    (
                        "unit".to_string(),
                        JsonSchema::string_enum(
                            vec![json!("minutes"), json!("hours"), json!("days")],
                            Some("Interval unit.".to_string()),
                        ),
                    ),
                ]),
                Some(vec![
                    "type".to_string(),
                    "amount".to_string(),
                    "unit".to_string(),
                ]),
                Some(false.into()),
            ),
            JsonSchema::object(
                BTreeMap::from([
                    (
                        "type".to_string(),
                        JsonSchema::string_enum(
                            vec![json!("cron")],
                            Some("Use a five-field cron expression.".to_string()),
                        ),
                    ),
                    (
                        "expression".to_string(),
                        JsonSchema::string(Some(
                            "Standard five-field cron expression such as */5 * * * *.".to_string(),
                        )),
                    ),
                ]),
                Some(vec!["type".to_string(), "expression".to_string()]),
                Some(false.into()),
            ),
        ],
        Some("Loop schedule spec for create.".to_string()),
    )
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
                json!("create"),
                json!("list"),
                json!("stop"),
                json!("resume"),
                json!("clear"),
            ])
        );
        assert_eq!(tool.parameters.required, Some(vec!["action".to_string()]));
    }
}
