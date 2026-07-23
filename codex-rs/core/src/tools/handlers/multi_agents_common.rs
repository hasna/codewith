use crate::agent::AgentStatus;
use crate::config::Config;
use crate::config::DEFAULT_MULTI_AGENT_V2_MIN_WAIT_TIMEOUT_MS;
use crate::config::HARD_MAX_MULTI_AGENT_V2_TIMEOUT_MS;
use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::session::default_model_for_provider_id;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use codex_known_provider_models::fallback_models_for_provider;
use codex_known_provider_models::provider_for_fallback_model;
use codex_login::load_auth_profile;
use codex_login::validate_auth_profile_name;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_model_provider_info::model_gateway_for_provider;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErr;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::protocol::CollabAgentRef;
use codex_protocol::protocol::CollabAgentStatusEntry;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::user_input::UserInput;
use serde::Serialize;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Minimum wait timeout to prevent tight polling loops from burning CPU.
pub(crate) const MIN_WAIT_TIMEOUT_MS: i64 = DEFAULT_MULTI_AGENT_V2_MIN_WAIT_TIMEOUT_MS;
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: i64 = 30_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: i64 = HARD_MAX_MULTI_AGENT_V2_TIMEOUT_MS;

pub(crate) fn function_arguments(payload: ToolPayload) -> Result<String, FunctionCallError> {
    match payload {
        ToolPayload::Function { arguments } => Ok(arguments),
        _ => Err(FunctionCallError::RespondToModel(
            "collab handler received unsupported payload".to_string(),
        )),
    }
}

pub(crate) fn tool_output_json_text<T>(value: &T, tool_name: &str) -> String
where
    T: Serialize,
{
    serde_json::to_string(value).unwrap_or_else(|err| {
        JsonValue::String(format!("failed to serialize {tool_name} result: {err}")).to_string()
    })
}

pub(crate) fn tool_output_response_item<T>(
    call_id: &str,
    payload: &ToolPayload,
    value: &T,
    success: Option<bool>,
    tool_name: &str,
) -> ResponseInputItem
where
    T: Serialize,
{
    FunctionToolOutput::from_text(tool_output_json_text(value, tool_name), success)
        .to_response_item(call_id, payload)
}

pub(crate) fn tool_output_code_mode_result<T>(value: &T, tool_name: &str) -> JsonValue
where
    T: Serialize,
{
    serde_json::to_value(value).unwrap_or_else(|err| {
        JsonValue::String(format!("failed to serialize {tool_name} result: {err}"))
    })
}

pub(crate) fn build_wait_agent_statuses(
    statuses: &HashMap<ThreadId, AgentStatus>,
    receiver_agents: &[CollabAgentRef],
) -> Vec<CollabAgentStatusEntry> {
    if statuses.is_empty() {
        return Vec::new();
    }

    let mut entries = Vec::with_capacity(statuses.len());
    let mut seen = HashMap::with_capacity(receiver_agents.len());
    for receiver_agent in receiver_agents {
        seen.insert(receiver_agent.thread_id, ());
        if let Some(status) = statuses.get(&receiver_agent.thread_id) {
            entries.push(CollabAgentStatusEntry {
                thread_id: receiver_agent.thread_id,
                agent_nickname: receiver_agent.agent_nickname.clone(),
                agent_role: receiver_agent.agent_role.clone(),
                status: status.clone(),
            });
        }
    }

    let mut extras = statuses
        .iter()
        .filter(|(thread_id, _)| !seen.contains_key(thread_id))
        .map(|(thread_id, status)| CollabAgentStatusEntry {
            thread_id: *thread_id,
            agent_nickname: None,
            agent_role: None,
            status: status.clone(),
        })
        .collect::<Vec<_>>();
    extras.sort_by_key(|entry| entry.thread_id.to_string());
    entries.extend(extras);
    entries
}

pub(crate) fn collab_spawn_error(err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::UnsupportedOperation(message) if message == "thread manager dropped" => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        CodexErr::UnsupportedOperation(message) => FunctionCallError::RespondToModel(message),
        err => FunctionCallError::RespondToModel(format!("collab spawn failed: {err}")),
    }
}

pub(crate) fn collab_agent_error(agent_id: ThreadId, err: CodexErr) -> FunctionCallError {
    match err {
        CodexErr::ThreadNotFound(id) => {
            FunctionCallError::RespondToModel(format!("agent with id {id} not found"))
        }
        CodexErr::InternalAgentDied => {
            FunctionCallError::RespondToModel(format!("agent with id {agent_id} is closed"))
        }
        CodexErr::UnsupportedOperation(_) => {
            FunctionCallError::RespondToModel("collab manager unavailable".to_string())
        }
        // A rejected delivery (most commonly a full mailbox: "mailbox context
        // queue is full; limit is 8 messages") is recoverable — surface the
        // reason to the sending model so it can wait for the target to drain and
        // retry, rather than being told the send succeeded.
        CodexErr::InvalidRequest(message) => FunctionCallError::RespondToModel(message),
        err => FunctionCallError::RespondToModel(format!("collab tool failed: {err}")),
    }
}

pub(crate) fn thread_spawn_source(
    parent_thread_id: ThreadId,
    parent_session_source: &SessionSource,
    depth: i32,
    agent_role: Option<&str>,
    task_name: Option<String>,
) -> Result<SessionSource, FunctionCallError> {
    let agent_path = task_name
        .as_deref()
        .map(|task_name| {
            parent_session_source
                .get_agent_path()
                .unwrap_or_else(AgentPath::root)
                .join(task_name)
                .map_err(FunctionCallError::RespondToModel)
        })
        .transpose()?;
    Ok(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth,
        agent_path,
        agent_nickname: None,
        agent_role: agent_role.map(str::to_string),
    }))
}

pub(crate) fn parse_collab_input(
    message: Option<String>,
    items: Option<Vec<UserInput>>,
) -> Result<Op, FunctionCallError> {
    match (message, items) {
        (Some(_), Some(_)) => Err(FunctionCallError::RespondToModel(
            "Provide either message or items, but not both".to_string(),
        )),
        (None, None) => Err(FunctionCallError::RespondToModel(
            "Provide one of: message or items".to_string(),
        )),
        (Some(message), None) => {
            if message.trim().is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Empty message can't be sent to an agent".to_string(),
                ));
            }
            Ok(vec![UserInput::Text {
                text: message,
                text_elements: Vec::new(),
            }]
            .into())
        }
        (None, Some(items)) => {
            if items.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "Items can't be empty".to_string(),
                ));
            }
            Ok(items.into())
        }
    }
}

/// Builds the base config snapshot for a newly spawned sub-agent.
///
/// The returned config starts from the parent's effective config and then refreshes the
/// runtime-owned fields carried on `turn`, including model selection, reasoning settings,
/// approval policy, sandbox, and cwd. Role-specific overrides are layered after this step;
/// skipping this helper and cloning stale config state directly can send the child agent out with
/// the wrong provider or runtime policy.
pub(crate) fn build_agent_spawn_config(
    base_instructions: &BaseInstructions,
    turn: &TurnContext,
) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn)?;
    config.base_instructions = Some(base_instructions.text.clone());
    Ok(config)
}

pub(crate) fn build_agent_resume_config(turn: &TurnContext) -> Result<Config, FunctionCallError> {
    let mut config = build_agent_shared_config(turn)?;
    // For resume, keep base instructions sourced from rollout/session metadata.
    config.base_instructions = None;
    Ok(config)
}

fn build_agent_shared_config(turn: &TurnContext) -> Result<Config, FunctionCallError> {
    let base_config = turn.config.clone();
    let mut config = (*base_config).clone();
    config.model = Some(turn.model_info.slug.clone());
    config.model_provider = turn.provider.info().clone();
    config.model_reasoning_effort = turn
        .reasoning_effort
        .clone()
        .or_else(|| turn.model_info.default_reasoning_level.clone());
    config.model_reasoning_summary = Some(turn.reasoning_summary);
    config.developer_instructions = turn.developer_instructions.clone();
    config.compact_prompt = turn.compact_prompt.clone();
    apply_spawn_agent_runtime_overrides(&mut config, turn)?;

    Ok(config)
}

/// A full-history fork inherits the parent agent's type, model, and reasoning effort, so any
/// caller-supplied override for those fields cannot be honored.
///
/// Historically this combination was rejected outright, but that failed routed workers that fork
/// with full history while still passing `agent_type`/`model`/`reasoning_effort` (fields the model
/// naturally fills in). Because the fork path already ignores those overrides — the child config is
/// built from the parent — the reject was the *only* thing turning an otherwise-valid spawn into a
/// runtime failure. We now normalize instead: the inapplicable overrides are dropped and the fork
/// proceeds with the inherited values.
///
/// Returns a human-readable notice naming the ignored fields (for logging/telemetry), or `None`
/// when no inapplicable override was supplied.
pub(crate) fn full_fork_ignored_overrides_notice(
    agent_type: Option<&str>,
    model: Option<&str>,
    provider: Option<&str>,
    reasoning_effort: Option<&ReasoningEffort>,
) -> Option<String> {
    let mut ignored: Vec<&str> = Vec::new();
    if agent_type.is_some() {
        ignored.push("agent_type");
    }
    if model.is_some() {
        ignored.push("model");
    }
    if provider.is_some() {
        ignored.push("provider");
    }
    if reasoning_effort.is_some() {
        ignored.push("reasoning_effort");
    }
    if ignored.is_empty() {
        return None;
    }
    Some(format!(
        "Full-history forked agents inherit the parent agent type, model, provider, and reasoning effort; ignoring the supplied {} override(s). Spawn without a full-history fork to choose a different agent type, model, provider, or reasoning effort.",
        ignored.join(", ")
    ))
}

/// Copies runtime-only turn state onto a child config before it is handed to `AgentControl`.
///
/// These values are chosen by the live turn rather than persisted config, so leaving them stale
/// can make a child agent disagree with its parent about approval policy, cwd, or sandboxing.
pub(crate) fn apply_spawn_agent_runtime_overrides(
    config: &mut Config,
    turn: &TurnContext,
) -> Result<(), FunctionCallError> {
    config
        .permissions
        .approval_policy
        .set(turn.approval_policy.value())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("approval_policy is invalid: {err}"))
        })?;
    config.permissions.shell_environment_policy = turn.shell_environment_policy.clone();
    config.codex_linux_sandbox_exe = turn.codex_linux_sandbox_exe.clone();
    #[allow(deprecated)]
    let turn_cwd = turn.cwd.clone();
    config.cwd = turn_cwd;
    config
        .permissions
        .set_permission_profile(turn.permission_profile())
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("permission_profile is invalid: {err}"))
        })?;
    Ok(())
}

/// Applies caller-requested model, provider, and reasoning-effort overrides onto a child spawn
/// config so a sub-agent can run on any model of any provider configured for the session.
///
/// * The provider is `requested_provider` when set and configured, otherwise it is inferred from
///   the model slug (namespaced `provider/model` slugs and the known-provider catalog), and finally
///   falls back to the parent provider when no other configured provider claims the model. Routing
///   to a non-parent provider rewrites the child's `model_provider`, `model_provider_id`, and
///   `model_gateway_id` exactly like the main-session model switch; per-provider credentials come
///   from the shared auth manager (optionally scoped by `auth_profile`).
/// * The model is validated against the effective provider's catalog — the bundled OpenAI catalog
///   (models.json) plus the known-provider-models fallback catalog (Anthropic/Google/xAI/…). Unknown
///   models fail fast with the list of available models rather than silently falling back.
/// * The reasoning effort is validated against the selected model's supported reasoning levels.
///
/// Omitting all three overrides is a backward-compatible no-op that inherits the parent config.
pub(crate) async fn apply_requested_spawn_agent_model_overrides(
    session: &Session,
    turn: &TurnContext,
    config: &mut Config,
    requested_model: Option<&str>,
    requested_provider: Option<&str>,
    requested_reasoning_effort: Option<ReasoningEffort>,
) -> Result<(), FunctionCallError> {
    let requested_model = requested_model
        .map(str::trim)
        .filter(|model| !model.is_empty());
    let requested_provider = requested_provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty());

    if requested_model.is_none()
        && requested_provider.is_none()
        && requested_reasoning_effort.is_none()
    {
        return Ok(());
    }

    let parent_provider_id = config.model_provider_id.clone();
    if let Some(provider_id) =
        resolve_spawn_agent_provider_id(config, requested_model, requested_provider)?
        && provider_id != parent_provider_id
    {
        apply_spawn_agent_provider_routing(config, &provider_id)?;
    }
    let provider_changed = config.model_provider_id != parent_provider_id;

    // Effort-only override that keeps the inherited provider/model: validate against the parent
    // model and return without touching model selection.
    if requested_model.is_none() && !provider_changed {
        if let Some(reasoning_effort) = requested_reasoning_effort {
            validate_spawn_agent_reasoning_effort(
                &turn.model_info.slug,
                &turn.model_info.supported_reasoning_levels,
                &reasoning_effort,
            )?;
            config.model_reasoning_effort = Some(reasoning_effort);
        }
        return Ok(());
    }

    // Validate the model and read its metadata from the target provider's catalog. When the
    // provider is unchanged we reuse the session's default manager to preserve existing behavior;
    // a routed provider needs its own provider-scoped manager.
    let models_manager: SharedModelsManager = if provider_changed {
        session
            .models_manager_for_config_provider_id(config, /*model_provider_id*/ None)
            .await
    } else {
        session.services.models_manager.clone()
    };
    let models_manager_config = config.to_models_manager_config();

    let selected_model_name = match requested_model {
        Some(requested_model) => {
            let resolved_model =
                models_manager.resolve_model_for_auth(requested_model, &models_manager_config);
            resolve_spawn_agent_model_name(
                &models_manager,
                &config.model_provider_id,
                requested_model,
                &resolved_model,
            )
            .await?
        }
        // A bare provider override with no explicit model runs the provider's default model.
        None => default_model_for_provider_id(&config.model_provider_id)
            .map(str::to_string)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "spawn_agent could not determine a default model for provider `{}`",
                    config.model_provider_id
                ))
            })?,
    };

    let selected_model_info = models_manager
        .get_model_info(&selected_model_name, &models_manager_config)
        .await;
    config.model = Some(selected_model_name.clone());
    if let Some(reasoning_effort) = requested_reasoning_effort {
        validate_spawn_agent_reasoning_effort(
            &selected_model_name,
            &selected_model_info.supported_reasoning_levels,
            &reasoning_effort,
        )?;
        config.model_reasoning_effort = Some(reasoning_effort);
    } else {
        config.model_reasoning_effort = selected_model_info.default_reasoning_level;
    }

    Ok(())
}

/// Resolves which provider a spawned agent should run on.
///
/// Returns `Some(provider_id)` to route the child to a specific configured provider, or `None` to
/// inherit the parent provider. The inference mirrors the main-session model switch
/// (`request_processors::infer_model_provider_from_model` / `SessionConfiguration::apply`).
fn resolve_spawn_agent_provider_id(
    config: &Config,
    requested_model: Option<&str>,
    requested_provider: Option<&str>,
) -> Result<Option<String>, FunctionCallError> {
    if let Some(requested_provider) = requested_provider {
        if !config.model_providers.contains_key(requested_provider) {
            return Err(unknown_spawn_agent_provider_error(
                config,
                requested_provider,
            ));
        }
        return Ok(Some(requested_provider.to_string()));
    }

    let Some(model) = requested_model else {
        return Ok(None);
    };

    let current_provider = config.model_provider_id.as_str();
    // A model already served by the parent provider stays on it.
    if provider_for_fallback_model(model, [current_provider]).is_some() {
        return Ok(None);
    }
    // Namespaced slugs (e.g. `anthropic/claude-...`) route to the named provider when configured,
    // but never re-route an `openai/...` slug off a non-OpenAI parent.
    if let Some((candidate, _)) = model.split_once('/') {
        if current_provider != OPENAI_PROVIDER_ID && candidate == OPENAI_PROVIDER_ID {
            return Ok(None);
        }
        if config.model_providers.contains_key(candidate) {
            return Ok(Some(candidate.to_string()));
        }
    }
    // Otherwise route to the single configured provider whose catalog lists this model.
    Ok(
        provider_for_fallback_model(model, config.model_providers.keys().map(String::as_str))
            .map(str::to_string),
    )
}

/// Points a child spawn config at a different configured provider.
///
/// Rewrites the model provider, provider id, and gateway exactly like the main-session provider
/// switch (`apply_model_provider_id`) so the sub-agent authenticates and routes through the
/// requested provider. Per-provider credentials are resolved by the shared auth manager; the auth
/// profile is inherited unless the caller pins one via `auth_profile`.
fn apply_spawn_agent_provider_routing(
    config: &mut Config,
    provider_id: &str,
) -> Result<(), FunctionCallError> {
    let provider = config
        .model_providers
        .get(provider_id)
        .cloned()
        .ok_or_else(|| unknown_spawn_agent_provider_error(config, provider_id))?;
    config.model_gateway_id = model_gateway_for_provider(provider_id).to_string();
    config.model_provider_id = provider_id.to_string();
    config.model_provider = provider;
    Ok(())
}

fn unknown_spawn_agent_provider_error(config: &Config, provider_id: &str) -> FunctionCallError {
    let mut configured: Vec<&str> = config.model_providers.keys().map(String::as_str).collect();
    configured.sort_unstable();
    let configured = if configured.is_empty() {
        "none".to_string()
    } else {
        configured.join(", ")
    };
    FunctionCallError::RespondToModel(format!(
        "Unknown provider `{provider_id}` for spawn_agent. Configured providers: {configured}"
    ))
}

pub(crate) async fn apply_spawn_agent_service_tier(
    session: &Session,
    config: &mut Config,
    parent_service_tier: Option<&str>,
    requested_service_tier: Option<&str>,
) -> Result<(), FunctionCallError> {
    let candidate_service_tiers = [
        config.service_tier.clone(),
        requested_service_tier.map(str::to_string),
        parent_service_tier.map(str::to_string),
    ];
    if candidate_service_tiers.iter().all(Option::is_none) {
        config.service_tier = None;
        return Ok(());
    }

    let model = config.model.clone().ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "spawn_agent could not resolve the child model for service tier validation".to_string(),
        )
    })?;
    let model_info = session
        .services
        .models_manager
        .get_model_info(model.as_str(), &config.to_models_manager_config())
        .await;

    if let Some(requested_service_tier) = requested_service_tier
        && !model_info.supports_service_tier(requested_service_tier)
    {
        let supported_service_tiers = if model_info.service_tiers.is_empty() {
            "none".to_string()
        } else {
            model_info
                .service_tiers
                .iter()
                .map(|tier| tier.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(FunctionCallError::RespondToModel(format!(
            "Service tier `{requested_service_tier}` is not supported for model `{model}`. Supported service tiers: {supported_service_tiers}"
        )));
    }

    config.service_tier =
        candidate_service_tiers
            .into_iter()
            .flatten()
            .find(|candidate_service_tier| {
                model_info.supports_service_tier(candidate_service_tier.as_str())
            });
    Ok(())
}

/// Validates a requested named auth profile and pins it onto the child spawn config.
///
/// Validation happens before spawn so an unknown, malformed, or unusable profile fails fast with a
/// clear, name-only error. The check reuses the shared login profile helpers rather than
/// duplicating profile lookup. Only the caller-supplied profile label appears in errors; no
/// credential values, tokens, or auth-storage internals are surfaced.
pub(crate) fn apply_spawn_agent_auth_profile(
    config: &mut Config,
    requested_auth_profile: Option<&str>,
) -> Result<(), FunctionCallError> {
    let Some(requested) = requested_auth_profile
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    else {
        return Ok(());
    };

    validate_auth_profile_name(requested).map_err(|_| {
        FunctionCallError::RespondToModel(format!(
            "Auth profile `{requested}` is not a valid profile name. Use letters, numbers, dots, dashes, or underscores, and start with a letter or number."
        ))
    })?;

    let profile = load_auth_profile(
        &config.codex_home,
        config.cli_auth_credentials_store_mode,
        requested,
    )
        .map_err(|_| {
            FunctionCallError::RespondToModel(format!(
                "Auth profile `{requested}` was not found or is unavailable in this session. Create or select it with `codewith profile`, or omit auth_profile to inherit the current profile."
            ))
        })?;

    if profile.auth_mode.is_none() {
        return Err(FunctionCallError::RespondToModel(format!(
            "Auth profile `{requested}` has no usable credentials. Sign in with `codewith login --auth-profile {requested}`, or choose another profile."
        )));
    }

    config.selected_auth_profile = Some(requested.to_string());
    Ok(())
}

pub(crate) fn reject_forked_spawn_auth_profile(
    requested_auth_profile: Option<&str>,
    forked: bool,
) -> Result<(), FunctionCallError> {
    if !forked
        || requested_auth_profile
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
            .is_none()
    {
        return Ok(());
    }

    Err(FunctionCallError::RespondToModel(
        "auth_profile cannot be combined with forked conversation history. Use fork_context=false or fork_turns=\"none\" when spawning under a different auth profile."
            .to_string(),
    ))
}

/// Validates a requested model against the effective provider's catalog and returns the slug to run.
///
/// Validation is catalog-driven across every configured provider: the provider-scoped manager's
/// bundled/remote catalog (models.json for OpenAI-compatible providers) plus the
/// known-provider-models fallback catalog (Anthropic/Google/xAI/Cerebras/…), which is the only
/// source of truth for providers that ship no bundled catalog offline. Unknown models fail fast
/// rather than silently falling back, so the spawning agent can trust the requested config.
async fn resolve_spawn_agent_model_name(
    models_manager: &SharedModelsManager,
    provider_id: &str,
    requested_model: &str,
    resolved_model: &str,
) -> Result<String, FunctionCallError> {
    // The bare OpenAI flagship alias resolves to an auth-scoped slug (gpt-5.6-sol under ChatGPT
    // auth); trust `resolve_model_for_auth` for it rather than requiring a catalog hit.
    if requested_model == "gpt-5.6" && provider_id == OPENAI_PROVIDER_ID {
        return Ok(resolved_model.to_string());
    }

    let available_models = models_manager.list_models(RefreshStrategy::Offline).await;
    if available_models
        .iter()
        .any(|model| model.model == resolved_model)
    {
        return Ok(resolved_model.to_string());
    }

    // Providers without a bundled catalog (Anthropic/Google/xAI/…) are validated against the
    // known-provider-models fallback catalog instead.
    if fallback_models_for_provider(provider_id)
        .iter()
        .any(|model| model.id == resolved_model)
    {
        return Ok(resolved_model.to_string());
    }

    let mut available: Vec<String> = available_models
        .iter()
        .map(|model| model.model.clone())
        .chain(
            fallback_models_for_provider(provider_id)
                .iter()
                .map(|model| model.id.to_string()),
        )
        .collect();
    available.sort();
    available.dedup();
    let available = if available.is_empty() {
        "none".to_string()
    } else {
        available.join(", ")
    };
    Err(FunctionCallError::RespondToModel(format!(
        "Unknown model `{requested_model}` for spawn_agent on provider `{provider_id}`. Available models: {available}"
    )))
}

fn validate_spawn_agent_reasoning_effort(
    model: &str,
    supported_reasoning_levels: &[ReasoningEffortPreset],
    requested_reasoning_effort: &ReasoningEffort,
) -> Result<(), FunctionCallError> {
    if supported_reasoning_levels
        .iter()
        .any(|preset| &preset.effort == requested_reasoning_effort)
    {
        return Ok(());
    }

    let supported = supported_reasoning_levels
        .iter()
        .map(|preset| preset.effort.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(FunctionCallError::RespondToModel(format!(
        "Reasoning effort `{requested_reasoning_effort}` is not supported for model `{model}`. Supported reasoning efforts: {supported}"
    )))
}

#[cfg(test)]
mod auth_profile_tests {
    use super::*;
    use crate::config::ConfigBuilder;
    use codex_app_server_protocol::AuthMode;
    use codex_login::AuthDotJson;
    use codex_login::AuthProfileMetadata;
    use codex_login::save_auth_profile;
    use codex_login::save_auth_profile_metadata;
    use tempfile::TempDir;

    async fn test_config() -> (TempDir, Config) {
        let home = TempDir::new().expect("create temp dir");
        let config = ConfigBuilder::without_managed_config_for_tests()
            .codex_home(home.path().to_path_buf())
            .build()
            .await
            .expect("load default test config");
        (home, config)
    }

    const FAKE_API_KEY: &str = "fake-test-api-key-not-a-secret";

    fn api_key_auth() -> AuthDotJson {
        AuthDotJson {
            auth_mode: Some(AuthMode::ApiKey),
            openai_api_key: Some(FAKE_API_KEY.to_string()),
            tokens: None,
            last_refresh: None,
            agent_identity: None,
            personal_access_token: None,
        }
    }

    fn error_message(result: Result<(), FunctionCallError>) -> String {
        match result {
            Err(FunctionCallError::RespondToModel(message)) => message,
            other => panic!("expected RespondToModel error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn omitting_auth_profile_is_a_backward_compatible_noop() {
        let (_home, mut config) = test_config().await;
        config.selected_auth_profile = None;

        apply_spawn_agent_auth_profile(&mut config, None).expect("no-op");
        assert_eq!(config.selected_auth_profile, None);

        // Whitespace-only requests are treated as "not provided".
        apply_spawn_agent_auth_profile(&mut config, Some("   ")).expect("no-op");
        assert_eq!(config.selected_auth_profile, None);
    }

    #[tokio::test]
    async fn selects_a_known_usable_profile() {
        let (_home, mut config) = test_config().await;
        save_auth_profile(
            &config.codex_home,
            config.cli_auth_credentials_store_mode,
            "account001",
            &api_key_auth(),
        )
        .expect("save profile");

        apply_spawn_agent_auth_profile(&mut config, Some("account001")).expect("select profile");
        assert_eq!(config.selected_auth_profile.as_deref(), Some("account001"));
    }

    #[tokio::test]
    async fn unknown_profile_fails_before_spawn_without_leaking_secrets() {
        let (_home, mut config) = test_config().await;
        let original_auth_profile = config.selected_auth_profile.clone();
        save_auth_profile(
            &config.codex_home,
            config.cli_auth_credentials_store_mode,
            "account001",
            &api_key_auth(),
        )
        .expect("save profile");

        let message = error_message(apply_spawn_agent_auth_profile(
            &mut config,
            Some("missing_profile"),
        ));
        assert!(message.contains("missing_profile"), "message: {message}");
        assert!(message.contains("was not found"), "message: {message}");
        assert!(
            !message.contains(FAKE_API_KEY),
            "error must not leak credentials: {message}"
        );
        // A failed selection must not mutate the config.
        assert_eq!(config.selected_auth_profile, original_auth_profile);
    }

    #[tokio::test]
    async fn malformed_profile_name_is_rejected() {
        let (_home, mut config) = test_config().await;
        let original_auth_profile = config.selected_auth_profile.clone();

        let message = error_message(apply_spawn_agent_auth_profile(
            &mut config,
            Some("bad/name"),
        ));
        assert!(message.contains("bad/name"), "message: {message}");
        assert!(
            message.contains("not a valid profile name"),
            "message: {message}"
        );
        assert_eq!(config.selected_auth_profile, original_auth_profile);
    }

    #[tokio::test]
    async fn profile_without_usable_credentials_is_rejected() {
        let (_home, mut config) = test_config().await;
        let original_auth_profile = config.selected_auth_profile.clone();
        // Metadata-only profile: exists but has no loadable credentials.
        save_auth_profile_metadata(
            &config.codex_home,
            "account002",
            AuthProfileMetadata::default(),
        )
        .expect("save profile metadata");

        let message = error_message(apply_spawn_agent_auth_profile(
            &mut config,
            Some("account002"),
        ));
        assert!(message.contains("account002"), "message: {message}");
        assert!(
            message.contains("was not found") || message.contains("no usable credentials"),
            "message: {message}"
        );
        assert_eq!(config.selected_auth_profile, original_auth_profile);
    }

    #[test]
    fn forked_spawn_rejects_auth_profile_to_avoid_cross_profile_history_leaks() {
        let message = error_message(reject_forked_spawn_auth_profile(
            Some("account001"),
            /*forked*/ true,
        ));
        assert!(message.contains("auth_profile"), "message: {message}");
        assert!(message.contains("forked"), "message: {message}");
    }

    #[test]
    fn non_forked_spawn_allows_auth_profile_validation_to_continue() {
        reject_forked_spawn_auth_profile(Some("account001"), /*forked*/ false)
            .expect("non-forked auth-profile spawn should be allowed");
        reject_forked_spawn_auth_profile(None, /*forked*/ true)
            .expect("forking without auth_profile should be allowed");
        reject_forked_spawn_auth_profile(Some("   "), /*forked*/ true)
            .expect("blank auth_profile is treated as omitted");
    }
}

#[cfg(test)]
mod collab_error_tests {
    use super::*;

    #[test]
    fn full_mailbox_error_is_surfaced_to_the_sending_model() {
        // A rejected delivery (MailboxQueueFull) reaches the tool as
        // CodexErr::InvalidRequest; it must become a RespondToModel error so the
        // sender's model learns the message was dropped and can retry, rather than
        // being told the send succeeded.
        let err = collab_agent_error(
            ThreadId::new(),
            CodexErr::InvalidRequest(
                "mailbox context queue is full; limit is 8 messages".to_string(),
            ),
        );
        assert_eq!(
            err,
            FunctionCallError::RespondToModel(
                "mailbox context queue is full; limit is 8 messages".to_string()
            )
        );
    }
}
