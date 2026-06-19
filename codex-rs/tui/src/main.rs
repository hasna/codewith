use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_arg0::arg0_dispatch_or_else;
use codex_config::LoaderOverrides;
use codex_tui::AppExitInfo;
use codex_tui::Cli;
use codex_tui::ExitReason;
use codex_tui::TmuxHandoffAttachMode;
use codex_tui::TmuxHandoffExit;
use codex_tui::run_main;
use codex_utils_cli::CliConfigOverrides;
use codex_utils_cli::resume_hint;
use supports_color::Stream;

fn format_exit_messages(exit_info: AppExitInfo, color_enabled: bool) -> Vec<String> {
    let AppExitInfo {
        token_usage,
        thread_id,
        thread_name,
        ..
    } = exit_info;

    let mut lines = Vec::new();
    if !token_usage.is_zero() {
        lines.push(token_usage.to_string());
    }

    if let Some(resume_cmd) = resume_hint(thread_name.as_deref(), thread_id) {
        let command = if color_enabled {
            format!("\u{1b}[36m{resume_cmd}\u{1b}[39m")
        } else {
            resume_cmd
        };
        lines.push(format!("To continue this session, run {command}"));
    }

    lines
}

fn run_tmux_handoff(handoff: TmuxHandoffExit) -> anyhow::Result<()> {
    let tmux_command = match handoff.attach_mode {
        TmuxHandoffAttachMode::Attach => "attach-session",
        TmuxHandoffAttachMode::SwitchClient => "switch-client",
    };
    let status = std::process::Command::new("tmux")
        .args([tmux_command, "-t", &handoff.target])
        .status()
        .map_err(|err| anyhow::anyhow!("failed to run tmux {tmux_command}: {err}"))?;
    if !status.success() {
        anyhow::bail!("tmux {tmux_command} failed with status {status}");
    }
    Ok(())
}

#[derive(Parser, Debug)]
struct TopCli {
    #[clap(flatten)]
    config_overrides: CliConfigOverrides,

    #[clap(flatten)]
    inner: Cli,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(|arg0_paths: Arg0DispatchPaths| async move {
        let top_cli = TopCli::parse();
        let mut inner = top_cli.inner;
        inner
            .config_overrides
            .raw_overrides
            .splice(0..0, top_cli.config_overrides.raw_overrides);
        let exit_info = run_main(
            inner,
            arg0_paths,
            LoaderOverrides::default(),
            /*explicit_remote_endpoint*/ None,
        )
        .await?;
        match exit_info.exit_reason {
            ExitReason::Fatal(message) => {
                eprintln!("ERROR: {message}");
                std::process::exit(1);
            }
            ExitReason::UserRequested => {}
        }

        if let Some(tmux_handoff) = exit_info.tmux_handoff.clone() {
            return run_tmux_handoff(tmux_handoff);
        }
        let color_enabled = supports_color::on(Stream::Stdout).is_some();
        for line in format_exit_messages(exit_info, color_enabled) {
            println!("{line}");
        }
        Ok(())
    })
}
