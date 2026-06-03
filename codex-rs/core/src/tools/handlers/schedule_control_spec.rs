//! Responses API tool definition for managing thread schedules.

use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const MANAGE_SCHEDULE_TOOL_NAME: &str = "manage_schedule";

pub fn create_manage_schedule_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "action".to_string(),
            JsonSchema::string_enum(
                vec![
                    json!("create"),
                    json!("list"),
                    json!("update"),
                    json!("pause"),
                    json!("resume"),
                    json!("delete"),
                ],
                Some(
                    "Required. Use create to add a schedule, list to inspect schedules, update to change a schedule, pause/resume to toggle future runs, and delete to remove a schedule."
                        .to_string(),
                ),
            ),
        ),
        (
            "schedule_id".to_string(),
            JsonSchema::string(Some(
                "The schedule id for update, pause, resume, or delete. Omit only when exactly one non-expired schedule exists in this thread."
                    .to_string(),
            )),
        ),
        (
            "prompt".to_string(),
            JsonSchema::string(Some(
                "Prompt to run. Required for create and optional for update.".to_string(),
            )),
        ),
        (
            "schedule".to_string(),
            schedule_spec_schema(),
        ),
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
                "Optional Unix timestamp in seconds when the schedule expires. Defaults to seven days from creation."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: MANAGE_SCHEDULE_TOOL_NAME.to_string(),
        description: r#"Manage scheduled prompts for the current thread.
Use `list` before mutating when the user did not name a specific schedule.
`create` adds a scheduled prompt with an interval, cron expression, or dynamic one-minute cadence.
`pause` stops future scheduled runs without aborting an already running turn.
`delete` removes the selected schedule.
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

fn schedule_spec_schema() -> JsonSchema {
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
        Some("Schedule spec for create or update.".to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn manage_schedule_tool_exposes_expected_actions() {
        let ToolSpec::Function(tool) = create_manage_schedule_tool() else {
            panic!("manage_schedule should be a function tool");
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
                json!("update"),
                json!("pause"),
                json!("resume"),
                json!("delete"),
            ])
        );
        assert_eq!(tool.parameters.required, Some(vec!["action".to_string()]));
    }
}
