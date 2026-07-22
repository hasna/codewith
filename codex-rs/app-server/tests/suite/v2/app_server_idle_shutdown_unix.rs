//! Lifecycle regression tests for per-session, TUI-owned `--listen unix://`
//! app-servers.
//!
//! A Codewith session runs `codewith` (TUI) → `codewith app-server --listen
//! unix://` (per-session backend). When the TUI is killed by a signal (e.g.
//! SIGHUP from a closing terminal/tmux pane) it runs no cleanup, so the backend
//! used to be reparented to init and linger forever — serving no TUI and leaking
//! resources. With `--exit-on-idle-ms`, the backend now exits shortly after its
//! last client disconnects (or after startup if a client never connects), while
//! servers launched WITHOUT the flag keep running as before.
//!
//! Gated to Linux (the deployment target for these per-session daemons) to avoid
//! macOS unix-socket path-length limits and slow-CI startup-timing flakiness.
//! The cross-platform gate logic and daemon-spawn wiring are covered by unit
//! tests in `codex-rs/app-server/src/lib.rs` and
//! `codex-rs/app-server-daemon/src/backend/pid_tests.rs`.

use std::path::Path;
use std::process::Stdio;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use app_test_support::DISABLE_PLUGIN_STARTUP_TASKS_ARG;
use tempfile::TempDir;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::process::Child;
use tokio::process::Command;
use tokio::time::Duration;
use tokio::time::Instant;
use tokio::time::sleep;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async;

use super::connection_handling_websocket::create_config_toml;

type UnixWsClient = WebSocketStream<UnixStream>;

/// Reconnect grace passed to `--exit-on-idle-ms`. Kept short so the tests run
/// fast; the disconnect path does not race this window because the socket only
/// binds after the (slow) startup work completes and any client that connects
/// before the processor loop runs is buffered and disarms the timer on the first
/// loop iteration.
const IDLE_GRACE: Duration = Duration::from_millis(750);

/// Upper bound for the process to notice a disconnect, run the idle grace,
/// drain, and exit. Generous relative to `IDLE_GRACE` to absorb slow cold
/// starts of the freshly built binary on a loaded builder.
const EXIT_WINDOW: Duration = Duration::from_secs(30);

async fn spawn_unix_app_server(
    codex_home: &Path,
    socket_path: &Path,
    extra_args: &[&str],
) -> Result<Child> {
    let program = codex_utils_cargo_bin::cargo_bin("codex-app-server")
        .context("should find app-server binary")?;
    let listen_url = format!("unix://{}", socket_path.display());
    let mut cmd = Command::new(program);
    cmd.arg("--listen")
        .arg(&listen_url)
        .arg(DISABLE_PLUGIN_STARTUP_TASKS_ARG)
        .args(extra_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("CODEWITH_HOME", codex_home)
        .env("CODEX_HOME", codex_home)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .context("failed to spawn unix app-server process")?;

    // Drain stderr to the test log so a failing startup is diagnosable and the
    // pipe never fills.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("[unix app-server stderr] {line}");
            }
        });
    }

    Ok(child)
}

/// Wait for the control socket to accept a connection, then complete the
/// websocket handshake. This doubles as the readiness signal and the client
/// connection whose lifecycle drives idle shutdown.
async fn connect_unix_ws(socket_path: &Path) -> Result<UnixWsClient> {
    let deadline = Instant::now() + EXIT_WINDOW;
    loop {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => match client_async("ws://localhost/", stream).await {
                Ok((ws, _response)) => return Ok(ws),
                Err(err) => {
                    if Instant::now() >= deadline {
                        bail!("failed websocket handshake over unix control socket: {err}");
                    }
                }
            },
            Err(_) => {
                if Instant::now() >= deadline {
                    bail!(
                        "unix control socket never became connectable at {}",
                        socket_path.display()
                    );
                }
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_exit(process: &mut Child, window: Duration) -> Result<std::process::ExitStatus> {
    timeout(window, process.wait())
        .await
        .context("timed out waiting for app-server process exit")?
        .context("failed waiting for app-server process exit")
}

async fn assert_still_running(process: &mut Child, window: Duration) -> Result<()> {
    match timeout(window, process.wait()).await {
        Err(_) => Ok(()),
        Ok(Ok(status)) => bail!("app-server exited unexpectedly: {status}"),
        Ok(Err(err)) => Err(err).context("failed waiting for app-server process"),
    }
}

fn idle_ms_arg() -> String {
    IDLE_GRACE.as_millis().to_string()
}

#[tokio::test]
async fn per_session_app_server_exits_after_last_client_disconnects() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), "http://127.0.0.1:1", "never")?;
    let socket_path = codex_home.path().join("app-server.sock");

    let idle_ms = idle_ms_arg();
    let mut process = spawn_unix_app_server(
        codex_home.path(),
        &socket_path,
        &["--exit-on-idle-ms", &idle_ms],
    )
    .await?;

    // The connected client keeps the backend alive.
    let ws = connect_unix_ws(&socket_path).await?;
    assert_still_running(&mut process, IDLE_GRACE * 2).await?;

    // Simulate the owning TUI going away without cleanup: drop the connection,
    // closing the underlying unix stream (EOF on the server side).
    drop(ws);

    let status = wait_for_exit(&mut process, EXIT_WINDOW).await?;
    assert!(
        status.success(),
        "expected clean idle-shutdown exit, got {status}"
    );
    Ok(())
}

#[tokio::test]
async fn per_session_app_server_exits_when_no_client_ever_connects() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), "http://127.0.0.1:1", "never")?;
    let socket_path = codex_home.path().join("app-server.sock");

    let idle_ms = idle_ms_arg();
    let mut process = spawn_unix_app_server(
        codex_home.path(),
        &socket_path,
        &["--exit-on-idle-ms", &idle_ms],
    )
    .await?;

    // A backend whose owning TUI dies before it ever connects must still exit
    // rather than linger as an orphan reparented to init.
    let status = wait_for_exit(&mut process, EXIT_WINDOW).await?;
    assert!(
        status.success(),
        "expected clean idle-shutdown exit, got {status}"
    );
    Ok(())
}

#[tokio::test]
async fn unix_app_server_without_idle_flag_stays_alive_after_disconnect() -> Result<()> {
    let codex_home = TempDir::new()?;
    create_config_toml(codex_home.path(), "http://127.0.0.1:1", "never")?;
    let socket_path = codex_home.path().join("app-server.sock");

    // No `--exit-on-idle-ms`: idle shutdown is opt-in, so the server must stay
    // available for the next connection (no regression for standalone daemons or
    // manually launched control sockets).
    let mut process = spawn_unix_app_server(codex_home.path(), &socket_path, &[]).await?;

    let ws = connect_unix_ws(&socket_path).await?;
    drop(ws);

    assert_still_running(&mut process, IDLE_GRACE * 4).await?;
    // `kill_on_drop` tears the process down at end of scope.
    Ok(())
}
