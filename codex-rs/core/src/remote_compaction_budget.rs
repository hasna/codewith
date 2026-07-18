use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use codex_api::RequestTelemetry;
use codex_api::TransportError;
use http::StatusCode;

/// Maximum actual model-provider requests issued by one remote compaction operation.
pub(crate) const MAX_REMOTE_COMPACTION_REQUEST_ATTEMPTS: usize = 4;

/// A request-attempt budget shared by every retry layer in one remote compaction operation.
#[derive(Debug, Clone)]
pub(crate) struct RemoteCompactionRequestBudget {
    remaining: Arc<AtomicUsize>,
}

impl RemoteCompactionRequestBudget {
    pub(crate) fn new() -> Self {
        Self {
            remaining: Arc::new(AtomicUsize::new(MAX_REMOTE_COMPACTION_REQUEST_ATTEMPTS)),
        }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.remaining.load(Ordering::Relaxed)
    }

    pub(crate) fn try_start_request(&self) -> bool {
        self.remaining
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }

    pub(crate) fn max_retries_for_next_request(&self, configured_max_retries: u64) -> u64 {
        let available_retries = self.remaining().saturating_sub(1);
        configured_max_retries.min(u64::try_from(available_retries).unwrap_or(u64::MAX))
    }

    /// Wraps unary-request telemetry so each completed transport attempt spends this budget.
    pub(crate) fn counting_telemetry(
        &self,
        inner: Arc<dyn RequestTelemetry>,
    ) -> Arc<dyn RequestTelemetry> {
        Arc::new(RemoteCompactionRequestTelemetry {
            budget: self.clone(),
            inner,
        })
    }
}

struct RemoteCompactionRequestTelemetry {
    budget: RemoteCompactionRequestBudget,
    inner: Arc<dyn RequestTelemetry>,
}

impl RequestTelemetry for RemoteCompactionRequestTelemetry {
    fn on_request(
        &self,
        attempt: u64,
        status: Option<StatusCode>,
        error: Option<&TransportError>,
        duration: Duration,
    ) {
        let consumed = self.budget.try_start_request();
        debug_assert!(consumed, "remote compaction request exceeded its budget");
        self.inner.on_request(attempt, status, error, duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn request_budget_is_shared_across_clones_and_never_underflows() {
        let budget = RemoteCompactionRequestBudget::new();
        let clone = budget.clone();

        assert_eq!(budget.remaining(), 4);
        assert!(budget.try_start_request());
        assert!(clone.try_start_request());
        assert_eq!(budget.remaining(), 2);
        assert!(clone.try_start_request());
        assert!(budget.try_start_request());
        assert!(!clone.try_start_request());
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn configured_retries_are_capped_by_remaining_actual_requests() {
        let budget = RemoteCompactionRequestBudget::new();

        assert_eq!(
            budget.max_retries_for_next_request(/*configured_max_retries*/ 10),
            3
        );
        assert!(budget.try_start_request());
        assert!(budget.try_start_request());
        assert_eq!(
            budget.max_retries_for_next_request(/*configured_max_retries*/ 10),
            1
        );
        assert_eq!(
            budget.max_retries_for_next_request(/*configured_max_retries*/ 0),
            0
        );
    }
}
