use std::sync::Arc;
use std::sync::Weak;

use codex_protocol::items::TurnItem;
use codex_tools::ConversationHistory;
use codex_tools::ExtensionTurnItem;
use codex_tools::ToolCall as ExtensionToolCall;
use codex_tools::ToolName;
use codex_tools::ToolSearchInfo;
use codex_tools::ToolSpec;
use codex_tools::TurnItemEmissionFuture;
use codex_tools::TurnItemEmitter;

use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::stream_events_utils::TurnItemContributorPolicy;
use crate::stream_events_utils::finalize_turn_item;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

pub(crate) struct ExtensionToolAdapter {
    executor: Arc<dyn codex_tools::ToolExecutor<ExtensionToolCall>>,
    tool_name: ToolName,
    spec: ToolSpec,
    exposure: crate::tools::registry::ToolExposure,
    search_info: Option<ToolSearchInfo>,
    supports_parallel_tool_calls: bool,
}

impl ExtensionToolAdapter {
    pub(crate) fn new(executor: Arc<dyn codex_tools::ToolExecutor<ExtensionToolCall>>) -> Self {
        let tool_name = executor.tool_name();
        let spec = executor.spec();
        let exposure = executor.exposure();
        let search_info = if exposure == crate::tools::registry::ToolExposure::Deferred {
            executor
                .search_info()
                .filter(|search_info| search_info.is_valid_projection_for(&tool_name, &spec))
        } else {
            None
        };
        Self {
            tool_name,
            spec,
            exposure,
            search_info,
            supports_parallel_tool_calls: executor.supports_parallel_tool_calls(),
            executor,
        }
    }

    pub(crate) fn with_host_spec(mut self, host_spec: ToolSpec) -> Self {
        self.spec = host_spec;
        self
    }
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for ExtensionToolAdapter {
    fn tool_name(&self) -> ToolName {
        self.tool_name.clone()
    }

    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn exposure(&self) -> crate::tools::registry::ToolExposure {
        self.exposure
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.supports_parallel_tool_calls
    }

    fn search_info(&self) -> Option<ToolSearchInfo> {
        self.search_info.clone()
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        self.executor
            .handle(to_extension_call(&invocation).await)
            .await
    }
}

impl CoreToolRuntime for ExtensionToolAdapter {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

struct CoreTurnItemEmitter {
    session: Weak<Session>,
    turn: Weak<TurnContext>,
}

fn extension_turn_item(item: ExtensionTurnItem) -> TurnItem {
    match item {
        ExtensionTurnItem::WebSearch(item) => TurnItem::WebSearch(item),
        ExtensionTurnItem::ImageGeneration(mut item) => {
            item.saved_path = None;
            TurnItem::ImageGeneration(item)
        }
    }
}

impl TurnItemEmitter for CoreTurnItemEmitter {
    fn emit_started<'a>(&'a self, item: ExtensionTurnItem) -> TurnItemEmissionFuture<'a> {
        Box::pin(async move {
            let (Some(session), Some(turn)) = (self.session.upgrade(), self.turn.upgrade()) else {
                return;
            };
            session
                .emit_turn_item_started(turn.as_ref(), &extension_turn_item(item))
                .await;
        })
    }

    fn emit_completed<'a>(&'a self, item: ExtensionTurnItem) -> TurnItemEmissionFuture<'a> {
        Box::pin(async move {
            let (Some(session), Some(turn)) = (self.session.upgrade(), self.turn.upgrade()) else {
                return;
            };
            let mut item = extension_turn_item(item);
            finalize_turn_item(
                session.as_ref(),
                turn.as_ref(),
                TurnItemContributorPolicy::Run(turn.extension_data.as_ref()),
                &mut item,
                turn.collaboration_mode.mode == codex_protocol::config_types::ModeKind::Plan,
            )
            .await;
            session.emit_turn_item_completed(turn.as_ref(), item).await;
        })
    }
}

async fn to_extension_call(invocation: &ToolInvocation) -> ExtensionToolCall {
    let conversation_history =
        ConversationHistory::new(invocation.session.clone_history().await.into_raw_items());
    ExtensionToolCall {
        turn_id: invocation.turn.sub_id.clone(),
        call_id: invocation.call_id.clone(),
        tool_name: invocation.tool_name.clone(),
        model: invocation.turn.model_info.slug.clone(),
        truncation_policy: invocation.turn.truncation_policy,
        conversation_history,
        turn_item_emitter: Arc::new(CoreTurnItemEmitter {
            session: Arc::downgrade(&invocation.session),
            turn: Arc::downgrade(&invocation.turn),
        }),
        payload: invocation.payload.clone(),
    }
}

#[cfg(test)]
#[path = "extension_tools_tests.rs"]
mod tests;
