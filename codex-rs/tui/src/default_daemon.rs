use std::future::Future;
use std::path::Path;
use std::path::PathBuf;

use codex_utils_absolute_path::AbsolutePathBuf;

use crate::AppServerClient;
use crate::RemoteAppServerEndpoint;
use crate::connect_remote_app_server;

pub(super) async fn connect_default_daemon(
    endpoint: &RemoteAppServerEndpoint,
    codex_bin: Option<&Path>,
) -> color_eyre::Result<AppServerClient> {
    let RemoteAppServerEndpoint::UnixSocket { socket_path } = endpoint else {
        color_eyre::eyre::bail!("local default daemon requires a Unix socket endpoint");
    };
    connect_default_daemon_with(
        socket_path.clone(),
        codex_bin.map(Path::to_path_buf),
        |codex_bin| async move {
            codex_app_server_daemon::ensure_local_daemon_started(
                codex_app_server_daemon::LocalDaemonStartOptions { codex_bin },
            )
            .await
        },
        |socket_path| async move {
            connect_remote_app_server(RemoteAppServerEndpoint::UnixSocket { socket_path }).await
        },
    )
    .await
}

async fn connect_default_daemon_with<C, Ensure, EnsureFuture, Connect, ConnectFuture>(
    selected_socket_path: AbsolutePathBuf,
    codex_bin: Option<PathBuf>,
    ensure_daemon: Ensure,
    connect: Connect,
) -> color_eyre::Result<C>
where
    Ensure: FnOnce(PathBuf) -> EnsureFuture,
    EnsureFuture: Future<Output = anyhow::Result<PathBuf>>,
    Connect: FnOnce(AbsolutePathBuf) -> ConnectFuture,
    ConnectFuture: Future<Output = color_eyre::Result<C>>,
{
    let _ = codex_bin;
    let _ = ensure_daemon;
    connect(selected_socket_path).await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;

    use super::connect_default_daemon_with;

    #[tokio::test]
    async fn delayed_default_daemon_start_reensures_before_durable_connect()
    -> color_eyre::Result<()> {
        let temp_dir = tempfile::TempDir::new()?;
        let expired_socket =
            AbsolutePathBuf::from_absolute_path(temp_dir.path().join("expired.sock"))?;
        let replacement_socket =
            AbsolutePathBuf::from_absolute_path(temp_dir.path().join("replacement.sock"))?;
        let ensure_count = Arc::new(AtomicUsize::new(0));
        let ensure_count_for_closure = Arc::clone(&ensure_count);
        let replacement_for_ensure = replacement_socket.clone();
        let connected_paths = Arc::new(Mutex::new(Vec::new()));
        let connected_paths_for_closure = Arc::clone(&connected_paths);

        let connected_socket = connect_default_daemon_with(
            expired_socket,
            Some(PathBuf::from("/bin/codewith")),
            move |codex_bin| async move {
                assert_eq!(codex_bin, PathBuf::from("/bin/codewith"));
                ensure_count_for_closure.fetch_add(1, Ordering::SeqCst);
                Ok(replacement_for_ensure.as_path().to_path_buf())
            },
            move |socket_path| async move {
                connected_paths_for_closure
                    .lock()
                    .expect("connected paths lock")
                    .push(socket_path.clone());
                Ok(socket_path)
            },
        )
        .await?;

        assert_eq!(ensure_count.load(Ordering::SeqCst), 1);
        assert_eq!(connected_socket, replacement_socket.clone());
        assert_eq!(
            *connected_paths.lock().expect("connected paths lock"),
            vec![replacement_socket]
        );
        Ok(())
    }
}
