use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::config::Config;
use crate::function_tool::FunctionCallError;
use crate::session::tests::make_session_and_context;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::policy::test_mcp_policy;
use crate::tools::policy::test_mcp_policy_with_state;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolRegistry;
use crate::turn_diff_tracker::TurnDiffTracker;
use codex_config::ToolPolicy;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall as ExtensionToolCall;
use codex_extension_api::ToolExecutor;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolName;
use codex_tools::ToolOutput;
use codex_tools::ToolSpec;
use codex_tools::default_namespace_description;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_test::internal::MockWriter;

use super::MAX_INFINITY_AGENT_ARGUMENT_BYTES;
use super::ToolCall;
use super::ToolCallSource;
use super::ToolRouter;
use super::ToolRouterParams;
use super::extension_tool_executors;

struct ExtensionEchoContributor;

struct CountingHandler {
    tool_name: ToolName,
    invocations: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl ToolExecutor<ToolInvocation> for CountingHandler {
    fn tool_name(&self) -> ToolName {
        self.tool_name.clone()
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: self.tool_name.name.clone(),
            description: "Counting test tool.".to_string(),
            strict: true,
            parameters: codex_extension_api::parse_tool_input_schema(&json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }))
            .expect("counting schema should parse"),
            output_schema: None,
            defer_loading: None,
        })
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FunctionToolOutput::from_text(
            "ok".to_string(),
            Some(true),
        )))
    }
}

impl CoreToolRuntime for CountingHandler {}

impl codex_extension_api::ToolContributor for ExtensionEchoContributor {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        _thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ExtensionToolCall>>> {
        vec![Arc::new(ExtensionEchoExecutor)]
    }
}

struct ExtensionEchoExecutor;

#[async_trait::async_trait]
impl ToolExecutor<ExtensionToolCall> for ExtensionEchoExecutor {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced("extension/", "echo")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "extension/".to_string(),
            description: default_namespace_description("extension/"),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "echo".to_string(),
                description: "Echoes arguments through an extension tool.".to_string(),
                strict: true,
                parameters: codex_extension_api::parse_tool_input_schema(&json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" },
                    },
                    "required": ["message"],
                    "additionalProperties": false,
                }))
                .expect("extension schema should parse"),
                output_schema: None,
                defer_loading: None,
            })],
        })
    }

    async fn handle(
        &self,
        call: ExtensionToolCall,
    ) -> Result<Box<dyn codex_tools::ToolOutput>, codex_tools::FunctionCallError> {
        let arguments: serde_json::Value =
            serde_json::from_str(call.function_arguments()?).expect("test arguments should parse");
        Ok(Box::new(codex_tools::JsonToolOutput::new(json!({
            "arguments": arguments,
            "callId": call.call_id,
            "conversationHistory": call.conversation_history.items(),
            "ok": true,
        }))))
    }
}

fn extension_tool_test_registry() -> Arc<ExtensionRegistry<Config>> {
    let mut builder = ExtensionRegistryBuilder::new();
    builder.tool_contributor(Arc::new(ExtensionEchoContributor));
    Arc::new(builder.build())
}

#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "test builds a router from session-owned MCP manager state"
)]
async fn parallel_support_does_not_match_namespaced_local_tool_names() -> anyhow::Result<()> {
    let (session, turn) = make_session_and_context().await;
    let mcp_tools = session
        .services
        .mcp_connection_manager
        .read()
        .await
        .list_all_tools()
        .await;
    let router = ToolRouter::from_turn_context(
        &turn,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: Some(mcp_tools),
            discoverable_tools: None,
            extension_tool_executors: Vec::new(),
            dynamic_tools: turn.dynamic_tools.as_slice(),
        },
    );

    let parallel_tool_name = ["exec_command", "shell_command"]
        .into_iter()
        .find(|name| {
            router.tool_supports_parallel(&ToolCall {
                tool_name: ToolName::plain(*name),
                call_id: "call-parallel-tool".to_string(),
                payload: ToolPayload::Function {
                    arguments: "{}".to_string(),
                },
            })
        })
        .expect("test session should expose a parallel shell-like tool");

    assert!(!router.tool_supports_parallel(&ToolCall {
        tool_name: ToolName::namespaced("mcp__server__", parallel_tool_name),
        call_id: "call-namespaced-tool".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    }));

    Ok(())
}

#[tokio::test]
async fn build_tool_call_uses_namespace_for_registry_name() -> anyhow::Result<()> {
    let tool_name = "create_event".to_string();

    let call = ToolRouter::build_tool_call(ResponseItem::FunctionCall {
        id: None,
        name: tool_name.clone(),
        namespace: Some("mcp__codex_apps__calendar".to_string()),
        arguments: "{}".to_string(),
        call_id: "call-namespace".to_string(),
    })?
    .expect("function_call should produce a tool call");

    assert_eq!(
        call.tool_name,
        ToolName::namespaced("mcp__codex_apps__calendar", tool_name)
    );
    assert_eq!(call.call_id, "call-namespace");
    match call.payload {
        ToolPayload::Function { arguments } => {
            assert_eq!(arguments, "{}");
        }
        other => panic!("expected function payload, got {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn build_custom_tool_call_uses_namespace_for_registry_name() -> anyhow::Result<()> {
    let tool_name = "exec".to_string();

    let call = ToolRouter::build_tool_call(ResponseItem::CustomToolCall {
        id: None,
        status: None,
        call_id: "call-namespace".to_string(),
        name: tool_name.clone(),
        namespace: Some("mcp__python".to_string()),
        input: "print('hello')".to_string(),
    })?
    .expect("custom_tool_call should produce a tool call");

    assert_eq!(
        call,
        ToolCall {
            tool_name: ToolName::namespaced("mcp__python", tool_name),
            call_id: "call-namespace".to_string(),
            payload: ToolPayload::Custom {
                input: "print('hello')".to_string(),
            },
        }
    );

    Ok(())
}

#[tokio::test]
async fn mcp_parallel_support_uses_handler_data() -> anyhow::Result<()> {
    let (_, turn) = make_session_and_context().await;
    let router = ToolRouter::from_turn_context(
        &turn,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: Some(vec![
                mcp_tool_info(
                    "echo",
                    /*supports_parallel_tool_calls*/ true,
                    "mcp__echo__",
                    "query_with_delay",
                ),
                mcp_tool_info(
                    "hello_echo",
                    /*supports_parallel_tool_calls*/ false,
                    "mcp__hello_echo__",
                    "query_with_delay",
                ),
            ]),
            discoverable_tools: None,
            extension_tool_executors: Vec::new(),
            dynamic_tools: turn.dynamic_tools.as_slice(),
        },
    );

    let call = ToolCall {
        tool_name: ToolName::namespaced("mcp__echo__", "query_with_delay"),
        call_id: "call-handler".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    };
    assert!(router.tool_supports_parallel(&call));

    let different_server_call = ToolCall {
        tool_name: ToolName::namespaced("mcp__hello_echo__", "query_with_delay"),
        call_id: "call-other-server".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    };
    assert!(!router.tool_supports_parallel(&different_server_call));

    Ok(())
}

#[tokio::test]
async fn tools_without_handlers_do_not_support_parallel() -> anyhow::Result<()> {
    let (_, turn) = make_session_and_context().await;
    let router = ToolRouter::from_turn_context(
        &turn,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: None,
            discoverable_tools: None,
            extension_tool_executors: Vec::new(),
            dynamic_tools: turn.dynamic_tools.as_slice(),
        },
    );

    assert!(!router.tool_supports_parallel(&ToolCall {
        tool_name: ToolName::plain("web_search"),
        call_id: "call-web-search".to_string(),
        payload: ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    }));

    Ok(())
}

#[tokio::test]
async fn specs_filter_deferred_dynamic_tools() -> anyhow::Result<()> {
    let (_, turn) = make_session_and_context().await;
    let hidden_tool = "hidden_dynamic_tool";
    let visible_tool = "visible_dynamic_tool";
    let dynamic_tools = vec![
        DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: hidden_tool.to_string(),
            description: "Hidden until discovered.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            defer_loading: true,
        },
        DynamicToolSpec {
            namespace: Some("codex_app".to_string()),
            name: visible_tool.to_string(),
            description: "Visible immediately.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            defer_loading: false,
        },
    ];

    let router = ToolRouter::from_turn_context(
        &turn,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: None,
            discoverable_tools: None,
            extension_tool_executors: Vec::new(),
            dynamic_tools: &dynamic_tools,
        },
    );

    assert_eq!(
        namespace_function_names(&router.model_visible_specs(), "codex_app"),
        vec![visible_tool.to_string()]
    );

    Ok(())
}

fn mcp_tool_info(
    server_name: &str,
    supports_parallel_tool_calls: bool,
    callable_namespace: &str,
    tool_name: &str,
) -> codex_mcp::ToolInfo {
    codex_mcp::ToolInfo {
        server_name: server_name.to_string(),
        supports_parallel_tool_calls,
        server_origin: None,
        callable_name: tool_name.to_string(),
        callable_namespace: callable_namespace.to_string(),
        namespace_description: None,
        tool: rmcp::model::Tool::new(
            tool_name.to_string(),
            "Test MCP tool",
            Arc::new(rmcp::model::object(json!({
                "type": "object",
            }))),
        ),
        connector_id: None,
        connector_name: None,
        plugin_display_names: Vec::new(),
    }
}

fn set_turn_infinity_policy(
    turn: &mut TurnContext,
    policy: Option<Arc<crate::tools::policy::VerifiedToolPolicy>>,
) {
    let mut config = (*turn.config).clone();
    config.tools_policy = Some(ToolPolicy::InfinityAgent);
    config.infinity_agent_policy = policy;
    turn.config = Arc::new(config);
}

fn counting_infinity_router(
    policy: Arc<crate::tools::policy::VerifiedToolPolicy>,
    tool_name: ToolName,
    invocations: Arc<AtomicUsize>,
) -> ToolRouter {
    let handler = Arc::new(CountingHandler {
        tool_name,
        invocations,
    }) as Arc<dyn CoreToolRuntime>;
    ToolRouter::from_infinity_policy(ToolRegistry::from_tools([handler]), Vec::new(), policy)
}

fn infinity_call(tool_name: ToolName, arguments: String) -> ToolCall {
    ToolCall {
        tool_name,
        call_id: "call-infinity".to_string(),
        payload: ToolPayload::Function { arguments },
    }
}

async fn assert_infinity_dispatch_rejected_without_handler(
    router: &ToolRouter,
    session: Arc<crate::session::session::Session>,
    turn: Arc<TurnContext>,
    call: ToolCall,
    invocations: &AtomicUsize,
) {
    let result = router
        .dispatch_tool_call_with_code_mode_result(
            session,
            turn,
            CancellationToken::new(),
            Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
            call,
            ToolCallSource::Direct,
        )
        .await;

    assert!(result.is_err(), "invalid AuthCapsule call must be rejected");
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        0,
        "rejected AuthCapsule call must not reach the handler"
    );
}

#[tokio::test]
async fn infinity_agent_dispatch_rejections_never_invoke_handlers() {
    let tool = mcp_tool_info(
        "infinity",
        /*supports_parallel_tool_calls*/ false,
        "mcp__infinity",
        "infinity_run_get",
    );
    let tool_name = tool.canonical_tool_name();
    let valid_policy = test_mcp_policy(std::slice::from_ref(&tool));
    let (session, mut turn) = make_session_and_context().await;
    set_turn_infinity_policy(&mut turn, Some(Arc::clone(&valid_policy)));
    let session = Arc::new(session);
    let turn = Arc::new(turn);

    let invocations = Arc::new(AtomicUsize::new(0));
    let router = ToolRouter::from_parts(
        ToolRegistry::from_tools([Arc::new(CountingHandler {
            tool_name: tool_name.clone(),
            invocations: Arc::clone(&invocations),
        }) as Arc<dyn CoreToolRuntime>]),
        Vec::new(),
    );
    assert_infinity_dispatch_rejected_without_handler(
        &router,
        Arc::clone(&session),
        Arc::clone(&turn),
        infinity_call(tool_name.clone(), "{}".to_string()),
        invocations.as_ref(),
    )
    .await;

    let (_, mut missing_policy_turn) = make_session_and_context().await;
    set_turn_infinity_policy(&mut missing_policy_turn, None);
    let invocations = Arc::new(AtomicUsize::new(0));
    let router = counting_infinity_router(
        Arc::clone(&valid_policy),
        tool_name.clone(),
        Arc::clone(&invocations),
    );
    assert_infinity_dispatch_rejected_without_handler(
        &router,
        Arc::clone(&session),
        Arc::new(missing_policy_turn),
        infinity_call(tool_name.clone(), "{}".to_string()),
        invocations.as_ref(),
    )
    .await;

    let invocations = Arc::new(AtomicUsize::new(0));
    let mismatched_policy = test_mcp_policy_with_state(
        std::slice::from_ref(&tool),
        "different-policy-digest".to_string(),
        chrono::Utc::now() + chrono::Duration::hours(1),
    );
    let router = counting_infinity_router(
        mismatched_policy,
        tool_name.clone(),
        Arc::clone(&invocations),
    );
    assert_infinity_dispatch_rejected_without_handler(
        &router,
        Arc::clone(&session),
        Arc::clone(&turn),
        infinity_call(tool_name.clone(), "{}".to_string()),
        invocations.as_ref(),
    )
    .await;

    let expired_policy = test_mcp_policy_with_state(
        std::slice::from_ref(&tool),
        valid_policy.digest().to_string(),
        chrono::Utc::now() - chrono::Duration::seconds(1),
    );
    let (_, mut expired_turn) = make_session_and_context().await;
    set_turn_infinity_policy(&mut expired_turn, Some(Arc::clone(&expired_policy)));
    let invocations = Arc::new(AtomicUsize::new(0));
    let router =
        counting_infinity_router(expired_policy, tool_name.clone(), Arc::clone(&invocations));
    assert_infinity_dispatch_rejected_without_handler(
        &router,
        Arc::clone(&session),
        Arc::new(expired_turn),
        infinity_call(tool_name.clone(), "{}".to_string()),
        invocations.as_ref(),
    )
    .await;

    for invalid_call in [
        infinity_call(
            ToolName::namespaced("mcp__infinity", "infinity_result_get"),
            "{}".to_string(),
        ),
        infinity_call(tool_name.clone(), r#"{"value":1,"value":2}"#.to_string()),
        infinity_call(
            tool_name.clone(),
            "x".repeat(MAX_INFINITY_AGENT_ARGUMENT_BYTES + 1),
        ),
    ] {
        let invocations = Arc::new(AtomicUsize::new(0));
        let router = counting_infinity_router(
            Arc::clone(&valid_policy),
            tool_name.clone(),
            Arc::clone(&invocations),
        );
        assert_infinity_dispatch_rejected_without_handler(
            &router,
            Arc::clone(&session),
            Arc::clone(&turn),
            invalid_call,
            invocations.as_ref(),
        )
        .await;
    }

    let invocations = Arc::new(AtomicUsize::new(0));
    let router = counting_infinity_router(
        Arc::clone(&valid_policy),
        tool_name.clone(),
        Arc::clone(&invocations),
    );
    router
        .dispatch_tool_call_with_code_mode_result(
            Arc::clone(&session),
            Arc::clone(&turn),
            CancellationToken::new(),
            Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
            infinity_call(tool_name, "{}".to_string()),
            ToolCallSource::Direct,
        )
        .await
        .expect("valid AuthCapsule call should reach the handler");
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn infinity_agent_rejected_arguments_are_redacted_before_history() {
    const FIRST_CANARY: &str = "rejected-canary-one";
    const SECOND_CANARY: &str = "rejected-canary-two";

    let tool = mcp_tool_info(
        "infinity",
        /*supports_parallel_tool_calls*/ false,
        "mcp__infinity",
        "infinity_run_get",
    );
    let tool_name = tool.canonical_tool_name();
    let policy = test_mcp_policy(std::slice::from_ref(&tool));
    let (session, mut turn) = make_session_and_context().await;
    set_turn_infinity_policy(&mut turn, Some(Arc::clone(&policy)));
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let invocations = Arc::new(AtomicUsize::new(0));
    let router = Arc::new(counting_infinity_router(
        policy,
        tool_name,
        Arc::clone(&invocations),
    ));
    let tracker = Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new()));
    let mut ctx = crate::stream_events_utils::HandleOutputCtx {
        sess: Arc::clone(&session),
        turn_context: Arc::clone(&turn),
        turn_store: Arc::new(ExtensionData::new(turn.sub_id.clone())),
        tool_runtime: crate::tools::parallel::ToolCallRuntime::new(
            router,
            Arc::clone(&session),
            Arc::clone(&turn),
            tracker,
        ),
        cancellation_token: CancellationToken::new(),
    };
    let item = ResponseItem::FunctionCall {
        id: None,
        name: "infinity_run_get".to_string(),
        namespace: Some("mcp__infinity".to_string()),
        arguments: format!(r#"{{"value":"{FIRST_CANARY}","value":"{SECOND_CANARY}"}}"#),
        call_id: "call-redacted".to_string(),
    };
    let log_buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_level(true)
        .with_ansi(false)
        .with_max_level(Level::INFO)
        .with_span_events(FmtSpan::NONE)
        .with_writer(MockWriter::new(log_buffer))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let output = crate::stream_events_utils::handle_output_item_done(&mut ctx, item, None)
        .await
        .expect("ambiguous arguments should be answered without executing");

    assert!(output.needs_follow_up);
    assert!(output.tool_future.is_none());
    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    let history = session.clone_history().await.raw_items().to_vec();
    let serialized = serde_json::to_string(&history).expect("history should serialize");
    let logs =
        String::from_utf8(log_buffer.lock().expect("log buffer lock").clone()).expect("utf8 logs");
    assert!(!serialized.contains(FIRST_CANARY));
    assert!(!serialized.contains(SECOND_CANARY));
    assert!(!logs.contains(FIRST_CANARY));
    assert!(!logs.contains(SECOND_CANARY));
    assert!(!logs.contains("ToolCall:"));
    assert!(matches!(
        history.first(),
        Some(ResponseItem::FunctionCall { arguments, .. }) if arguments == "{}"
    ));
}

#[tokio::test]
async fn extension_tool_executors_are_model_visible_and_dispatchable() -> anyhow::Result<()> {
    let (mut session, turn) = make_session_and_context().await;
    session.services.extensions = extension_tool_test_registry();
    let history_item = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "extension history".to_string(),
        }],
        phase: None,
    };
    session
        .record_conversation_items(&turn, std::slice::from_ref(&history_item))
        .await;

    let router = ToolRouter::from_turn_context(
        &turn,
        ToolRouterParams {
            deferred_mcp_tools: None,
            mcp_tools: None,
            discoverable_tools: None,
            extension_tool_executors: extension_tool_executors(&session),
            dynamic_tools: turn.dynamic_tools.as_slice(),
        },
    );

    assert!(
        router.model_visible_specs().iter().any(
            |spec| matches!(spec, ToolSpec::Namespace(namespace)
            if namespace.name == "extension/"
                && namespace.tools.iter().any(|tool| matches!(
                    tool,
                    ResponsesApiNamespaceTool::Function(tool) if tool.name == "echo"
                )))
        ),
        "expected extension-provided tool to be visible to the model"
    );

    let call = ToolRouter::build_tool_call(ResponseItem::FunctionCall {
        id: None,
        name: "echo".to_string(),
        namespace: Some("extension/".to_string()),
        arguments: json!({ "message": "hello" }).to_string(),
        call_id: "call-extension".to_string(),
    })?
    .expect("function_call should produce a tool call");
    let result = router
        .dispatch_tool_call_with_code_mode_result(
            Arc::new(session),
            Arc::new(turn),
            CancellationToken::new(),
            Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new())),
            call,
            ToolCallSource::Direct,
        )
        .await?;

    let response = result.into_response();
    match response {
        ResponseInputItem::FunctionCallOutput { call_id, output } => {
            assert_eq!(call_id, "call-extension");
            let FunctionCallOutputBody::Text(text) = output.body else {
                panic!("expected text function call output")
            };
            let value: serde_json::Value =
                serde_json::from_str(&text).expect("extension tool output should be json");
            assert_eq!(
                value,
                json!({
                    "arguments": { "message": "hello" },
                    "callId": "call-extension",
                    "conversationHistory": [history_item],
                    "ok": true,
                })
            );
        }
        other => panic!("expected function call output, got {other:?}"),
    }

    Ok(())
}

fn namespace_function_names(specs: &[ToolSpec], namespace_name: &str) -> Vec<String> {
    specs
        .iter()
        .find_map(|spec| match spec {
            ToolSpec::Namespace(namespace) if namespace.name == namespace_name => Some(
                namespace
                    .tools
                    .iter()
                    .map(|tool| match tool {
                        ResponsesApiNamespaceTool::Function(tool) => tool.name.clone(),
                    })
                    .collect(),
            ),
            ToolSpec::Function(_)
            | ToolSpec::Freeform(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. }
            | ToolSpec::AnthropicWebSearch { .. }
            | ToolSpec::OpenRouterWebSearch { .. }
            | ToolSpec::XaiWebSearch { .. }
            | ToolSpec::XiaomiWebSearch { .. }
            | ToolSpec::QwenWebSearch { .. }
            | ToolSpec::ZaiWebSearch { .. }
            | ToolSpec::Namespace(_) => None,
        })
        .unwrap_or_default()
}
