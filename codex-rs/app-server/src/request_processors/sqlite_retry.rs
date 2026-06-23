use std::future::Future;
use std::time::Duration;
use tracing::debug;

pub(super) async fn retry_transient_sqlite_busy<T, F, Fut>(
    operation: &str,
    mut f: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut delay = Duration::from_millis(25);
    for attempt in 0..5 {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) if is_transient_sqlite_busy(&err) && attempt < 4 => {
                debug!(
                    operation,
                    attempt = attempt + 1,
                    "retrying app-server operation after SQLite busy: {err}"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("retry loop should return on success or final error")
}

pub(super) fn is_transient_sqlite_busy(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("database is locked") || message.contains("database is busy")
}
