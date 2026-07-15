use super::*;
use crate::app_event::ConnectorsSnapshot;
use crate::chatwidget::connectors::ConnectorsCacheState;
use crate::status::RATE_LIMIT_STALE_THRESHOLD_MINUTES;
use base64::Engine;
use codex_app_server_protocol::AppInfo;
use codex_app_server_protocol::AuthMode;
use codex_app_server_protocol::HookErrorInfo;
use codex_app_server_protocol::HooksListEntry;
use codex_app_server_protocol::HooksListResponse;
use codex_app_server_protocol::MarketplaceRemoveResponse;
use codex_app_server_protocol::McpAuthStatus;
use codex_app_server_protocol::McpServerStatus;
use codex_app_server_protocol::RequestId;
use codex_config::types::AuthCredentialsStoreMode;
use codex_config::types::McpServerConfig;
use codex_config::types::McpServerTransportConfig;
use codex_features::Stage;
use codex_login::AuthDotJson;
use codex_login::AuthProfileSubscriptionProvider;
use codex_login::TokenData;
use codex_login::list_auth_profiles;
use codex_login::save_auth_profile;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::mcp::McpServerInfo;
use codex_protocol::mcp::Resource;
use codex_protocol::mcp::ResourceTemplate;
use codex_protocol::mcp::Tool;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::sync::Arc;

fn provider_picker_preset(
    slug: &str,
    default_reasoning_effort: ReasoningEffortConfig,
) -> ModelPreset {
    let reasoning_description = match default_reasoning_effort {
        ReasoningEffortConfig::None => "none",
        ReasoningEffortConfig::Minimal => "minimal",
        ReasoningEffortConfig::Low => "low",
        ReasoningEffortConfig::Medium => "medium",
        ReasoningEffortConfig::High => "high",
        ReasoningEffortConfig::XHigh => "extra high",
        ReasoningEffortConfig::Custom(ref effort) => effort.as_str(),
    }
    .to_string();

    ModelPreset {
        id: slug.to_string(),
        model: slug.to_string(),
        display_name: slug.to_string(),
        description: format!("{slug} description"),
        default_reasoning_effort: default_reasoning_effort.clone(),
        supported_reasoning_efforts: vec![ReasoningEffortPreset {
            effort: default_reasoning_effort,
            description: reasoning_description,
        }],
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        is_default: false,
        upgrade: None,
        show_in_picker: true,
        availability_nux: None,
        supported_in_api: true,
        input_modalities: default_input_modalities(),
    }
}

fn provider_picker_preset_with_reasoning(
    slug: &str,
    default_reasoning_effort: ReasoningEffortConfig,
    supported_reasoning_efforts: &[ReasoningEffortConfig],
) -> ModelPreset {
    let mut preset = provider_picker_preset(slug, default_reasoning_effort);
    preset.supported_reasoning_efforts = supported_reasoning_efforts
        .iter()
        .map(|effort| ReasoningEffortPreset {
            effort: effort.clone(),
            description: match effort {
                ReasoningEffortConfig::None => "none",
                ReasoningEffortConfig::Minimal => "minimal",
                ReasoningEffortConfig::Low => "low",
                ReasoningEffortConfig::Medium => "medium",
                ReasoningEffortConfig::High => "high",
                ReasoningEffortConfig::XHigh => "extra high",
                ReasoningEffortConfig::Custom(effort) => effort.as_str(),
            }
            .to_string(),
        })
        .collect();
    preset
}

fn save_popup_auth_profile(chat: &ChatWidget, name: &str) {
    save_auth_profile(
        &chat.config.codex_home,
        AuthCredentialsStoreMode::File,
        name,
        &AuthDotJson {
            auth_mode: Some(AuthMode::ApiKey),
            openai_api_key: Some(format!("{name}-key")),
            personal_access_token: None,
            tokens: None,
            last_refresh: None,
            agent_identity: None,
        },
    )
    .expect("save auth profile");
}

fn save_popup_chatgpt_auth_profile(chat: &ChatWidget, name: &str, email: &str) {
    let id_token = fake_chatgpt_jwt(email, &format!("{name}-account"), "pro");
    save_auth_profile(
        &chat.config.codex_home,
        AuthCredentialsStoreMode::File,
        name,
        &AuthDotJson {
            auth_mode: Some(AuthMode::Chatgpt),
            openai_api_key: None,
            personal_access_token: None,
            tokens: Some(TokenData {
                id_token: codex_login::token_data::parse_chatgpt_jwt_claims(&id_token)
                    .expect("id token should parse"),
                access_token: format!("{name}-access-token"),
                refresh_token: format!("{name}-refresh-token"),
                account_id: Some(format!("{name}-account")),
            }),
            last_refresh: None,
            agent_identity: None,
        },
    )
    .expect("save ChatGPT auth profile");
}

fn fake_chatgpt_jwt(email: &str, account_id: &str, plan_type: &str) -> String {
    let payload = serde_json::json!({
        "email": email,
        "https://api.openai.com/auth": {
            "chatgpt_account_id": account_id,
            "chatgpt_plan_type": plan_type,
        },
    });
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = encode(br#"{"alg":"none","typ":"JWT"}"#);
    let payload_b64 = encode(&serde_json::to_vec(&payload).expect("serialize payload"));
    let signature_b64 = encode(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}

fn mcp_test_statuses() -> Vec<McpServerStatus> {
    vec![
        mcp_test_server(
            "docs",
            McpAuthStatus::NotLoggedIn,
            vec![mcp_tool(
                "search_docs",
                Some("Search docs"),
                Some("Search indexed documentation."),
            )],
            vec![mcp_resource(
                "handbook",
                "file:///workspace/handbook.md",
                Some("Team handbook"),
            )],
            vec![mcp_resource_template(
                "ticket",
                "ticket://{id}",
                Some("Ticket by ID"),
            )],
        ),
        mcp_test_server(
            "linear",
            McpAuthStatus::BearerToken,
            vec![mcp_tool(
                "create_issue",
                Some("Create issue"),
                Some("Create a Linear issue."),
            )],
            Vec::new(),
            Vec::new(),
        ),
    ]
}

fn mcp_test_server(
    name: &str,
    auth_status: McpAuthStatus,
    tools: Vec<Tool>,
    resources: Vec<Resource>,
    resource_templates: Vec<ResourceTemplate>,
) -> McpServerStatus {
    McpServerStatus {
        name: name.to_string(),
        server_info: Some(McpServerInfo {
            name: format!("{name}-server"),
            title: Some(format!("{name} MCP")),
            version: "1.2.3".to_string(),
            description: Some(format!("{name} server description")),
            icons: None,
            website_url: Some(format!("https://example.com/{name}")),
        }),
        tools: tools
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect::<HashMap<_, _>>(),
        resources,
        resource_templates,
        auth_status,
    }
}

fn mcp_tool(name: &str, title: Option<&str>, description: Option<&str>) -> Tool {
    Tool {
        name: name.to_string(),
        title: title.map(str::to_string),
        description: description.map(str::to_string),
        input_schema: json!({"type": "object", "properties": {}}),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    }
}

fn mcp_test_config(enabled: bool, disabled_tools: Option<Vec<&str>>) -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@scope/mcp-server".to_string()],
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        },
        environment_id: "local".to_string(),
        enabled,
        required: false,
        supports_parallel_tool_calls: false,
        disabled_reason: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        default_tools_approval_mode: None,
        enabled_tools: None,
        disabled_tools: disabled_tools.map(|tools| tools.into_iter().map(str::to_string).collect()),
        scopes: None,
        oauth: None,
        oauth_resource: None,
        tools: HashMap::new(),
    }
}

fn set_mcp_test_config(chat: &mut ChatWidget, entries: Vec<(&str, McpServerConfig)>) {
    chat.config.mcp_servers = crate::legacy_core::config::Constrained::allow_any(
        entries
            .into_iter()
            .map(|(name, config)| (name.to_string(), config))
            .collect(),
    );
}

fn mcp_resource(name: &str, uri: &str, description: Option<&str>) -> Resource {
    Resource {
        annotations: None,
        description: description.map(str::to_string),
        mime_type: Some("text/markdown".to_string()),
        name: name.to_string(),
        size: None,
        title: Some(format!("{name} resource")),
        uri: uri.to_string(),
        icons: None,
        meta: None,
    }
}

fn mcp_resource_template(
    name: &str,
    uri_template: &str,
    description: Option<&str>,
) -> ResourceTemplate {
    ResourceTemplate {
        annotations: None,
        uri_template: uri_template.to_string(),
        name: name.to_string(),
        title: Some(format!("{name} template")),
        description: description.map(str::to_string),
        mime_type: Some("application/json".to_string()),
    }
}

#[tokio::test]
async fn realtime_error_closes_without_followup_closed_info() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.realtime_conversation.phase = RealtimeConversationPhase::Active;

    chat.on_realtime_error(ThreadRealtimeErrorNotification {
        thread_id: ThreadId::new().to_string(),
        message: "boom".to_string(),
    });
    next_realtime_close_op(&mut op_rx);

    chat.on_realtime_conversation_closed(ThreadRealtimeClosedNotification {
        thread_id: ThreadId::new().to_string(),
        reason: Some("error".to_string()),
    });

    let rendered = drain_insert_history(&mut rx)
        .into_iter()
        .map(|lines| lines_to_single_string(&lines))
        .collect::<Vec<_>>();
    insta::assert_snapshot!(rendered.join("\n\n"), @"■ Realtime voice error: boom");
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn deleted_realtime_meter_uses_shared_stop_path() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.realtime_conversation.phase = RealtimeConversationPhase::Active;
    let placeholder_id = chat.bottom_pane.insert_recording_meter_placeholder("⠤⠤⠤⠤");
    chat.realtime_conversation.meter_placeholder_id = Some(placeholder_id.clone());

    assert!(chat.stop_realtime_conversation_for_deleted_meter(&placeholder_id));

    next_realtime_close_op(&mut op_rx);
    assert_eq!(chat.realtime_conversation.meter_placeholder_id, None);
    assert_eq!(
        chat.realtime_conversation.phase,
        RealtimeConversationPhase::Stopping
    );
}

#[tokio::test]
async fn experimental_mode_plan_is_ignored_on_startup() {
    let codex_home = tempdir().expect("tempdir");
    let cfg = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .cli_overrides(vec![
            (
                "features.collaboration_modes".to_string(),
                TomlValue::Boolean(true),
            ),
            (
                "tui.experimental_mode".to_string(),
                TomlValue::String("plan".to_string()),
            ),
        ])
        .build()
        .await
        .expect("config");
    let resolved_model = crate::legacy_core::test_support::get_model_offline(cfg.model.as_deref());
    let session_telemetry = test_session_telemetry(&cfg, resolved_model.as_str());
    let init = ChatWidgetInit {
        config: cfg.clone(),
        frame_requester: FrameRequester::test_dummy(),
        app_event_tx: AppEventSender::new(unbounded_channel::<AppEvent>().0),
        workspace_command_runner: None,
        initial_user_message: None,
        enhanced_keys_supported: false,
        has_chatgpt_account: false,
        model_catalog: test_model_catalog(&cfg),
        feedback: codex_feedback::CodexFeedback::new(),
        is_first_run: true,
        status_account_display: None,
        runtime_model_provider_base_url: None,
        initial_plan_type: None,
        model: Some(resolved_model.clone()),
        startup_tooltip_override: None,
        status_line_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        terminal_title_invalid_items_warned: Arc::new(AtomicBool::new(false)),
        session_telemetry,
    };

    let chat = ChatWidget::new_with_app_event(init);
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Default);
    assert_eq!(chat.current_model(), resolved_model);
}

#[tokio::test]
async fn plugins_popup_loading_state_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    chat.add_plugins_output();

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("Loading available plugins..."),
        "expected /plugins to open in a loading state before the marketplace arrives, got:\n{popup}"
    );
    assert_chatwidget_snapshot!("plugins_popup_loading_state", popup);
}

#[tokio::test]
async fn mcp_control_center_popup_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_mcp_control_center();

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("View all MCPs"));
    assert!(popup.contains("Add new MCP"));
    assert_chatwidget_snapshot!("mcp_control_center_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenMcpManager {
            detail: McpServerStatusDetail::Full,
        })
    );
}

#[tokio::test]
async fn mcp_agent_mutation_approval_popup_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let request_id = RequestId::String("mcp-request-1".to_string());

    chat.open_mcp_agent_mutation_confirmation(
        request_id.clone(),
        McpAgentMutationApprovalSummary {
            title: "Add MCP server `docs`".to_string(),
            rows: vec![
                ("Server".to_string(), "docs".to_string()),
                (
                    "Transport".to_string(),
                    "stdio; command: npx".to_string(),
                ),
                (
                    "Args / cwd".to_string(),
                    "args: -y, @scope/docs-mcp, --project, docs; cwd: /workspace/docs"
                        .to_string(),
                ),
                (
                    "Env / headers".to_string(),
                    "env vars: DOCS_API_KEY; headers: not applicable".to_string(),
                ),
                (
                    "Tool config".to_string(),
                    "default approval: prompt; enabled: none; disabled: none".to_string(),
                ),
                (
                    "Scope / refresh".to_string(),
                    "user config.toml mcp_servers.<name>; MCP refresh is queued for loaded threads; new tools are available before the next turn."
                        .to_string(),
                ),
            ],
        },
    );

    let popup = render_bottom_popup(&chat, /*width*/ 112);
    assert!(popup.contains("Approve MCP Config Change"));
    assert!(popup.contains("Approve and save"));
    assert!(popup.contains("DOCS_API_KEY"));
    assert_chatwidget_snapshot!("mcp_agent_mutation_approval_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::DenyAgentMcpMutation { request_id: id }) if id == request_id
    );

    chat.open_mcp_agent_mutation_confirmation(
        request_id.clone(),
        McpAgentMutationApprovalSummary {
            title: "Enable MCP server `docs`".to_string(),
            rows: vec![
                ("Server".to_string(), "docs".to_string()),
                ("Change".to_string(), "enabled = true".to_string()),
            ],
        },
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::ConfirmAgentMcpMutation { request_id: id }) if id == request_id
    );
}

#[tokio::test]
async fn background_terminal_manager_popup_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.unified_exec_processes.push(UnifiedExecProcessSummary {
        key: "proc-1".to_string(),
        call_id: "call-1".to_string(),
        command_display: "bun run dev".to_string(),
        recent_chunks: vec!["ready on :3000".to_string(), "compiled /app".to_string()],
    });

    chat.open_background_terminal_manager();

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("Stop all"));
    assert!(popup.contains("Print snapshot"));
    assert!(popup.contains("bun run dev"));
    assert_chatwidget_snapshot!("background_terminal_manager_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(rx.try_recv(), Ok(AppEvent::PrintBackgroundTerminals));

    chat.open_background_terminal_manager();
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenBackgroundTerminalStopConfirmation)
    );

    chat.open_background_terminal_stop_confirmation();
    let confirmation = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        confirmation.contains("Stop Background Terminals"),
        "got confirmation:\n{confirmation}"
    );
    assert!(
        confirmation.contains("Back"),
        "got confirmation:\n{confirmation}"
    );
    assert!(
        confirmation.contains("Stop all"),
        "got confirmation:\n{confirmation}"
    );
    assert!(
        confirmation.contains("running background terminal"),
        "got confirmation:\n{confirmation}"
    );
    assert_chatwidget_snapshot!("background_terminal_stop_confirmation_popup", confirmation);

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenBackgroundTerminalManager));

    chat.open_background_terminal_stop_confirmation();
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(rx.try_recv(), Ok(AppEvent::StopBackgroundTerminals));
}

#[tokio::test]
async fn background_terminal_manager_empty_allows_print_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_background_terminal_manager();

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("Print snapshot"), "got popup:\n{popup}");
    assert!(
        popup.contains("No background terminals"),
        "got popup:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    assert_matches!(rx.try_recv(), Ok(AppEvent::PrintBackgroundTerminals));
}

#[tokio::test]
async fn mcp_manager_loading_popup_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    chat.open_mcp_manager(McpServerStatusDetail::Full);

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("Loading configured servers"));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::FetchMcpInventory {
            detail: McpServerStatusDetail::Full,
            thread_id: Some(actual_thread_id),
            target: crate::app_event::McpInventoryTarget::Manager,
        }) if actual_thread_id == thread_id
    );
    assert_chatwidget_snapshot!("mcp_manager_loading_popup", popup);
}

#[tokio::test]
async fn mcp_manager_empty_popup_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_mcp_manager_loaded(
        Ok(Vec::new()),
        McpServerStatusDetail::Full,
        /*thread_id*/ None,
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("No MCP servers configured"));
    assert!(popup.contains("Add new MCP"));
    assert!(!popup.contains("Reload MCP tools"));
    assert_chatwidget_snapshot!("mcp_manager_empty_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenMcpAddServer));
}

#[tokio::test]
async fn mcp_manager_configured_only_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_mcp_test_config(
        &mut chat,
        vec![(
            "docs",
            mcp_test_config(/*enabled*/ false, /*disabled_tools*/ None),
        )],
    );

    chat.on_mcp_manager_loaded(
        Ok(Vec::new()),
        McpServerStatusDetail::Full,
        /*thread_id*/ None,
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("0 servers, 0 tools, 1 configured, 1 disabled"));
    assert!(popup.contains("docs"));
    assert!(popup.contains("Disabled in config"));
    assert!(!popup.contains("No MCP servers configured"));
    assert_chatwidget_snapshot!("mcp_manager_configured_only_popup", popup);
}

#[tokio::test]
async fn mcp_manager_failure_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_mcp_manager_loaded(
        Err("app-server unavailable".to_string()),
        McpServerStatusDetail::Full,
        /*thread_id*/ None,
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("Inventory unavailable"));
    assert_chatwidget_snapshot!("mcp_manager_failure_popup", popup);
}

#[tokio::test]
async fn mcp_manager_ready_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.on_mcp_manager_loaded(
        Ok(mcp_test_statuses()),
        McpServerStatusDetail::Full,
        /*thread_id*/ None,
    );

    let popup = render_bottom_popup(&chat, /*width*/ 112);
    assert!(popup.contains("2 servers, 2 tools, 1 need login"));
    assert!(
        popup.find("docs").expect("docs row") < popup.find("linear").expect("linear row"),
        "expected login-needed server to sort first, got:\n{popup}"
    );
    assert_chatwidget_snapshot!("mcp_manager_ready_popup", popup);
}

#[tokio::test]
async fn mcp_manager_server_detail_snapshot_and_oauth_action() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let docs = mcp_test_statuses()
        .into_iter()
        .find(|status| status.name == "docs")
        .expect("docs MCP status");

    chat.open_mcp_server_details(docs);

    let popup = render_bottom_popup(&chat, /*width*/ 112);
    assert!(popup.contains("OAuth login"));
    assert!(popup.contains("Diagnose"));
    assert!(popup.contains("Managed outside mcp_servers"));
    assert!(!popup.contains("Reload MCP tools"));
    assert!(popup.contains("Search docs"));
    assert_chatwidget_snapshot!("mcp_manager_server_detail_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::StartMcpServerOauthLogin { name }) if name == "docs"
    );
}

#[tokio::test]
async fn mcp_manager_server_and_tool_config_actions() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_mcp_test_config(
        &mut chat,
        vec![(
            "linear",
            mcp_test_config(/*enabled*/ true, Some(vec!["create_issue"])),
        )],
    );
    let linear = mcp_test_statuses()
        .into_iter()
        .find(|status| status.name == "linear")
        .expect("linear MCP status");

    chat.open_mcp_server_details(linear.clone());

    let popup = render_bottom_popup(&chat, /*width*/ 112);
    assert!(popup.contains("Disable MCP server"));
    assert!(popup.contains("Disabled · Create a Linear issue."));

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::SetMcpServerEnabled { name, enabled }) if name == "linear" && !enabled
    );

    chat.open_mcp_server_details(linear);
    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    }
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::SetMcpToolEnabled {
            server,
            tool,
            enabled,
        }) if server == "linear" && tool == "create_issue" && enabled
    );
}

#[tokio::test]
async fn marketplace_upgrade_loading_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    chat.open_marketplace_upgrade_loading_popup(Some("debug"));

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    let upgrade_lines = popup
        .lines()
        .map(str::trim)
        .filter(|line| line.contains("Upgrading"))
        .collect::<Vec<_>>()
        .join(" | ");
    insta::assert_snapshot!(
        upgrade_lines,
        @"Upgrading debug marketplace... | ›    Upgrading debug marketplace...  This updates when marketplace upgrade completes."
    );
}

#[tokio::test]
async fn marketplace_upgrade_failure_includes_backend_messages_snapshot() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);
    let cwd = chat.config.cwd.clone();

    chat.on_marketplace_upgrade_loaded(
        cwd.to_path_buf(),
        Ok(MarketplaceUpgradeResponse {
            selected_marketplaces: vec!["debug".to_string(), "tools".to_string()],
            upgraded_roots: Vec::new(),
            errors: vec![
                MarketplaceUpgradeErrorInfo {
                    marketplace_name: "debug".to_string(),
                    message: "git ls-remote marketplace source failed with status 128: authentication failed".to_string(),
                },
                MarketplaceUpgradeErrorInfo {
                    marketplace_name: "tools".to_string(),
                    message: "failed to validate upgraded marketplace root: marketplace root does not contain a supported manifest".to_string(),
                },
            ],
        }),
    );

    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(
        rendered.trim(),
        @"■ Failed to upgrade 2 marketplaces: debug: git ls-remote marketplace source failed with status 128: authentication failed; tools: failed to validate upgraded marketplace root: marketplace root does not contain a supported manifest"
    );
}

#[tokio::test]
async fn hooks_popup_shows_list_diagnostics() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let cwd = chat.config.cwd.clone();

    chat.on_hooks_loaded(
        cwd.to_path_buf(),
        Ok(HooksListResponse {
            data: vec![HooksListEntry {
                cwd: cwd.to_path_buf(),
                hooks: Vec::new(),
                warnings: vec!["skipped invalid matcher for PreToolUse".to_string()],
                errors: vec![HookErrorInfo {
                    path: test_path_buf("/tmp/hooks.json"),
                    message: "failed to parse hooks config".to_string(),
                }],
            }],
        }),
    );

    let popup = normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 112));
    assert_chatwidget_snapshot!("hooks_popup_shows_list_diagnostics", popup);
}

#[tokio::test]
async fn plugins_popup_snapshot_shows_all_marketplaces_and_sorts_installed_then_name() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let response = plugins_test_response(vec![
        plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-bravo",
                "bravo",
                Some("Bravo Search"),
                Some("Search docs and tickets."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-alpha",
                "alpha",
                Some("Alpha Sync"),
                Some("Already installed but disabled."),
                /*installed*/ true,
                /*enabled*/ false,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-starter",
                "starter",
                Some("Starter"),
                Some("Included by default."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::InstalledByDefault,
            ),
        ]),
        plugins_test_repo_marketplace(vec![plugins_test_summary(
            "plugin-hidden",
            "hidden",
            Some("Hidden Repo Plugin"),
            Some("Should not be shown in /plugins."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        )]),
    ]);
    let popup = render_loaded_plugins_popup(&mut chat, response);
    assert_chatwidget_snapshot!("plugins_popup_curated_marketplace", popup);
    assert!(
        popup.contains("Hidden Repo Plugin"),
        "expected /plugins to include non-curated marketplaces, got:\n{popup}"
    );
    assert!(
        plugins_test_popup_row_position(&popup, "Alpha Sync")
            < plugins_test_popup_row_position(&popup, "Bravo Search")
            && plugins_test_popup_row_position(&popup, "Bravo Search")
                < plugins_test_popup_row_position(&popup, "Hidden Repo Plugin")
            && plugins_test_popup_row_position(&popup, "Hidden Repo Plugin")
                < plugins_test_popup_row_position(&popup, "Starter"),
        "expected /plugins rows to sort installed plugins first, then alphabetically, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_truncates_long_descriptions_in_list_rows() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let response = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        plugins_test_summary(
            "plugin-alpha",
            "alpha",
            Some("Alpha"),
            Some("Short description."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
        plugins_test_summary(
            "plugin-verbose",
            "verbose",
            Some("Verbose Plugin"),
            Some("This description keeps going and going until the row would normally wrap."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
    ])]);

    let cwd = chat.config.cwd.to_path_buf();
    chat.on_plugins_loaded(cwd, Ok(response));
    chat.add_plugins_output();

    let popup = render_bottom_popup(&chat, /*width*/ 70);
    let verbose_row = popup
        .lines()
        .find(|line| line.contains("Verbose Plugin"))
        .expect("expected verbose plugin row in popup");
    insta::assert_snapshot!(
        verbose_row,
        @"  [-] Verbose Plugin  Available · ChatGPT Marketplace · This descri…"
    );
    assert!(
        !popup
            .contains("This description keeps going and going until the row would normally wrap."),
        "expected the long plugin description to truncate instead of wrapping, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_add_marketplace_tab_opens_prompt_and_submits_source() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let cwd = chat.config.cwd.to_path_buf();
    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(Vec::new())]),
    );

    while rx.try_recv().is_ok() {}
    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    }

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("Add a marketplace from a Git repo or local root."),
        "expected Add Marketplace tab, got:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    match rx.try_recv() {
        Ok(AppEvent::OpenMarketplaceAddPrompt) => {}
        other => panic!("expected OpenMarketplaceAddPrompt event, got {other:?}"),
    }

    chat.open_marketplace_add_prompt();
    let prompt = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        prompt.contains("owner/repo, git URL, or local marketplace path"),
        "expected marketplace source prompt, got:\n{prompt}"
    );

    chat.handle_paste("owner/repo".to_string());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match rx.try_recv() {
        Ok(AppEvent::OpenMarketplaceAddLoading { source }) => {
            assert_eq!(source, "owner/repo");
        }
        other => panic!("expected OpenMarketplaceAddLoading event, got {other:?}"),
    }
    match rx.try_recv() {
        Ok(AppEvent::FetchMarketplaceAdd {
            cwd: event_cwd,
            source,
        }) => {
            assert_eq!(event_cwd, cwd);
            assert_eq!(source, "owner/repo");
        }
        other => panic!("expected FetchMarketplaceAdd event, got {other:?}"),
    }
}

#[tokio::test]
async fn plugins_popup_upgrades_user_configured_git_marketplace_from_marketplace_tab() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let cwd = chat.config.cwd.to_path_buf();
    let temp = tempdir().expect("tempdir");
    let config_toml_path = temp.path().join("config.toml").abs();
    chat.config.config_layer_stack = ConfigLayerStack::default().with_user_config(
        &config_toml_path,
        toml::from_str::<TomlValue>(
            "[marketplaces.repo]\nsource_type = \"git\"\nsource = \"https://github.com/owner/repo.git\"\n",
        )
        .expect("marketplace config"),
    );

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![
            plugins_test_curated_marketplace(Vec::new()),
            plugins_test_repo_marketplace(vec![plugins_test_summary(
                "plugin-debug",
                "debug",
                Some("Debug Plugin"),
                Some("Debug marketplace plugin."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            )]),
        ]),
    );

    while rx.try_recv().is_ok() {}
    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    }

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("Repo Marketplace.")
            && popup.contains("ctrl + u upgrade")
            && popup.contains("ctrl + r remove")
            && popup.contains("Debug Plugin"),
        "expected upgradeable user-configured marketplace tab, got:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

    match rx.try_recv() {
        Ok(AppEvent::OpenMarketplaceUpgradeLoading { marketplace_name }) => {
            assert_eq!(marketplace_name, Some("repo".to_string()));
        }
        other => panic!("expected OpenMarketplaceUpgradeLoading event, got {other:?}"),
    }
    match rx.try_recv() {
        Ok(AppEvent::FetchMarketplaceUpgrade {
            cwd: event_cwd,
            marketplace_name,
        }) => {
            assert_eq!(event_cwd, cwd);
            assert_eq!(marketplace_name, Some("repo".to_string()));
        }
        other => panic!("expected FetchMarketplaceUpgrade event, got {other:?}"),
    }
    let no_more_events = rx.try_recv();
    assert!(
        no_more_events.is_err(),
        "expected no duplicate marketplace upgrade events, got {no_more_events:?}"
    );
}

#[tokio::test]
async fn marketplace_add_success_refreshes_to_new_marketplace_tab() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let cwd = chat.config.cwd.to_path_buf();
    let marketplace_root = plugins_test_absolute_path("marketplaces/debug");
    let marketplace_path =
        plugins_test_absolute_path("marketplaces/debug/.agents/plugins/marketplace.json");
    let temp = tempdir().expect("tempdir");
    let config_toml_path = temp.path().join("config.toml").abs();
    chat.config.config_layer_stack = ConfigLayerStack::default().with_user_config(
        &config_toml_path,
        toml::from_str::<TomlValue>(
            "[marketplaces.debug]\nsource_type = \"git\"\nsource = \"https://github.com/owner/debug.git\"\n",
        )
        .expect("marketplace config"),
    );
    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(Vec::new())]),
    );
    chat.open_marketplace_add_loading_popup("owner/repo");
    let loading_popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        !loading_popup.contains("owner/repo"),
        "expected marketplace loading popup to avoid echoing the source, got:\n{loading_popup}"
    );
    chat.on_marketplace_add_loaded(
        cwd.clone(),
        "owner/repo".to_string(),
        Ok(MarketplaceAddResponse {
            marketplace_name: "debug".to_string(),
            installed_root: marketplace_root,
            already_added: false,
        }),
    );
    chat.on_plugins_loaded(
        cwd,
        Ok(plugins_test_response(vec![
            plugins_test_curated_marketplace(Vec::new()),
            PluginMarketplaceEntry {
                name: "debug".to_string(),
                path: Some(marketplace_path),
                interface: Some(MarketplaceInterface {
                    display_name: Some("Debug Marketplace".to_string()),
                }),
                plugins: vec![plugins_test_summary(
                    "plugin-debug",
                    "debug",
                    Some("Debug Plugin"),
                    Some("Debug marketplace plugin."),
                    /*installed*/ false,
                    /*enabled*/ true,
                    PluginInstallPolicy::Available,
                )],
            },
        ])),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert_chatwidget_snapshot!("plugins_popup_newly_installed_marketplace", popup);
    assert!(
        popup.contains("Debug Marketplace installed successfully.")
            && popup.contains("ctrl + u upgrade")
            && popup.contains("ctrl + r remove")
            && popup.contains("Debug Plugin"),
        "expected marketplace add refresh to switch to the new marketplace tab, got:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Esc));
    chat.add_plugins_output();
    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    }

    let reopened_popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        reopened_popup.contains("Installed 0 of 1 Debug Marketplace plugins.")
            && !reopened_popup.contains("installed successfully"),
        "expected reopening the marketplace tab later to use the normal header, got:\n{reopened_popup}"
    );
}

#[tokio::test]
async fn plugins_popup_removes_user_configured_marketplace_flow() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);
    let cwd = chat.config.cwd.to_path_buf();
    let temp = tempdir().expect("tempdir");
    let config_toml_path = temp.path().join("config.toml").abs();
    chat.config.config_layer_stack = ConfigLayerStack::default().with_user_config(
        &config_toml_path,
        toml::from_str::<TomlValue>(
            "[marketplaces.repo]\nsource_type = \"git\"\nsource = \"https://github.com/owner/repo.git\"\n",
        )
        .expect("marketplace config"),
    );

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![
            plugins_test_curated_marketplace(Vec::new()),
            plugins_test_repo_marketplace(vec![plugins_test_summary(
                "plugin-debug",
                "debug",
                Some("Debug Plugin"),
                Some("Debug marketplace plugin."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            )]),
        ]),
    );
    while rx.try_recv().is_ok() {}

    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    }
    let repo_tab = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        repo_tab.contains("Repo Marketplace.")
            && repo_tab.contains("ctrl + u upgrade")
            && repo_tab.contains("ctrl + r remove")
            && repo_tab.contains("Debug Plugin"),
        "expected removable user-configured marketplace tab, got:\n{repo_tab}"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
    let confirmation = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        confirmation.contains("Remove Repo Marketplace marketplace?")
            && confirmation.contains("Remove marketplace")
            && confirmation.contains("Back to plugins"),
        "expected marketplace removal confirmation, got:\n{confirmation}"
    );
    assert_chatwidget_snapshot!(
        "plugins_popup_marketplace_remove_confirmation",
        confirmation
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let marketplace_display_name = match rx.try_recv() {
        Ok(AppEvent::OpenMarketplaceRemoveLoading {
            marketplace_display_name,
        }) => marketplace_display_name,
        other => panic!("expected OpenMarketplaceRemoveLoading event, got {other:?}"),
    };
    assert_eq!(marketplace_display_name, "Repo Marketplace");
    match rx.try_recv() {
        Ok(AppEvent::FetchMarketplaceRemove {
            cwd: event_cwd,
            marketplace_name,
            marketplace_display_name,
        }) => {
            assert_eq!(event_cwd, cwd);
            assert_eq!(marketplace_name, "repo");
            assert_eq!(marketplace_display_name, "Repo Marketplace");
        }
        other => panic!("expected FetchMarketplaceRemove event, got {other:?}"),
    }

    chat.open_marketplace_remove_loading_popup(&marketplace_display_name);
    let loading = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        loading.contains("Removing Repo Marketplace...")
            && loading.contains("Removing marketplace..."),
        "expected marketplace removal loading state, got:\n{loading}"
    );

    chat.on_marketplace_remove_loaded(
        cwd.clone(),
        "repo".to_string(),
        marketplace_display_name,
        Ok(MarketplaceRemoveResponse {
            marketplace_name: "repo".to_string(),
            installed_root: Some(plugins_test_absolute_path("marketplaces/repo")),
        }),
    );
    chat.on_plugins_loaded(
        cwd,
        Ok(plugins_test_response(vec![
            plugins_test_curated_marketplace(Vec::new()),
        ])),
    );

    let refreshed = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        refreshed.contains("Browse plugins from available marketplaces.")
            && !refreshed.contains("Repo Marketplace")
            && !refreshed.contains("Debug Plugin")
            && !refreshed.contains("ctrl + r remove"),
        "expected refreshed plugin list without removed marketplace, got:\n{refreshed}"
    );
}

#[tokio::test]
async fn plugin_detail_popup_snapshot_shows_install_actions_and_capability_summaries() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let summary = plugins_test_summary(
        "plugin-figma",
        "figma",
        Some("Figma"),
        Some("Design handoff."),
        /*installed*/ false,
        /*enabled*/ true,
        PluginInstallPolicy::Available,
    );
    let response = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        summary.clone(),
    ])]);
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(cwd.to_path_buf(), Ok(response));
    chat.add_plugins_output();
    chat.on_plugin_detail_loaded(
        cwd.to_path_buf(),
        Ok(PluginReadResponse {
            plugin: plugins_test_detail(
                summary,
                Some("Turn Figma files into implementation context."),
                &["design-review", "extract-copy"],
                &[
                    (codex_app_server_protocol::HookEventName::PreToolUse, 1),
                    (codex_app_server_protocol::HookEventName::Stop, 2),
                ],
                &[("Figma", true), ("Slack", false)],
                &["figma-mcp", "docs-mcp"],
            ),
        }),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert_chatwidget_snapshot!(
        "plugin_detail_popup_installable",
        strip_osc8_for_snapshot(&popup)
    );
}

#[tokio::test]
async fn plugin_detail_popup_hides_disclosure_for_installed_plugins() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let summary = plugins_test_summary(
        "plugin-figma",
        "figma",
        Some("Figma"),
        Some("Design handoff."),
        /*installed*/ true,
        /*enabled*/ true,
        PluginInstallPolicy::Available,
    );
    let response = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        summary.clone(),
    ])]);
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(cwd.to_path_buf(), Ok(response));
    chat.add_plugins_output();
    chat.on_plugin_detail_loaded(
        cwd.to_path_buf(),
        Ok(PluginReadResponse {
            plugin: plugins_test_detail(
                summary,
                Some("Turn Figma files into implementation context."),
                &["design-review", "extract-copy"],
                &[
                    (codex_app_server_protocol::HookEventName::PreToolUse, 1),
                    (codex_app_server_protocol::HookEventName::Stop, 2),
                ],
                &[("Figma", true), ("Slack", false)],
                &["figma-mcp", "docs-mcp"],
            ),
        }),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        !popup.contains("Data shared with this app is subject to the app's"),
        "expected installed plugin details to hide the disclosure line, got:\n{popup}"
    );
    assert_chatwidget_snapshot!(
        "plugin_detail_popup_installed",
        strip_osc8_for_snapshot(&popup)
    );
}

#[tokio::test]
async fn plugin_detail_error_popup_skips_disabled_row_numbering() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let response = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        plugins_test_summary(
            "plugin-figma",
            "figma",
            Some("Figma"),
            Some("Design handoff."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
    ])]);
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(cwd.to_path_buf(), Ok(response));
    chat.add_plugins_output();
    chat.on_plugin_detail_loaded(
        cwd.to_path_buf(),
        Err("Failed to load plugin details.".to_string()),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert_chatwidget_snapshot!("plugin_detail_error_popup", popup);
}

#[tokio::test]
async fn plugins_popup_refresh_preserves_selected_row_position() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let initial = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        plugins_test_summary(
            "plugin-notion",
            "notion",
            Some("Notion"),
            Some("Workspace docs."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
        plugins_test_summary(
            "plugin-slack",
            "slack",
            Some("Slack"),
            Some("Team chat."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
    ])]);
    render_loaded_plugins_popup(&mut chat, initial);
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));

    let before = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        before.contains("› [-] Slack"),
        "expected Slack to be selected before refresh, got:\n{before}"
    );

    let refreshed = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        plugins_test_summary(
            "plugin-airtable",
            "airtable",
            Some("Airtable"),
            Some("Structured records."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
        plugins_test_summary(
            "plugin-notion",
            "notion",
            Some("Notion"),
            Some("Workspace docs."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
        plugins_test_summary(
            "plugin-slack",
            "slack",
            Some("Slack"),
            Some("Team chat."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
    ])]);
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(cwd.to_path_buf(), Ok(refreshed));

    let after = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        after.contains("› [-] Notion"),
        "expected refresh to preserve the selected row position, got:\n{after}"
    );
    assert!(
        after.contains("Airtable"),
        "expected refreshed popup to include the updated plugin list, got:\n{after}"
    );
    assert!(
        after.contains("Slack"),
        "expected refreshed popup to include the updated plugin list, got:\n{after}"
    );
}

#[tokio::test]
async fn plugins_popup_refreshes_installed_counts_after_install() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let initial = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        plugins_test_summary(
            "plugin-calendar",
            "calendar",
            Some("Calendar"),
            Some("Schedule management."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
        plugins_test_summary(
            "plugin-drive",
            "drive",
            Some("Drive"),
            Some("Document access."),
            /*installed*/ true,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
    ])]);
    let before = render_loaded_plugins_popup(&mut chat, initial);
    assert!(
        before.contains("Installed 1 of 2 available plugins."),
        "expected initial installed count before refresh, got:\n{before}"
    );
    assert!(
        before.contains("Available"),
        "expected pre-install popup copy before refresh, got:\n{before}"
    );

    let refreshed = plugins_test_response(vec![plugins_test_curated_marketplace(vec![
        plugins_test_summary(
            "plugin-calendar",
            "calendar",
            Some("Calendar"),
            Some("Schedule management."),
            /*installed*/ true,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
        plugins_test_summary(
            "plugin-drive",
            "drive",
            Some("Drive"),
            Some("Document access."),
            /*installed*/ true,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        ),
    ])]);
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(cwd.to_path_buf(), Ok(refreshed));

    let after = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        after.contains("Installed 2 of 2 available plugins."),
        "expected /plugins to refresh installed counts after install, got:\n{after}"
    );
    assert!(
        after.contains("Installed   Space to disable; Enter view details."),
        "expected refreshed selected row copy to reflect the installed plugin state, got:\n{after}"
    );
}

#[tokio::test]
async fn plugins_popup_space_toggles_installed_plugin_from_list() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let cwd = chat.config.cwd.to_path_buf();
    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ true,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-drive",
                "drive",
                Some("Drive"),
                Some("Document access."),
                /*installed*/ true,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
        ])]),
    );

    while rx.try_recv().is_ok() {}
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    match rx.try_recv() {
        Ok(AppEvent::SetPluginEnabled {
            cwd: event_cwd,
            plugin_id,
            enabled,
        }) => {
            assert_eq!(event_cwd, cwd);
            assert_eq!(plugin_id, "plugin-drive");
            assert!(!enabled);
        }
        other => panic!("expected SetPluginEnabled event, got {other:?}"),
    }

    chat.on_plugin_enabled_set(
        cwd,
        "plugin-drive".to_string(),
        /*enabled*/ false,
        Ok(()),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("› [ ] Drive"),
        "expected selected plugin row to stay selected after refresh, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_space_on_uninstalled_row_does_not_start_search() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-drive",
                "drive",
                Some("Drive"),
                Some("Document access."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
        ])]),
    );

    while rx.try_recv().is_ok() {}
    let before = render_bottom_popup(&chat, /*width*/ 100);
    chat.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let after = render_bottom_popup(&chat, /*width*/ 100);

    assert!(
        rx.try_recv().is_err(),
        "did not expect Space on an uninstalled plugin to emit an event"
    );
    assert_eq!(after, before);
}

#[tokio::test]
async fn plugins_popup_space_with_active_search_does_not_toggle_installed_plugin() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ true,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-drive",
                "drive",
                Some("Drive"),
                Some("Document access."),
                /*installed*/ true,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
        ])]),
    );

    while rx.try_recv().is_ok() {}
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    type_plugins_search_query(&mut chat, "dr");
    chat.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert!(
        rx.try_recv().is_err(),
        "did not expect Space with an active plugin search to emit a toggle event"
    );
}

#[tokio::test]
async fn plugins_popup_search_filters_visible_rows_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-slack",
                "slack",
                Some("Slack"),
                Some("Team chat."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-drive",
                "drive",
                Some("Drive"),
                Some("Document access."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
        ])]),
    );

    type_plugins_search_query(&mut chat, "sla");

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert_chatwidget_snapshot!("plugins_popup_search_filtered", popup);
    assert!(
        !popup.contains("Calendar") && !popup.contains("Drive"),
        "expected search to leave only matching rows visible, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_installed_tab_filters_rows_and_clears_search() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ true,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-slack",
                "slack",
                Some("Slack"),
                Some("Team chat."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
        ])]),
    );

    type_plugins_search_query(&mut chat, "sla");
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("Installed plugins.") && popup.contains("Showing 1 installed plugins."),
        "expected Installed tab header, got:\n{popup}"
    );
    assert!(
        popup.contains("Calendar") && !popup.contains("Slack"),
        "expected Installed tab to show only installed plugins, got:\n{popup}"
    );
    assert!(
        !popup.contains("sla"),
        "expected tab switch to clear search query, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_openai_curated_tab_omits_marketplace_in_rows() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![
            plugins_test_curated_marketplace(vec![plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            )]),
            plugins_test_repo_marketplace(vec![plugins_test_summary(
                "plugin-repo",
                "repo",
                Some("Repo Plugin"),
                Some("Repo-only plugin."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            )]),
        ]),
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    chat.handle_key_event(KeyEvent::from(KeyCode::Right));

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("Hasna Curated marketplace."),
        "expected Hasna Curated tab header, got:\n{popup}"
    );
    assert!(
        popup.contains("Calendar") && !popup.contains("Repo Plugin"),
        "expected Hasna Curated tab to show only official marketplace plugins, got:\n{popup}"
    );
    assert!(
        !popup.contains("ChatGPT Marketplace ·"),
        "expected marketplace-specific rows to omit marketplace labels, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_refresh_preserves_duplicate_marketplace_tab_by_path() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let response = plugins_test_response(vec![
        PluginMarketplaceEntry {
            name: "duplicate".to_string(),
            path: Some(plugins_test_absolute_path(
                "marketplaces/home/marketplace.json",
            )),
            interface: Some(MarketplaceInterface {
                display_name: Some("Duplicate Marketplace".to_string()),
            }),
            plugins: vec![plugins_test_summary(
                "plugin-home",
                "home",
                Some("Home Plugin"),
                Some("Home marketplace plugin."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            )],
        },
        PluginMarketplaceEntry {
            name: "duplicate".to_string(),
            path: Some(plugins_test_absolute_path(
                "marketplaces/repo/marketplace.json",
            )),
            interface: Some(MarketplaceInterface {
                display_name: Some("Duplicate Marketplace".to_string()),
            }),
            plugins: vec![plugins_test_summary(
                "plugin-repo",
                "repo",
                Some("Repo Plugin"),
                Some("Repo marketplace plugin."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            )],
        },
    ]);
    let cwd = chat.config.cwd.to_path_buf();
    chat.on_plugins_loaded(cwd.clone(), Ok(response.clone()));
    chat.add_plugins_output();

    for _ in 0..4 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    }

    chat.on_plugins_loaded(cwd, Ok(response));

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        popup.contains("Duplicate Marketplace (2/2)."),
        "expected refresh to preserve the second duplicate marketplace tab, got:\n{popup}"
    );
    assert!(
        popup.contains("Repo Plugin") && !popup.contains("Home Plugin"),
        "expected second duplicate marketplace rows after refresh, got:\n{popup}"
    );
}

#[tokio::test]
async fn plugins_popup_search_no_matches_and_backspace_restores_results() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![
            plugins_test_summary(
                "plugin-calendar",
                "calendar",
                Some("Calendar"),
                Some("Schedule management."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
            plugins_test_summary(
                "plugin-slack",
                "slack",
                Some("Slack"),
                Some("Team chat."),
                /*installed*/ false,
                /*enabled*/ true,
                PluginInstallPolicy::Available,
            ),
        ])]),
    );

    type_plugins_search_query(&mut chat, "zzz");

    let no_matches = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        no_matches.contains("zzz"),
        "expected popup to show the typed search query, got:\n{no_matches}"
    );
    assert!(
        no_matches.contains("no matches"),
        "expected popup to render the no-matches UX, got:\n{no_matches}"
    );

    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::from(KeyCode::Backspace));
    }

    let restored = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        restored.contains("Calendar") && restored.contains("Slack"),
        "expected clearing the query to restore the plugin rows, got:\n{restored}"
    );
    assert!(
        !restored.contains("no matches"),
        "did not expect the no-matches state after clearing the query, got:\n{restored}"
    );
}

#[tokio::test]
async fn apps_popup_stays_loading_until_final_snapshot_updates() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);
    let notion_id = "unit_test_apps_popup_refresh_connector_1";
    let linear_id = "unit_test_apps_popup_refresh_connector_2";

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: notion_id.to_string(),
                name: "Notion".to_string(),
                description: Some("Workspace docs".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/notion".to_string()),
                is_accessible: true,
                is_enabled: true,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ false,
    );
    chat.add_connectors_output();
    assert!(
        chat.connectors.prefetch_in_flight,
        "expected /apps to trigger a forced connectors refresh"
    );

    let before = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        before.contains("Loading installed and available apps..."),
        "expected /apps to stay in the loading state until the full list arrives, got:\n{before}"
    );
    assert_chatwidget_snapshot!("apps_popup_loading_state", before);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![
                AppInfo {
                    id: notion_id.to_string(),
                    name: "Notion".to_string(),
                    description: Some("Workspace docs".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/notion".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
                AppInfo {
                    id: linear_id.to_string(),
                    name: "Linear".to_string(),
                    description: Some("Project tracking".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/linear".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
            ],
        }),
        /*is_final*/ true,
    );

    let after = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        after.contains("Installed 2 of 2 available apps."),
        "expected refreshed apps popup snapshot, got:\n{after}"
    );
    assert!(
        after.contains("Linear"),
        "expected refreshed popup to include new connector, got:\n{after}"
    );
}

#[tokio::test]
async fn apps_notification_update_excludes_inaccessible_apps_from_mentions() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);
    chat.bottom_pane
        .set_composer_text("$".to_string(), Vec::new(), Vec::new());

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![
                AppInfo {
                    id: "google_drive".to_string(),
                    name: "Google Drive".to_string(),
                    description: Some("Connected files".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/google-drive".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
                AppInfo {
                    id: "arabica_uae".to_string(),
                    name: "% Arabica UAE".to_string(),
                    description: Some("Directory-only app".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/arabica".to_string()),
                    is_accessible: false,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
            ],
        }),
        /*is_final*/ false,
    );

    assert_matches!(
        &chat.connectors.partial_snapshot,
        Some(snapshot)
            if snapshot
                .connectors
                .iter()
                .find(|connector| connector.id == "arabica_uae")
                .is_some_and(|connector| !connector.is_accessible)
    );

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Google Drive"),
        "expected accessible apps to appear in the mention popup, got:\n{popup}"
    );
    assert!(
        !popup.contains("% Arabica UAE"),
        "did not expect an inaccessible directory app in the mention popup, got:\n{popup}"
    );
}

#[tokio::test]
async fn apps_refresh_failure_keeps_existing_full_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);
    let notion_id = "unit_test_apps_refresh_failure_connector_1";
    let linear_id = "unit_test_apps_refresh_failure_connector_2";

    let full_connectors = vec![
        AppInfo {
            id: notion_id.to_string(),
            name: "Notion".to_string(),
            description: Some("Workspace docs".to_string()),
            logo_url: None,
            logo_url_dark: None,
            distribution_channel: None,
            branding: None,
            app_metadata: None,
            labels: None,
            install_url: Some("https://example.test/notion".to_string()),
            is_accessible: true,
            is_enabled: true,
            plugin_display_names: Vec::new(),
        },
        AppInfo {
            id: linear_id.to_string(),
            name: "Linear".to_string(),
            description: Some("Project tracking".to_string()),
            logo_url: None,
            logo_url_dark: None,
            distribution_channel: None,
            branding: None,
            app_metadata: None,
            labels: None,
            install_url: Some("https://example.test/linear".to_string()),
            is_accessible: false,
            is_enabled: true,
            plugin_display_names: Vec::new(),
        },
    ];
    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: full_connectors.clone(),
        }),
        /*is_final*/ true,
    );

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: notion_id.to_string(),
                name: "Notion".to_string(),
                description: Some("Workspace docs".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/notion".to_string()),
                is_accessible: true,
                is_enabled: true,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ false,
    );
    chat.on_connectors_loaded(
        Err("failed to load apps".to_string()),
        /*is_final*/ true,
    );

    assert_matches!(
        &chat.connectors.cache,
        ConnectorsCacheState::Ready(snapshot) if snapshot.connectors == full_connectors
    );

    chat.add_connectors_output();
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Installed 1 of 2 available apps."),
        "expected previous full snapshot to be preserved, got:\n{popup}"
    );
}

#[tokio::test]
async fn apps_popup_preserves_selected_app_across_refresh() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![
                AppInfo {
                    id: "notion".to_string(),
                    name: "Notion".to_string(),
                    description: Some("Workspace docs".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/notion".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
                AppInfo {
                    id: "slack".to_string(),
                    name: "Slack".to_string(),
                    description: Some("Team chat".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/slack".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
            ],
        }),
        /*is_final*/ true,
    );
    chat.add_connectors_output();
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));

    let before = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        before.contains("› Slack"),
        "expected Slack to be selected before refresh, got:\n{before}"
    );

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![
                AppInfo {
                    id: "airtable".to_string(),
                    name: "Airtable".to_string(),
                    description: Some("Spreadsheets".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/airtable".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
                AppInfo {
                    id: "notion".to_string(),
                    name: "Notion".to_string(),
                    description: Some("Workspace docs".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/notion".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
                AppInfo {
                    id: "slack".to_string(),
                    name: "Slack".to_string(),
                    description: Some("Team chat".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/slack".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
            ],
        }),
        /*is_final*/ true,
    );

    let after = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        after.contains("› Slack"),
        "expected Slack to stay selected after refresh, got:\n{after}"
    );
    assert!(
        !after.contains("› Notion"),
        "did not expect selection to reset to Notion after refresh, got:\n{after}"
    );
}

#[tokio::test]
async fn apps_refresh_failure_with_cached_snapshot_triggers_pending_force_refetch() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);
    chat.connectors.prefetch_in_flight = true;
    chat.connectors.force_refetch_pending = true;

    let full_connectors = vec![AppInfo {
        id: "unit_test_apps_refresh_failure_pending_connector".to_string(),
        name: "Notion".to_string(),
        description: Some("Workspace docs".to_string()),
        logo_url: None,
        logo_url_dark: None,
        distribution_channel: None,
        branding: None,
        app_metadata: None,
        labels: None,
        install_url: Some("https://example.test/notion".to_string()),
        is_accessible: true,
        is_enabled: true,
        plugin_display_names: Vec::new(),
    }];
    chat.connectors.cache = ConnectorsCacheState::Ready(ConnectorsSnapshot {
        connectors: full_connectors.clone(),
    });

    chat.on_connectors_loaded(
        Err("failed to load apps".to_string()),
        /*is_final*/ true,
    );

    assert!(chat.connectors.prefetch_in_flight);
    assert!(!chat.connectors.force_refetch_pending);
    assert_matches!(
        &chat.connectors.cache,
        ConnectorsCacheState::Ready(snapshot) if snapshot.connectors == full_connectors
    );
}

#[tokio::test]
async fn apps_popup_keeps_existing_full_snapshot_while_partial_refresh_loads() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);

    let full_connectors = vec![
        AppInfo {
            id: "unit_test_connector_1".to_string(),
            name: "Notion".to_string(),
            description: Some("Workspace docs".to_string()),
            logo_url: None,
            logo_url_dark: None,
            distribution_channel: None,
            branding: None,
            app_metadata: None,
            labels: None,
            install_url: Some("https://example.test/notion".to_string()),
            is_accessible: true,
            is_enabled: true,
            plugin_display_names: Vec::new(),
        },
        AppInfo {
            id: "unit_test_connector_2".to_string(),
            name: "Linear".to_string(),
            description: Some("Project tracking".to_string()),
            logo_url: None,
            logo_url_dark: None,
            distribution_channel: None,
            branding: None,
            app_metadata: None,
            labels: None,
            install_url: Some("https://example.test/linear".to_string()),
            is_accessible: false,
            is_enabled: true,
            plugin_display_names: Vec::new(),
        },
    ];
    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: full_connectors.clone(),
        }),
        /*is_final*/ true,
    );
    chat.add_connectors_output();

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![
                AppInfo {
                    id: "unit_test_connector_1".to_string(),
                    name: "Notion".to_string(),
                    description: Some("Workspace docs".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/notion".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
                AppInfo {
                    id: "connector_openai_hidden".to_string(),
                    name: "Hidden OpenAI".to_string(),
                    description: Some("Should be filtered".to_string()),
                    logo_url: None,
                    logo_url_dark: None,
                    distribution_channel: None,
                    branding: None,
                    app_metadata: None,
                    labels: None,
                    install_url: Some("https://example.test/hidden-openai".to_string()),
                    is_accessible: true,
                    is_enabled: true,
                    plugin_display_names: Vec::new(),
                },
            ],
        }),
        /*is_final*/ false,
    );

    assert_matches!(
        &chat.connectors.cache,
        ConnectorsCacheState::Ready(snapshot) if snapshot.connectors == full_connectors
    );

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Installed 1 of 2 available apps."),
        "expected popup to keep the last full snapshot while partial refresh loads, got:\n{popup}"
    );
    assert!(
        !popup.contains("Hidden OpenAI"),
        "expected popup to ignore partial refresh rows until the full list arrives, got:\n{popup}"
    );
}

#[tokio::test]
async fn apps_refresh_failure_without_full_snapshot_falls_back_to_installed_apps() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: "unit_test_apps_refresh_failure_fallback_connector".to_string(),
                name: "Notion".to_string(),
                description: Some("Workspace docs".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/notion".to_string()),
                is_accessible: true,
                is_enabled: true,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ false,
    );

    chat.add_connectors_output();
    let loading_popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        loading_popup.contains("Loading installed and available apps..."),
        "expected /apps to keep showing loading before the final result, got:\n{loading_popup}"
    );

    chat.on_connectors_loaded(
        Err("failed to load apps".to_string()),
        /*is_final*/ true,
    );

    assert_matches!(
        &chat.connectors.cache,
        ConnectorsCacheState::Ready(snapshot) if snapshot.connectors.len() == 1
    );

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Installed 1 of 1 available apps."),
        "expected /apps to fall back to the installed apps snapshot, got:\n{popup}"
    );
    assert!(
        popup.contains("Installed. Press Enter to open the app page"),
        "expected the fallback popup to behave like the installed apps view, got:\n{popup}"
    );
}

#[tokio::test]
async fn apps_popup_shows_disabled_status_for_installed_but_disabled_apps() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: "connector_1".to_string(),
                name: "Notion".to_string(),
                description: Some("Workspace docs".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/notion".to_string()),
                is_accessible: true,
                is_enabled: false,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ true,
    );

    chat.add_connectors_output();
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Installed · Disabled. Press Enter to open the app page"),
        "expected selected app description to include disabled status, got:\n{popup}"
    );
    assert!(
        popup.contains("enable/disable this app."),
        "expected selected app description to mention enable/disable action, got:\n{popup}"
    );
}

#[tokio::test]
async fn apps_refresh_preserves_toggled_enabled_state() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: "connector_1".to_string(),
                name: "Notion".to_string(),
                description: Some("Workspace docs".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/notion".to_string()),
                is_accessible: true,
                is_enabled: true,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ true,
    );
    chat.update_connector_enabled("connector_1", /*enabled*/ false);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: "connector_1".to_string(),
                name: "Notion".to_string(),
                description: Some("Workspace docs".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/notion".to_string()),
                is_accessible: true,
                is_enabled: true,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ true,
    );

    assert_matches!(
        &chat.connectors.cache,
        ConnectorsCacheState::Ready(snapshot)
            if snapshot
                .connectors
                .iter()
                .find(|connector| connector.id == "connector_1")
                .is_some_and(|connector| !connector.is_enabled)
    );

    chat.add_connectors_output();
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Installed · Disabled. Press Enter to open the app page"),
        "expected disabled status to persist after reload, got:\n{popup}"
    );
}

#[tokio::test]
async fn apps_popup_for_not_installed_app_uses_install_only_selected_description() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);
    chat.config
        .features
        .enable(Feature::Apps)
        .expect("test config should allow feature update");
    chat.bottom_pane.set_connectors_enabled(/*enabled*/ true);

    chat.on_connectors_loaded(
        Ok(ConnectorsSnapshot {
            connectors: vec![AppInfo {
                id: "connector_2".to_string(),
                name: "Linear".to_string(),
                description: Some("Project tracking".to_string()),
                logo_url: None,
                logo_url_dark: None,
                distribution_channel: None,
                branding: None,
                app_metadata: None,
                labels: None,
                install_url: Some("https://example.test/linear".to_string()),
                is_accessible: false,
                is_enabled: true,
                plugin_display_names: Vec::new(),
            }],
        }),
        /*is_final*/ true,
    );

    chat.add_connectors_output();
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Can be installed. Press Enter to open the app page to install"),
        "expected selected app description to be install-only for not-installed apps, got:\n{popup}"
    );
    assert!(
        !popup.contains("enable/disable this app."),
        "did not expect enable/disable text for not-installed apps, got:\n{popup}"
    );
}

#[tokio::test]
async fn experimental_features_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    let features = vec![
        ExperimentalFeatureItem {
            feature: Feature::JsRepl,
            name: "JavaScript REPL".to_string(),
            description: "Enable a persistent Node-backed JavaScript REPL for interactive website debugging and other inline JavaScript execution capabilities.".to_string(),
            enabled: false,
        },
        ExperimentalFeatureItem {
            feature: Feature::ShellTool,
            name: "Shell tool".to_string(),
            description: "Allow the model to run shell commands.".to_string(),
            enabled: true,
        },
    ];
    let view = ExperimentalFeaturesView::new(
        features,
        chat.app_event_tx.clone(),
        crate::keymap::RuntimeKeymap::defaults().list,
    );
    chat.bottom_pane.show_view(Box::new(view));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("experimental_features_popup", popup);
}

#[tokio::test]
async fn experimental_features_toggle_saves_on_exit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    let expected_feature = Feature::JsRepl;
    let view = ExperimentalFeaturesView::new(
        vec![ExperimentalFeatureItem {
            feature: expected_feature,
            name: "JavaScript REPL".to_string(),
            description: "Enable a persistent Node-backed JavaScript REPL for interactive website debugging and other inline JavaScript execution capabilities.".to_string(),
            enabled: false,
        }],
        chat.app_event_tx.clone(),
        crate::keymap::RuntimeKeymap::defaults().list,
    );
    chat.bottom_pane.show_view(Box::new(view));

    chat.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert!(
        rx.try_recv().is_err(),
        "expected no updates until saving the popup"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let mut updates = None;
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::UpdateFeatureFlags {
            updates: event_updates,
        } = event
        {
            updates = Some(event_updates);
            break;
        }
    }

    let updates = updates.expect("expected UpdateFeatureFlags event");
    assert_eq!(updates, vec![(expected_feature, true)]);
}

#[tokio::test]
async fn experimental_popup_lists_registry_features_and_omits_stable_guardian_approval() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let guardian_stage = FEATURES
        .iter()
        .find(|spec| spec.id == Feature::GuardianApproval)
        .map(|spec| spec.stage)
        .expect("expected guardian approval feature metadata");

    assert_eq!(guardian_stage, Stage::Stable);

    chat.open_experimental_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 120);
    assert!(
        popup.contains("Workflows"),
        "expected experimental workflows feature in popup, got:\n{popup}"
    );
    assert!(
        popup.contains(
            "Show workflow commands for drafting, saving, and running thread workflow specs."
        ),
        "expected workflows feature description in popup, got:\n{popup}"
    );
    assert!(
        !popup.contains("Auto-review"),
        "expected stable auto-review feature to be omitted from experimental popup, got:\n{popup}"
    );
    assert_chatwidget_snapshot!("experimental_features_registry_popup", popup);
}

#[tokio::test]
async fn multi_agent_enable_prompt_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_multi_agent_enable_prompt();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("multi_agent_enable_prompt", popup);
}

#[tokio::test]
async fn multi_agent_enable_prompt_updates_feature_and_emits_notice() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_multi_agent_enable_prompt();
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateFeatureFlags { updates }) if updates == vec![(Feature::Collab, true)]
    );
    let cell = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => cell,
        other => panic!("expected InsertHistoryCell event, got {other:?}"),
    };
    let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 120));
    assert!(rendered.contains("Subagents will be enabled in the next session."));
}

#[tokio::test]
async fn memories_enable_prompt_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ false);

    chat.open_memories_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("memories_enable_prompt", popup);
}

#[tokio::test]
async fn memories_enable_prompt_updates_feature_without_notice() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ false);

    chat.open_memories_popup();
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateFeatureFlags { updates }) if updates == vec![(Feature::MemoryTool, true)]
    );
    assert!(
        rx.try_recv().is_err(),
        "memory enable prompt should not emit the success notice before persistence succeeds"
    );
}

#[tokio::test]
async fn memories_settings_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ true);
    chat.config.memories.use_memories = true;
    chat.config.memories.generate_memories = false;

    chat.open_memories_popup();

    let popup = strip_osc8_for_snapshot(&render_bottom_popup(&chat, /*width*/ 80));
    assert_chatwidget_snapshot!("memories_settings_popup", popup);
}

#[tokio::test]
async fn memories_reset_confirmation_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ true);
    chat.config.memories.use_memories = true;
    chat.config.memories.generate_memories = false;

    chat.open_memories_popup();
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("memories_reset_confirmation", popup);
}

#[tokio::test]
async fn memories_settings_toggle_saves_on_enter() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ true);
    chat.config.memories.use_memories = true;
    chat.config.memories.generate_memories = false;

    chat.open_memories_popup();
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Char(' ')));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateMemorySettings {
            use_memories: true,
            generate_memories: true,
        })
    );
}

#[tokio::test]
async fn memories_reset_confirmation_sends_event_on_confirm() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ true);
    chat.config.memories.use_memories = true;
    chat.config.memories.generate_memories = false;

    chat.open_memories_popup();
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::ResetMemories));
}

#[tokio::test]
async fn model_selection_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.open_model_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("model_selection_popup", popup);
}

fn profile_usage_snapshot(
    secondary_used_percent: i32,
    primary_used_percent: i32,
) -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: Some("codex".to_string()),
        limit_name: Some("codex".to_string()),
        primary: Some(RateLimitWindow {
            used_percent: primary_used_percent,
            window_duration_mins: Some(7 * 24 * 60),
            resets_at: None,
        }),
        secondary: Some(RateLimitWindow {
            used_percent: secondary_used_percent,
            window_duration_mins: Some(5 * 60),
            resets_at: None,
        }),
        credits: None,
        individual_limit: None,
        plan_type: None,
        rate_limit_reached_type: None,
    }
}

#[tokio::test]
async fn profile_selection_popup_snapshot_and_selection() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    save_popup_auth_profile(&chat, "work");
    save_popup_chatgpt_auth_profile(&chat, "personal", "el@elyratelier.com");
    while rx.try_recv().is_ok() {}

    chat.open_profile_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("profile_selection_popup", popup);
    assert!(popup.contains("default"));
    assert!(popup.contains("personal"));
    assert!(popup.contains("work"));
    assert!(popup.contains("Log in new profile"));
    assert!(popup.contains("ChatGPT Pro / el@elyratelier.com"));
    assert!(!popup.contains("ChatGPT / Pro"));
    assert!(!popup.contains("ChatGPT / ApiKey"));
    assert!(!popup.contains("Press Enter to confirm or Esc to go back"));
    assert!(popup.contains("Press enter to confirm or esc to go back"));
    assert!(popup.contains("Enter switch / l relogin / r rename"));

    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("profile_selection_popup_profile_actions", popup);
    assert!(popup.contains("usage unknown"));
    assert!(!popup.contains("› 2. personal            ChatGPT Pro / el@elyratelier.com"));
    assert!(!popup.contains("Press Enter to confirm or Esc to go back"));
    assert!(popup.contains("Press enter to confirm or esc to go back"));
    drain_profile_usage_refresh_events(&mut rx);

    chat.open_profile_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::SwitchAuthProfile {
            profile,
            reason,
            resume_queued_input,
        ..
        })
            if profile.as_deref() == Some("personal")
                && reason == crate::app_event::AuthProfileSwitchReason::Manual
                && !resume_queued_input
    );

    chat.open_profile_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::ReloginAuthProfile { profile, .. }) if profile == "personal"
    );

    chat.open_profile_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenAuthProfileRenamePrompt { profile, .. }) if profile == "personal"
    );

    chat.open_auth_profile_settings_popup("personal".to_string(), chat.rate_limit_reset_generation);
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenAuthProfileDeleteConfirm { profile, .. }) if profile == "personal"
    );

    chat.open_profile_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::MoveAuthProfile { profile, direction, .. })
            if profile == "personal"
                && direction == codex_login::AuthProfileMoveDirection::Down
    );

    chat.open_profile_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::MoveAuthProfile { profile, direction, .. })
            if profile == "personal"
                && direction == codex_login::AuthProfileMoveDirection::Up
    );

    chat.open_profile_popup();
    for _ in 0..3 {
        chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenAuthProfileLoginPrompt { .. })
    );
}

#[tokio::test]
async fn profile_login_prompt_snapshot_and_submit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    while rx.try_recv().is_ok() {}

    chat.open_auth_profile_login_prompt(chat.rate_limit_reset_generation);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("profile_login_prompt", popup);
    assert!(popup.contains("ChatGPT"));
    assert!(!popup.contains("Claude.ai"));
    assert!(!popup.contains("Claude Code"));

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let subscription_provider = match rx.try_recv() {
        Ok(AppEvent::OpenAuthProfileNamePrompt {
            subscription_provider,
            ..
        }) => subscription_provider,
        event => panic!("expected profile provider name prompt event, got {event:?}"),
    };
    assert_eq!(
        subscription_provider,
        AuthProfileSubscriptionProvider::ChatGpt
    );
    chat.open_auth_profile_name_prompt(subscription_provider, chat.rate_limit_reset_generation);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("profile_login_name_prompt", popup);
    assert!(popup.contains("Profile name"));

    for ch in " work-dev ".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::LoginNewAuthProfile {
            profile,
            subscription_provider,
            ..
        }) if profile == "work-dev"
            && subscription_provider == AuthProfileSubscriptionProvider::ChatGpt
    );
}

fn drain_profile_usage_refresh_events(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    while let Ok(event) = rx.try_recv() {
        assert!(
            matches!(
                event,
                AppEvent::RefreshRateLimits {
                    origin: RateLimitRefreshOrigin::Heartbeat,
                    ..
                }
            ),
            "unexpected event while draining profile usage refreshes: {event:?}"
        );
    }
}

#[tokio::test]
async fn profile_login_validation_errors_do_not_start_browser_login() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    while rx.try_recv().is_ok() {}

    chat.start_auth_profile_login(
        "nested/work".to_string(),
        AuthProfileSubscriptionProvider::ChatGpt,
        chat.rate_limit_reset_generation,
    );
    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Invalid auth profile name"),
        "expected invalid name error, got:\n{rendered}"
    );
}

#[tokio::test]
async fn profile_login_duplicate_errors_do_not_start_browser_login() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    save_popup_auth_profile(&chat, "personal");
    while rx.try_recv().is_ok() {}

    chat.start_auth_profile_login(
        "personal".to_string(),
        AuthProfileSubscriptionProvider::ChatGpt,
        chat.rate_limit_reset_generation,
    );
    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Auth profile `personal` already exists."),
        "expected duplicate profile error, got:\n{rendered}"
    );
}

#[tokio::test]
async fn profile_login_api_only_mode_points_to_cli_flow() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.forced_login_method = Some(codex_protocol::config_types::ForcedLoginMethod::Api);
    while rx.try_recv().is_ok() {}

    chat.start_auth_profile_login(
        "work".to_string(),
        AuthProfileSubscriptionProvider::ChatGpt,
        chat.rate_limit_reset_generation,
    );
    let rendered = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("ChatGPT browser login is disabled"),
        "expected API-only login guidance, got:\n{rendered}"
    );
}

#[tokio::test]
async fn external_profile_login_creates_metadata_profile() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    while rx.try_recv().is_ok() {}

    chat.start_auth_profile_login(
        "claude-work".to_string(),
        AuthProfileSubscriptionProvider::ClaudeAi,
        chat.rate_limit_reset_generation,
    );

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::AuthProfileLoginCompleted {
            profile,
            success: true,
            error: None,
            ..
        }) if profile == "claude-work"
    );
    let profiles = list_auth_profiles(&chat.config.codex_home, AuthCredentialsStoreMode::File)
        .expect("list auth profiles");
    assert_eq!(
        profiles
            .iter()
            .map(|profile| (
                profile.name.as_str(),
                profile.subscription_provider,
                profile.auth_mode
            ))
            .collect::<Vec<_>>(),
        vec![(
            "claude-work",
            AuthProfileSubscriptionProvider::ClaudeAi,
            None
        )]
    );
}

#[tokio::test]
async fn profile_selection_popup_shows_usage_hints() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.selected_auth_profile = Some("work".to_string());
    save_popup_auth_profile(&chat, "work");
    save_popup_chatgpt_auth_profile(&chat, "personal", "el@elyratelier.com");
    while rx.try_recv().is_ok() {}

    chat.on_rate_limit_snapshot(Some(profile_usage_snapshot(
        /*secondary_used_percent*/ 58, /*primary_used_percent*/ 20,
    )));

    let stale_snapshot = rate_limit_snapshot_display_for_limit(
        &profile_usage_snapshot(
            /*secondary_used_percent*/ 70, /*primary_used_percent*/ 40,
        ),
        "codex".to_string(),
        Local::now() - chrono::Duration::minutes(RATE_LIMIT_STALE_THRESHOLD_MINUTES + 1),
    );
    let mut stale_snapshots = BTreeMap::new();
    stale_snapshots.insert("codex".to_string(), stale_snapshot);
    chat.auth_profile_rate_limit_snapshots_by_profile
        .insert(Some("personal".to_string()), stale_snapshots);

    chat.open_profile_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));

    let popup = render_bottom_popup(&chat, /*width*/ 120);
    assert_chatwidget_snapshot!("profile_selection_popup_usage_hints", popup);
    assert!(!popup.contains("usage unknown"));
    assert!(!popup.contains("Press Enter to confirm or Esc to go back"));
    assert!(popup.contains("Press enter to confirm or esc to go back"));
    assert!(!popup.contains("ChatGPT Pro / el@elyratelier.com"));
    assert!(popup.contains("stale weekly 60% left, 5h 30% left"));
    assert!(popup.contains("weekly"));
    assert!(popup.contains("ChatGPT API key"));
    assert!(popup.contains("Enter switch / l relogin / r rename"));
}

#[tokio::test]
async fn profile_popup_requests_usage_heartbeat_when_selected_usage_is_missing() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.selected_auth_profile = Some("work".to_string());
    set_chatgpt_auth(&mut chat);
    save_popup_chatgpt_auth_profile(&chat, "work", "work@example.com");
    while rx.try_recv().is_ok() {}

    chat.open_profile_popup();
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::Heartbeat,
            target: RateLimitRefreshTarget::Named(profile),
        }) if profile == "work"
    );

    chat.open_profile_popup();
    assert_no_rate_limit_refresh_event(&mut rx);
}

#[tokio::test]
async fn profile_popup_skips_usage_heartbeat_when_selected_usage_is_fresh() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.selected_auth_profile = Some("work".to_string());
    set_chatgpt_auth(&mut chat);
    save_popup_chatgpt_auth_profile(&chat, "work", "work@example.com");
    chat.on_rate_limit_snapshot(Some(profile_usage_snapshot(
        /*secondary_used_percent*/ 20, /*primary_used_percent*/ 10,
    )));
    while rx.try_recv().is_ok() {}

    chat.open_profile_popup();
    assert_no_rate_limit_refresh_event(&mut rx);
}

#[tokio::test]
async fn usage_heartbeat_includes_saved_chatgpt_profile_when_active_session_is_api_key() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    save_popup_chatgpt_auth_profile(&chat, "work", "work@example.com");

    assert!(!chat.should_prefetch_rate_limits());
    assert_eq!(
        chat.auth_profile_usage_refresh_targets(),
        vec![RateLimitRefreshTarget::Named("work".to_string())]
    );
}

#[tokio::test]
async fn usage_heartbeat_failure_backs_off_profile_refresh() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    save_popup_chatgpt_auth_profile(&chat, "work", "work@example.com");

    chat.record_auth_profile_usage_heartbeat_failure(Some("work".to_string()));
    assert!(chat.auth_profile_usage_refresh_targets().is_empty());

    chat.record_auth_profile_usage_heartbeat_success(Some("work".to_string()));
    assert_eq!(
        chat.auth_profile_usage_refresh_targets(),
        vec![RateLimitRefreshTarget::Named("work".to_string())]
    );
}

fn assert_no_rate_limit_refresh_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) {
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, AppEvent::RefreshRateLimits { .. }),
            "unexpected rate-limit refresh event: {event:?}"
        );
    }
}

#[tokio::test]
async fn profile_delete_confirm_defaults_to_cancel() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    while rx.try_recv().is_ok() {}

    chat.open_auth_profile_delete_confirm("personal".to_string(), chat.rate_limit_reset_generation);
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(rx.try_recv().is_err());

    chat.open_auth_profile_delete_confirm("personal".to_string(), chat.rate_limit_reset_generation);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::DeleteAuthProfile { profile, .. }) if profile == "personal"
    );
}

#[tokio::test]
async fn config_popup_snapshot_and_toggle() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.auth_profile_auto_switch.enabled = false;
    chat.config.auth_profile_auto_switch.on_5h_limit = true;
    chat.config.auth_profile_auto_switch.on_weekly_limit = true;
    chat.config.disable_paste_burst = false;
    while rx.try_recv().is_ok() {}

    chat.open_config_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 90);
    assert_chatwidget_snapshot!("config_popup", popup);
    assert!(popup.contains("Account & automation"));
    assert!(popup.contains("AI context"));
    assert!(popup.contains("Interface & privacy"));

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenConfigSection { section })
            if section == crate::common_config_options::CommonConfigSection::AccountAutomation
    );

    chat.open_config_section_popup(
        crate::common_config_options::CommonConfigSection::AccountAutomation,
    );
    let account_popup = render_bottom_popup(&chat, /*width*/ 90);
    assert_chatwidget_snapshot!("config_account_automation_popup", account_popup);
    assert!(account_popup.contains("Update checks"));
    assert!(account_popup.contains("Codewith"));
    assert!(account_popup.contains("Auth profile auto-switch"));
    assert!(account_popup.contains("Session recap"));

    chat.open_config_section_popup(crate::common_config_options::CommonConfigSection::AiContext);
    let ai_context_popup = render_bottom_popup(&chat, /*width*/ 90);
    assert_chatwidget_snapshot!("config_ai_context_popup", ai_context_popup);
    assert!(ai_context_popup.contains("Environment context"));
    assert!(ai_context_popup.contains("Permission instructions"));
    assert!(ai_context_popup.contains("Skill instructions"));

    chat.open_config_section_popup(
        crate::common_config_options::CommonConfigSection::InterfacePrivacy,
    );
    let interface_popup = render_bottom_popup(&chat, /*width*/ 90);
    assert_chatwidget_snapshot!("config_interface_privacy_popup", interface_popup);
    assert!(interface_popup.contains("Paste burst detection"));
    assert!(interface_popup.contains("Animations"));
    assert!(interface_popup.contains("Tooltips"));
    assert!(interface_popup.contains("Feedback"));

    chat.open_config_section_popup(
        crate::common_config_options::CommonConfigSection::AccountAutomation,
    );
    chat.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateConfigValue {
            key_path,
            value,
            label,
        }) if key_path == "auth_profile_auto_switch.enabled"
            && value == serde_json::json!(true)
            && label == "Auth profile auto-switch"
    );
}

#[tokio::test]
async fn config_agent_max_threads_popup_selects_value() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    // Start from a known current value so the initial highlight is deterministic (index 0 = "1").
    chat.config.agent_max_threads = Some(1);
    while rx.try_recv().is_ok() {}

    chat.open_agent_max_threads_popup();
    let popup = render_bottom_popup(&chat, /*width*/ 90);
    assert!(popup.contains("Agent subagent threads"), "{popup}");
    assert!(popup.contains("(default)"), "{popup}");

    // Presets are [1, 2, 3, 4, 6, 8, 12]; move from "1" to "4" and select it.
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::UpdateConfigValue {
            key_path,
            value,
            label,
        }) if key_path == "agents.max_threads"
            && value == serde_json::json!(4)
            && label == "Agent subagent thread limit"
    );
}

#[tokio::test]
async fn config_agent_max_threads_menu_item_disabled_under_multi_agent_v2() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config
        .features
        .enable(codex_features::Feature::MultiAgentV2)
        .expect("enable multi_agent_v2");
    while rx.try_recv().is_ok() {}

    chat.open_config_popup();
    let popup = render_bottom_popup(&chat, /*width*/ 90);
    assert!(popup.contains("Agent subagent threads"), "{popup}");
    assert!(popup.contains("multi_agent_v2"), "{popup}");
}

#[tokio::test]
async fn provider_selection_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    let openai_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
    let cerebras_provider = ModelProviderInfo::create_cerebras_provider();
    let nvidia_provider = ModelProviderInfo::create_nvidia_provider();
    let openrouter_provider = ModelProviderInfo::create_openrouter_provider();
    chat.config.model_provider_id = "openai".to_string();
    chat.config.model_provider = openai_provider.clone();
    chat.config.model_providers = HashMap::from([
        ("cerebras".to_string(), cerebras_provider),
        ("nvidia".to_string(), nvidia_provider),
        ("openai".to_string(), openai_provider),
        ("openrouter".to_string(), openrouter_provider),
    ]);
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openai".to_string(),
        crate::legacy_core::test_support::all_model_presets().clone(),
    )));

    chat.open_provider_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("provider_selection_popup", popup);
    assert!(
        !popup.contains("OPENROUTER_API_KEY"),
        "provider picker should not render secret environment keys:\n{popup}"
    );
    assert!(
        !popup.contains("CEREBRAS_API_KEY"),
        "provider picker should not render secret environment keys:\n{popup}"
    );
    assert!(
        !popup.contains("NVIDIA_API_KEY"),
        "provider picker should not render secret environment keys:\n{popup}"
    );
}

#[tokio::test]
async fn provider_selection_shows_configured_providers() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    let openai_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
    let openrouter_provider = ModelProviderInfo::create_openrouter_provider();
    chat.config.model_provider_id = "openai".to_string();
    chat.config.model_provider = openai_provider.clone();
    chat.config.model_providers = HashMap::from([
        ("openai".to_string(), openai_provider),
        ("openrouter".to_string(), openrouter_provider),
        (
            "corp".to_string(),
            ModelProviderInfo {
                name: "Corp Provider".to_string(),
                base_url: Some("https://corp.example.com/v1".to_string()),
                ..ModelProviderInfo::default()
            },
        ),
    ]);
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openai".to_string(),
        crate::legacy_core::test_support::all_model_presets().clone(),
    )));

    chat.open_provider_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("OpenAI"));
    assert!(popup.contains("OpenRouter"));
    assert!(popup.contains("Corp Provider"));
}

#[tokio::test]
async fn provider_selection_emits_selected_provider_id() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    let openai_provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
    let openrouter_provider = ModelProviderInfo::create_openrouter_provider();
    chat.config.model_provider_id = "openai".to_string();
    chat.config.model_provider = openai_provider.clone();
    chat.config.model_providers = HashMap::from([
        ("openai".to_string(), openai_provider),
        ("openrouter".to_string(), openrouter_provider),
    ]);
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openai".to_string(),
        crate::legacy_core::test_support::all_model_presets().clone(),
    )));
    while rx.try_recv().is_ok() {}

    chat.open_provider_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::SelectModelProvider { provider_id }) if provider_id == "openrouter"
    );
}

#[tokio::test]
async fn current_provider_model_selection_sends_provider_and_model() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.model_provider_id = "nvidia".to_string();
    let preset =
        provider_picker_preset("deepseek-ai/deepseek-v4-flash", ReasoningEffortConfig::High);
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "nvidia".to_string(),
        vec![preset.clone()],
    )));
    while rx.try_recv().is_ok() {}

    chat.open_all_models_popup(vec![preset]);
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort: Some(ReasoningEffortConfig::High),
            } if provider_id == "nvidia" && model == "deepseek-ai/deepseek-v4-flash"
        )),
        "expected provider-scoped model selection event; events: {events:?}"
    );
    assert!(
        events.iter().all(|event| !matches!(
            event,
            AppEvent::UpdateModel(_)
                | AppEvent::UpdateReasoningEffort(_)
                | AppEvent::PersistModelSelection { .. }
        )),
        "provider-scoped selection should own the active thread mutation; events: {events:?}"
    );
    assert!(chat.bottom_pane.no_modal_or_popup_active());
}

#[tokio::test]
async fn current_provider_model_selection_without_reasoning_options_closes_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.model_provider_id = "openrouter".to_string();
    let mut preset =
        provider_picker_preset("deepseek/deepseek-v4-flash", ReasoningEffortConfig::None);
    preset.supported_reasoning_efforts.clear();
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openrouter".to_string(),
        vec![preset.clone()],
    )));
    while rx.try_recv().is_ok() {}

    chat.open_all_models_popup(vec![preset]);
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort: Some(ReasoningEffortConfig::None),
            } if provider_id == "openrouter" && model == "deepseek/deepseek-v4-flash"
        )),
        "expected provider-scoped model selection event; events: {events:?}"
    );
    assert!(chat.bottom_pane.no_modal_or_popup_active());
}

#[tokio::test]
async fn non_openai_provider_model_selection_with_multiple_reasoning_options_closes_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.model_provider_id = "openrouter".to_string();
    let preset = provider_picker_preset_with_reasoning(
        "z-ai/glm-5.1",
        ReasoningEffortConfig::Medium,
        &[
            ReasoningEffortConfig::Low,
            ReasoningEffortConfig::Medium,
            ReasoningEffortConfig::High,
        ],
    );
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openrouter".to_string(),
        vec![preset.clone()],
    )));
    while rx.try_recv().is_ok() {}

    chat.open_all_models_popup(vec![preset]);
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort: Some(ReasoningEffortConfig::Medium),
            } if provider_id == "openrouter" && model == "z-ai/glm-5.1"
        )),
        "expected provider-scoped model selection event; events: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AppEvent::OpenReasoningPopup { .. })),
        "provider-scoped model selection should not open the reasoning picker; events: {events:?}"
    );
    assert!(chat.bottom_pane.no_modal_or_popup_active());
}

#[tokio::test]
async fn personality_selection_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.open_personality_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("personality_selection_popup", popup);
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn realtime_audio_selection_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.open_realtime_audio_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("realtime_audio_selection_popup", popup);
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn realtime_audio_selection_popup_narrow_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.open_realtime_audio_popup();

    let popup = render_bottom_popup(&chat, /*width*/ 56);
    assert_chatwidget_snapshot!("realtime_audio_selection_popup_narrow", popup);
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn realtime_microphone_picker_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.config.realtime_audio.microphone = Some("Studio Mic".to_string());
    chat.open_realtime_audio_device_selection_with_names(
        RealtimeAudioDeviceKind::Microphone,
        vec!["Built-in Mic".to_string(), "USB Mic".to_string()],
    );

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("realtime_microphone_picker_popup", popup);
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn realtime_audio_picker_emits_persist_event() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.open_realtime_audio_device_selection_with_names(
        RealtimeAudioDeviceKind::Speaker,
        vec!["Desk Speakers".to_string(), "Headphones".to_string()],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::PersistRealtimeAudioDeviceSelection {
            kind: RealtimeAudioDeviceKind::Speaker,
            name: Some(name),
        }) if name == "Headphones"
    );
}

#[tokio::test]
async fn model_picker_hides_show_in_picker_false_models_from_cache() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("test-visible-model")).await;
    chat.thread_id = Some(ThreadId::new());
    let preset = |slug: &str, show_in_picker: bool| ModelPreset {
        id: slug.to_string(),
        model: slug.to_string(),
        display_name: slug.to_string(),
        description: format!("{slug} description"),
        default_reasoning_effort: ReasoningEffortConfig::Medium,
        supported_reasoning_efforts: vec![ReasoningEffortPreset {
            effort: ReasoningEffortConfig::Medium,
            description: "medium".to_string(),
        }],
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        is_default: false,
        upgrade: None,
        show_in_picker,
        availability_nux: None,
        supported_in_api: true,
        input_modalities: default_input_modalities(),
    };

    chat.open_model_popup_with_presets(vec![
        preset("test-visible-model", true),
        preset("test-hidden-model", false),
    ]);
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("model_picker_filters_hidden_models", popup);
    assert!(
        popup.contains("test-visible-model"),
        "expected visible model to appear in picker:\n{popup}"
    );
    assert!(
        !popup.contains("test-hidden-model"),
        "expected hidden model to be excluded from picker:\n{popup}"
    );
}

#[tokio::test]
async fn inactive_provider_model_selection_switches_provider_and_model() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.model_provider_id = "openai".to_string();
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openrouter".to_string(),
        vec![provider_picker_preset(
            "codex-auto-balanced",
            ReasoningEffortConfig::Medium,
        )],
    )));
    while rx.try_recv().is_ok() {}

    chat.open_model_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort: Some(ReasoningEffortConfig::Medium),
            } if provider_id == "openrouter" && model == "codex-auto-balanced"
        )),
        "expected provider-scoped model selection event; events: {events:?}"
    );
    assert!(
        events.iter().all(|event| !matches!(
            event,
            AppEvent::UpdateModel(_)
                | AppEvent::UpdateReasoningEffort(_)
                | AppEvent::PersistModelSelection { .. }
        )),
        "provider switch event should own the active thread mutation; events: {events:?}"
    );
}

#[tokio::test]
async fn inactive_provider_reasoning_selection_switches_provider_and_model() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.model_provider_id = "openai".to_string();
    let preset = provider_picker_preset("openrouter/deepseek-v3.2", ReasoningEffortConfig::High);
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "openrouter".to_string(),
        vec![preset.clone()],
    )));
    while rx.try_recv().is_ok() {}

    chat.open_reasoning_popup(preset);

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort: Some(ReasoningEffortConfig::High),
            } if provider_id == "openrouter" && model == "openrouter/deepseek-v3.2"
        )),
        "expected provider-scoped model selection event; events: {events:?}"
    );
    assert!(
        events.iter().all(|event| !matches!(
            event,
            AppEvent::UpdateModel(_)
                | AppEvent::UpdateReasoningEffort(_)
                | AppEvent::PersistModelSelection { .. }
        )),
        "provider switch event should own the active thread mutation; events: {events:?}"
    );
}

#[tokio::test]
async fn current_provider_reasoning_selection_sends_provider_and_model() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.model_provider_id = "nvidia".to_string();
    let preset =
        provider_picker_preset("deepseek-ai/deepseek-v4-flash", ReasoningEffortConfig::High);
    chat.set_model_catalog(Arc::new(ModelCatalog::new_for_provider(
        "nvidia".to_string(),
        vec![preset.clone()],
    )));
    while rx.try_recv().is_ok() {}

    chat.open_reasoning_popup(preset);

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::SelectModelProviderModel {
                provider_id,
                model,
                effort: Some(ReasoningEffortConfig::High),
            } if provider_id == "nvidia" && model == "deepseek-ai/deepseek-v4-flash"
        )),
        "expected provider-scoped reasoning selection event; events: {events:?}"
    );
    assert!(
        events.iter().all(|event| !matches!(
            event,
            AppEvent::UpdateModel(_)
                | AppEvent::UpdateReasoningEffort(_)
                | AppEvent::PersistModelSelection { .. }
        )),
        "provider-scoped selection should own the active thread mutation; events: {events:?}"
    );
}

#[tokio::test]
async fn server_overloaded_error_does_not_switch_models() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.set_model("gpt-5.3-codex");
    while rx.try_recv().is_ok() {}
    while op_rx.try_recv().is_ok() {}

    handle_error(
        &mut chat,
        "server overloaded",
        Some(CodexErrorInfo::ServerOverloaded),
    );

    while let Ok(event) = rx.try_recv() {
        if let AppEvent::UpdateModel(model) = event {
            assert_eq!(
                model, "gpt-5.3-codex",
                "did not expect model switch on server-overloaded error"
            );
        }
    }

    while let Ok(event) = op_rx.try_recv() {
        if let Op::OverrideTurnContext { model, .. } = event {
            assert!(
                model.is_none(),
                "did not expect OverrideTurnContext model update on server-overloaded error"
            );
        }
    }
}

#[tokio::test]
async fn model_reasoning_selection_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;

    set_chatgpt_auth(&mut chat);
    chat.set_reasoning_effort(Some(ReasoningEffortConfig::High));

    let mut preset = get_available_model(&chat, "gpt-5.4");
    preset.supported_reasoning_efforts.insert(
        2,
        ReasoningEffortPreset {
            effort: ReasoningEffortConfig::Custom("max".to_string()),
            description: "Maximum available reasoning".to_string(),
        },
    );
    chat.open_reasoning_popup(preset);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("model_reasoning_selection_popup", popup);
}

#[tokio::test]
async fn model_reasoning_selection_popup_applies_custom_effort() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    let custom_effort = ReasoningEffortConfig::Custom("max".to_string());
    chat.set_reasoning_effort(Some(ReasoningEffortConfig::XHigh));

    let mut preset = get_available_model(&chat, "gpt-5.4");
    preset
        .supported_reasoning_efforts
        .push(ReasoningEffortPreset {
            effort: custom_effort.clone(),
            description: "Maximum available reasoning".to_string(),
        });
    chat.open_reasoning_popup(preset);
    while rx.try_recv().is_ok() {}

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let selected_effort_events = std::iter::from_fn(|| rx.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::UpdateReasoningEffort(effort) => Some((None, effort)),
            AppEvent::PersistModelSelection { model, effort } => Some((Some(model), effort)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected_effort_events,
        vec![
            (None, Some(custom_effort.clone())),
            (Some("gpt-5.4".to_string()), Some(custom_effort)),
        ]
    );
}

#[tokio::test]
async fn model_reasoning_selection_popup_extra_high_warning_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;

    set_chatgpt_auth(&mut chat);
    chat.set_reasoning_effort(Some(ReasoningEffortConfig::XHigh));

    let preset = get_available_model(&chat, "gpt-5.2");
    chat.open_reasoning_popup(preset);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("model_reasoning_selection_popup_extra_high_warning", popup);
}

async fn assert_reasoning_shortcuts_update_effort(
    key_events: [KeyEvent; 2],
    expected_effort: ReasoningEffortConfig,
    expect_model_update: bool,
) {
    for key_event in key_events {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
        chat.thread_id = Some(ThreadId::new());
        chat.set_reasoning_effort(Some(ReasoningEffortConfig::Medium));

        chat.handle_key_event(key_event);

        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        if expect_model_update {
            assert!(
                events.iter().any(
                    |event| matches!(event, AppEvent::UpdateModel(model) if model == "gpt-5.4")
                ),
                "expected model update event for {key_event:?}; events: {events:?}"
            );
        }
        assert!(
            events.iter().any(|event| matches!(
                event,
                AppEvent::UpdateReasoningEffort(Some(effort)) if effort == &expected_effort
            )),
            "expected reasoning update event for {key_event:?}; events: {events:?}"
        );
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, AppEvent::PersistModelSelection { .. })),
            "expected no model persistence event for {key_event:?}; events: {events:?}"
        );
    }
}

#[tokio::test]
async fn reasoning_up_shortcuts_raise_reasoning_effort() {
    assert_reasoning_shortcuts_update_effort(
        [
            KeyEvent::new(KeyCode::Char('.'), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT),
        ],
        ReasoningEffortConfig::High,
        /*expect_model_update*/ true,
    )
    .await;
}

#[tokio::test]
async fn reasoning_down_shortcuts_lower_reasoning_effort() {
    assert_reasoning_shortcuts_update_effort(
        [
            KeyEvent::new(KeyCode::Char(','), KeyModifiers::ALT),
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
        ],
        ReasoningEffortConfig::Low,
        /*expect_model_update*/ false,
    )
    .await;
}

#[tokio::test]
async fn reasoning_shortcut_clears_armed_quit_shortcut() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_reasoning_effort(Some(ReasoningEffortConfig::Medium));
    chat.arm_quit_shortcut(key_hint::ctrl(KeyCode::Char('c')));

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::ALT));

    assert!(!chat.bottom_pane.quit_shortcut_hint_visible());
    assert!(chat.quit_shortcut_expires_at.is_none());
    assert!(chat.quit_shortcut_key.is_none());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AppEvent::Exit(_))),
        "did not expect reasoning shortcut to quit; events: {events:?}"
    );
}

#[tokio::test]
async fn reasoning_shortcut_is_ignored_with_model_popup_open() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_reasoning_effort(Some(ReasoningEffortConfig::Medium));
    chat.open_model_popup();

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::ALT));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AppEvent::UpdateReasoningEffort(_))),
        "did not expect reasoning update while popup is active; events: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AppEvent::PersistModelSelection { .. })),
        "did not expect model persistence while popup is active; events: {events:?}"
    );
}

#[tokio::test]
async fn reasoning_popup_shows_extra_high_with_space() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;

    set_chatgpt_auth(&mut chat);

    let preset = get_available_model(&chat, "gpt-5.4");
    chat.open_reasoning_popup(preset);

    let popup = render_bottom_popup(&chat, /*width*/ 120);
    assert!(
        popup.contains("Extra high"),
        "expected popup to include 'Extra high'; popup: {popup}"
    );
    assert!(
        !popup.contains("Extrahigh"),
        "expected popup not to include 'Extrahigh'; popup: {popup}"
    );
}

#[tokio::test]
async fn single_reasoning_option_skips_selection() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    let single_effort = vec![ReasoningEffortPreset {
        effort: ReasoningEffortConfig::High,
        description: "Greater reasoning depth for complex or ambiguous problems".to_string(),
    }];
    let preset = ModelPreset {
        id: "model-with-single-reasoning".to_string(),
        model: "model-with-single-reasoning".to_string(),
        display_name: "model-with-single-reasoning".to_string(),
        description: "".to_string(),
        default_reasoning_effort: ReasoningEffortConfig::High,
        supported_reasoning_efforts: single_effort,
        supports_personality: false,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        is_default: false,
        upgrade: None,
        show_in_picker: true,
        availability_nux: None,
        supported_in_api: true,
        input_modalities: default_input_modalities(),
    };
    chat.open_reasoning_popup(preset);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        !popup.contains("Select Reasoning Level"),
        "expected reasoning selection popup to be skipped"
    );

    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }

    assert!(
        events
            .iter()
            .any(|ev| matches!(ev, AppEvent::UpdateReasoningEffort(Some(effort)) if *effort == ReasoningEffortConfig::High)),
        "expected reasoning effort to be applied automatically; events: {events:?}"
    );
}

#[tokio::test]
async fn feedback_selection_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    // Open the feedback category selection popup via slash command.
    chat.dispatch_command(SlashCommand::Feedback);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("feedback_selection_popup", popup);
}

#[tokio::test]
async fn feedback_upload_consent_popup_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_selection_view(crate::bottom_pane::feedback_upload_consent_params(
        chat.app_event_tx.clone(),
        crate::app_event::FeedbackCategory::Bug,
        chat.current_rollout_path.clone(),
        Some("auto-review-rollout-thread-1.jsonl".to_string()),
        /*include_windows_sandbox_log*/ true,
        &codex_feedback::FeedbackDiagnostics::new(vec![codex_feedback::FeedbackDiagnostic {
            headline: "Proxy environment variables are set and may affect connectivity."
                .to_string(),
            details: vec!["HTTPS_PROXY = hello".to_string()],
        }]),
    ));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("feedback_upload_consent_popup", popup);
}

#[tokio::test]
async fn feedback_good_result_consent_popup_includes_connectivity_diagnostics_filename() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_selection_view(crate::bottom_pane::feedback_upload_consent_params(
        chat.app_event_tx.clone(),
        crate::app_event::FeedbackCategory::GoodResult,
        chat.current_rollout_path.clone(),
        Some("auto-review-rollout-thread-1.jsonl".to_string()),
        /*include_windows_sandbox_log*/ false,
        &codex_feedback::FeedbackDiagnostics::new(vec![codex_feedback::FeedbackDiagnostic {
            headline: "Proxy environment variables are set and may affect connectivity."
                .to_string(),
            details: vec!["HTTPS_PROXY = hello".to_string()],
        }]),
    ));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("feedback_good_result_consent_popup", popup);
}

#[tokio::test]
async fn reasoning_popup_escape_returns_to_model_popup() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    chat.open_model_popup();

    let preset = get_available_model(&chat, "gpt-5.4");
    chat.open_reasoning_popup(preset);

    let before_escape = render_bottom_popup(&chat, /*width*/ 80);
    assert!(before_escape.contains("Select Reasoning Level"));

    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    let after_escape = render_bottom_popup(&chat, /*width*/ 80);
    assert!(after_escape.contains("Select Model"));
    assert!(!after_escape.contains("Select Reasoning Level"));
}
