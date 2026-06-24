use super::*;
use assert_matches::assert_matches;
use chrono::TimeZone;
use codex_app_server_protocol::AuthMode;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::AuthDotJson;
use codex_login::save_auth_profile;
use codex_model_provider_info::MINIMAX_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;

use crate::app_event::MiniMaxUsageRefreshOrigin;
use crate::minimax_usage::MiniMaxUsageBucket;
use crate::minimax_usage::MiniMaxUsageSnapshot;
use crate::minimax_usage::MiniMaxUsageWindow;
use crate::status::StatusAccountDisplay;

#[tokio::test]
async fn status_command_renders_immediately_and_refreshes_rate_limits_for_chatgpt_auth() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);

    chat.show_status_report();

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output before refresh request, got {other:?}"),
    };
    assert!(
        !rendered.contains("refreshing limits"),
        "expected /status to avoid transient refresh text in terminal history, got: {rendered}"
    );
    let request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
            target: RateLimitRefreshTarget::Selected,
        }) => request_id,
        other => panic!("expected rate-limit refresh request, got {other:?}"),
    };
    pretty_assertions::assert_eq!(request_id, 0);
}

#[tokio::test]
async fn status_command_refresh_updates_cached_limits_for_future_status_outputs() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);

    chat.show_status_report();

    match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(_)) => {}
        other => panic!("expected status output before refresh request, got {other:?}"),
    }
    let first_request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
            target: RateLimitRefreshTarget::Selected,
        }) => request_id,
        other => panic!("expected rate-limit refresh request, got {other:?}"),
    };

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 92.0)));
    chat.finish_status_rate_limit_refresh(first_request_id);
    drain_insert_history(&mut rx);

    chat.show_status_report();
    let refreshed = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected refreshed status output, got {other:?}"),
    };
    assert!(
        refreshed.contains("8% left"),
        "expected a future /status output to use refreshed cached limits, got: {refreshed}"
    );
}

#[tokio::test]
async fn status_command_renders_immediately_without_rate_limit_refresh() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.show_status_report();

    assert_matches!(rx.try_recv(), Ok(AppEvent::InsertHistoryCell(_)));
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AppEvent::RefreshRateLimits { .. })),
        "non-ChatGPT sessions should not request a rate-limit refresh for /status"
    );
}

#[tokio::test]
async fn stats_command_refreshes_minimax_usage_for_minimax_provider() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.config.model_provider_id = MINIMAX_PROVIDER_ID.to_string();
    chat.config.model_provider = ModelProviderInfo::create_minimax_provider();

    chat.show_status_report();

    let cell = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => cell,
        other => panic!("expected stats output before MiniMax usage refresh, got {other:?}"),
    };
    let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 100));
    assert!(
        rendered.contains("status") && rendered.contains("refreshing Token Plan usage"),
        "expected the status report to render MiniMax refresh state, got: {rendered}"
    );
    assert!(
        !rendered.contains("Limits"),
        "expected MiniMax /stats not to show generic ChatGPT limits, got: {rendered}"
    );

    let request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshMiniMaxUsage {
            origin: MiniMaxUsageRefreshOrigin::StatusCommand { request_id },
        }) => request_id,
        other => panic!("expected MiniMax usage refresh request, got {other:?}"),
    };
    pretty_assertions::assert_eq!(request_id, 0);

    chat.finish_status_minimax_usage_refresh(request_id, Ok(minimax_usage_snapshot()));
    let refreshed = lines_to_single_string(&cell.display_lines(/*width*/ 100));
    assert!(
        refreshed.contains("Token Plan general bucket")
            && refreshed.contains("72% left")
            && refreshed.contains("20 / 100 used"),
        "expected MiniMax usage data to update /stats output, got: {refreshed}"
    );
}

#[tokio::test]
async fn status_command_reflects_auth_profile_switch_account_state() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.update_account_state(
        Some(StatusAccountDisplay::ChatGpt {
            email: Some("old@example.com".to_string()),
            plan: Some("Pro".to_string()),
        }),
        /*plan_type*/ None,
        /*has_chatgpt_account*/ true,
    );
    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 92.0)));
    drain_insert_history(&mut rx);
    save_auth_profile(
        &chat.config.codex_home,
        AuthCredentialsStoreMode::File,
        "work",
        &AuthDotJson {
            auth_mode: Some(AuthMode::ApiKey),
            openai_api_key: Some("work-key".to_string()),
            personal_access_token: None,
            tokens: None,
            last_refresh: None,
            agent_identity: None,
        },
    )
    .expect("save auth profile");

    chat.set_auth_profile(Some("work".to_string()));
    chat.show_status_report();

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output, got {other:?}"),
    };
    assert!(
        rendered.contains("API key configured"),
        "expected /status to use the switched auth profile account state, got: {rendered}"
    );
    assert!(
        !rendered.contains("old@example.com"),
        "expected /status not to show stale account metadata, got: {rendered}"
    );
    assert!(
        !rendered.contains("8% left"),
        "expected /status not to show stale rate-limit metadata, got: {rendered}"
    );
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AppEvent::RefreshRateLimits { .. })),
        "API key profiles should not request a ChatGPT rate-limit refresh"
    );
}

#[tokio::test]
async fn status_command_opens_interactive_panel_instead_of_dumping_text() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    assert!(
        chat.bottom_pane.no_modal_or_popup_active(),
        "precondition: no popup before /status"
    );

    chat.dispatch_command(SlashCommand::Status);

    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "/status should open the interactive status panel"
    );
    let popup = normalize_snapshot_paths(render_bottom_popup(&chat, /*width*/ 100))
        .lines()
        .map(|line| {
            if let Some(version_start) = line.find("Version    ") {
                format!("{}Version    <test-version>", &line[..version_start])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_chatwidget_snapshot!("status_panel_overview", popup);
    assert!(
        !std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AppEvent::InsertHistoryCell(_))),
        "/status should no longer dump a static history cell"
    );
}

#[tokio::test]
async fn stats_alias_opens_interactive_panel() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Stats);

    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "/stats should open the same interactive status panel"
    );
}

fn minimax_usage_snapshot() -> MiniMaxUsageSnapshot {
    let resets_at = chrono::Local
        .with_ymd_and_hms(2026, 6, 12, 18, 0, 0)
        .single();
    MiniMaxUsageSnapshot {
        buckets: vec![MiniMaxUsageBucket {
            name: "general".to_string(),
            interval: MiniMaxUsageWindow {
                remaining_percent: 72.0,
                used_count: Some(0),
                total_count: Some(0),
                resets_at,
            },
            weekly: MiniMaxUsageWindow {
                remaining_percent: 80.0,
                used_count: Some(20),
                total_count: Some(100),
                resets_at,
            },
        }],
    }
}

#[tokio::test]
async fn status_command_uses_catalog_default_reasoning_when_config_empty() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.config.model_reasoning_effort = None;

    chat.show_status_report();

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output, got {other:?}"),
    };
    assert!(
        rendered.contains("gpt-5.4 (reasoning medium, summaries auto)"),
        "expected /status to render the catalog default reasoning effort, got: {rendered}"
    );
}

#[tokio::test]
async fn status_command_renders_instruction_sources_from_thread_session() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.instruction_source_paths = vec![chat.config.cwd.join("AGENTS.md")];

    chat.show_status_report();

    let rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected status output, got {other:?}"),
    };
    assert!(
        rendered.contains("Agents.md"),
        "expected /status to render app-server instruction sources, got: {rendered}"
    );
    assert!(
        !rendered.contains("Agents.md  <none>"),
        "expected /status to avoid stale <none> when app-server provided instruction sources, got: {rendered}"
    );
}

#[tokio::test]
async fn status_command_overlapping_refreshes_update_matching_cells_only() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    set_chatgpt_auth(&mut chat);

    chat.show_status_report();
    match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(_)) => {}
        other => panic!("expected first status output, got {other:?}"),
    }
    let first_request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
            target: RateLimitRefreshTarget::Selected,
        }) => request_id,
        other => panic!("expected first refresh request, got {other:?}"),
    };

    chat.show_status_report();
    let second_rendered = match rx.try_recv() {
        Ok(AppEvent::InsertHistoryCell(cell)) => {
            lines_to_single_string(&cell.display_lines(/*width*/ 80))
        }
        other => panic!("expected second status output, got {other:?}"),
    };
    let second_request_id = match rx.try_recv() {
        Ok(AppEvent::RefreshRateLimits {
            origin: RateLimitRefreshOrigin::StatusCommand { request_id },
            target: RateLimitRefreshTarget::Selected,
        }) => request_id,
        other => panic!("expected second refresh request, got {other:?}"),
    };

    assert_ne!(first_request_id, second_request_id);
    assert!(
        !second_rendered.contains("refreshing limits"),
        "expected /status to avoid transient refresh text in terminal history, got: {second_rendered}"
    );

    chat.finish_status_rate_limit_refresh(first_request_id);
    pretty_assertions::assert_eq!(chat.refreshing_status_outputs.len(), 1);

    chat.on_rate_limit_snapshot(Some(snapshot(/*percent*/ 92.0)));
    chat.finish_status_rate_limit_refresh(second_request_id);
    assert!(chat.refreshing_status_outputs.is_empty());
}
