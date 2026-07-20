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
use codex_app_server_protocol::ThreadExternalAgentExecutionSurface;
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
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;

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
        .send_thread_external_agent_start_request(external_agent_start_params(
            thread.id.clone(),
            "cursor",
            "inspect the auth wiring",
            ThreadExternalAgentMode::Plan,
        ))
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
        .send_thread_external_agent_start_request(external_agent_start_params(
            thread.id.clone(),
            "grok",
            "inspect the auth wiring",
            ThreadExternalAgentMode::Plan,
        ))
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
        .send_thread_external_agent_start_request(external_agent_start_params(
            thread.id,
            "grok-build",
            "   ",
            ThreadExternalAgentMode::Plan,
        ))
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
        .send_thread_external_agent_start_request(external_agent_start_params(
            default_profile_thread.id,
            "claude",
            "inspect the auth wiring",
            ThreadExternalAgentMode::Plan,
        ))
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

fn external_agent_start_params(
    thread_id: String,
    runtime_id: &str,
    task: &str,
    mode: ThreadExternalAgentMode,
) -> ThreadExternalAgentStartParams {
    ThreadExternalAgentStartParams {
        thread_id,
        runtime_id: runtime_id.to_string(),
        task: task.to_string(),
        mode,
        model: None,
        execution_surface: None,
        managed: false,
    }
}

/// Managed Cursor runs are accepted end-to-end now that Codewith's action
/// executor mediates them: `managed: true` no longer gates, and a run starts on
/// the requested execution surface with a discovered model.
#[tokio::test]
async fn thread_external_agent_managed_cursor_run_is_accepted() -> Result<()> {
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

    let mut params = external_agent_start_params(
        thread.id.clone(),
        "cursor",
        "refactor the auth module",
        ThreadExternalAgentMode::Plan,
    );
    params.managed = true;
    params.execution_surface = Some(ThreadExternalAgentExecutionSurface::SdkLocal);
    params.model = Some("auto".to_string());

    let external_agent_id = mcp.send_thread_external_agent_start_request(params).await?;
    let external_agent_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(external_agent_id)),
    )
    .await??;
    let response: ThreadExternalAgentStartResponse = to_response(external_agent_resp)?;
    assert_eq!(
        response.status,
        ThreadExternalAgentStartStatus::Started,
        "managed cursor runs must be accepted: {}",
        response.message
    );
    let run_id = response.run_id.expect("managed run id");

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
    assert_eq!(
        started.event,
        ThreadExternalAgentEvent::RunStarted {
            runtime_id: "cursor".to_string(),
            mode: ThreadExternalAgentMode::Plan,
            task: "refactor the auth module".to_string(),
        }
    );

    // Best-effort cancel so the fake harness subprocess does not linger.
    let cancel_id = mcp
        .send_thread_external_agent_cancel_request(ThreadExternalAgentCancelParams {
            thread_id: thread.id.clone(),
            run_id,
        })
        .await?;
    let _cancel_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(cancel_id)),
    )
    .await??;

    Ok(())
}

/// Execution-surface, model, and managed-mode selections are validated against
/// each runtime's advertised descriptor before a run starts.
#[tokio::test]
async fn thread_external_agent_selection_is_validated_against_the_descriptor() -> Result<()> {
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

    // The cloud execution surface is discoverable but gated pending the cloud harness.
    let mut cloud = external_agent_start_params(
        thread.id.clone(),
        "cursor",
        "inspect the repo",
        ThreadExternalAgentMode::Plan,
    );
    cloud.execution_surface = Some(ThreadExternalAgentExecutionSurface::Cloud);
    let cloud_response = send_external_agent_start(&mut mcp, cloud).await?;
    assert_eq!(cloud_response.status, ThreadExternalAgentStartStatus::Gated);
    assert!(
        cloud_response.message.contains("cloud execution surface"),
        "unexpected cloud gate message: {}",
        cloud_response.message
    );

    // Unknown models are rejected against the descriptor's advertised set.
    let mut bad_model = external_agent_start_params(
        thread.id.clone(),
        "cursor",
        "inspect the repo",
        ThreadExternalAgentMode::Plan,
    );
    bad_model.model = Some("totally-made-up-model".to_string());
    let bad_model_response = send_external_agent_start(&mut mcp, bad_model).await?;
    assert_eq!(
        bad_model_response.status,
        ThreadExternalAgentStartStatus::Gated
    );
    assert!(
        bad_model_response.message.contains("unknown model"),
        "unexpected model gate message: {}",
        bad_model_response.message
    );

    // Managed mode is gated for runtimes that do not advertise it yet.
    let mut managed_grok = external_agent_start_params(
        thread.id,
        "grok-build",
        "inspect the repo",
        ThreadExternalAgentMode::Plan,
    );
    managed_grok.managed = true;
    let managed_grok_response = send_external_agent_start(&mut mcp, managed_grok).await?;
    assert_eq!(
        managed_grok_response.status,
        ThreadExternalAgentStartStatus::Gated
    );
    assert!(
        managed_grok_response
            .message
            .contains("does not support managed mode"),
        "unexpected managed gate message: {}",
        managed_grok_response.message
    );

    Ok(())
}

async fn send_external_agent_start(
    mcp: &mut McpProcess,
    params: ThreadExternalAgentStartParams,
) -> Result<ThreadExternalAgentStartResponse> {
    let request_id = mcp.send_thread_external_agent_start_request(params).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(to_response(response)?)
}

fn path_with_fake_bin(bin_dir: &Path) -> Result<String> {
    let mut paths = vec![bin_dir.to_path_buf()];
    if cfg!(windows) {
        let system_root = std::env::var_os("SystemRoot")
            .ok_or_else(|| anyhow::anyhow!("SystemRoot is required on Windows"))?;
        paths.push(PathBuf::from(system_root).join("System32"));
    } else {
        paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
    }
    let path = std::env::join_paths(paths)?;
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
        "@echo off\r\nif \"%2\"==\"--help\" exit /b 0\r\ntimeout /t 30 /nobreak >NUL\r\n",
    )?;
    Ok(())
}
