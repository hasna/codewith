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
    write_fake_claude_executable(&bin_dir)?;
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
            .starts_with("Claude Code external-agent runtime is gated: missing auth."),
        "expected Claude local-login readiness gate, got: {}",
        claude_response.message
    );
    assert!(
        !claude_response.message.contains("auth profile"),
        "Claude runtime should not be gated on a generic Claude.ai auth profile: {}",
        claude_response.message
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn thread_external_agent_claude_starts_with_bedrock_profile_env_and_sanitized_launch()
-> Result<()> {
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
    append_shell_env_policy(
        codex_home.as_path(),
        &[
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("AWS_PROFILE", "dev"),
            ("AWS_REGION", "us-east-1"),
            ("ANTHROPIC_MODEL", "claude-sonnet-5"),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", "claude-sonnet-5"),
            ("OPENAI_API_KEY", "must-not-leak"),
        ],
    )?;
    write_mock_provider_models_cache(codex_home.as_path())?;

    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir(&bin_dir)?;
    write_env_echoing_fake_claude_executable(&bin_dir)?;
    let path = path_with_fake_bin(&bin_dir)?;

    let mut mcp =
        McpProcess::new_with_env(codex_home.as_path(), &[("PATH", Some(path.as_str()))]).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            auth_profile: Some(None),
            ..ThreadStartParams::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(start_resp)?;

    let claude_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id,
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
    let response: ThreadExternalAgentStartResponse = to_response(claude_resp)?;
    assert_eq!(
        response.status,
        ThreadExternalAgentStartStatus::Started,
        "Claude should start with Bedrock AWS_PROFILE env and no profile: {}",
        response.message
    );
    let run_id = response.run_id.expect("claude external-agent run id");

    let output = read_external_agent_output_until_completed(&mut mcp, &run_id).await?;
    assert!(
        output.contains("ARGS:--safe-mode"),
        "expected safe-mode args in fake Claude output: {output}"
    );
    assert!(
        output.contains("--permission-mode plan"),
        "expected plan permission mode in fake Claude output: {output}"
    );
    assert!(
        output.contains("--output-format stream-json"),
        "expected stream-json output mode in fake Claude output: {output}"
    );
    assert!(
        output.contains("CLAUDE_CODE_USE_BEDROCK=present"),
        "expected Bedrock selector in fake Claude env: {output}"
    );
    assert!(
        output.contains("AWS_PROFILE=present"),
        "expected AWS_PROFILE in fake Claude env: {output}"
    );
    assert!(
        output.contains("AWS_REGION=present"),
        "expected AWS_REGION in fake Claude env: {output}"
    );
    assert!(
        output.contains("ANTHROPIC_MODEL=present"),
        "expected model override in fake Claude env: {output}"
    );
    assert!(
        output.contains("ANTHROPIC_DEFAULT_SONNET_MODEL=present"),
        "expected default model override in fake Claude env: {output}"
    );
    assert!(
        output.contains("OPENAI_API_KEY=absent"),
        "unrelated provider auth leaked into fake Claude env: {output}"
    );

    Ok(())
}

#[tokio::test]
async fn thread_external_agent_claude_gates_incomplete_provider_selector_without_profile_requirement()
-> Result<()> {
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
    append_shell_env_policy(codex_home.as_path(), &[("CLAUDE_CODE_USE_FOUNDRY", "1")])?;
    write_mock_provider_models_cache(codex_home.as_path())?;

    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir(&bin_dir)?;
    write_fake_claude_executable(&bin_dir)?;
    let path = path_with_fake_bin(&bin_dir)?;

    let mut mcp =
        McpProcess::new_with_env(codex_home.as_path(), &[("PATH", Some(path.as_str()))]).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            auth_profile: Some(None),
            ..ThreadStartParams::default()
        })
        .await?;
    let start_resp: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(start_resp)?;

    let claude_id = mcp
        .send_thread_external_agent_start_request(ThreadExternalAgentStartParams {
            thread_id: thread.id,
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
    let response: ThreadExternalAgentStartResponse = to_response(claude_resp)?;
    assert_eq!(
        response.status,
        ThreadExternalAgentStartStatus::Gated,
        "incomplete Foundry selector should gate: {}",
        response.message
    );
    assert_eq!(response.run_id, None);
    assert!(
        response.message.contains("missing auth")
            && response.message.contains("Foundry")
            && !response.message.contains("auth profile"),
        "expected provider-auth gate without subscription-profile wording: {}",
        response.message
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

async fn read_external_agent_output_until_completed(
    mcp: &mut McpProcess,
    run_id: &str,
) -> Result<String> {
    let mut output = String::new();
    loop {
        let notification = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_notification_message("thread/externalAgent/event"),
        )
        .await??;
        let Some(params) = notification.params else {
            anyhow::bail!("external-agent event params missing");
        };
        let event: ThreadExternalAgentEventNotification = serde_json::from_value(params)?;
        if event.run_id != run_id {
            continue;
        }
        match event.event {
            ThreadExternalAgentEvent::OutputTextDelta { text } => output.push_str(&text),
            ThreadExternalAgentEvent::Completed { .. } => return Ok(output),
            ThreadExternalAgentEvent::Failed { message } => {
                anyhow::bail!("external-agent run failed: {message}");
            }
            _ => {}
        }
    }
}

fn append_shell_env_policy(codex_home: &Path, vars: &[(&str, &str)]) -> Result<()> {
    use std::io::Write;

    let config_toml = codex_home.join("config.toml");
    let mut file = std::fs::OpenOptions::new().append(true).open(config_toml)?;
    writeln!(
        file,
        "\n[shell_environment_policy]\ninherit = \"core\"\n\n[shell_environment_policy.set]"
    )?;
    for (name, value) in vars {
        writeln!(file, "{name} = {}", toml_string(value))?;
    }
    Ok(())
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(unix)]
fn write_env_echoing_fake_claude_executable(bin_dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("claude");
    std::fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  exit 1
fi
present() {
  eval "value=\${$1:-}"
  if [ -n "$value" ]; then
    printf '%s=present;' "$1"
  else
    printf '%s=absent;' "$1"
  fi
}
printf 'ARGS:%s\n' "$*"
printf 'ENV:'
present CLAUDE_CODE_USE_BEDROCK
present AWS_PROFILE
present AWS_REGION
present ANTHROPIC_MODEL
present ANTHROPIC_DEFAULT_SONNET_MODEL
present OPENAI_API_KEY
printf '\n'
printf '{"type":"result","result":"done"}\n'
"#,
    )?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
fn write_fake_claude_executable(bin_dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = bin_dir.join("claude");
    std::fs::write(
        &path,
        "#!/bin/sh\nif [ \"$1\" = \"auth\" ] && [ \"$2\" = \"status\" ]; then\n  exit 1\nfi\nsleep 30\n",
    )?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(windows)]
fn write_fake_claude_executable(bin_dir: &Path) -> Result<()> {
    std::fs::write(
        bin_dir.join("claude.cmd"),
        "@echo off\r\nif \"%1\"==\"auth\" exit /b 1\r\nping -n 30 127.0.0.1 >NUL\r\n",
    )?;
    Ok(())
}

fn path_with_fake_bin(bin_dir: &Path) -> Result<String> {
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf()).chain(std::env::split_paths(&existing_path)),
    )?;
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
