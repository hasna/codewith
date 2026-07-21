use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::policy::VerifiedToolPolicy;
use crate::tools::registry::AnyToolResult;
use crate::tools::registry::ToolArgumentDiffConsumer;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec_plan::build_tool_router;
use codex_mcp::ToolInfo;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::SearchToolCallParams;
use codex_tools::DiscoverableTool;
use codex_tools::ToolCall as ExtensionToolCall;
use codex_tools::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

pub use crate::tools::context::ToolCallSource;

const MAX_INFINITY_AGENT_ARGUMENT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub tool_name: ToolName,
    pub call_id: String,
    pub payload: ToolPayload,
}

pub struct ToolRouter {
    registry: ToolRegistry,
    model_visible_specs: Vec<ToolSpec>,
    // Fail-closed policy-build error recorded by `from_policy_error` and surfaced
    // via the unit-tested `ensure_policy_ready` readiness gate. Production dispatch
    // fail-closes independently on a missing verified policy, so this is not yet
    // wired into the hot path.
    #[allow(dead_code)]
    policy_error: Option<String>,
    infinity_agent_policy: Option<Arc<VerifiedToolPolicy>>,
}

pub(crate) struct ToolRouterParams<'a> {
    pub(crate) mcp_tools: Option<Vec<ToolInfo>>,
    pub(crate) deferred_mcp_tools: Option<Vec<ToolInfo>>,
    pub(crate) discoverable_tools: Option<Vec<DiscoverableTool>>,
    pub(crate) extension_tool_executors: Vec<Arc<dyn ToolExecutor<ExtensionToolCall>>>,
    pub(crate) dynamic_tools: &'a [DynamicToolSpec],
}

impl ToolRouter {
    pub fn from_turn_context(turn_context: &TurnContext, params: ToolRouterParams<'_>) -> Self {
        build_tool_router(turn_context, params)
    }

    pub(crate) fn from_parts(registry: ToolRegistry, model_visible_specs: Vec<ToolSpec>) -> Self {
        Self {
            registry,
            model_visible_specs,
            policy_error: None,
            infinity_agent_policy: None,
        }
    }

    pub(crate) fn from_infinity_policy(
        registry: ToolRegistry,
        model_visible_specs: Vec<ToolSpec>,
        policy: Arc<VerifiedToolPolicy>,
    ) -> Self {
        Self {
            registry,
            model_visible_specs,
            policy_error: None,
            infinity_agent_policy: Some(policy),
        }
    }

    pub(crate) fn from_policy_error(error: impl Into<String>) -> Self {
        Self {
            registry: ToolRegistry::from_tools(std::iter::empty::<
                Arc<dyn crate::tools::registry::CoreToolRuntime>,
            >()),
            model_visible_specs: Vec::new(),
            policy_error: Some(error.into()),
            infinity_agent_policy: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn ensure_policy_ready(&self) -> Result<(), String> {
        if let Some(error) = &self.policy_error {
            return Err(error.clone());
        }
        if let Some(policy) = &self.infinity_agent_policy {
            policy
                .ensure_active(chrono::Utc::now())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn model_visible_specs(&self) -> Vec<ToolSpec> {
        self.model_visible_specs.clone()
    }

    #[cfg(test)]
    pub(crate) fn registered_tool_names_for_test(&self) -> Vec<ToolName> {
        self.registry.tool_names_for_test()
    }

    #[cfg(test)]
    pub(crate) fn tool_exposure_for_test(
        &self,
        name: &ToolName,
    ) -> Option<crate::tools::registry::ToolExposure> {
        self.registry.tool_exposure(name)
    }

    pub(crate) fn create_diff_consumer(
        &self,
        tool_name: &ToolName,
    ) -> Option<Box<dyn ToolArgumentDiffConsumer>> {
        self.registry.create_diff_consumer(tool_name)
    }

    pub fn tool_supports_parallel(&self, call: &ToolCall) -> bool {
        self.registry
            .supports_parallel_tool_calls(&call.tool_name)
            .unwrap_or(false)
    }

    pub fn tool_waits_for_runtime_cancellation(&self, call: &ToolCall) -> bool {
        self.registry
            .waits_for_runtime_cancellation(&call.tool_name)
            .unwrap_or(false)
    }

    #[instrument(level = "trace", skip_all, err(level = "debug"))]
    pub fn build_tool_call(item: ResponseItem) -> Result<Option<ToolCall>, FunctionCallError> {
        match item {
            ResponseItem::FunctionCall {
                name,
                namespace,
                arguments,
                call_id,
                ..
            } => {
                let tool_name = ToolName::new(namespace, name);
                Ok(Some(ToolCall {
                    tool_name,
                    call_id,
                    payload: ToolPayload::Function { arguments },
                }))
            }
            ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                execution,
                arguments,
                ..
            } if execution == "client" => {
                let arguments: SearchToolCallParams =
                    serde_json::from_value(arguments).map_err(|err| {
                        FunctionCallError::RespondToModel(format!(
                            "failed to parse tool_search arguments: {err}"
                        ))
                    })?;
                Ok(Some(ToolCall {
                    tool_name: ToolName::plain("tool_search"),
                    call_id,
                    payload: ToolPayload::ToolSearch { arguments },
                }))
            }
            ResponseItem::ToolSearchCall { .. } => Ok(None),
            ResponseItem::CustomToolCall {
                name,
                namespace,
                input,
                call_id,
                ..
            } => Ok(Some(ToolCall {
                tool_name: ToolName::new(namespace, name),
                call_id,
                payload: ToolPayload::Custom { input },
            })),
            _ => Ok(None),
        }
    }

    #[allow(dead_code)]
    #[instrument(level = "trace", skip_all, err(level = "debug"))]
    pub async fn dispatch_tool_call_with_code_mode_result(
        &self,
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        cancellation_token: CancellationToken,
        tracker: SharedTurnDiffTracker,
        call: ToolCall,
        source: ToolCallSource,
    ) -> Result<AnyToolResult, FunctionCallError> {
        self.dispatch_tool_call_with_code_mode_result_inner(
            session,
            turn,
            cancellation_token,
            tracker,
            call,
            source,
            /*terminal_outcome_reached*/ None,
        )
        .await
    }

    #[instrument(level = "trace", skip_all, err(level = "debug"))]
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn dispatch_tool_call_with_terminal_outcome(
        &self,
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        cancellation_token: CancellationToken,
        tracker: SharedTurnDiffTracker,
        call: ToolCall,
        source: ToolCallSource,
        terminal_outcome_reached: Arc<AtomicBool>,
    ) -> Result<AnyToolResult, FunctionCallError> {
        self.dispatch_tool_call_with_code_mode_result_inner(
            session,
            turn,
            cancellation_token,
            tracker,
            call,
            source,
            Some(terminal_outcome_reached),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_tool_call_with_code_mode_result_inner(
        &self,
        session: Arc<Session>,
        turn: Arc<TurnContext>,
        cancellation_token: CancellationToken,
        tracker: SharedTurnDiffTracker,
        call: ToolCall,
        source: ToolCallSource,
        terminal_outcome_reached: Option<Arc<AtomicBool>>,
    ) -> Result<AnyToolResult, FunctionCallError> {
        let ToolCall {
            tool_name,
            call_id,
            payload,
        } = call;

        if turn.config.is_infinity_agent() {
            let policy = self.infinity_agent_policy.as_ref().ok_or_else(|| {
                FunctionCallError::Fatal(
                    "Infinity Agent tool dispatch has no router-bound verified process policy"
                        .to_string(),
                )
            })?;
            let config_policy = turn.config.infinity_agent_policy.as_ref().ok_or_else(|| {
                FunctionCallError::Fatal(
                    "Infinity Agent turn has no verified process policy".to_string(),
                )
            })?;
            if policy.digest() != config_policy.digest() {
                return Err(FunctionCallError::Fatal(
                    "Infinity Agent router policy does not match the turn policy".to_string(),
                ));
            }
            policy
                .authorize_dispatch(&tool_name, chrono::Utc::now())
                .map_err(|error| {
                    FunctionCallError::Fatal(format!(
                        "Infinity Agent tool dispatch rejected before handler execution: {error}"
                    ))
                })?;
            let ToolPayload::Function { arguments } = &payload else {
                return Err(FunctionCallError::Fatal(
                    "Infinity Agent received a non-function tool payload".to_string(),
                ));
            };
            validate_infinity_agent_arguments(arguments)?;
        }

        let invocation = ToolInvocation {
            session,
            turn,
            cancellation_token,
            tracker,
            call_id,
            tool_name,
            source,
            payload,
        };

        self.registry
            .dispatch_any_with_terminal_outcome(invocation, terminal_outcome_reached)
            .await
    }
}

fn validate_infinity_agent_arguments(arguments: &str) -> Result<(), FunctionCallError> {
    if arguments.len() > MAX_INFINITY_AGENT_ARGUMENT_BYTES {
        return Err(FunctionCallError::RespondToModel(
            "Infinity Agent rejected oversized tool arguments before handler execution".to_string(),
        ));
    }
    codex_protocol::strict_json::validate_slice_no_duplicates(arguments.as_bytes()).map_err(
        |error| {
            FunctionCallError::RespondToModel(format!(
                "Infinity Agent rejected ambiguous tool arguments before handler execution: {error}"
            ))
        },
    )
}

pub(crate) fn extension_tool_executors(
    session: &Session,
) -> Vec<Arc<dyn ToolExecutor<ExtensionToolCall>>> {
    session
        .services
        .extensions
        .tool_contributors()
        .iter()
        .flat_map(|contributor| {
            contributor.tools(
                &session.services.session_extension_data,
                &session.services.thread_extension_data,
            )
        })
        .collect()
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
