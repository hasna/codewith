use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value as JsonValue;

const CLAUDE_AGENT_SDK_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_BETAS",
    "ANTHROPIC_CUSTOM_HEADERS",
    "API_FORCE_IDLE_TIMEOUT",
    "API_TIMEOUT_MS",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH",
    "CLAUDE_CODE_SKIP_BEDROCK_AUTH",
    "CLAUDE_CODE_SKIP_FOUNDRY_AUTH",
    "CLAUDE_CODE_SKIP_MANTLE_AUTH",
    "CLAUDE_CODE_SKIP_VERTEX_AUTH",
    "DISABLE_PROMPT_CACHING",
    "ENABLE_PROMPT_CACHING_1H",
    "ENABLE_TOOL_SEARCH",
];

const CLAUDE_MODEL_CONFIG_ENV_VARS: &[&str] = &[
    "ANTHROPIC_CUSTOM_MODEL_OPTION",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
    "ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL_AWS_REGION",
    "CLAUDE_CODE_ALWAYS_ENABLE_EFFORT",
    "CLAUDE_CODE_DISABLE_1M_CONTEXT",
    "CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "MAX_THINKING_TOKENS",
];

const CLAUDE_MODEL_CONFIG_ENV_PREFIXES: &[&str] = &["ANTHROPIC_DEFAULT_", "VERTEX_REGION_CLAUDE_"];

const CLAUDE_AWS_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_AWS_BASE_URL",
    "ANTHROPIC_AWS_WORKSPACE_ID",
    "ANTHROPIC_BEDROCK_BASE_URL",
    "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
    "ANTHROPIC_BEDROCK_SERVICE_TIER",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_CONFIG_FILE",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_DEFAULT_REGION",
    "AWS_EC2_METADATA_DISABLED",
    "AWS_ENDPOINT_URL",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_ROLE_ARN",
    "AWS_ROLE_SESSION_NAME",
    "AWS_SDK_LOAD_CONFIG",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_STS_REGIONAL_ENDPOINTS",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
];

const CLAUDE_VERTEX_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_VERTEX_BASE_URL",
    "ANTHROPIC_VERTEX_PROJECT_ID",
    "CLOUD_ML_REGION",
    "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE",
    "CLOUDSDK_CONFIG",
    "CLOUDSDK_CORE_PROJECT",
    "GCLOUD_PROJECT",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_AUTH_SUPPRESS_CREDENTIALS_WARNINGS",
    "GOOGLE_CLOUD_PROJECT",
    "GOOGLE_CLOUD_QUOTA_PROJECT",
    "GOOGLE_PROJECT",
];

const CLAUDE_FOUNDRY_AUTH_ENV_VARS: &[&str] = &[
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "AZURE_AUTHORITY_HOST",
    "AZURE_CLIENT_CERTIFICATE_PATH",
    "AZURE_CLIENT_ID",
    "AZURE_CLIENT_SECRET",
    "AZURE_CONFIG_DIR",
    "AZURE_FEDERATED_TOKEN_FILE",
    "AZURE_PASSWORD",
    "AZURE_TENANT_ID",
    "AZURE_USERNAME",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentSdkAuthEnv {
    Ready,
    MissingCredentials(String),
    NotConfigured,
}

pub(crate) fn add_agent_sdk_auth_env(
    env: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
) {
    copy_env_vars(env, source_env, CLAUDE_AGENT_SDK_AUTH_ENV_VARS);
    copy_env_vars(env, source_env, CLAUDE_MODEL_CONFIG_ENV_VARS);
    copy_env_vars_with_prefix(env, source_env, CLAUDE_MODEL_CONFIG_ENV_PREFIXES);

    if bedrock_route_selected(source_env) {
        copy_env_vars(env, source_env, CLAUDE_AWS_AUTH_ENV_VARS);
    }
    if vertex_route_selected(source_env) {
        copy_env_vars(env, source_env, CLAUDE_VERTEX_AUTH_ENV_VARS);
        copy_env_vars_with_prefix(env, source_env, &["VERTEX_REGION_CLAUDE_"]);
    }
    if foundry_route_selected(source_env) {
        copy_env_vars(env, source_env, CLAUDE_FOUNDRY_AUTH_ENV_VARS);
    }
}

pub(crate) fn agent_sdk_auth_env(source_env: &BTreeMap<String, String>) -> AgentSdkAuthEnv {
    let settings = ClaudeSettingsSignals::from_source_env(source_env);
    let signals = ClaudeAuthSignals {
        source_env,
        settings,
    };
    if bedrock_signals_route_selected(&signals) {
        return bedrock_auth_env(&signals);
    }
    if vertex_signals_route_selected(&signals) {
        return vertex_auth_env(&signals);
    }
    if foundry_signals_route_selected(&signals) {
        return foundry_auth_env(&signals);
    }
    if signals.value_is_set("ANTHROPIC_API_KEY")
        || signals.value_is_set("ANTHROPIC_AUTH_TOKEN")
        || (signals.value_is_set("ANTHROPIC_BASE_URL")
            && signals.value_is_set("ANTHROPIC_CUSTOM_HEADERS"))
    {
        return AgentSdkAuthEnv::Ready;
    }
    AgentSdkAuthEnv::NotConfigured
}

struct ClaudeAuthSignals<'a> {
    source_env: &'a BTreeMap<String, String>,
    settings: ClaudeSettingsSignals,
}

impl ClaudeAuthSignals<'_> {
    fn value_is_set(&self, name: &str) -> bool {
        env_value_is_set(self.source_env, name) || self.settings.env_vars.contains(name)
    }

    fn flag_is_enabled(&self, name: &str) -> bool {
        env_flag_is_enabled(self.source_env, name) || self.settings.enabled_flags.contains(name)
    }

    fn setting_is_set(&self, name: &str) -> bool {
        self.settings.settings.contains(name)
    }

    fn any_value_is_set(&self, names: &[&str]) -> bool {
        names.iter().any(|name| self.value_is_set(name))
    }

    fn any_value_starts_with(&self, prefixes: &[&str]) -> bool {
        self.source_env.iter().any(|(name, value)| {
            prefixes.iter().any(|prefix| name.starts_with(prefix)) && !value.trim().is_empty()
        }) || self
            .settings
            .env_vars
            .iter()
            .any(|name| prefixes.iter().any(|prefix| name.starts_with(prefix)))
    }
}

#[derive(Default)]
struct ClaudeSettingsSignals {
    env_vars: BTreeSet<String>,
    enabled_flags: BTreeSet<String>,
    settings: BTreeSet<String>,
}

impl ClaudeSettingsSignals {
    fn from_source_env(source_env: &BTreeMap<String, String>) -> Self {
        let mut signals = Self::default();
        for path in claude_settings_paths(source_env) {
            signals.merge_settings_path(path);
        }
        signals
    }

    fn merge_settings_path(&mut self, path: PathBuf) {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(value) = serde_json::from_str::<JsonValue>(&contents) else {
            return;
        };
        self.merge_settings_value(&value);
    }

    fn merge_settings_value(&mut self, value: &JsonValue) {
        if let Some(env) = value.get("env").and_then(JsonValue::as_object) {
            for (name, value) in env {
                if json_env_value_is_set(value) {
                    self.env_vars.insert(name.clone());
                }
                if json_env_flag_is_enabled(value) {
                    self.enabled_flags.insert(name.clone());
                }
            }
        }
        for name in ["awsAuthRefresh", "awsCredentialExport", "gcpAuthRefresh"] {
            if value.get(name).is_some_and(json_setting_value_is_set) {
                self.settings.insert(name.to_string());
            }
        }
    }
}

fn claude_settings_paths(source_env: &BTreeMap<String, String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(config_dir) = source_env.get("CLAUDE_CONFIG_DIR")
        && !config_dir.trim().is_empty()
    {
        push_unique_path(&mut paths, PathBuf::from(config_dir).join("settings.json"));
    }
    for name in ["HOME", "USERPROFILE"] {
        if let Some(home) = source_env.get(name)
            && !home.trim().is_empty()
        {
            push_unique_path(
                &mut paths,
                PathBuf::from(home).join(".claude/settings.json"),
            );
        }
    }
    paths
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn json_env_value_is_set(value: &JsonValue) -> bool {
    match value {
        JsonValue::String(value) => !value.trim().is_empty(),
        JsonValue::Bool(_) | JsonValue::Number(_) => true,
        _ => false,
    }
}

fn json_env_flag_is_enabled(value: &JsonValue) -> bool {
    match value {
        JsonValue::String(value) => flag_value_is_enabled(value),
        JsonValue::Bool(value) => *value,
        JsonValue::Number(number) => number.as_i64().is_none_or(|value| value != 0),
        _ => false,
    }
}

fn json_setting_value_is_set(value: &JsonValue) -> bool {
    match value {
        JsonValue::String(value) => !value.trim().is_empty(),
        JsonValue::Bool(value) => *value,
        JsonValue::Number(number) => number.as_i64().is_none_or(|value| value != 0),
        _ => false,
    }
}

fn bedrock_route_selected(source_env: &BTreeMap<String, String>) -> bool {
    env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_BEDROCK")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_ANTHROPIC_AWS")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_MANTLE")
        || bedrock_auth_skipped(source_env)
}

fn vertex_route_selected(source_env: &BTreeMap<String, String>) -> bool {
    env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_VERTEX")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_VERTEX_AUTH")
}

fn foundry_route_selected(source_env: &BTreeMap<String, String>) -> bool {
    env_flag_is_enabled(source_env, "CLAUDE_CODE_USE_FOUNDRY")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_FOUNDRY_AUTH")
}

fn bedrock_signals_route_selected(signals: &ClaudeAuthSignals<'_>) -> bool {
    signals.flag_is_enabled("CLAUDE_CODE_USE_BEDROCK")
        || signals.flag_is_enabled("CLAUDE_CODE_USE_ANTHROPIC_AWS")
        || signals.flag_is_enabled("CLAUDE_CODE_USE_MANTLE")
        || bedrock_signals_auth_skipped(signals)
}

fn bedrock_auth_skipped(source_env: &BTreeMap<String, String>) -> bool {
    env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_BEDROCK_AUTH")
        || env_flag_is_enabled(source_env, "CLAUDE_CODE_SKIP_MANTLE_AUTH")
}

fn bedrock_signals_auth_skipped(signals: &ClaudeAuthSignals<'_>) -> bool {
    signals.flag_is_enabled("CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH")
        || signals.flag_is_enabled("CLAUDE_CODE_SKIP_BEDROCK_AUTH")
        || signals.flag_is_enabled("CLAUDE_CODE_SKIP_MANTLE_AUTH")
}

fn bedrock_auth_env(signals: &ClaudeAuthSignals<'_>) -> AgentSdkAuthEnv {
    if anthropic_aws_route_selected(signals)
        && !signals.value_is_set("ANTHROPIC_AWS_WORKSPACE_ID")
        && !signals.value_is_set("ANTHROPIC_AWS_API_KEY")
    {
        return AgentSdkAuthEnv::MissingCredentials(
            "Claude Platform on AWS routing needs ANTHROPIC_AWS_API_KEY or \
             ANTHROPIC_AWS_WORKSPACE_ID plus AWS credential-chain auth"
                .to_string(),
        );
    }
    if bedrock_signals_auth_skipped(signals)
        || signals.value_is_set("AWS_BEARER_TOKEN_BEDROCK")
        || signals.value_is_set("ANTHROPIC_AWS_API_KEY")
        || (signals.value_is_set("AWS_ACCESS_KEY_ID")
            && signals.value_is_set("AWS_SECRET_ACCESS_KEY"))
        || signals.any_value_is_set(&[
            "AWS_CONFIG_FILE",
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            "AWS_DEFAULT_REGION",
            "AWS_PROFILE",
            "AWS_REGION",
            "AWS_SHARED_CREDENTIALS_FILE",
            "AWS_WEB_IDENTITY_TOKEN_FILE",
        ])
        || signals.setting_is_set("awsAuthRefresh")
        || signals.setting_is_set("awsCredentialExport")
    {
        return AgentSdkAuthEnv::Ready;
    }
    AgentSdkAuthEnv::MissingCredentials(
        "Claude Code Bedrock/Claude Platform on AWS routing needs an AWS credential-chain signal \
         such as AWS_PROFILE, AWS_REGION/default IAM role credentials, awsAuthRefresh, \
         awsCredentialExport, AWS_BEARER_TOKEN_BEDROCK, ANTHROPIC_AWS_API_KEY, \
         AWS_ACCESS_KEY_ID with AWS_SECRET_ACCESS_KEY, or an explicit gateway skip flag"
            .to_string(),
    )
}

fn anthropic_aws_route_selected(signals: &ClaudeAuthSignals<'_>) -> bool {
    signals.flag_is_enabled("CLAUDE_CODE_USE_ANTHROPIC_AWS")
}

fn vertex_signals_route_selected(signals: &ClaudeAuthSignals<'_>) -> bool {
    signals.flag_is_enabled("CLAUDE_CODE_USE_VERTEX")
        || signals.flag_is_enabled("CLAUDE_CODE_SKIP_VERTEX_AUTH")
}

fn vertex_auth_env(signals: &ClaudeAuthSignals<'_>) -> AgentSdkAuthEnv {
    if signals.flag_is_enabled("CLAUDE_CODE_SKIP_VERTEX_AUTH")
        || signals.any_value_is_set(&[
            "ANTHROPIC_VERTEX_BASE_URL",
            "ANTHROPIC_VERTEX_PROJECT_ID",
            "CLOUD_ML_REGION",
            "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE",
            "CLOUDSDK_CONFIG",
            "CLOUDSDK_CORE_PROJECT",
            "GCLOUD_PROJECT",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_CLOUD_QUOTA_PROJECT",
            "GOOGLE_PROJECT",
        ])
        || signals.any_value_starts_with(&["VERTEX_REGION_CLAUDE_"])
        || signals.setting_is_set("gcpAuthRefresh")
    {
        return AgentSdkAuthEnv::Ready;
    }
    AgentSdkAuthEnv::MissingCredentials(
        "Claude Code Vertex routing needs an ADC/service-account/default-chain signal such as \
         GOOGLE_APPLICATION_CREDENTIALS, gcloud/CLOUDSDK config, ANTHROPIC_VERTEX_PROJECT_ID, \
         CLOUD_ML_REGION, gcpAuthRefresh, or CLAUDE_CODE_SKIP_VERTEX_AUTH for a gateway"
            .to_string(),
    )
}

fn foundry_signals_route_selected(signals: &ClaudeAuthSignals<'_>) -> bool {
    signals.flag_is_enabled("CLAUDE_CODE_USE_FOUNDRY")
        || signals.flag_is_enabled("CLAUDE_CODE_SKIP_FOUNDRY_AUTH")
}

fn foundry_auth_env(signals: &ClaudeAuthSignals<'_>) -> AgentSdkAuthEnv {
    if signals.flag_is_enabled("CLAUDE_CODE_SKIP_FOUNDRY_AUTH")
        || signals.value_is_set("ANTHROPIC_FOUNDRY_API_KEY")
        || signals.value_is_set("ANTHROPIC_FOUNDRY_RESOURCE")
        || signals.value_is_set("ANTHROPIC_FOUNDRY_BASE_URL")
    {
        return AgentSdkAuthEnv::Ready;
    }
    AgentSdkAuthEnv::MissingCredentials(
        "Claude Code Foundry routing needs ANTHROPIC_FOUNDRY_RESOURCE, \
         ANTHROPIC_FOUNDRY_BASE_URL, or ANTHROPIC_FOUNDRY_API_KEY so the runtime can use \
         API-key, Entra ID/default-chain, or gateway authentication"
            .to_string(),
    )
}

fn copy_env_vars(
    env: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
    names: &[&str],
) {
    for name in names {
        if let Some(value) = source_env.get(*name)
            && !value.trim().is_empty()
        {
            env.insert((*name).to_string(), value.clone());
        }
    }
}

fn copy_env_vars_with_prefix(
    env: &mut BTreeMap<String, String>,
    source_env: &BTreeMap<String, String>,
    prefixes: &[&str],
) {
    for (name, value) in source_env {
        if prefixes.iter().any(|prefix| name.starts_with(prefix)) && !value.trim().is_empty() {
            env.insert(name.clone(), value.clone());
        }
    }
}

fn env_value_is_set(source_env: &BTreeMap<String, String>, name: &str) -> bool {
    source_env
        .get(name)
        .is_some_and(|value| !value.trim().is_empty())
}

fn env_flag_is_enabled(source_env: &BTreeMap<String, String>, name: &str) -> bool {
    source_env
        .get(name)
        .is_some_and(|value| flag_value_is_enabled(value))
}

fn flag_value_is_enabled(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn env_from(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn auth_env_ready_for_direct_api_key_or_token() {
        assert_eq!(
            agent_sdk_auth_env(&env_from(&[("ANTHROPIC_API_KEY", "test-value")])),
            AgentSdkAuthEnv::Ready
        );
        assert_eq!(
            agent_sdk_auth_env(&env_from(&[("ANTHROPIC_AUTH_TOKEN", "test-value")])),
            AgentSdkAuthEnv::Ready
        );
    }

    #[test]
    fn auth_env_not_configured_without_indicators() {
        assert_eq!(
            agent_sdk_auth_env(&env_from(&[("PATH", "/bin")])),
            AgentSdkAuthEnv::NotConfigured
        );
    }

    #[test]
    fn auth_env_ready_for_bedrock_and_aws_provider_routes() {
        for env in [
            env_from(&[
                ("CLAUDE_CODE_USE_BEDROCK", "1"),
                ("AWS_BEARER_TOKEN_BEDROCK", "test-value"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_ANTHROPIC_AWS", "1"),
                ("ANTHROPIC_AWS_API_KEY", "test-value"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_ANTHROPIC_AWS", "1"),
                ("ANTHROPIC_AWS_WORKSPACE_ID", "workspace"),
                ("AWS_PROFILE", "dev"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_BEDROCK", "1"),
                ("AWS_ACCESS_KEY_ID", "test-value"),
                ("AWS_SECRET_ACCESS_KEY", "test-value"),
            ]),
            env_from(&[("CLAUDE_CODE_USE_BEDROCK", "1"), ("AWS_PROFILE", "dev")]),
            env_from(&[
                ("CLAUDE_CODE_USE_BEDROCK", "1"),
                ("AWS_REGION", "us-east-1"),
            ]),
            env_from(&[("CLAUDE_CODE_SKIP_BEDROCK_AUTH", "1")]),
        ] {
            assert_eq!(agent_sdk_auth_env(&env), AgentSdkAuthEnv::Ready);
        }
    }

    #[test]
    fn auth_env_gates_anthropic_aws_selector_without_workspace() {
        let outcome = agent_sdk_auth_env(&env_from(&[
            ("CLAUDE_CODE_USE_ANTHROPIC_AWS", "1"),
            ("AWS_REGION", "us-east-1"),
        ]));

        assert!(
            matches!(outcome, AgentSdkAuthEnv::MissingCredentials(_)),
            "Claude Platform on AWS selector without workspace should gate: {outcome:?}"
        );
    }

    #[test]
    fn auth_env_ready_for_bedrock_helper_settings() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            temp_dir.path().join("settings.json"),
            r#"{"awsAuthRefresh":"aws sso login --profile dev","env":{"CLAUDE_CODE_USE_BEDROCK":"1"}}"#,
        )
        .expect("write settings");
        let config_dir = temp_dir.path().to_string_lossy().into_owned();
        let env = env_from(&[("CLAUDE_CONFIG_DIR", config_dir.as_str())]);

        assert_eq!(agent_sdk_auth_env(&env), AgentSdkAuthEnv::Ready);
    }

    #[test]
    fn auth_env_gates_bedrock_selector_without_route_evidence() {
        let outcome = agent_sdk_auth_env(&env_from(&[("CLAUDE_CODE_USE_BEDROCK", "1")]));

        assert!(
            matches!(outcome, AgentSdkAuthEnv::MissingCredentials(_)),
            "Bedrock selector without auth evidence should gate: {outcome:?}"
        );
    }

    #[test]
    fn auth_env_ready_for_vertex_adc_service_account_and_gateway_modes() {
        for env in [
            env_from(&[
                ("CLAUDE_CODE_USE_VERTEX", "1"),
                ("GOOGLE_APPLICATION_CREDENTIALS", "/tmp/gcp.json"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_VERTEX", "1"),
                ("ANTHROPIC_VERTEX_PROJECT_ID", "project"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_VERTEX", "1"),
                ("VERTEX_REGION_CLAUDE_HAIKU_4_5", "us-east5"),
            ]),
            env_from(&[("CLAUDE_CODE_SKIP_VERTEX_AUTH", "1")]),
        ] {
            assert_eq!(agent_sdk_auth_env(&env), AgentSdkAuthEnv::Ready);
        }
    }

    #[test]
    fn auth_env_ready_for_vertex_gcp_refresh_setting() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(
            temp_dir.path().join("settings.json"),
            r#"{"gcpAuthRefresh":"gcloud auth application-default login","env":{"CLAUDE_CODE_USE_VERTEX":true}}"#,
        )
        .expect("write settings");
        let config_dir = temp_dir.path().to_string_lossy().into_owned();
        let env = env_from(&[("CLAUDE_CONFIG_DIR", config_dir.as_str())]);

        assert_eq!(agent_sdk_auth_env(&env), AgentSdkAuthEnv::Ready);
    }

    #[test]
    fn auth_env_gates_vertex_selector_without_route_evidence() {
        let outcome = agent_sdk_auth_env(&env_from(&[("CLAUDE_CODE_USE_VERTEX", "1")]));

        assert!(
            matches!(outcome, AgentSdkAuthEnv::MissingCredentials(_)),
            "Vertex selector without route evidence should gate: {outcome:?}"
        );
    }

    #[test]
    fn auth_env_ready_for_foundry_api_key_entra_and_base_url_modes() {
        for env in [
            env_from(&[
                ("CLAUDE_CODE_USE_FOUNDRY", "1"),
                ("ANTHROPIC_FOUNDRY_API_KEY", "test-value"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_FOUNDRY", "1"),
                ("ANTHROPIC_FOUNDRY_RESOURCE", "resource"),
            ]),
            env_from(&[
                ("CLAUDE_CODE_USE_FOUNDRY", "1"),
                (
                    "ANTHROPIC_FOUNDRY_BASE_URL",
                    "https://example.invalid/anthropic",
                ),
            ]),
            env_from(&[("CLAUDE_CODE_SKIP_FOUNDRY_AUTH", "1")]),
        ] {
            assert_eq!(agent_sdk_auth_env(&env), AgentSdkAuthEnv::Ready);
        }
    }

    #[test]
    fn auth_env_gates_foundry_selector_without_resource() {
        let outcome = agent_sdk_auth_env(&env_from(&[("CLAUDE_CODE_USE_FOUNDRY", "1")]));

        assert!(
            matches!(outcome, AgentSdkAuthEnv::MissingCredentials(_)),
            "Foundry selector without resource should gate: {outcome:?}"
        );
    }

    #[test]
    fn provider_selector_takes_precedence_over_direct_key() {
        let outcome = agent_sdk_auth_env(&env_from(&[
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("ANTHROPIC_API_KEY", "test-value"),
        ]));

        assert!(
            matches!(outcome, AgentSdkAuthEnv::MissingCredentials(_)),
            "selected provider without provider auth evidence should not fall back to the direct key: {outcome:?}"
        );
    }

    #[test]
    fn launch_env_copies_provider_and_model_config_without_unrelated_keys() {
        let source = env_from(&[
            ("ANTHROPIC_MODEL", "claude-sonnet-5"),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", "claude-sonnet-5"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("AWS_PROFILE", "dev"),
            ("AWS_REGION", "us-east-1"),
            ("CLAUDE_CODE_USE_VERTEX", "1"),
            ("GOOGLE_APPLICATION_CREDENTIALS", "/tmp/gcp.json"),
            ("CLAUDE_CODE_USE_FOUNDRY", "1"),
            ("ANTHROPIC_FOUNDRY_RESOURCE", "resource"),
            ("OPENAI_API_KEY", "test-value"),
        ]);
        let mut env = BTreeMap::new();

        add_agent_sdk_auth_env(&mut env, &source);

        assert_eq!(
            env,
            env_from(&[
                ("ANTHROPIC_DEFAULT_SONNET_MODEL", "claude-sonnet-5"),
                ("ANTHROPIC_FOUNDRY_RESOURCE", "resource"),
                ("ANTHROPIC_MODEL", "claude-sonnet-5"),
                ("AWS_PROFILE", "dev"),
                ("AWS_REGION", "us-east-1"),
                ("CLAUDE_CODE_USE_BEDROCK", "1"),
                ("CLAUDE_CODE_USE_FOUNDRY", "1"),
                ("CLAUDE_CODE_USE_VERTEX", "1"),
                ("GOOGLE_APPLICATION_CREDENTIALS", "/tmp/gcp.json"),
            ])
        );
    }
}
