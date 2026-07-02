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
                "Unix timestamp in seconds for the one-time run. Required for schedule type `once`."
                    .to_string(),
            )),
        ),
        (
            "expires_at".to_string(),
            JsonSchema::integer(Some(
                "Optional Unix timestamp in seconds when the schedule expires. One-time schedules do not expire by default."
                    .to_string(),
            )),
        ),
        (
            "verbose".to_string(),
            JsonSchema::boolean(Some(
                "Optional. Defaults to false. When true, return full prompts and lease timestamps instead of the compact summary."
                    .to_string(),
            )),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: MANAGE_SCHEDULE_TOOL_NAME.to_string(),
        description: r#"Manage scheduled prompts for the current thread.
Use `list` before mutating when the user did not name a specific schedule.
List and mutation responses are compact by default: they include ids, timing, status, prompt previews, and counts. Set `verbose=true` only when full prompts or lease details are needed.
`create` adds a one-time scheduled prompt. For requests such as "in 3 minutes", "tomorrow at 9", or "at 10:30", use schedule type `once` and set `next_run_at`.
Do not create recurring interval, cron, or dynamic schedules with this tool. Recurring work belongs in `/loop`.
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
        vec![JsonSchema::object(
            BTreeMap::from([(
                "type".to_string(),
                JsonSchema::string_enum(
                    vec![json!("once")],
                    Some(
                        "Run once at next_run_at. Use this for calendar-style schedules."
                            .to_string(),
                    ),
                ),
            )]),
            Some(vec!["type".to_string()]),
            Some(false.into()),
        )],
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
