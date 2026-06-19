use codex_protocol::ThreadId;
use codex_protocol::config_types::SandboxMode;
use codex_protocol::protocol::AskForApproval;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use crate::legacy_core::config::Config;

const MAX_TMUX_SESSION_NAME_CHARS: usize = 80;
const MAX_TMUX_WINDOW_NAME_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxHandoffAttachMode {
    Attach,
    SwitchClient,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TmuxHandoffExit {
    pub session_name: String,
    pub window_name: String,
    pub target: String,
    pub attach_mode: TmuxHandoffAttachMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct TmuxHandoffSummary {
    pub(crate) session_name: String,
    pub(crate) window_name: String,
    pub(crate) attach_target: String,
    pub(crate) handoff_command: String,
    pub(crate) attach_mode: TmuxHandoffAttachMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TmuxHandoffDestination {
    NewSession {
        name: Option<String>,
    },
    ExistingSession {
        session_name: String,
        window_name: Option<String>,
    },
}

impl Default for TmuxHandoffDestination {
    fn default() -> Self {
        Self::NewSession { name: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TmuxHandoffPlan {
    pub(crate) session_name: String,
    pub(crate) window_name: String,
    pub(crate) cwd: PathBuf,
    pub(crate) command: Vec<String>,
    pub(crate) shell_command: String,
    pub(crate) replace_existing: bool,
    target: TmuxHandoffPlanTarget,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TmuxHandoffLaunchOptions {
    pub(crate) cli_config_overrides: Vec<(String, toml::Value)>,
    pub(crate) config_profile: Option<String>,
    pub(crate) remote: Option<String>,
    pub(crate) approval_policy: Option<AskForApproval>,
    pub(crate) sandbox_mode: Option<SandboxMode>,
    pub(crate) additional_writable_roots: Vec<PathBuf>,
    pub(crate) bypass_hook_trust: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TmuxHandoffPlanTarget {
    NewSession,
    ExistingSession,
}

pub(crate) fn build_tmux_handoff_plan(
    config: &Config,
    thread_id: ThreadId,
    destination: TmuxHandoffDestination,
    replace_existing: bool,
    current_model: &str,
    launch_options: &TmuxHandoffLaunchOptions,
) -> Result<TmuxHandoffPlan, String> {
    let (target, session_name, window_name) = match destination {
        TmuxHandoffDestination::NewSession { name } => {
            let session_name = match name {
                Some(name) => normalize_tmux_name(&name)?,
                None => default_tmux_name_for_cwd(&config.cwd),
            };
            (
                TmuxHandoffPlanTarget::NewSession,
                session_name.clone(),
                session_name,
            )
        }
        TmuxHandoffDestination::ExistingSession {
            session_name,
            window_name,
        } => {
            let session_name = validate_existing_tmux_session_name(&session_name)?;
            let window_name = match window_name {
                Some(window_name) => normalize_tmux_window_name(&window_name)?,
                None => default_tmux_name_for_cwd(&config.cwd),
            };
            (
                TmuxHandoffPlanTarget::ExistingSession,
                session_name,
                window_name,
            )
        }
    };
    let cwd = config.cwd.to_path_buf();
    let mut command = vec![
        codewith_program(config),
        "resume".to_string(),
        thread_id.to_string(),
    ];
    if let Some(remote) = launch_options
        .remote
        .as_deref()
        .filter(|remote| !remote.trim().is_empty())
    {
        command.push("--remote".to_string());
        command.push(remote.to_string());
    }
    if let Some(profile) = launch_options
        .config_profile
        .as_deref()
        .filter(|profile| !profile.trim().is_empty())
    {
        command.push("--profile".to_string());
        command.push(profile.to_string());
    }
    for (key, value) in &launch_options.cli_config_overrides {
        command.push("-c".to_string());
        command.push(format!("{key}={value}"));
    }
    if !config.model_provider_id.trim().is_empty() {
        command.push("-c".to_string());
        command.push(format!(
            "model_provider={}",
            toml::Value::String(config.model_provider_id.clone())
        ));
    }
    command.push("--cd".to_string());
    command.push(cwd.display().to_string());
    if !current_model.trim().is_empty() {
        command.push("-m".to_string());
        command.push(current_model.to_string());
    } else if let Some(model) = config
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
    {
        command.push("-m".to_string());
        command.push(model.to_string());
    }
    if let Some(auth_profile) = config
        .selected_auth_profile
        .as_deref()
        .filter(|profile| !profile.trim().is_empty())
    {
        command.push("--auth-profile".to_string());
        command.push(auth_profile.to_string());
    }
    if let Some(approval_policy) = launch_options
        .approval_policy
        .as_ref()
        .copied()
        .and_then(approval_policy_cli_arg)
    {
        command.push("--ask-for-approval".to_string());
        command.push(approval_policy.to_string());
    }
    if let Some(sandbox_mode) = launch_options.sandbox_mode {
        command.push("--sandbox".to_string());
        command.push(sandbox_mode.to_string());
    }
    for root in &launch_options.additional_writable_roots {
        command.push("--add-dir".to_string());
        command.push(root.display().to_string());
    }
    if launch_options.bypass_hook_trust {
        command.push("--dangerously-bypass-hook-trust".to_string());
    }

    let shell_command = shlex::try_join(command.iter().map(String::as_str))
        .map_err(|err| format!("failed to quote tmux command: {err}"))?;

    Ok(TmuxHandoffPlan {
        session_name,
        window_name,
        cwd,
        command,
        shell_command,
        replace_existing,
        target,
    })
}

pub(crate) fn create_tmux_handoff_session(
    plan: &TmuxHandoffPlan,
) -> Result<TmuxHandoffSummary, String> {
    let attach_mode = current_attach_mode();
    if plan.target == TmuxHandoffPlanTarget::NewSession
        && attach_mode == TmuxHandoffAttachMode::SwitchClient
        && current_tmux_session_name()?.as_deref() == Some(plan.session_name.as_str())
    {
        return Err(format!(
            "This Codewith session is already running in tmux session `{}`.",
            plan.session_name
        ));
    }
    ensure_tmux_available()?;
    let attach_target = match plan.target {
        TmuxHandoffPlanTarget::NewSession => {
            if tmux_session_exists(&plan.session_name)? {
                if plan.replace_existing {
                    kill_tmux_session(&plan.session_name)?;
                } else {
                    return Err(format!(
                        "tmux session `{}` already exists. Omit `--no-replace` to replace it.",
                        plan.session_name
                    ));
                }
            }

            let status = Command::new("tmux")
                .args(["new-session", "-d", "-s", &plan.session_name])
                .args(["-n", &plan.window_name])
                .args(["-c", &plan.cwd.display().to_string()])
                .arg("--")
                .arg(&plan.shell_command)
                .status()
                .map_err(|err| format!("failed to start tmux: {err}"))?;
            if !status.success() {
                return Err(format!(
                    "tmux failed to create session `{}`",
                    plan.session_name
                ));
            }
            exact_tmux_target(&plan.session_name)
        }
        TmuxHandoffPlanTarget::ExistingSession => {
            if !tmux_session_exists(&plan.session_name)? {
                return Err(format!(
                    "tmux session `{}` does not exist. Omit `--session` to create a new tmux session automatically.",
                    plan.session_name
                ));
            }
            let target = exact_tmux_target(&plan.session_name);
            let output = Command::new("tmux")
                .args(["new-window", "-d", "-P", "-F", "#{window_id}"])
                .args(["-t", target.as_str()])
                .args(["-n", &plan.window_name])
                .args(["-c", &plan.cwd.display().to_string()])
                .arg("--")
                .arg(&plan.shell_command)
                .output()
                .map_err(|err| {
                    format!(
                        "failed to create tmux window `{}` in session `{}`: {err}",
                        plan.window_name, plan.session_name
                    )
                })?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr = stderr.trim();
                if stderr.is_empty() {
                    return Err(format!(
                        "tmux failed to create window `{}` in session `{}`",
                        plan.window_name, plan.session_name
                    ));
                }
                return Err(format!(
                    "tmux failed to create window `{}` in session `{}`: {stderr}",
                    plan.window_name, plan.session_name
                ));
            }
            let window_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if window_id.is_empty() {
                return Err(format!(
                    "tmux did not return a target for window `{}` in session `{}`",
                    plan.window_name, plan.session_name
                ));
            }
            exact_tmux_window_target(&plan.session_name, &window_id)
        }
    };

    Ok(TmuxHandoffSummary {
        session_name: plan.session_name.clone(),
        window_name: plan.window_name.clone(),
        handoff_command: handoff_command(attach_mode, &attach_target),
        attach_target,
        attach_mode,
    })
}

pub(crate) fn handoff_command(mode: TmuxHandoffAttachMode, target: &str) -> String {
    let command = match mode {
        TmuxHandoffAttachMode::Attach => "attach-session",
        TmuxHandoffAttachMode::SwitchClient => "switch-client",
    };
    shlex::try_join(["tmux", command, "-t", target])
        .unwrap_or_else(|_| format!("tmux {command} -t {target}"))
}

impl TmuxHandoffSummary {
    pub(crate) fn exit(&self) -> TmuxHandoffExit {
        TmuxHandoffExit {
            session_name: self.session_name.clone(),
            window_name: self.window_name.clone(),
            target: self.attach_target.clone(),
            attach_mode: self.attach_mode,
        }
    }
}

pub(crate) fn exact_tmux_target(session_name: &str) -> String {
    format!("={session_name}")
}

fn exact_tmux_window_target(session_name: &str, window_id: &str) -> String {
    format!("{}:{window_id}", exact_tmux_target(session_name))
}

fn codewith_program(config: &Config) -> String {
    config
        .codex_self_exe
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "codewith".to_string())
}

fn approval_policy_cli_arg(policy: AskForApproval) -> Option<&'static str> {
    match policy {
        AskForApproval::UnlessTrusted => Some("untrusted"),
        AskForApproval::OnFailure => Some("on-failure"),
        AskForApproval::OnRequest => Some("on-request"),
        AskForApproval::Never => Some("never"),
        AskForApproval::Granular(_) => None,
    }
}

fn default_tmux_name_for_cwd(cwd: &Path) -> String {
    let name_source = codex_git_utils::get_git_repo_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    tmux_name_from_path(&name_source).unwrap_or_else(|| "codewith".to_string())
}

fn tmux_name_from_path(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| {
            let normalized = normalize_tmux_name(name).ok()?;
            (!normalized.is_empty()).then_some(normalized)
        })
}

fn normalize_tmux_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("tmux session name cannot be empty.".to_string());
    }
    let normalized = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, ':' | '/' | '\\' | '=') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let normalized = normalized
        .chars()
        .take(MAX_TMUX_SESSION_NAME_CHARS)
        .collect::<String>();
    if normalized.is_empty() {
        return Err("tmux session name cannot be empty.".to_string());
    }
    Ok(normalized)
}

fn normalize_tmux_window_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("tmux window name cannot be empty.".to_string());
    }
    let normalized = name
        .chars()
        .map(|ch| if ch.is_control() { '-' } else { ch })
        .take(MAX_TMUX_WINDOW_NAME_CHARS)
        .collect::<String>();
    if normalized.is_empty() {
        return Err("tmux window name cannot be empty.".to_string());
    }
    Ok(normalized)
}

fn validate_existing_tmux_session_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("tmux session name cannot be empty.".to_string());
    }
    if name
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, ':' | '='))
    {
        return Err(
            "tmux session name cannot contain control characters, `:`, or `=`.".to_string(),
        );
    }
    Ok(name.to_string())
}

fn current_attach_mode() -> TmuxHandoffAttachMode {
    if std::env::var_os("TMUX").is_some() || std::env::var_os("TMUX_PANE").is_some() {
        TmuxHandoffAttachMode::SwitchClient
    } else {
        TmuxHandoffAttachMode::Attach
    }
}

fn ensure_tmux_available() -> Result<(), String> {
    let status = Command::new("tmux")
        .arg("-V")
        .status()
        .map_err(|err| format!("tmux is not available: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("tmux is not available.".to_string())
    }
}

fn tmux_session_exists(session_name: &str) -> Result<bool, String> {
    let target = exact_tmux_target(session_name);
    let status = Command::new("tmux")
        .args(["has-session", "-t", target.as_str()])
        .status()
        .map_err(|err| format!("failed to inspect tmux sessions: {err}"))?;
    Ok(status.success())
}

fn kill_tmux_session(session_name: &str) -> Result<(), String> {
    let target = exact_tmux_target(session_name);
    let status = Command::new("tmux")
        .args(["kill-session", "-t", target.as_str()])
        .status()
        .map_err(|err| format!("failed to replace tmux session `{session_name}`: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("failed to replace tmux session `{session_name}`"))
    }
}

fn current_tmux_session_name() -> Result<Option<String>, String> {
    if current_attach_mode() != TmuxHandoffAttachMode::SwitchClient {
        return Ok(None);
    }
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#S"])
        .output()
        .map_err(|err| format!("failed to inspect current tmux session: {err}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!name.is_empty()).then_some(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy_core::config::ConfigBuilder;
    use crate::legacy_core::config::ConfigOverrides;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;

    async fn test_config(cwd: AbsolutePathBuf) -> Config {
        ConfigBuilder::default()
            .codex_home(tempfile::tempdir().expect("tempdir").keep())
            .harness_overrides(ConfigOverrides {
                cwd: Some(cwd.to_path_buf()),
                codex_self_exe: Some(PathBuf::from("/usr/local/bin/codewith")),
                auth_profile: Some(Some("work".to_string())),
                ..Default::default()
            })
            .build()
            .await
            .expect("test config")
    }

    #[test]
    fn default_name_uses_repo_directory_name() {
        let cwd = test_path_buf("/home/user/open-codewith").abs();
        assert_eq!(default_tmux_name_for_cwd(&cwd), "open-codewith");
    }

    #[test]
    fn default_name_uses_git_root_from_nested_cwd() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let repo = temp_dir.path().join("repo-name");
        std::fs::create_dir_all(repo.join(".git")).expect("create git dir");
        let nested_cwd = repo.join("crates").join("tui").join("src");
        std::fs::create_dir_all(&nested_cwd).expect("create nested cwd");

        assert_eq!(default_tmux_name_for_cwd(&nested_cwd), "repo-name");
    }

    #[test]
    fn tmux_name_is_normalized_for_target_safety() {
        assert_eq!(
            normalize_tmux_name(" repo:name / branch\nmain ").expect("valid name"),
            "repo-name---branch-main"
        );
        assert_eq!(normalize_tmux_name("=repo").expect("valid name"), "-repo");
        assert!(normalize_tmux_name(" \n\t ").is_err());
    }

    #[tokio::test]
    async fn plan_resumes_current_thread_with_cwd_model_and_auth_profile() {
        let config = test_config(test_path_buf("/home/user/open-codewith").abs()).await;
        let thread_id =
            ThreadId::from_string("123e4567-e89b-12d3-a456-426614174000").expect("thread id");
        let plan = build_tmux_handoff_plan(
            &config,
            thread_id,
            TmuxHandoffDestination::NewSession {
                name: Some("named session".to_string()),
            },
            /*replace_existing*/ true,
            "gpt-test",
            &TmuxHandoffLaunchOptions::default(),
        )
        .expect("handoff plan");

        assert_eq!(plan.session_name, "named-session");
        assert_eq!(plan.window_name, "named-session");
        assert!(plan.replace_existing);
        assert_eq!(
            plan.command,
            vec![
                "/usr/local/bin/codewith",
                "resume",
                "123e4567-e89b-12d3-a456-426614174000",
                "-c",
                "model_provider=\"openai\"",
                "--cd",
                "/home/user/open-codewith",
                "-m",
                "gpt-test",
                "--auth-profile",
                "work",
            ]
        );
        assert!(plan.shell_command.contains("codewith resume"));
    }

    #[tokio::test]
    async fn plan_preserves_launch_options_that_affect_runtime_settings() {
        let config = test_config(test_path_buf("/home/user/open-codewith").abs()).await;
        let thread_id =
            ThreadId::from_string("123e4567-e89b-12d3-a456-426614174000").expect("thread id");
        let launch_options = TmuxHandoffLaunchOptions {
            cli_config_overrides: vec![(
                "web_search".to_string(),
                toml::Value::String("live".to_string()),
            )],
            config_profile: Some("work-config".to_string()),
            remote: Some("unix:///tmp/codewith.sock".to_string()),
            approval_policy: Some(AskForApproval::Never),
            sandbox_mode: Some(SandboxMode::WorkspaceWrite),
            additional_writable_roots: vec![PathBuf::from("/tmp/extra")],
            bypass_hook_trust: true,
        };
        let plan = build_tmux_handoff_plan(
            &config,
            thread_id,
            TmuxHandoffDestination::default(),
            /*replace_existing*/ true,
            "gpt-test",
            &launch_options,
        )
        .expect("handoff plan");

        assert_eq!(
            plan.command,
            vec![
                "/usr/local/bin/codewith",
                "resume",
                "123e4567-e89b-12d3-a456-426614174000",
                "--remote",
                "unix:///tmp/codewith.sock",
                "--profile",
                "work-config",
                "-c",
                "web_search=\"live\"",
                "-c",
                "model_provider=\"openai\"",
                "--cd",
                "/home/user/open-codewith",
                "-m",
                "gpt-test",
                "--auth-profile",
                "work",
                "--ask-for-approval",
                "never",
                "--sandbox",
                "workspace-write",
                "--add-dir",
                "/tmp/extra",
                "--dangerously-bypass-hook-trust",
            ]
        );
    }

    #[tokio::test]
    async fn plan_targets_existing_session_with_new_window() {
        let config = test_config(test_path_buf("/home/user/open-codewith").abs()).await;
        let thread_id =
            ThreadId::from_string("123e4567-e89b-12d3-a456-426614174000").expect("thread id");
        let plan = build_tmux_handoff_plan(
            &config,
            thread_id,
            TmuxHandoffDestination::ExistingSession {
                session_name: "dev session".to_string(),
                window_name: Some("Codewith window".to_string()),
            },
            /*replace_existing*/ false,
            "gpt-test",
            &TmuxHandoffLaunchOptions::default(),
        )
        .expect("handoff plan");

        assert_eq!(plan.target, TmuxHandoffPlanTarget::ExistingSession);
        assert_eq!(plan.session_name, "dev session");
        assert_eq!(plan.window_name, "Codewith window");
        assert!(!plan.replace_existing);
    }

    #[test]
    fn handoff_command_targets_exact_session_name() {
        assert_eq!(
            handoff_command(
                TmuxHandoffAttachMode::Attach,
                &exact_tmux_target("named-session")
            ),
            "tmux attach-session -t '=named-session'"
        );
        assert_eq!(
            handoff_command(
                TmuxHandoffAttachMode::SwitchClient,
                &exact_tmux_window_target("named-session", "@42"),
            ),
            "tmux switch-client -t '=named-session:@42'"
        );
    }
}
