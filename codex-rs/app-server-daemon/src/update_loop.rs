#[cfg(unix)]
use std::future::Future;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command as StdCommand;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(unix))]
use anyhow::bail;
#[cfg(unix)]
use codex_utils_home_dir::find_codex_home;
#[cfg(unix)]
use futures::FutureExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use tokio::io::AsyncWriteExt;
#[cfg(unix)]
use tokio::process::Command;
#[cfg(unix)]
use tokio::signal::unix::Signal;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
#[cfg(unix)]
use tokio::signal::unix::signal;
#[cfg(unix)]
use tokio::time::sleep;

#[cfg(unix)]
use crate::managed_install::ExecutableIdentity;
#[cfg(unix)]
use crate::managed_install::executable_identity;
#[cfg(unix)]
use crate::managed_install::managed_codex_bin;
#[cfg(unix)]
use crate::managed_install::resolved_managed_codex_bin;

#[cfg(unix)]
const INITIAL_UPDATE_DELAY: Duration = Duration::from_secs(5 * 60);
#[cfg(unix)]
const UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[cfg(unix)]
pub(crate) async fn run() -> Result<()> {
    let mut terminate =
        signal(SignalKind::terminate()).context("failed to install updater shutdown handler")?;
    let running_updater_identity = current_updater_identity().await?;
    let codex_home = find_codex_home().context("failed to resolve CODEWITH_HOME")?;
    let managed_codex_bin = managed_codex_bin(&codex_home);
    if sleep_or_terminate(INITIAL_UPDATE_DELAY, &mut terminate).await {
        return Ok(());
    }
    loop {
        match perform_update_cycle(
            &running_updater_identity,
            &managed_codex_bin,
            install_latest_standalone,
        )
        .await
        {
            Ok(outcome) => {
                if terminate.recv().now_or_never().flatten().is_some() {
                    return Ok(());
                }
                if let Err(err) = complete_update_cycle(outcome, reexec_managed_updater) {
                    eprintln!("{}", update_failure_diagnostic(&err));
                }
            }
            Err(err) => eprintln!("{}", update_failure_diagnostic(&err)),
        }
        if sleep_or_terminate(UPDATE_INTERVAL, &mut terminate).await {
            return Ok(());
        }
    }
}

#[cfg(not(unix))]
pub(crate) async fn run() -> Result<()> {
    bail!("pid-managed updater loop is unsupported on this platform")
}

#[cfg(unix)]
async fn sleep_or_terminate(duration: Duration, terminate: &mut Signal) -> bool {
    tokio::select! {
        _ = sleep(duration) => false,
        _ = terminate.recv() => true,
    }
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum UpdateCycleOutcome {
    Unchanged,
    Reexec(PathBuf),
}

#[cfg(unix)]
async fn perform_update_cycle<Install, InstallFuture>(
    running_updater_identity: &ExecutableIdentity,
    managed_codex_bin: &Path,
    install_latest: Install,
) -> Result<UpdateCycleOutcome>
where
    Install: FnOnce() -> InstallFuture,
    InstallFuture: Future<Output = Result<()>>,
{
    install_latest().await?;

    let managed_codex_bin = resolved_managed_codex_bin(managed_codex_bin).await?;
    let managed_identity = executable_identity(&managed_codex_bin).await?;
    if running_updater_identity != &managed_identity {
        Ok(UpdateCycleOutcome::Reexec(managed_codex_bin))
    } else {
        Ok(UpdateCycleOutcome::Unchanged)
    }
}

#[cfg(unix)]
fn complete_update_cycle<Reexec>(outcome: UpdateCycleOutcome, reexec: Reexec) -> Result<()>
where
    Reexec: FnOnce(&Path) -> Result<()>,
{
    if let UpdateCycleOutcome::Reexec(managed_codex_bin) = outcome {
        reexec(&managed_codex_bin)?;
    }
    Ok(())
}

#[cfg(unix)]
async fn current_updater_identity() -> Result<ExecutableIdentity> {
    let current_exe =
        std::env::current_exe().context("failed to resolve current updater executable")?;
    executable_identity(&current_exe).await
}

#[cfg(unix)]
fn update_failure_diagnostic(err: &anyhow::Error) -> String {
    format!("Codewith app-server daemon updater failed: {err:#}")
}

#[cfg(unix)]
pub(crate) fn reexec_managed_updater(managed_codex_bin: &Path) -> Result<()> {
    let err = StdCommand::new(managed_codex_bin)
        .args(["app-server", "daemon", "pid-update-loop"])
        .exec();
    Err(err).with_context(|| {
        format!(
            "failed to replace updater with managed Codewith binary {}",
            managed_codex_bin.display()
        )
    })
}

#[cfg(unix)]
async fn install_latest_standalone() -> Result<()> {
    let script = reqwest::get(
        "https://raw.githubusercontent.com/hasna/codewith/main/scripts/install/install.sh",
    )
    .await
    .context("failed to fetch standalone Codewith updater")?
    .error_for_status()
    .context("standalone Codewith updater request failed")?
    .bytes()
    .await
    .context("failed to read standalone Codewith updater")?;

    let mut child = Command::new("/bin/sh")
        .arg("-s")
        .env_remove("CODEWITH_RELEASE")
        .env_remove("CODEX_RELEASE")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to invoke standalone Codewith updater")?;
    let mut stdin = child
        .stdin
        .take()
        .context("standalone Codewith updater stdin was unavailable")?;
    stdin
        .write_all(&script)
        .await
        .context("failed to pass standalone Codewith updater to shell")?;
    drop(stdin);
    let status = child
        .wait()
        .await
        .context("failed to wait for standalone Codewith updater")?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("standalone Codewith updater exited with status {status}")
    }
}

#[cfg(all(test, unix))]
#[path = "update_loop_tests.rs"]
mod tests;
