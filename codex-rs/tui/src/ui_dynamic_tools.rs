use codex_app_server_protocol::DynamicToolSpec;
use serde_json::json;

pub(crate) const UI_TOOLS_NAMESPACE: &str = "codewith_ui";
pub(crate) const STATUSLINE_TOOL: &str = "configure_statusline";
pub(crate) const TERMINAL_TITLE_TOOL: &str = "configure_terminal_title";
pub(crate) const CONFIG_TOOL: &str = "configure_config";
pub(crate) const TMUX_TOOL: &str = "tmux";

pub(crate) fn dynamic_tool_specs() -> Vec<DynamicToolSpec> {
    vec![
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: STATUSLINE_TOOL.to_string(),
            description: "Inspect or update the Codewith TUI statusline. Use action=list_options before action=set to get valid item IDs without loading them into context upfront.".to_string(),
            input_schema: status_surface_schema("Ordered statusline item IDs for action=set."),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: TERMINAL_TITLE_TOOL.to_string(),
            description: "Inspect or update the Codewith terminal title items. Use action=list_options before action=set to get valid item IDs without loading them into context upfront.".to_string(),
            input_schema: status_surface_schema("Ordered terminal-title item IDs for action=set."),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: CONFIG_TOOL.to_string(),
            description: "Inspect or update the same curated config toggles shown by /config. Use action=list_options before action=set to get valid option IDs without exposing every config.toml key upfront.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "Use list_options to inspect current safe toggles; use set to apply one or more logical toggle states."
                    },
                    "updates": {
                        "type": "array",
                        "description": "Config toggle updates for action=set.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "option_id": {
                                    "type": "string",
                                    "description": "Option ID returned by list_options."
                                },
                                "enabled": {
                                    "type": "boolean",
                                    "description": "Logical enabled state to apply."
                                }
                            },
                            "required": ["option_id", "enabled"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: TMUX_TOOL.to_string(),
            description: "Move the current interactive Codewith session into tmux. Use only when the user explicitly asks to use tmux, move into tmux, reopen in tmux, or continue in tmux. Do not use proactively; explicit_user_request must be true.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "explicit_user_request": {
                        "type": "boolean",
                        "description": "Must be true, and only set true when the user explicitly asked to move this Codewith session into tmux."
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional tmux session and window name. Defaults to the current repo or directory name."
                    },
                    "replace": {
                        "type": "boolean",
                        "description": "Whether to replace an existing tmux session with the same name. Defaults to true for a seamless restart."
                    }
                },
                "required": ["explicit_user_request"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
    ]
}

pub(crate) fn is_owned_ui_tool(namespace: Option<&str>, tool: &str) -> bool {
    namespace == Some(UI_TOOLS_NAMESPACE)
        && matches!(
            tool,
            STATUSLINE_TOOL | TERMINAL_TITLE_TOOL | CONFIG_TOOL | TMUX_TOOL
        )
}

fn status_surface_schema(item_ids_description: &'static str) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "description": "Use list_options to inspect valid item IDs and current selection; use set to apply item_ids."
            },
            "item_ids": {
                "type": "array",
                "description": item_ids_description,
                "items": { "type": "string" }
            },
            "use_theme_colors": {
                "type": "boolean",
                "description": "Statusline only. Whether to use theme colors."
            }
        },
        "required": ["action"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_dynamic_tools_are_deferred_without_option_enums() {
        let specs = dynamic_tool_specs();
        assert_eq!(specs.len(), 4);
        assert!(specs.iter().all(|tool| tool.defer_loading));
        assert!(
            specs
                .iter()
                .all(|tool| tool.namespace.as_deref() == Some(UI_TOOLS_NAMESPACE))
        );

        let rendered_schema = specs
            .iter()
            .map(|tool| tool.input_schema.to_string())
            .collect::<String>();
        assert!(!rendered_schema.contains("model-with-reasoning"));
        assert!(!rendered_schema.contains("auth-profile-auto-switch"));

        let tmux = specs
            .iter()
            .find(|tool| tool.name == TMUX_TOOL)
            .expect("tmux tool spec");
        assert!(tmux.description.contains("explicitly asks"));
        assert_eq!(
            tmux.input_schema["required"],
            serde_json::json!(["explicit_user_request"])
        );
    }
}
