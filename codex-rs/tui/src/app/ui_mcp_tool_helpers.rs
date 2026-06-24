//! Validation, redaction, and display helpers for agent-requested MCP management.

use codex_config::types::AppToolApproval;
use codex_config::types::McpServerConfig;
use codex_config::types::McpServerEnvVar;
use codex_config::types::McpServerTransportConfig;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use url::Url;

#[cfg(test)]
use super::ui_mcp_tool::McpArgs;

pub(super) fn existing_server_approval_rows(
    name: &str,
    config: &McpServerConfig,
) -> Vec<(String, String)> {
    let mut rows = vec![("Server".to_string(), name.to_string())];
    rows.extend(existing_transport_approval_rows(&config.transport));
    rows.push((
        "Tool config".to_string(),
        format!(
            "default approval: {}; enabled: {}; disabled: {}",
            app_tool_approval_label(config.default_tools_approval_mode),
            list_or_none(config.enabled_tools.as_deref().unwrap_or_default()),
            list_or_none(config.disabled_tools.as_deref().unwrap_or_default())
        ),
    ));
    rows
}

fn existing_transport_approval_rows(transport: &McpServerTransportConfig) -> Vec<(String, String)> {
    match transport {
        McpServerTransportConfig::Stdio {
            command,
            args,
            env,
            env_vars,
            cwd,
        } => vec![
            (
                "Transport".to_string(),
                format!("stdio; command: {command}"),
            ),
            (
                "Args / cwd".to_string(),
                format!(
                    "args: {}; cwd: {}",
                    existing_arg_summary(args),
                    cwd.as_ref()
                        .map(|cwd| cwd.display().to_string())
                        .unwrap_or_else(|| "not set".to_string())
                ),
            ),
            (
                "Env / headers".to_string(),
                format!(
                    "inline env keys: {}; env vars: {}; headers: not applicable",
                    env.as_ref()
                        .map(|env| sorted_string_keys(env.keys()))
                        .map(|keys| list_or_none(&keys))
                        .unwrap_or_else(|| "none".to_string()),
                    list_or_none(&env_var_names(env_vars))
                ),
            ),
        ],
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => vec![
            (
                "Transport".to_string(),
                format!(
                    "streamable_http; url: {}",
                    redacted_url_for_existing_config(url)
                ),
            ),
            (
                "Auth / headers".to_string(),
                format!(
                    "bearer env: {}; inline headers: {}; env headers: {}",
                    optional_or_none(bearer_token_env_var.clone()),
                    http_headers
                        .as_ref()
                        .map(|headers| sorted_string_keys(headers.keys()))
                        .map(|keys| list_or_none(&keys))
                        .unwrap_or_else(|| "none".to_string()),
                    env_http_headers
                        .as_ref()
                        .map(|headers| {
                            headers
                                .iter()
                                .map(|(name, env_var)| format!("{name}=env:{env_var}"))
                                .collect::<Vec<_>>()
                        })
                        .map(|mut values| {
                            values.sort();
                            list_or_none(&values)
                        })
                        .unwrap_or_else(|| "none".to_string())
                ),
            ),
            (
                "Env / cwd".to_string(),
                "env vars: not applicable; cwd: not applicable".to_string(),
            ),
        ],
    }
}

fn existing_arg_summary(args: &[String]) -> String {
    match args.len() {
        0 => "none".to_string(),
        1 => "1 arg configured".to_string(),
        count => format!("{count} args configured"),
    }
}

fn env_var_names(env_vars: &[McpServerEnvVar]) -> Vec<String> {
    let mut names = env_vars
        .iter()
        .map(|env_var| env_var.name().to_string())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn sorted_string_keys<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut keys = keys.cloned().collect::<Vec<_>>();
    keys.sort();
    keys
}

pub(super) fn required_string(
    value: Option<String>,
    message: &'static str,
) -> Result<String, String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| message.to_string())
}

pub(super) fn validate_argv_command(command: &str) -> Result<(), String> {
    validate_no_control_chars(command, "command")?;
    if command.chars().any(char::is_whitespace) {
        return Err(
            "command must be one executable/path; pass command arguments in args instead"
                .to_string(),
        );
    }
    if command.contains('|')
        || command.contains(';')
        || command.contains('&')
        || command.contains('`')
        || command.contains("$(")
    {
        return Err("command must be argv-style, not a shell expression".to_string());
    }
    Ok(())
}

pub(super) fn normalize_args(args: Option<Vec<String>>) -> Result<Vec<String>, String> {
    let args = args.unwrap_or_default();
    for arg in &args {
        validate_stdio_arg(arg)?;
    }
    Ok(args)
}

fn validate_stdio_arg(arg: &str) -> Result<(), String> {
    validate_no_control_chars(arg, "arg")?;
    if let Some((name, value)) = arg.split_once('=') {
        let name = name.trim_start_matches('-');
        if is_secretish_name(name) || is_secretish_value(value) {
            return Err(
                "stdio args must not include inline secret-looking values; use env_vars instead"
                    .to_string(),
            );
        }
    }
    let lower = arg.trim().to_ascii_lowercase();
    if lower.starts_with("sk-") || lower.starts_with("bearer ") {
        return Err(
            "stdio args must not include inline secret-looking values; use env_vars instead"
                .to_string(),
        );
    }
    Ok(())
}

pub(super) fn normalize_env_var_names(values: Vec<String>) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let name = value.trim().to_string();
        validate_env_var_name(&name)?;
        if !seen.insert(name.clone()) {
            return Err(format!("duplicate env var name `{name}`"));
        }
        normalized.push(name);
    }
    Ok(normalized)
}

pub(super) fn validate_env_var_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("env var name must not be empty".to_string());
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err("env var name must not be empty".to_string());
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(format!(
            "env var name `{name}` must start with a letter or underscore"
        ));
    }
    if chars.any(|ch| !(ch == '_' || ch.is_ascii_alphanumeric())) {
        return Err(format!(
            "env var name `{name}` may contain only letters, numbers, and underscore"
        ));
    }
    Ok(())
}

pub(super) fn validate_tool_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("tool name must not be empty".to_string());
    }
    validate_no_control_chars(name, "tool name")
}

fn validate_no_control_chars(value: &str, label: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        Err(format!("{label} must not contain control characters"))
    } else {
        Ok(())
    }
}

pub(super) fn validate_mcp_http_url(value: &str) -> Result<(), String> {
    let url =
        Url::parse(value).map_err(|err| format!("invalid MCP server URL `{value}`: {err}"))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(format!(
                "MCP HTTP server URL must use http or https, got `{scheme}`"
            ));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("MCP HTTP server URL must not include inline credentials".to_string());
    }
    if url
        .query_pairs()
        .any(|(key, value)| is_secretish_name(&key) || is_secretish_value(&value))
    {
        return Err(
            "MCP HTTP server URL must not include secret-looking query parameters; use bearer_token_env_var or env_http_headers"
                .to_string(),
        );
    }
    Ok(())
}

fn redacted_url_for_existing_config(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "<configured invalid URL>".to_string();
    };
    let had_credentials = !url.username().is_empty() || url.password().is_some();
    if had_credentials {
        let _ = url.set_username("redacted");
        let _ = url.set_password(Some("redacted"));
    }
    let query_pairs = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    if !query_pairs.is_empty() {
        url.set_query(None);
        let mut query = url.query_pairs_mut();
        for (key, value) in query_pairs {
            if is_secretish_name(&key) || is_secretish_value(&value) {
                query.append_pair(&key, "redacted");
            } else {
                query.append_pair(&key, &value);
            }
        }
    }
    url.to_string()
}

pub(super) fn normalize_plain_http_headers(
    headers: HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut normalized = HashMap::new();
    for (name, value) in headers {
        let name = normalize_header_name(name)?;
        let value = required_string(Some(value), "HTTP header value must not be empty")?;
        validate_no_control_chars(&value, "HTTP header value")?;
        if is_secretish_name(&name) || is_secretish_value(&value) {
            return Err(format!(
                "HTTP header `{name}` looks secret-bearing; use env_http_headers or bearer_token_env_var instead"
            ));
        }
        normalized.insert(name, value);
    }
    Ok(normalized)
}

pub(super) fn normalize_env_http_headers(
    headers: HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    let mut normalized = HashMap::new();
    for (name, env_var) in headers {
        let name = normalize_header_name(name)?;
        let env_var = required_string(Some(env_var), "env HTTP header value must not be empty")?;
        validate_env_var_name(&env_var)?;
        normalized.insert(name, env_var);
    }
    Ok(normalized)
}

fn normalize_header_name(name: String) -> Result<String, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("HTTP header name must not be empty".to_string());
    }
    validate_no_control_chars(&name, "HTTP header name")?;
    if name.contains(':') {
        return Err(format!("HTTP header name `{name}` must not include `:`"));
    }
    Ok(name)
}

#[derive(Debug, Clone)]
pub(super) struct ToolOptions {
    pub(super) enabled_tools: Vec<String>,
    pub(super) disabled_tools: Vec<String>,
    pub(super) default_tools_approval_mode: Option<AppToolApproval>,
}

impl ToolOptions {
    pub(super) fn approval_label(&self) -> &'static str {
        app_tool_approval_label(self.default_tools_approval_mode)
    }
}

fn app_tool_approval_label(mode: Option<AppToolApproval>) -> &'static str {
    match mode {
        Some(AppToolApproval::Auto) => "auto",
        Some(AppToolApproval::Prompt) => "prompt",
        Some(AppToolApproval::Approve) => "approve",
        None => "not set",
    }
}

pub(super) fn normalize_tool_options(
    enabled_tools: Option<Vec<String>>,
    disabled_tools: Option<Vec<String>>,
    default_tools_approval_mode: Option<String>,
) -> Result<ToolOptions, String> {
    let enabled_tools = normalize_tool_list(enabled_tools.unwrap_or_default(), "enabled_tools")?;
    let disabled_tools = normalize_tool_list(disabled_tools.unwrap_or_default(), "disabled_tools")?;
    for tool in &enabled_tools {
        if disabled_tools.iter().any(|disabled| disabled == tool) {
            return Err(format!(
                "tool `{tool}` cannot be present in both enabled_tools and disabled_tools"
            ));
        }
    }
    let default_tools_approval_mode = default_tools_approval_mode
        .map(|value| parse_tool_approval_mode(&value))
        .transpose()?;
    Ok(ToolOptions {
        enabled_tools,
        disabled_tools,
        default_tools_approval_mode,
    })
}

fn normalize_tool_list(values: Vec<String>, label: &str) -> Result<Vec<String>, String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        validate_tool_name(&value)?;
        if !seen.insert(value.clone()) {
            return Err(format!("duplicate {label} entry `{value}`"));
        }
        normalized.push(value);
    }
    normalized.sort();
    Ok(normalized)
}

fn parse_tool_approval_mode(value: &str) -> Result<AppToolApproval, String> {
    match value.trim() {
        "auto" => Ok(AppToolApproval::Auto),
        "prompt" => Ok(AppToolApproval::Prompt),
        "approve" => Ok(AppToolApproval::Approve),
        value => Err(format!(
            "unknown default_tools_approval_mode `{value}`; expected auto, prompt, or approve"
        )),
    }
}

pub(super) fn apply_tool_options_to_config(
    config: &mut JsonValue,
    options: &ToolOptions,
) -> Result<(), String> {
    if !options.enabled_tools.is_empty() {
        config["enabled_tools"] = json!(options.enabled_tools);
    }
    if !options.disabled_tools.is_empty() {
        config["disabled_tools"] = json!(options.disabled_tools);
    }
    if let Some(mode) = options.default_tools_approval_mode {
        config["default_tools_approval_mode"] =
            serde_json::to_value(mode).map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn is_secretish_name(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value == "authorization"
        || value == "proxy-authorization"
        || value == "cookie"
        || value == "set-cookie"
        || value.contains("api-key")
        || value.contains("apikey")
        || value.contains("token")
        || value.contains("secret")
        || value.ends_with("-key")
}

fn is_secretish_value(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    value.starts_with("sk-")
        || lower.starts_with("bearer ")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("password")
        || (value.len() >= 32 && value.chars().all(|ch| ch.is_ascii_alphanumeric()))
}

pub(super) fn redacted_mcp_transport(transport: &McpServerTransportConfig) -> JsonValue {
    match transport {
        McpServerTransportConfig::Stdio {
            command,
            args,
            env,
            env_vars,
            cwd,
        } => json!({
            "type": "stdio",
            "command": command,
            "args_count": args.len(),
            "env": env
                .as_ref()
                .map(|env| redacted_keys(env.keys()))
                .unwrap_or_default(),
            "env_vars": env_vars
                .iter()
                .map(|env_var| env_var.name().to_string())
                .collect::<Vec<_>>(),
            "cwd_configured": cwd.is_some(),
        }),
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => json!({
            "type": "streamable_http",
            "url_configured": !url.is_empty(),
            "bearer_token_env_var": bearer_token_env_var,
            "http_headers": http_headers
                .as_ref()
                .map(|headers| redacted_keys(headers.keys()))
                .unwrap_or_default(),
            "env_http_headers": env_http_headers,
        }),
    }
}

fn redacted_keys<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<JsonValue> {
    let mut keys = keys.collect::<Vec<_>>();
    keys.sort();
    keys.into_iter()
        .map(|key| {
            json!({
                "name": key,
                "value": "<redacted>",
            })
        })
        .collect()
}

pub(super) fn http_header_summary(
    headers: &HashMap<String, String>,
    env_headers: &HashMap<String, String>,
) -> String {
    let mut pairs = headers
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>();
    pairs.extend(
        env_headers
            .iter()
            .map(|(name, env_var)| format!("{name}=env:{env_var}")),
    );
    pairs.sort();
    list_or_none(&pairs)
}

pub(super) fn list_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".to_string()
    } else {
        values.join(", ")
    }
}

pub(super) fn optional_or_none(value: Option<String>) -> String {
    value.unwrap_or_else(|| "none".to_string())
}

pub(super) fn mcp_server_scope_label() -> String {
    "user config.toml mcp_servers.<name>".to_string()
}

pub(super) fn refresh_label() -> String {
    "MCP refresh is queued for loaded threads; new tools are available before the next turn."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::types::McpServerEnvVar;
    use pretty_assertions::assert_eq;

    #[test]
    fn mcp_transport_summary_redacts_raw_secret_values() {
        let stdio = McpServerTransportConfig::Stdio {
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "server".to_string(),
                "--token=arg-secret".to_string(),
            ],
            env: Some(HashMap::from([(
                "API_KEY".to_string(),
                "sk-secret".to_string(),
            )])),
            env_vars: vec![McpServerEnvVar::Name("SAFE_ENV_NAME".to_string())],
            cwd: Some(std::path::PathBuf::from("/tmp/secret-project")),
        };
        let summary = redacted_mcp_transport(&stdio);
        assert_eq!(summary["args_count"], 3);
        assert_eq!(summary["env"][0]["name"], "API_KEY");
        assert_eq!(summary["env"][0]["value"], "<redacted>");
        assert_eq!(summary["env_vars"][0], "SAFE_ENV_NAME");
        assert_eq!(summary["cwd_configured"], true);
        let rendered = summary.to_string();
        assert!(!rendered.contains("sk-secret"));
        assert!(!rendered.contains("arg-secret"));
        assert!(!rendered.contains("secret-project"));

        let http = McpServerTransportConfig::StreamableHttp {
            url: "https://example.com/mcp?token=query-secret".to_string(),
            bearer_token_env_var: Some("MCP_TOKEN".to_string()),
            http_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "Bearer raw-secret".to_string(),
            )])),
            env_http_headers: Some(HashMap::from([(
                "X-Api-Key".to_string(),
                "MCP_API_KEY".to_string(),
            )])),
        };
        let summary = redacted_mcp_transport(&http);
        assert_eq!(summary["http_headers"][0]["name"], "Authorization");
        assert_eq!(summary["http_headers"][0]["value"], "<redacted>");
        assert_eq!(summary["url_configured"], true);
        assert_eq!(summary["bearer_token_env_var"], "MCP_TOKEN");
        let rendered = summary.to_string();
        assert!(!rendered.contains("raw-secret"));
        assert!(!rendered.contains("query-secret"));
    }

    #[test]
    fn rejects_inline_secret_http_inputs() {
        let err = serde_json::from_value::<McpArgs>(json!({
            "action": "add_streamable_http",
            "name": "docs",
            "url": "https://example.com/mcp",
            "bearer_token": "inline-secret",
        }))
        .expect_err("unknown inline bearer token field should fail");
        assert!(err.to_string().contains("unknown field"));

        let err = validate_mcp_http_url("https://example.com/mcp?token=abc")
            .expect_err("secret query params should fail");
        assert!(err.contains("secret-looking query"));

        let err = normalize_plain_http_headers(HashMap::from([(
            "Authorization".to_string(),
            "Bearer abc".to_string(),
        )]))
        .expect_err("plain auth header should fail");
        assert!(err.contains("env_http_headers"));

        let err = normalize_args(Some(vec!["--token=abc123".to_string()]))
            .expect_err("stdio token arg should fail");
        assert!(err.contains("env_vars"));
    }

    #[test]
    fn normalizes_tool_options() {
        let options = normalize_tool_options(
            Some(vec!["write".to_string(), "read".to_string()]),
            Some(vec!["delete".to_string()]),
            Some("prompt".to_string()),
        )
        .expect("valid tool options");

        assert_eq!(
            options.enabled_tools,
            vec!["read".to_string(), "write".to_string()]
        );
        assert_eq!(options.disabled_tools, vec!["delete".to_string()]);
        assert_eq!(
            options.default_tools_approval_mode,
            Some(AppToolApproval::Prompt)
        );
    }

    #[test]
    fn existing_server_approval_rows_redact_secret_config_values() {
        let config = McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: "https://user:pass@example.com/mcp?token=query-secret&team=eng".to_string(),
                bearer_token_env_var: Some("DOCS_TOKEN".to_string()),
                http_headers: Some(HashMap::from([(
                    "Authorization".to_string(),
                    "Bearer raw-secret".to_string(),
                )])),
                env_http_headers: Some(HashMap::from([(
                    "X-Api-Key".to_string(),
                    "DOCS_API_KEY".to_string(),
                )])),
            },
            environment_id: "local".to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: Some(AppToolApproval::Prompt),
            enabled_tools: Some(vec!["search".to_string()]),
            disabled_tools: Some(vec!["delete".to_string()]),
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: HashMap::new(),
        };

        let rendered = existing_server_approval_rows("docs", &config)
            .into_iter()
            .map(|(label, value)| format!("{label}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("https://redacted:redacted@example.com/mcp"));
        assert!(rendered.contains("token=redacted"));
        assert!(rendered.contains("team=eng"));
        assert!(rendered.contains("inline headers: Authorization"));
        assert!(rendered.contains("env headers: X-Api-Key=env:DOCS_API_KEY"));
        assert!(rendered.contains("default approval: prompt"));
        assert!(!rendered.contains("query-secret"));
        assert!(!rendered.contains("raw-secret"));
    }
}
