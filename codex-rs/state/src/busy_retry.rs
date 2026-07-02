//! Shared retry policy for transient SQLite busy/locked errors.
//!
//! The state DB is written by many concurrent processes (CLI invocations,
//! the app-server daemon, background-agent workers). Under write-lock or
//! WAL-snapshot contention SQLite surfaces transient errors:
//!
//! - `SQLITE_BUSY` (code 5, "database is locked"): another writer held the
//!   lock past the connection's `busy_timeout`.
//! - `SQLITE_BUSY_SNAPSHOT` (extended code 517): a read transaction tried to
//!   upgrade to a write after another writer advanced the WAL. SQLite
//!   returns this immediately *without* invoking the busy handler, so
//!   `busy_timeout` cannot absorb it; the statement or transaction must be
//!   restarted at the application level.
//!
//! Neither error means the write is invalid -- only that it raced another
//! writer -- so durable write paths retry with exponential backoff plus
//! jitter instead of failing hard.

use std::future::Future;
use std::time::Duration;

/// Backoff configuration for retrying transient SQLite busy errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusyRetryPolicy {
    /// Base delay before the first retry.
    pub initial_delay: Duration,
    /// Upper bound for a single backoff delay.
    pub max_delay: Duration,
    /// Total time budget spent sleeping between retries. Once cumulative
    /// sleep reaches this budget the next busy error is returned to the
    /// caller.
    pub total_delay_budget: Duration,
}

impl Default for BusyRetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(25),
            max_delay: Duration::from_secs(2),
            total_delay_budget: Duration::from_secs(15),
        }
    }
}

impl BusyRetryPolicy {
    fn next_base_delay(&self, current: Duration) -> Duration {
        current.saturating_mul(2).min(self.max_delay)
    }

    /// Applies "equal jitter": the returned delay lies in
    /// `[base / 2, base]`, so concurrent writers spread out instead of
    /// retrying in lockstep.
    fn jittered(base: Duration, fraction: f64) -> Duration {
        let half = base / 2;
        half + Duration::from_secs_f64(half.as_secs_f64() * fraction.clamp(0.0, 1.0))
    }
}

/// Returns true when any error in the chain looks like a transient SQLite
/// busy/locked condition (`SQLITE_BUSY` and its extended variants, including
/// `SQLITE_BUSY_SNAPSHOT`).
pub fn is_transient_busy_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("database is locked")
            || message.contains("database is busy")
            || message.contains("(code: 5)")
            || message.contains("(code: 261)")
            || message.contains("(code: 517)")
            || message.contains("(code: 773)")
    })
}

/// Retries `f` on transient SQLite busy errors using the default policy
/// (exponential backoff with jitter, ~15s total sleep budget).
pub async fn retry_on_busy<T, F, Fut>(operation: &str, f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    retry_on_busy_with_policy(BusyRetryPolicy::default(), operation, f).await
}

/// Retries `f` on transient SQLite busy errors using `policy`.
///
/// Non-busy errors are returned immediately. Busy errors are retried until
/// the cumulative sleep time reaches `policy.total_delay_budget`, after
/// which the final busy error is returned.
pub async fn retry_on_busy_with_policy<T, F, Fut>(
    policy: BusyRetryPolicy,
    operation: &str,
    mut f: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut base_delay = policy.initial_delay.min(policy.max_delay);
    let mut slept = Duration::ZERO;
    let mut attempt: u64 = 0;
    loop {
        attempt += 1;
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) if is_transient_busy_error(&err) && slept < policy.total_delay_budget => {
                let remaining = policy.total_delay_budget.saturating_sub(slept);
                let delay =
                    BusyRetryPolicy::jittered(base_delay, rand::random::<f64>()).min(remaining);
                tracing::debug!(
                    operation,
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    "retrying after transient SQLite busy: {err}"
                );
                tokio::time::sleep(delay).await;
                slept = slept.saturating_add(delay);
                base_delay = policy.next_base_delay(base_delay);
            }
            Err(err) => {
                if attempt > 1 {
                    tracing::warn!(
                        operation,
                        attempt,
                        slept_ms = slept.as_millis() as u64,
                        "giving up on transient SQLite busy retries: {err}"
                    );
                }
                return Err(err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::ConnectOptions;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::sqlite::SqliteJournalMode;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    fn busy_error() -> anyhow::Error {
        anyhow::anyhow!("error returned from database: (code: 5) database is locked")
    }

    fn busy_snapshot_error() -> anyhow::Error {
        anyhow::anyhow!("error returned from database: (code: 517) database is locked")
    }

    fn tiny_policy() -> BusyRetryPolicy {
        BusyRetryPolicy {
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(4),
            total_delay_budget: Duration::from_millis(60),
        }
    }

    #[test]
    fn default_policy_budget_is_at_least_15_seconds() {
        let policy = BusyRetryPolicy::default();
        assert!(policy.total_delay_budget >= Duration::from_secs(15));
        assert!(policy.initial_delay <= policy.max_delay);
    }

    #[test]
    fn jittered_delay_stays_within_half_to_full_base() {
        let base = Duration::from_millis(100);
        assert_eq!(
            BusyRetryPolicy::jittered(base, 0.0),
            Duration::from_millis(50)
        );
        assert_eq!(
            BusyRetryPolicy::jittered(base, 1.0),
            Duration::from_millis(100)
        );
        let mid = BusyRetryPolicy::jittered(base, 0.5);
        assert!(mid >= Duration::from_millis(50) && mid <= Duration::from_millis(100));
        // Out-of-range fractions are clamped rather than exceeding the base.
        assert_eq!(
            BusyRetryPolicy::jittered(base, 7.0),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn detects_transient_busy_errors_including_context_chains() {
        assert!(is_transient_busy_error(&busy_error()));
        assert!(is_transient_busy_error(&busy_snapshot_error()));
        assert!(is_transient_busy_error(&anyhow::anyhow!(
            "database is busy"
        )));
        // Context wrapping (as done by `?`-with-context call sites) must not
        // hide the underlying busy error.
        assert!(is_transient_busy_error(
            &busy_snapshot_error().context("failed to append background agent start event")
        ));
        assert!(!is_transient_busy_error(&anyhow::anyhow!(
            "no such table: background_agent_events"
        )));
        assert!(!is_transient_busy_error(&anyhow::anyhow!(
            "error returned from database: (code: 1) SQL logic error"
        )));
    }

    #[tokio::test]
    async fn succeeds_after_transient_busy_failures() {
        let attempts = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&attempts);
        let result = retry_on_busy_with_policy(tiny_policy(), "test op", move || {
            let counter = Arc::clone(&counter);
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) < 3 {
                    Err(busy_snapshot_error())
                } else {
                    Ok(42_u64)
                }
            }
        })
        .await;
        assert_eq!(result.expect("retry should eventually succeed"), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn non_busy_errors_fail_without_retrying() {
        let attempts = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&attempts);
        let result: anyhow::Result<()> =
            retry_on_busy_with_policy(tiny_policy(), "test op", move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(anyhow::anyhow!("no such table: threads"))
                }
            })
            .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn returns_busy_error_after_exhausting_delay_budget() {
        let attempts = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&attempts);
        let result: anyhow::Result<()> =
            retry_on_busy_with_policy(tiny_policy(), "test op", move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Err(busy_error())
                }
            })
            .await;
        let err = result.expect_err("budget exhaustion should surface the busy error");
        assert!(is_transient_busy_error(&err));
        // Budget of 60ms with 1..4ms delays must allow several retries but
        // still terminate.
        let observed = attempts.load(Ordering::SeqCst);
        assert!(observed > 3, "expected several attempts, got {observed}");
    }

    #[tokio::test]
    async fn retries_through_a_real_write_lock_held_by_another_connection() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!("codex-busy-retry-{}", uuid::Uuid::now_v7()));
        tokio::fs::create_dir_all(&dir).await?;
        let db_path = dir.join("busy.sqlite");
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            // Disable the driver-level busy handler so contention surfaces
            // immediately as SQLITE_BUSY and exercises our retry loop.
            .busy_timeout(Duration::ZERO);

        let mut holder = options.clone().connect().await?;
        sqlx::query("CREATE TABLE busy_probe (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .execute(&mut holder)
            .await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut holder).await?;
        sqlx::query("INSERT INTO busy_probe (v) VALUES ('held')")
            .execute(&mut holder)
            .await?;

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            sqlx::query("COMMIT")
                .execute(&mut holder)
                .await
                .expect("commit held transaction");
        });

        let policy = BusyRetryPolicy {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            total_delay_budget: Duration::from_secs(10),
        };
        retry_on_busy_with_policy(policy, "insert under contention", || async {
            sqlx::query("INSERT INTO busy_probe (v) VALUES ('retried')")
                .execute(&writer)
                .await
                .map_err(anyhow::Error::from)?;
            Ok(())
        })
        .await
        .expect("write should succeed once the lock is released");

        release.await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM busy_probe")
            .fetch_one(&writer)
            .await?;
        assert_eq!(count, 2);
        writer.close().await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        Ok(())
    }
}
