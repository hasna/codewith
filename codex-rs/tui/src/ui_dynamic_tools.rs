use codex_app_server_protocol::DynamicToolSpec;
use serde_json::json;

pub(crate) const UI_TOOLS_NAMESPACE: &str = "codewith_ui";
pub(crate) const STATUSLINE_TOOL: &str = "configure_statusline";
pub(crate) const TERMINAL_TITLE_TOOL: &str = "configure_terminal_title";
pub(crate) const CONFIG_TOOL: &str = "configure_config";
pub(crate) const TMUX_TOOL: &str = "tmux";
pub(crate) const BACKGROUND_TERMINALS_TOOL: &str = "manage_background_terminals";
pub(crate) const MCP_TOOL: &str = "manage_mcp";
pub(crate) const BACKGROUND_AGENTS_TOOL: &str = "manage_agents";
pub(crate) const ACTIVE_SESSIONS_TOOL: &str = "active_sessions";
pub(crate) const SCHEDULES_TOOL: &str = "manage_schedules";
pub(crate) const MONITORS_TOOL: &str = "manage_monitors";
pub(crate) const SESSION_CONTROL_TOOL: &str = "session_control";
pub(crate) const CAPABILITIES_TOOL: &str = "capabilities";

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
            description: "Inspect or update the same curated config toggles shown by /config, grouped by settings section. Use action=list_options before action=set to get valid section and option IDs without exposing every config.toml key upfront.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "Use list_options to inspect current safe toggles and sections; use set to apply one or more logical toggle states during this session."
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
                        "description": "Optional new tmux session and window name. Defaults to the current repo or directory name. Do not combine with session."
                    },
                    "session": {
                        "type": "string",
                        "description": "Existing tmux session to move this Codewith session into by creating a new window there. If omitted, a new tmux session is created automatically."
                    },
                    "window": {
                        "type": "string",
                        "description": "Optional name for the new tmux window created inside session. Requires session. Defaults to the current repo or directory name."
                    },
                    "replace": {
                        "type": "boolean",
                        "description": "Whether to replace an existing newly-created tmux session with the same name. Defaults to true for a seamless restart. Ignored when session targets an existing tmux session."
                    }
                },
                "required": ["explicit_user_request"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: BACKGROUND_TERMINALS_TOOL.to_string(),
            description: "Inspect background terminal processes that are still running in this Codewith session. action=stop_all only opens an interactive user confirmation popup; it does not stop processes directly.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: open, list, stop_all. stop_all opens a user confirmation popup."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: MCP_TOOL.to_string(),
            description: "Inspect MCP servers, reload loaded MCP connections, or request a safe persistent MCP config change. Persistent changes are approval-gated: the TUI shows the exact server name, transport, command/URL, env/header names, tool approval mode, and config scope before anything is saved.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: open, list, reload, add_stdio, add_streamable_http, set_server_enabled, set_tool_enabled. add/set actions require user approval before writing config."
                    },
                    "name": {
                        "type": "string",
                        "description": "MCP server name for add_stdio, add_streamable_http, or set_server_enabled."
                    },
                    "command": {
                        "type": "string",
                        "description": "Executable/path for action=add_stdio. Pass arguments separately in args; do not provide a shell command string."
                    },
                    "args": {
                        "type": "array",
                        "description": "Argv arguments for action=add_stdio.",
                        "items": { "type": "string" }
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Optional working directory for action=add_stdio."
                    },
                    "env_vars": {
                        "type": "array",
                        "description": "Environment variable names to pass through for action=add_stdio. Secret values are not accepted inline.",
                        "items": { "type": "string" }
                    },
                    "url": {
                        "type": "string",
                        "description": "HTTP or HTTPS URL for action=add_streamable_http. Do not include credentials or secret query parameters."
                    },
                    "bearer_token_env_var": {
                        "type": "string",
                        "description": "Environment variable name containing the bearer token for action=add_streamable_http."
                    },
                    "http_headers": {
                        "type": "object",
                        "description": "Non-secret inline HTTP headers for action=add_streamable_http.",
                        "additionalProperties": { "type": "string" }
                    },
                    "env_http_headers": {
                        "type": "object",
                        "description": "HTTP headers whose values come from environment variables, e.g. Authorization: MCP_TOKEN.",
                        "additionalProperties": { "type": "string" }
                    },
                    "enabled_tools": {
                        "type": "array",
                        "description": "Optional tool allow-list for new servers.",
                        "items": { "type": "string" }
                    },
                    "disabled_tools": {
                        "type": "array",
                        "description": "Optional tool deny-list for new servers.",
                        "items": { "type": "string" }
                    },
                    "default_tools_approval_mode": {
                        "type": "string",
                        "description": "Optional default tool approval mode for new servers: auto, prompt, or approve."
                    },
                    "server": {
                        "type": "string",
                        "description": "MCP server name for action=set_tool_enabled."
                    },
                    "tool": {
                        "type": "string",
                        "description": "MCP tool name for action=set_tool_enabled."
                    },
                    "enabled": {
                        "type": "boolean",
                        "description": "Desired enabled state for set_server_enabled or set_tool_enabled."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: BACKGROUND_AGENTS_TOOL.to_string(),
            description: "Open or inspect durable background agents owned by the current thread. Starting, attaching, detaching, stopping, deleting, or reading global diagnostics requires the interactive /agent UI.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: open, list, read, logs."
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "Required for action=read or action=logs. The agent must belong to the current thread."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: ACTIVE_SESSIONS_TOOL.to_string(),
            description: "List loaded active-session peers across the app-server and send active-only messages by peer id. Messages are delivered only to loaded peers; there is no durable offline queue.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: list, send."
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Opaque cursor returned by a previous action=list call."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Optional page size for action=list."
                    },
                    "target_peer_id": {
                        "type": "string",
                        "description": "Required for action=send. Use a peer_id returned by action=list."
                    },
                    "message": {
                        "type": "string",
                        "encrypted": true,
                        "description": "Required for action=send. Message text to deliver to the target peer."
                    },
                    "wake": {
                        "type": "boolean",
                        "description": "For action=send, true asks the target to wake and process the message. Defaults to false, which queues for the next mailbox drain."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: SCHEDULES_TOOL.to_string(),
            description: "Open or inspect one-time schedules and recurring /loop schedules for the current session. Creating, pausing, resuming, deleting, and running schedules require the interactive /schedule or /loop UI.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: open, list."
                    },
                    "kind": {
                        "type": "string",
                        "description": "For action=open/list: one of once, loop, all. Defaults to all for list and once for open."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: MONITORS_TOOL.to_string(),
            description: "Open, list, or read monitors for the current session. Stopping, restarting, or deleting a monitor requires the interactive /monitor UI.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: open, list, read."
                    },
                    "monitor_id": {
                        "type": "string",
                        "description": "Required for action=read."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: SESSION_CONTROL_TOOL.to_string(),
            description: "Control the current Codewith session: recap, compact, fork, or rename. These actions use the same app-owned controls as slash commands.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: recap, compact, fork, rename."
                    },
                    "prompt": {
                        "type": "string",
                        "description": "Optional recap prompt for action=recap."
                    },
                    "name": {
                        "type": "string",
                        "description": "New session name for action=rename."
                    }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some(UI_TOOLS_NAMESPACE.to_string()),
            name: CAPABILITIES_TOOL.to_string(),
            description: "Read the current Codewith autonomy and permission posture or propose permission/profile upgrades. This tool is read-only and never changes permissions by itself.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "One of: inspect, propose_upgrade."
                    }
                },
                "required": ["action"],
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
            STATUSLINE_TOOL
                | TERMINAL_TITLE_TOOL
                | CONFIG_TOOL
                | TMUX_TOOL
                | BACKGROUND_TERMINALS_TOOL
                | MCP_TOOL
                | BACKGROUND_AGENTS_TOOL
                | ACTIVE_SESSIONS_TOOL
                | SCHEDULES_TOOL
                | MONITORS_TOOL
                | SESSION_CONTROL_TOOL
                | CAPABILITIES_TOOL
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
        assert_eq!(specs.len(), 12);
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
        assert!(tmux.input_schema["properties"]["session"].is_object());
        assert!(tmux.input_schema["properties"]["window"].is_object());

        let active_sessions = specs
            .iter()
            .find(|tool| tool.name == ACTIVE_SESSIONS_TOOL)
            .expect("active sessions tool spec");
        assert!(
            active_sessions
                .description
                .contains("no durable offline queue")
        );
        assert_eq!(
            active_sessions.input_schema["properties"]["message"]["encrypted"],
            serde_json::json!(true)
        );

        let mcp = specs
            .iter()
            .find(|tool| tool.name == MCP_TOOL)
            .expect("mcp tool spec");
        assert!(mcp.description.contains("approval-gated"));
        let mcp_action_description = mcp.input_schema["properties"]["action"]["description"]
            .as_str()
            .expect("action description");
        assert!(mcp_action_description.contains("add_stdio"));
        assert!(mcp_action_description.contains("set_tool_enabled"));

        for tool in [
            BACKGROUND_TERMINALS_TOOL,
            MCP_TOOL,
            BACKGROUND_AGENTS_TOOL,
            ACTIVE_SESSIONS_TOOL,
            SCHEDULES_TOOL,
            MONITORS_TOOL,
            SESSION_CONTROL_TOOL,
            CAPABILITIES_TOOL,
        ] {
            assert!(is_owned_ui_tool(Some(UI_TOOLS_NAMESPACE), tool));
        }
    }
}
