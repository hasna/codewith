use std::sync::Arc;

use crate::Prompt;
use crate::client::CompactConversationRequestSettings;
use crate::compact::CompactionAnalyticsAttempt;
use crate::compact::InitialContextInjection;
use crate::compact::compaction_status_from_result;
use crate::compact::insert_initial_context_before_last_real_user_or_summary;
use crate::compact_model_fallback::record_model_fallback;
use crate::compact_model_fallback::should_retry_with_current_model;
use crate::context_manager::ContextManager;
use crate::context_manager::TotalTokenUsageBreakdown;
use crate::context_manager::estimate_response_item_model_visible_bytes;
use crate::hook_runtime::PostCompactHookOutcome;
use crate::hook_runtime::PreCompactHookOutcome;
use crate::hook_runtime::run_post_compact_hooks;
use crate::hook_runtime::run_pre_compact_hooks;
use crate::remote_compaction_budget::RemoteCompactionRequestBudget;
use crate::session::session::Session;
use crate::session::turn::built_tools;
use crate::session::turn_context::TurnContext;
use crate::turn_metadata::CompactionTurnMetadata;
use codex_analytics::CompactionImplementation;
use codex_analytics::CompactionPhase;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionTrigger;
use codex_app_server_protocol::AuthMode;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::items::ContextCompactionItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::CompactedItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnStartedEvent;
use codex_rollout_trace::CompactionCheckpointTracePayload;
use codex_rollout_trace::CompactionTraceContext;
use codex_tools::ToolSpec;
use codex_utils_output_truncation::approx_token_count;
use codex_utils_output_truncation::approx_tokens_from_byte_count_i64;
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::info;

const CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE: &str =
    "Output exceeded the available model context and was truncated";

pub(crate) async fn run_inline_remote_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    fallback_turn_context: Option<Arc<TurnContext>>,
    initial_context_injection: InitialContextInjection,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()> {
    let compaction_metadata = CompactionTurnMetadata::new(
        CompactionTrigger::Auto,
        reason,
        CompactionImplementation::ResponsesCompact,
        phase,
    );
    run_remote_compact_task_inner(
        &sess,
        &turn_context,
        fallback_turn_context.as_ref(),
        initial_context_injection,
        compaction_metadata,
    )
    .await?;
    Ok(())
}

pub(crate) async fn run_remote_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
) -> CodexResult<()> {
    let start_event = EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_context.sub_id.clone(),
        trace_id: turn_context.trace_id.clone(),
        started_at: turn_context.turn_timing_state.started_at_unix_secs().await,
        model_context_window: turn_context.model_context_window(),
        collaboration_mode_kind: turn_context.collaboration_mode.mode,
    });
    sess.send_event(&turn_context, start_event).await;

    let compaction_metadata = CompactionTurnMetadata::new(
        CompactionTrigger::Manual,
        CompactionReason::UserRequested,
        CompactionImplementation::ResponsesCompact,
        CompactionPhase::StandaloneTurn,
    );
    run_remote_compact_task_inner(
        &sess,
        &turn_context,
        /*fallback_turn_context*/ None,
        InitialContextInjection::DoNotInject,
        compaction_metadata,
    )
    .await?;
    Ok(())
}

async fn run_remote_compact_task_inner(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    fallback_turn_context: Option<&Arc<TurnContext>>,
    initial_context_injection: InitialContextInjection,
    compaction_metadata: CompactionTurnMetadata,
) -> CodexResult<()> {
    let trigger = compaction_metadata.trigger();
    let reason = compaction_metadata.reason();
    let implementation = compaction_metadata.implementation();
    let phase = compaction_metadata.phase();
    let mut active_context_tokens_before = sess.get_total_token_usage().await;
    let attempt = CompactionAnalyticsAttempt::begin(
        sess.as_ref(),
        turn_context.as_ref(),
        trigger,
        reason,
        implementation,
        phase,
    )
    .await;
    let pre_compact_outcome = run_pre_compact_hooks(sess, turn_context, trigger).await;
    match pre_compact_outcome {
        PreCompactHookOutcome::Continue => {}
        PreCompactHookOutcome::Stopped { reason } => {
            let error = reason.unwrap_or_else(|| "PreCompact hook stopped execution".to_string());
            attempt
                .track(
                    sess.as_ref(),
                    codex_analytics::CompactionStatus::Interrupted,
                    Some(error),
                    Some(active_context_tokens_before),
                )
                .await;
            return Err(CodexErr::TurnAborted);
        }
    }
    let result = run_remote_compact_task_inner_impl(
        sess,
        turn_context,
        fallback_turn_context,
        initial_context_injection,
        compaction_metadata,
        &mut active_context_tokens_before,
    )
    .await;
    let status = compaction_status_from_result(&result);
    let error = result.as_ref().err().map(ToString::to_string);
    if result.is_ok() {
        let post_compact_outcome = run_post_compact_hooks(sess, turn_context, trigger).await;
        if let PostCompactHookOutcome::Stopped = post_compact_outcome {
            attempt
                .track(
                    sess.as_ref(),
                    status,
                    error,
                    Some(active_context_tokens_before),
                )
                .await;
            return Err(CodexErr::TurnAborted);
        }
    }
    attempt
        .track(
            sess.as_ref(),
            status,
            error.clone(),
            Some(active_context_tokens_before),
        )
        .await;
    if let Err(err) = result {
        sess.track_turn_codex_error(turn_context, &err);
        let event = EventMsg::Error(
            err.to_error_event(Some("Error running remote compact task".to_string())),
        );
        sess.send_event(turn_context, event).await;
        return Err(err);
    }
    Ok(())
}

async fn run_remote_compact_task_inner_impl(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    fallback_turn_context: Option<&Arc<TurnContext>>,
    initial_context_injection: InitialContextInjection,
    compaction_metadata: CompactionTurnMetadata,
    active_context_tokens_before: &mut i64,
) -> CodexResult<()> {
    let context_compaction_item = ContextCompactionItem::new();
    let compaction_id = context_compaction_item.id.clone();
    // Use the UI compaction item ID as the trace compaction ID so protocol lifecycle events,
    // endpoint attempts, and the installed history checkpoint all have one join key.
    let compaction_trace = sess.services.rollout_thread_trace.compaction_trace_context(
        turn_context.sub_id.as_str(),
        compaction_id.as_str(),
        turn_context.model_info.slug.as_str(),
        turn_context.provider.info().name.as_str(),
    );
    let compaction_item = TurnItem::ContextCompaction(context_compaction_item);
    sess.emit_turn_item_started(turn_context, &compaction_item)
        .await;
    let attempt = run_remote_compact_attempt(
        sess,
        turn_context,
        &compaction_trace,
        compaction_metadata,
        active_context_tokens_before,
    )
    .await;
    // When previous-model compaction is rejected in a way that may be model-specific (e.g. a
    // resumed thread references a retired model slug), retry once with the currently selected
    // model. The successful attempt's turn context then drives history processing so lifecycle
    // events and token accounting stay aligned with the model that actually compacted.
    let (attempt, compaction_turn_context) = match attempt {
        Ok(attempt) => (attempt, turn_context),
        Err(error) => {
            let Some(fallback_turn_context) = fallback_turn_context else {
                return Err(error);
            };
            if !should_retry_with_current_model(&error) {
                return Err(error);
            }
            let fallback_compaction_trace =
                sess.services.rollout_thread_trace.compaction_trace_context(
                    fallback_turn_context.sub_id.as_str(),
                    compaction_id.as_str(),
                    fallback_turn_context.model_info.slug.as_str(),
                    fallback_turn_context.provider.info().name.as_str(),
                );
            let fallback_result = run_remote_compact_attempt(
                sess,
                fallback_turn_context,
                &fallback_compaction_trace,
                compaction_metadata,
                active_context_tokens_before,
            )
            .await;
            record_model_fallback(
                &sess.services.session_telemetry,
                turn_context.model_info.slug.as_str(),
                fallback_turn_context.model_info.slug.as_str(),
                compaction_metadata.reason(),
                compaction_metadata.implementation(),
                fallback_result.as_ref().err(),
            );
            match fallback_result {
                Ok(attempt) => (attempt, fallback_turn_context),
                // Surface the original previous-model error so the retry does not change the
                // user-visible failure.
                Err(_) => return Err(error),
            }
        }
    };
    let RemoteCompactAttempt {
        new_history,
        trace_input_history,
    } = attempt;
    let new_history = process_compacted_history(
        sess.as_ref(),
        compaction_turn_context.as_ref(),
        new_history,
        initial_context_injection,
    )
    .await;

    let reference_context_item = match initial_context_injection {
        InitialContextInjection::DoNotInject => None,
        InitialContextInjection::BeforeLastUserMessage => {
            Some(compaction_turn_context.to_turn_context_item())
        }
    };
    let compacted_item = CompactedItem {
        message: String::new(),
        replacement_history: Some(new_history.clone()),
    };
    // Install is the semantic boundary where the compact endpoint's output becomes live
    // thread history. Keep it distinct from the later inference request so the reducer can
    // still represent repeated developer/context prefix items exactly as the model saw them.
    compaction_trace.record_installed(&CompactionCheckpointTracePayload {
        input_history: &trace_input_history,
        replacement_history: &new_history,
    });
    sess.replace_compacted_history(new_history, reference_context_item, compacted_item)
        .await;
    sess.recompute_token_usage(compaction_turn_context).await;

    sess.emit_turn_item_completed(compaction_turn_context, compaction_item)
        .await;
    Ok(())
}

/// A single `/responses/compact` attempt against one model's turn context.
struct RemoteCompactAttempt {
    /// Model-provided compacted transcript before canonical context is re-injected.
    new_history: Vec<ResponseItem>,
    /// History selected for remote compaction (after any output rewriting), recorded separately
    /// from the next sampling request so the trace reducer can represent repeated prefix items.
    trace_input_history: Vec<ResponseItem>,
}

/// Runs the compact request for a single model, returning its raw compacted history.
///
/// Extracted so that previous-model compaction can be retried with the currently selected model
/// without duplicating history preparation, prompt construction, or failure logging.
async fn run_remote_compact_attempt(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    compaction_trace: &CompactionTraceContext,
    compaction_metadata: CompactionTurnMetadata,
    active_context_tokens_before: &mut i64,
) -> CodexResult<RemoteCompactAttempt> {
    let mut history = sess.clone_history().await;
    let base_instructions = sess.get_base_instructions().await;
    let (rewritten_outputs, estimated_deleted_tokens) =
        trim_function_call_history_to_fit_context_window(
            &mut history,
            turn_context.as_ref(),
            &base_instructions,
        );
    if rewritten_outputs > 0 {
        info!(
            turn_id = %turn_context.sub_id,
            rewritten_outputs,
            "rewrote history outputs before remote compaction"
        );
    }
    if estimated_deleted_tokens > 0 {
        let max_local_deleted_tokens = sess
            .get_total_token_usage_breakdown()
            .await
            .estimated_tokens_of_items_added_since_last_successful_api_response;
        *active_context_tokens_before = (*active_context_tokens_before)
            .saturating_sub(estimated_deleted_tokens.min(max_local_deleted_tokens));
    }
    let tool_router = built_tools(
        sess.as_ref(),
        turn_context.as_ref(),
        &CancellationToken::new(),
    )
    .await?;
    let tools = tool_router.model_visible_specs();
    // Guarantee the compaction request fits the context window even when tool-output rewriting
    // above could not shrink it enough (a long thread dominated by prior summaries, retained user
    // messages, or reasoning). Trims only this local request clone, never session history.
    drop_oldest_history_to_fit_context_window(
        &mut history,
        turn_context.as_ref(),
        &base_instructions,
        estimate_tool_spec_tokens(&tools),
    );
    let prompt_input = history
        .clone()
        .for_prompt(&turn_context.model_info.input_modalities);
    let prompt = Prompt {
        input: prompt_input,
        tools,
        parallel_tool_calls: turn_context.model_info.supports_parallel_tool_calls,
        base_instructions,
        personality: turn_context.personality,
        output_schema: None,
        output_schema_strict: true,
    };
    let window_id = sess.runtime_model_client().current_window_id();
    let turn_metadata_header = turn_context
        .turn_metadata_state
        .current_header_value_for_compaction(&window_id, compaction_metadata);
    let request_budget = RemoteCompactionRequestBudget::new();
    let result = sess
        .runtime_model_client()
        .compact_conversation_history(
            &prompt,
            &turn_context.model_info,
            CompactConversationRequestSettings {
                effort: turn_context.reasoning_effort.clone(),
                summary: turn_context.reasoning_summary,
                service_tier: if sess.services.auth_manager.auth_mode() == Some(AuthMode::ApiKey) {
                    None
                } else {
                    turn_context.config.service_tier.clone()
                },
                request_budget: request_budget.clone(),
            },
            &turn_context.session_telemetry,
            compaction_trace,
            turn_metadata_header.as_deref(),
        )
        .await;
    let new_history = match result {
        Ok(new_history) => new_history,
        Err(err) => {
            let total_usage_breakdown = sess.get_total_token_usage_breakdown().await;
            let compact_request_log_data =
                build_compact_request_log_data(&prompt.input, &prompt.base_instructions.text);
            log_remote_compact_failure(
                turn_context,
                &compact_request_log_data,
                total_usage_breakdown,
                &err,
            );
            return Err(err);
        }
    };
    let trace_input_history = history.raw_items().to_vec();
    Ok(RemoteCompactAttempt {
        new_history,
        trace_input_history,
    })
}

pub(crate) async fn process_compacted_history(
    sess: &Session,
    turn_context: &TurnContext,
    mut compacted_history: Vec<ResponseItem>,
    initial_context_injection: InitialContextInjection,
) -> Vec<ResponseItem> {
    // Mid-turn compaction is the only path that must inject initial context above the last user
    // message in the replacement history. Pre-turn compaction instead injects context after the
    // compaction item, but mid-turn compaction keeps the compaction item last for model training.
    let initial_context = if matches!(
        initial_context_injection,
        InitialContextInjection::BeforeLastUserMessage
    ) {
        sess.build_initial_context(turn_context).await
    } else {
        Vec::new()
    };

    compacted_history.retain(should_keep_compacted_history_item);
    insert_initial_context_before_last_real_user_or_summary(compacted_history, initial_context)
}

/// Returns whether an item from remote compaction output should be preserved.
///
/// Called while processing the model-provided compacted transcript, before we
/// append fresh canonical context from the current session.
///
/// We drop:
/// - `developer` messages because remote output can include stale/duplicated
///   instruction content.
/// - non-user-content `user` messages (session prefix/instruction wrappers),
///   while preserving real user messages and persisted hook prompts.
///
/// This intentionally keeps:
/// - `assistant` messages (future remote compaction models may emit them)
/// - `user`-role warnings that parse as `TurnItem::UserMessage` and compaction-generated summary
///   messages. Legacy warning fragments are filtered by `parse_turn_item` before they reach this
///   check.
pub(crate) fn should_keep_compacted_history_item(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { role, .. } if role == "developer" => false,
        ResponseItem::Message { role, .. } if role == "user" => {
            matches!(
                crate::event_mapping::parse_turn_item(item),
                Some(TurnItem::UserMessage(_) | TurnItem::HookPrompt(_))
            )
        }
        ResponseItem::Message { role, .. } if role == "assistant" => true,
        ResponseItem::Message { .. } => false,
        ResponseItem::AgentMessage { .. } => true,
        ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. } => true,
        ResponseItem::CompactionTrigger => false,
        ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Other => false,
    }
}

#[derive(Debug)]
pub(crate) struct CompactRequestLogData {
    failing_compaction_request_model_visible_bytes: i64,
}

pub(crate) fn build_compact_request_log_data(
    input: &[ResponseItem],
    instructions: &str,
) -> CompactRequestLogData {
    let failing_compaction_request_model_visible_bytes = input
        .iter()
        .map(estimate_response_item_model_visible_bytes)
        .fold(
            i64::try_from(instructions.len()).unwrap_or(i64::MAX),
            i64::saturating_add,
        );

    CompactRequestLogData {
        failing_compaction_request_model_visible_bytes,
    }
}

pub(crate) fn log_remote_compact_failure(
    turn_context: &TurnContext,
    log_data: &CompactRequestLogData,
    total_usage_breakdown: TotalTokenUsageBreakdown,
    err: &CodexErr,
) {
    error!(
        turn_id = %turn_context.sub_id,
        last_api_response_total_tokens = total_usage_breakdown.last_api_response_total_tokens,
        all_history_items_model_visible_bytes = total_usage_breakdown.all_history_items_model_visible_bytes,
        estimated_tokens_of_items_added_since_last_successful_api_response = total_usage_breakdown.estimated_tokens_of_items_added_since_last_successful_api_response,
        estimated_bytes_of_items_added_since_last_successful_api_response = total_usage_breakdown.estimated_bytes_of_items_added_since_last_successful_api_response,
        model_context_window_tokens = ?turn_context.model_context_window(),
        failing_compaction_request_model_visible_bytes = log_data.failing_compaction_request_model_visible_bytes,
        compact_error = %err,
        "remote compaction failed"
    );
}

pub(crate) fn trim_function_call_history_to_fit_context_window(
    history: &mut ContextManager,
    turn_context: &TurnContext,
    base_instructions: &BaseInstructions,
) -> (usize, i64) {
    let Some(context_window) = turn_context.model_context_window() else {
        return (0, 0);
    };
    let mut rewritten_outputs = 0usize;
    let mut estimated_deleted_tokens = 0i64;
    let item_count = history.raw_items().len();

    for index in (0..item_count).rev() {
        let Some(estimated_tokens_before) =
            history.estimate_token_count_with_base_instructions(base_instructions)
        else {
            break;
        };
        if estimated_tokens_before <= context_window {
            break;
        }
        let Some(rewritten_item) = history
            .raw_items()
            .get(index)
            .and_then(rewritten_output_for_context_window)
        else {
            break;
        };
        let mut items = history.raw_items().to_vec();
        items[index] = rewritten_item;
        history.replace(items);
        let estimated_tokens_after = history
            .estimate_token_count_with_base_instructions(base_instructions)
            .unwrap_or_default();
        rewritten_outputs += 1;
        estimated_deleted_tokens = estimated_deleted_tokens
            .saturating_add(estimated_tokens_before.saturating_sub(estimated_tokens_after));
    }

    (rewritten_outputs, estimated_deleted_tokens)
}

/// Fraction of the model context window held back, when we must drop history to fit a
/// compaction request, for the compaction output the model generates (summary + reasoning) and
/// for slack in our lower-bound token estimate. Mirrors the 90%-of-window headroom convention in
/// [`codex_protocol::openai_models::ModelInfo::auto_compact_token_limit`].
const COMPACT_REQUEST_OUTPUT_RESERVE_DIVISOR: i64 = 10;

/// Approximate the model-visible token cost of the request's tools.
///
/// [`ContextManager::estimate_token_count_with_base_instructions`] counts only history items plus
/// base instructions; it does not account for the tool specs that ride along in every compaction
/// request. On long threads with many tools that omission can be large enough to push the real
/// request past the context window even when the history-only estimate looks safe, so it is folded
/// into the drop-oldest budget below.
pub(crate) fn estimate_tool_spec_tokens(tools: &[ToolSpec]) -> i64 {
    let bytes = serde_json::to_string(tools)
        .map(|serialized| i64::try_from(serialized.len()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    approx_tokens_from_byte_count_i64(bytes)
}

/// Last-resort bound that keeps a compaction request within the target model's context window.
///
/// [`trim_function_call_history_to_fit_context_window`] only rewrites oversized *tool outputs*.
/// After a thread has been compacted a few times its history is dominated by items that cannot be
/// rewritten (prior compaction summaries, retained user messages, reasoning), so a long thread
/// grows into a state where even after output rewriting the request still exceeds the window.
/// Sending it unbounded makes the compaction request itself fail with
/// [`CodexErr::ContextWindowExceeded`] — precisely when compaction is needed most.
///
/// As a last resort this drops the oldest history items (preserving call/output pairs via
/// [`ContextManager::remove_first_item`], matching the local compaction path) until the estimated
/// request fits `context_window` minus room for the request tools and the compaction output.
///
/// Only the local request clone is trimmed, never session history, so a genuinely unrecoverable
/// overflow still surfaces an error without destroying the user's thread. If base instructions
/// alone already exceed the budget, dropping items cannot make the request fit, so the history is
/// left untouched and the request proceeds (surfacing a graceful error instead of silently losing
/// the whole thread). At least one item is always retained so there is something to summarize.
/// Returns the number of history items dropped.
pub(crate) fn drop_oldest_history_to_fit_context_window(
    history: &mut ContextManager,
    turn_context: &TurnContext,
    base_instructions: &BaseInstructions,
    request_tool_tokens: i64,
) -> usize {
    let Some(context_window) = turn_context.model_context_window() else {
        return 0;
    };
    let output_reserve = (context_window / COMPACT_REQUEST_OUTPUT_RESERVE_DIVISOR).max(0);
    let budget = context_window
        .saturating_sub(request_tool_tokens.max(0))
        .saturating_sub(output_reserve);

    // Dropping items only reduces the estimate down toward the base-instructions floor. If that
    // floor already meets or exceeds the budget (e.g. tiny or heavily over-subscribed context
    // windows), trimming would destroy the whole thread without ever fitting, so leave it alone and
    // let the request surface a graceful error instead of silently losing everything.
    let base_tokens =
        i64::try_from(approx_token_count(&base_instructions.text)).unwrap_or(i64::MAX);
    if budget <= 0 || base_tokens >= budget {
        return 0;
    }

    let mut dropped = 0usize;
    while history.raw_items().len() > 1 {
        let Some(estimate) = history.estimate_token_count_with_base_instructions(base_instructions)
        else {
            break;
        };
        if estimate <= budget {
            break;
        }
        let before = history.raw_items().len();
        history.remove_first_item();
        let after = history.raw_items().len();
        if after >= before {
            // Defensive: guarantee forward progress so the loop always terminates even if a future
            // `remove_first_item` change ever fails to shrink the history.
            break;
        }
        dropped = dropped.saturating_add(before - after);
    }

    if dropped > 0 {
        info!(
            turn_id = %turn_context.sub_id,
            dropped,
            budget,
            model_context_window_tokens = ?turn_context.model_context_window(),
            "dropped oldest history to fit remote compaction request within the context window"
        );
    }

    dropped
}

fn rewritten_output_for_context_window(item: &ResponseItem) -> Option<ResponseItem> {
    Some(match item {
        ResponseItem::FunctionCallOutput { call_id, output } => ResponseItem::FunctionCallOutput {
            call_id: call_id.clone(),
            output: truncated_output_payload(output),
        },
        ResponseItem::CustomToolCallOutput {
            call_id,
            name,
            output,
        } => ResponseItem::CustomToolCallOutput {
            call_id: call_id.clone(),
            name: name.clone(),
            output: truncated_output_payload(output),
        },
        ResponseItem::ToolSearchOutput {
            call_id,
            status,
            execution,
            ..
        } => ResponseItem::ToolSearchOutput {
            call_id: call_id.clone(),
            status: status.clone(),
            execution: execution.clone(),
            tools: Vec::new(),
        },
        _ => return None,
    })
}

fn truncated_output_payload(output: &FunctionCallOutputPayload) -> FunctionCallOutputPayload {
    FunctionCallOutputPayload {
        body: FunctionCallOutputBody::Text(CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE.to_string()),
        success: output.success,
    }
}
