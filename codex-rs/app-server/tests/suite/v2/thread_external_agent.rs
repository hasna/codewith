use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::to_response;
use app_test_support::write_mock_provider_models_cache;
use app_test_support::write_mock_responses_config_toml;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadExternalAgentCancelParams;
use codex_app_server_protocol::ThreadExternalAgentCancelResponse;
use codex_app_server_protocol::ThreadExternalAgentEvent;
use codex_app_server_protocol::ThreadExternalAgentEventNotification;
use codex_app_server_protocol::ThreadExternalAgentMode;
use codex_app_server_protocol::ThreadExternalAgentPermissionOption;
use codex_app_server_protocol::ThreadExternalAgentPermissionRespondParams;
use codex_app_server_protocol::ThreadExternalAgentPermissionRespondResponse;
use codex_app_server_protocol::ThreadExternalAgentStartParams;
use codex_app_server_protocol::ThreadExternalAgentStartResponse;
use codex_app_server_protocol::ThreadExternalAgentStartStatus;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::MockServer;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn thread_external_agent_start_emits_run_event_and_validates_runtime() -> Result<()> {
    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;

    let server = create_mock_responses_server_sequence(vec![]).await;
    write_mock_responses_config_toml(
        codex_home.as_path(),
        &server.uri(),
        &BTreeMap::new(),
        /*auto_compact_limit*/ 200_000,
        /*requires_openai_auth*/ None,
        "mock_provider",
        "compact",
    )?;
    codex_login::save_auth_profile_metadata(
        codex_home.as_path(),
        "cursor-work",
        codex_login::AuthProfileMetadata {
            subscription_provider: codex_login::AuthProfileSubscriptionProvider::Cursor,
            last_permissions: None,
        },
    )?;
    write_mock_provider_models_cache(codex_home.as_path())?;
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir(&bin_dir)?;
    write_fake_executable(&bin_dir, "agent")?;
    write_fake_executable(&bin_dir, "cursor-agent")?;
    let path = path_with_fake_bin(&bin_dir)?;

    let mut mcp = McpProcess::new_with_env(
        codex_home.as_path(),
        &[
            ("CODEWITH_AUTH_PROFILE", Some("cursor-work")),
            ("PATH", Some(path.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(start_resp)?;

    let external_agent_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id.clone(),
            runtime_id: "cursor".to_string(),
            task: "inspect the auth wiring".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let external_agent_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(external_agent_id)),
    )
    .await??;
    let response: ThreadExternalAgentStartResponse = to_response(external_agent_resp)?;
    assert_eq!(
        response,
        ThreadExternalAgentStartResponse {
            status: ThreadExternalAgentStartStatus::Started,
            run_id: response.run_id.clone(),
            message: "external-agent run started".to_string(),
        }
    );
    let run_id = response.run_id.expect("external-agent run id");
    let started_notification = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/externalAgent/event"),
    )
    .await??;
    let started: ThreadExternalAgentEventNotification = serde_json::from_value(
        started_notification
            .params
            .expect("external-agent event params"),
    )?;
    assert_eq!(started.thread_id, thread.id);
    assert_eq!(started.run_id, run_id);
    assert_eq!(
        started.event,
        ThreadExternalAgentEvent::RunStarted {
            runtime_id: "cursor".to_string(),
            mode: ThreadExternalAgentMode::Plan,
            task: "inspect the auth wiring".to_string(),
        }
    );

    if cfg!(windows) {
        let failed_notification = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_notification_message("thread/externalAgent/event"),
        )
        .await??;
        let failed: ThreadExternalAgentEventNotification = serde_json::from_value(
            failed_notification
                .params
                .expect("external-agent failure event params"),
        )?;
        assert_eq!(failed.thread_id, thread.id);
        assert_eq!(failed.run_id, run_id);
        let ThreadExternalAgentEvent::Failed { message } = failed.event else {
            anyhow::bail!("expected external-agent failure event");
        };
        assert!(
            message.contains("platform sandbox is not available"),
            "unexpected failure message: {message}"
        );

        let inactive_response: ThreadExternalAgentCancelResponse =
            timeout(DEFAULT_TIMEOUT, async {
                loop {
                    let cancel_id = mcp
                        .send_thread_external_agent_cancel_request(
                            ThreadExternalAgentCancelParams {
                                thread_id: thread.id.clone(),
                                run_id: run_id.clone(),
                            },
                        )
                        .await?;
                    let cancel_resp: JSONRPCResponse = mcp
                        .read_stream_until_response_message(RequestId::Integer(cancel_id))
                        .await?;
                    let response = to_response::<ThreadExternalAgentCancelResponse>(cancel_resp)?;
                    if !response.cancelled {
                        return Ok::<ThreadExternalAgentCancelResponse, anyhow::Error>(response);
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await??;
        assert_eq!(
            inactive_response,
            ThreadExternalAgentCancelResponse {
                cancelled: false,
                message: format!("external-agent run `{run_id}` is not active"),
            }
        );
    } else {
        let cancel_id = mcp
            .send_thread_external_agent_cancel_request(ThreadExternalAgentCancelParams {
                thread_id: thread.id.clone(),
                run_id: run_id.clone(),
            })
            .await?;
        let cancel_resp: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(cancel_id)),
        )
        .await??;
        assert_eq!(
            to_response::<ThreadExternalAgentCancelResponse>(cancel_resp)?,
            ThreadExternalAgentCancelResponse {
                cancelled: true,
                message: "external-agent run cancellation requested".to_string(),
            }
        );
    }

    let grok_alias_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id.clone(),
            runtime_id: "grok".to_string(),
            task: "inspect the auth wiring".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let grok_alias_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(grok_alias_id)),
    )
    .await??;
    assert_eq!(
        to_response::<ThreadExternalAgentStartResponse>(grok_alias_resp)?,
        ThreadExternalAgentStartResponse {
            status: ThreadExternalAgentStartStatus::Gated,
            run_id: None,
            message: "use runtimeId `grok-build` for Grok Build external-agent runs".to_string(),
        }
    );

    let empty_task_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id,
            runtime_id: "grok-build".to_string(),
            task: "   ".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let empty_task_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(empty_task_id)),
    )
    .await??;
    assert_eq!(
        to_response::<ThreadExternalAgentStartResponse>(empty_task_resp)?,
        ThreadExternalAgentStartResponse {
            status: ThreadExternalAgentStartStatus::Gated,
            run_id: None,
            message: "task must not be empty".to_string(),
        }
    );

    let default_profile_thread_id = mcp
        .send_thread_start_request(ThreadStartParams {
            auth_profile: Some(None),
            ..ThreadStartParams::default()
        })
        .await?;
    let default_profile_thread_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(default_profile_thread_id)),
    )
    .await??;
    let ThreadStartResponse {
        thread: default_profile_thread,
        ..
    } = to_response(default_profile_thread_resp)?;
    let claude_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: default_profile_thread.id,
            runtime_id: "claude".to_string(),
            task: "inspect the auth wiring".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let claude_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(claude_id)),
    )
    .await??;
    let claude_response: ThreadExternalAgentStartResponse = to_response(claude_resp)?;
    assert_eq!(
        claude_response.status,
        ThreadExternalAgentStartStatus::Gated
    );
    assert_eq!(claude_response.run_id, None);
    assert!(
        claude_response
            .message
            .starts_with("Claude Code external-agent runtime is gated: missing runtime."),
        "expected Claude runtime readiness gate, got: {}",
        claude_response.message
    );
    assert!(
        !claude_response.message.contains("auth profile"),
        "Claude runtime should not be gated on a generic Claude.ai auth profile: {}",
        claude_response.message
    );

    Ok(())
}

#[tokio::test]
async fn thread_external_agent_permission_respond_unknown_request_is_not_accepted() -> Result<()> {
    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;

    let server = create_mock_responses_server_sequence(vec![]).await;
    write_mock_responses_config_toml(
        codex_home.as_path(),
        &server.uri(),
        &BTreeMap::new(),
        /*auto_compact_limit*/ 200_000,
        /*requires_openai_auth*/ None,
        "mock_provider",
        "compact",
    )?;
    write_mock_provider_models_cache(codex_home.as_path())?;

    let mut mcp = McpProcess::new(codex_home.as_path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(start_resp)?;

    // No permission request is pending, so answering an unknown request id is a
    // benign no-op that reports `accepted: false` rather than erroring.
    let respond_id = mcp
        .send_thread_external_agent_permission_respond_request(
            ThreadExternalAgentPermissionRespondParams {
                thread_id: thread.id.clone(),
                run_id: "ext_missing".to_string(),
                request_id: "perm-missing".to_string(),
                decision: ThreadExternalAgentPermissionOption::AllowOnce,
            },
        )
        .await?;
    let respond_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(respond_id)),
    )
    .await??;
    assert_eq!(
        to_response::<ThreadExternalAgentPermissionRespondResponse>(respond_resp)?,
        ThreadExternalAgentPermissionRespondResponse { accepted: false }
    );

    // Replaying the same response stays a no-op.
    let replay_id = mcp
        .send_thread_external_agent_permission_respond_request(
            ThreadExternalAgentPermissionRespondParams {
                thread_id: thread.id,
                run_id: "ext_missing".to_string(),
                request_id: "perm-missing".to_string(),
                decision: ThreadExternalAgentPermissionOption::AllowOnce,
            },
        )
        .await?;
    let replay_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(replay_id)),
    )
    .await??;
    assert_eq!(
        to_response::<ThreadExternalAgentPermissionRespondResponse>(replay_resp)?,
        ThreadExternalAgentPermissionRespondResponse { accepted: false }
    );

    Ok(())
}

/// Boot an app-server whose PATH exposes a fake, long-sleeping `cursor-agent`
/// so live external-agent runs can be started and cancelled deterministically.
async fn start_ready_external_agent_server() -> Result<(McpProcess, MockServer, TempDir)> {
    let tmp = TempDir::new()?;
    let codex_home = tmp.path().join("codex_home");
    std::fs::create_dir(&codex_home)?;

    let server = create_mock_responses_server_sequence(vec![]).await;
    write_mock_responses_config_toml(
        codex_home.as_path(),
        &server.uri(),
        &BTreeMap::new(),
        /*auto_compact_limit*/ 200_000,
        /*requires_openai_auth*/ None,
        "mock_provider",
        "compact",
    )?;
    codex_login::save_auth_profile_metadata(
        codex_home.as_path(),
        "cursor-work",
        codex_login::AuthProfileMetadata {
            subscription_provider: codex_login::AuthProfileSubscriptionProvider::Cursor,
            last_permissions: None,
        },
    )?;
    write_mock_provider_models_cache(codex_home.as_path())?;
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir(&bin_dir)?;
    write_fake_executable(&bin_dir, "agent")?;
    write_fake_executable(&bin_dir, "cursor-agent")?;
    let path = path_with_fake_bin(&bin_dir)?;

    let mut mcp = McpProcess::new_with_env(
        codex_home.as_path(),
        &[
            ("CODEWITH_AUTH_PROFILE", Some("cursor-work")),
            ("PATH", Some(path.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;
    Ok((mcp, server, tmp))
}

async fn start_thread(mcp: &mut McpProcess) -> Result<String> {
    let start_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(start_resp)?;
    Ok(thread.id)
}

#[tokio::test]
async fn thread_external_agent_cancel_rejects_empty_and_unknown_runs() -> Result<()> {
    let (mut mcp, _server, _tmp) = start_ready_external_agent_server().await?;
    let thread_id = start_thread(&mut mcp).await?;

    let empty_id = mcp
        .send_thread_external_agent_cancel_request(ThreadExternalAgentCancelParams {
            thread_id: thread_id.clone(),
            run_id: "   ".to_string(),
        })
        .await?;
    let empty_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(empty_id)),
    )
    .await??;
    assert_eq!(
        to_response::<ThreadExternalAgentCancelResponse>(empty_resp)?,
        ThreadExternalAgentCancelResponse {
            cancelled: false,
            message: "runId must not be empty".to_string(),
        }
    );

    let unknown_id = mcp
        .send_thread_external_agent_cancel_request(ThreadExternalAgentCancelParams {
            thread_id,
            run_id: "ext_does_not_exist".to_string(),
        })
        .await?;
    let unknown_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(unknown_id)),
    )
    .await??;
    assert_eq!(
        to_response::<ThreadExternalAgentCancelResponse>(unknown_resp)?,
        ThreadExternalAgentCancelResponse {
            cancelled: false,
            message: "external-agent run `ext_does_not_exist` is not active".to_string(),
        }
    );

    Ok(())
}

// Live runs spawn a sandboxed child process, which is not available on Windows
// CI, so the cancellation-lifecycle assertions are gated to unix.
//
// This exercises the end-to-end lifecycle of a live run: RunStarted is emitted,
// cancellation is requested, the run reaches a terminal state and is removed
// from the active-run registry so a follow-up cancel reports it inactive. The
// terminal event is accepted as either `Cancelled` (when the sandboxed child
// hangs until the cancellation token fires) or `Failed` (when the platform
// sandbox cannot launch the child in this environment) so the registry-cleanup
// guarantee is asserted deterministically regardless of sandbox availability.
#[cfg(unix)]
#[tokio::test]
async fn thread_external_agent_cancel_clears_active_run_after_terminal_event() -> Result<()> {
    let (mut mcp, _server, _tmp) = start_ready_external_agent_server().await?;
    let thread_id = start_thread(&mut mcp).await?;

    let start_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread_id.clone(),
            runtime_id: "cursor".to_string(),
            task: "inspect the auth wiring".to_string(),
            mode: ThreadExternalAgentMode::Plan,
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let response: ThreadExternalAgentStartResponse = to_response(start_resp)?;
    assert_eq!(response.status, ThreadExternalAgentStartStatus::Started);
    let run_id = response.run_id.expect("external-agent run id");

    // RunStarted is emitted before the fake agent is driven.
    let started_notification = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/externalAgent/event"),
    )
    .await??;
    let started: ThreadExternalAgentEventNotification =
        serde_json::from_value(started_notification.params.expect("event params"))?;
    assert_eq!(started.run_id, run_id);
    assert_eq!(
        started.event,
        ThreadExternalAgentEvent::RunStarted {
            runtime_id: "cursor".to_string(),
            mode: ThreadExternalAgentMode::Plan,
            task: "inspect the auth wiring".to_string(),
        }
    );

    // Request cancellation. Whether this wins the race against a fast sandbox
    // failure is environment-dependent, so the acknowledgement boolean is not
    // asserted here; the registry-cleanup poll below is the deterministic guard.
    let cancel_id = mcp
        .send_thread_external_agent_cancel_request(ThreadExternalAgentCancelParams {
            thread_id: thread_id.clone(),
            run_id: run_id.clone(),
        })
        .await?;
    let _cancel_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(cancel_id)),
    )
    .await??;

    // The run tears the child process down and emits a terminal event.
    let terminal_notification = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_notification_message("thread/externalAgent/event"),
    )
    .await??;
    let terminal: ThreadExternalAgentEventNotification =
        serde_json::from_value(terminal_notification.params.expect("event params"))?;
    assert_eq!(terminal.run_id, run_id);
    assert!(
        matches!(
            terminal.event,
            ThreadExternalAgentEvent::Cancelled { .. } | ThreadExternalAgentEvent::Failed { .. }
        ),
        "expected a terminal Cancelled or Failed event, got: {:?}",
        terminal.event
    );

    // Once the run finishes it is removed from the active registry, so a later
    // cancel reports the run is no longer active.
    let inactive_response: ThreadExternalAgentCancelResponse = timeout(DEFAULT_TIMEOUT, async {
        loop {
            let cancel_id = mcp
                .send_thread_external_agent_cancel_request(ThreadExternalAgentCancelParams {
                    thread_id: thread_id.clone(),
                    run_id: run_id.clone(),
                })
                .await?;
            let cancel_resp: JSONRPCResponse = mcp
                .read_stream_until_response_message(RequestId::Integer(cancel_id))
                .await?;
            let response = to_response::<ThreadExternalAgentCancelResponse>(cancel_resp)?;
            if !response.cancelled {
                return Ok::<ThreadExternalAgentCancelResponse, anyhow::Error>(response);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(
        inactive_response,
        ThreadExternalAgentCancelResponse {
            cancelled: false,
            message: format!("external-agent run `{run_id}` is not active"),
        }
    );

    Ok(())
}

fn path_with_fake_bin(bin_dir: &Path) -> Result<String> {
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path = std::env::join_paths(std::iter::once(bin_dir.to_path_buf()).chain(
        std::env::split_paths(&existing_path).filter(|dir| {
            !["claude", "claude.exe", "claude.cmd", "claude.bat"]
                .iter()
                .any(|program| dir.join(program).is_file())
        }),
    ))?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn write_fake_executable(bin_dir: &Path, name: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join(name);
    std::fs::write(&path, "#!/bin/sh\nsleep 30\n")?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(windows)]
fn write_fake_executable(bin_dir: &Path, name: &str) -> Result<()> {
    std::fs::write(
        bin_dir.join(format!("{name}.cmd")),
        "@echo off\r\nping -n 30 127.0.0.1 >NUL\r\n",
    )?;
    Ok(())
}
