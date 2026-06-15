use super::*;
use codex_app_server_protocol::ConfigEdit;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

impl App {
    pub(super) async fn add_mcp_server_from_spec(
        &mut self,
        app_server: &mut AppServerSession,
        spec: String,
    ) -> Result<(), String> {
        let parsed = match parse_mcp_add_spec(&spec) {
            Ok(parsed) => parsed,
            Err(err) => {
                self.chat_widget.add_error_message(err.clone());
                self.chat_widget.open_mcp_add_server();
                return Err(err);
            }
        };

        let edit = crate::config_update::replace_config_value(
            mcp_server_key_path(&parsed.name),
            parsed.config,
        );
        match write_mcp_config_edits(app_server, vec![edit]).await {
            Ok(()) => {
                self.refresh_in_memory_config_from_disk_best_effort("add MCP server")
                    .await;
                self.chat_widget.add_info_message(
                    format!("MCP server `{}` added.", parsed.name),
                    Some(
                        "Loaded threads pick up the new tools automatically before the next turn."
                            .to_string(),
                    ),
                );
                self.chat_widget
                    .open_mcp_manager(McpServerStatusDetail::Full);
                Ok(())
            }
            Err(err) => {
                let message = format!("Failed to add MCP server `{}`: {err}", parsed.name);
                self.chat_widget.add_error_message(message.clone());
                self.chat_widget.open_mcp_add_server();
                Err(message)
            }
        }
    }

    pub(super) async fn set_mcp_server_enabled(
        &mut self,
        app_server: &mut AppServerSession,
        name: String,
        enabled: bool,
    ) -> Result<(), String> {
        if !self.config.mcp_servers.get().contains_key(&name) {
            let message = format!("MCP server `{name}` is not directly configured in mcp_servers.");
            self.chat_widget.add_error_message(message.clone());
            return Err(message);
        }

        let edit = crate::config_update::replace_config_value(
            mcp_server_field_key_path(&name, "enabled"),
            serde_json::json!(enabled),
        );
        match write_mcp_config_edits(app_server, vec![edit]).await {
            Ok(()) => {
                self.refresh_in_memory_config_from_disk_best_effort("set MCP server enabled")
                    .await;
                self.chat_widget.add_info_message(
                    format!(
                        "MCP server `{name}` {}.",
                        if enabled { "enabled" } else { "disabled" }
                    ),
                    Some("MCP tools auto-refresh for loaded threads.".to_string()),
                );
                self.chat_widget
                    .open_mcp_manager(McpServerStatusDetail::Full);
                Ok(())
            }
            Err(err) => {
                let message = format!("Failed to update MCP server `{name}`: {err}");
                self.chat_widget.add_error_message(message.clone());
                Err(message)
            }
        }
    }

    pub(super) async fn set_mcp_tool_enabled(
        &mut self,
        app_server: &mut AppServerSession,
        server: String,
        tool: String,
        enabled: bool,
    ) -> Result<(), String> {
        let Some(config) = self.config.mcp_servers.get().get(&server) else {
            let message =
                format!("MCP server `{server}` is not directly configured in mcp_servers.");
            self.chat_widget.add_error_message(message.clone());
            return Err(message);
        };

        let edits = build_mcp_tool_enablement_edits(&server, &tool, enabled, config);
        match write_mcp_config_edits(app_server, edits).await {
            Ok(()) => {
                self.refresh_in_memory_config_from_disk_best_effort("set MCP tool enabled")
                    .await;
                self.chat_widget.add_info_message(
                    format!(
                        "MCP tool `{server}.{tool}` {}.",
                        if enabled { "enabled" } else { "disabled" }
                    ),
                    Some("MCP tools auto-refresh for loaded threads.".to_string()),
                );
                self.chat_widget
                    .open_mcp_manager(McpServerStatusDetail::Full);
                Ok(())
            }
            Err(err) => {
                let message = format!("Failed to update MCP tool `{server}.{tool}`: {err}");
                self.chat_widget.add_error_message(message.clone());
                Err(message)
            }
        }
    }
}

async fn write_mcp_config_edits(
    app_server: &AppServerSession,
    edits: Vec<ConfigEdit>,
) -> color_eyre::Result<()> {
    crate::config_update::write_config_batch(app_server.request_handle(), edits)
        .await
        .map(|_| ())
}

fn build_mcp_tool_enablement_edits(
    server: &str,
    tool: &str,
    enabled: bool,
    config: &codex_config::types::McpServerConfig,
) -> Vec<ConfigEdit> {
    let mut edits = Vec::new();
    if let Some(enabled_tools) = &config.enabled_tools {
        let mut enabled_tools = enabled_tools.clone();
        if enabled {
            if !enabled_tools.iter().any(|name| name == tool) {
                enabled_tools.push(tool.to_string());
                enabled_tools.sort();
            }
        } else {
            enabled_tools.retain(|name| name != tool);
        }
        edits.push(crate::config_update::replace_config_value(
            mcp_server_field_key_path(server, "enabled_tools"),
            serde_json::json!(enabled_tools),
        ));

        if enabled {
            let mut disabled_tools = config.disabled_tools.clone().unwrap_or_default();
            disabled_tools.retain(|name| name != tool);
            edits.push(disabled_tools_edit(server, disabled_tools));
        }
    } else {
        let mut disabled_tools = config.disabled_tools.clone().unwrap_or_default();
        if enabled {
            disabled_tools.retain(|name| name != tool);
        } else if !disabled_tools.iter().any(|name| name == tool) {
            disabled_tools.push(tool.to_string());
            disabled_tools.sort();
        }
        edits.push(disabled_tools_edit(server, disabled_tools));
    }
    edits
}

fn disabled_tools_edit(server: &str, disabled_tools: Vec<String>) -> ConfigEdit {
    let key_path = mcp_server_field_key_path(server, "disabled_tools");
    if disabled_tools.is_empty() {
        crate::config_update::clear_config_value(key_path)
    } else {
        crate::config_update::replace_config_value(key_path, serde_json::json!(disabled_tools))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedMcpAddSpec {
    name: String,
    config: JsonValue,
}

fn parse_mcp_add_spec(spec: &str) -> Result<ParsedMcpAddSpec, String> {
    let tokens = shlex::split(spec).ok_or_else(|| {
        "Could not parse MCP spec; check for unmatched quotes after `/mcp add`.".to_string()
    })?;
    if tokens.len() < 2 {
        return Err(mcp_add_usage());
    }

    let name = tokens[0].clone();
    validate_mcp_server_name(&name)?;
    let first = tokens[1].clone();
    let rest = &tokens[2..];

    if first.starts_with("http://") || first.starts_with("https://") {
        parse_http_mcp_add_spec(name, first, rest)
    } else {
        parse_stdio_mcp_add_spec(name, first, rest)
    }
}

fn parse_stdio_mcp_add_spec(
    name: String,
    command: String,
    tokens: &[String],
) -> Result<ParsedMcpAddSpec, String> {
    let mut args = Vec::new();
    let mut env = HashMap::new();
    let mut env_vars = Vec::new();
    let mut cwd = None;
    let mut index = 0;

    while index < tokens.len() {
        let token = &tokens[index];
        if let Some(value) = token.strip_prefix("--env=") {
            insert_key_value(&mut env, value, "--env")?;
        } else if token == "--env" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| "Missing KEY=VALUE after --env.".to_string())?;
            insert_key_value(&mut env, value, "--env")?;
        } else if let Some(value) = token.strip_prefix("--env-var=") {
            env_vars.push(non_empty_option_value(value, "--env-var")?.to_string());
        } else if token == "--env-var" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| "Missing KEY after --env-var.".to_string())?;
            env_vars.push(non_empty_option_value(value, "--env-var")?.to_string());
        } else if let Some(value) = token.strip_prefix("--cwd=") {
            cwd = Some(non_empty_option_value(value, "--cwd")?.to_string());
        } else if token == "--cwd" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| "Missing PATH after --cwd.".to_string())?;
            cwd = Some(non_empty_option_value(value, "--cwd")?.to_string());
        } else if token == "--bearer-env"
            || token.starts_with("--bearer-env=")
            || token == "--header"
            || token.starts_with("--header=")
            || token == "--env-header"
            || token.starts_with("--env-header=")
        {
            return Err(format!("{token} is only supported for HTTP MCP servers."));
        } else {
            args.push(token.clone());
        }
        index += 1;
    }

    let mut config = serde_json::json!({
        "command": command,
        "args": args,
        "enabled": true,
    });
    if !env.is_empty() {
        config["env"] = serde_json::json!(env);
    }
    if !env_vars.is_empty() {
        config["env_vars"] = serde_json::json!(env_vars);
    }
    if let Some(cwd) = cwd {
        config["cwd"] = serde_json::json!(cwd);
    }

    Ok(ParsedMcpAddSpec { name, config })
}

fn parse_http_mcp_add_spec(
    name: String,
    url: String,
    tokens: &[String],
) -> Result<ParsedMcpAddSpec, String> {
    let mut bearer_token_env_var = None;
    let mut http_headers = HashMap::new();
    let mut env_http_headers = HashMap::new();
    let mut index = 0;

    while index < tokens.len() {
        let token = &tokens[index];
        if let Some(value) = token.strip_prefix("--bearer-env=") {
            bearer_token_env_var = Some(non_empty_option_value(value, "--bearer-env")?.to_string());
        } else if token == "--bearer-env" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| "Missing KEY after --bearer-env.".to_string())?;
            bearer_token_env_var = Some(non_empty_option_value(value, "--bearer-env")?.to_string());
        } else if let Some(value) = token.strip_prefix("--header=") {
            insert_key_value(&mut http_headers, value, "--header")?;
        } else if token == "--header" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| "Missing NAME=VALUE after --header.".to_string())?;
            insert_key_value(&mut http_headers, value, "--header")?;
        } else if let Some(value) = token.strip_prefix("--env-header=") {
            insert_key_value(&mut env_http_headers, value, "--env-header")?;
        } else if token == "--env-header" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| "Missing NAME=ENV_VAR after --env-header.".to_string())?;
            insert_key_value(&mut env_http_headers, value, "--env-header")?;
        } else {
            return Err(format!("Unsupported HTTP MCP option `{token}`."));
        }
        index += 1;
    }

    let mut config = serde_json::json!({
        "url": url,
        "enabled": true,
    });
    if let Some(bearer_token_env_var) = bearer_token_env_var {
        config["bearer_token_env_var"] = serde_json::json!(bearer_token_env_var);
    }
    if !http_headers.is_empty() {
        config["http_headers"] = serde_json::json!(http_headers);
    }
    if !env_http_headers.is_empty() {
        config["env_http_headers"] = serde_json::json!(env_http_headers);
    }

    Ok(ParsedMcpAddSpec { name, config })
}

fn validate_mcp_server_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("MCP server name must not be empty.".to_string());
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return Err(
            "MCP server name may contain only letters, numbers, underscore, dash, or dot."
                .to_string(),
        );
    }
    Ok(())
}

fn insert_key_value(
    map: &mut HashMap<String, String>,
    value: &str,
    option: &str,
) -> Result<(), String> {
    let Some((key, value)) = value.split_once('=') else {
        return Err(format!("{option} expects KEY=VALUE."));
    };
    let key = non_empty_option_value(key, option)?;
    let value = non_empty_option_value(value, option)?;
    map.insert(key.to_string(), value.to_string());
    Ok(())
}

fn non_empty_option_value<'a>(value: &'a str, option: &str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{option} value must not be empty."))
    } else {
        Ok(value)
    }
}

fn mcp_add_usage() -> String {
    "Usage: /mcp add <name> <url-or-command...> [--env KEY=VALUE] [--env-var KEY] [--bearer-env KEY] [--cwd PATH]".to_string()
}

fn mcp_server_key_path(name: &str) -> String {
    format!("mcp_servers.{}", config_key_segment(name))
}

fn mcp_server_field_key_path(name: &str, field: &str) -> String {
    format!("{}.{}", mcp_server_key_path(name), field)
}

fn config_key_segment(segment: &str) -> String {
    JsonValue::String(segment.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::types::McpServerTransportConfig;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_stdio_mcp_add_spec() {
        let parsed = parse_mcp_add_spec(
            "docs npx -y @scope/docs-mcp --project docs --env API_KEY=secret --env-var HOME --cwd /tmp/docs",
        )
        .expect("valid stdio spec");

        assert_eq!(
            parsed,
            ParsedMcpAddSpec {
                name: "docs".to_string(),
                config: serde_json::json!({
                    "command": "npx",
                    "args": ["-y", "@scope/docs-mcp", "--project", "docs"],
                    "enabled": true,
                    "env": { "API_KEY": "secret" },
                    "env_vars": ["HOME"],
                    "cwd": "/tmp/docs",
                }),
            }
        );
    }

    #[test]
    fn parses_http_mcp_add_spec() {
        let parsed = parse_mcp_add_spec(
            "linear https://mcp.linear.app/mcp --bearer-env LINEAR_API_KEY --header X-Team=eng --env-header Authorization=LINEAR_AUTH",
        )
        .expect("valid http spec");

        assert_eq!(
            parsed,
            ParsedMcpAddSpec {
                name: "linear".to_string(),
                config: serde_json::json!({
                    "url": "https://mcp.linear.app/mcp",
                    "enabled": true,
                    "bearer_token_env_var": "LINEAR_API_KEY",
                    "http_headers": { "X-Team": "eng" },
                    "env_http_headers": { "Authorization": "LINEAR_AUTH" },
                }),
            }
        );
    }

    #[test]
    fn tool_enablement_updates_disabled_tools_by_default() {
        let config = codex_config::types::McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "npx".to_string(),
                args: Vec::new(),
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            environment_id: "local".to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: Some(vec!["old".to_string()]),
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        };

        let edits =
            build_mcp_tool_enablement_edits("docs", "search", /*enabled*/ false, &config);

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].key_path, "mcp_servers.\"docs\".disabled_tools");
        assert_eq!(edits[0].value, serde_json::json!(["old", "search"]));
    }

    #[test]
    fn tool_enablement_removes_disabled_tool() {
        let config = codex_config::types::McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: "npx".to_string(),
                args: Vec::new(),
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            environment_id: "local".to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: Some(vec!["search".to_string()]),
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        };

        let edits =
            build_mcp_tool_enablement_edits("docs", "search", /*enabled*/ true, &config);

        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].key_path, "mcp_servers.\"docs\".disabled_tools");
        assert_eq!(edits[0].value, serde_json::Value::Null);
    }
}
