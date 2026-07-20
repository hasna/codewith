use super::*;
use crate::active_session_bridge::ActiveChannelDeliveryMode;
use crate::active_session_bridge::ActiveChannelDeliveryOutcome;
use crate::active_session_bridge::ActiveChannelEndpoint;
use crate::active_session_bridge::ActiveChannelEndpointKind;
use crate::active_session_bridge::ActiveChannelEnvelope;
use crate::active_session_bridge::ActiveChannelRouter;
use crate::active_session_bridge::active_channel_communication;
use crate::active_session_registry::ActivePeerDirectory;
use crate::active_session_registry::ActivePeerFreshness;
use crate::active_session_registry::ActivePeerLookupError;
use crate::active_session_registry::LastSeenAt;
use crate::request_processors::thread_lifecycle::ListenerTaskContext;

use super::thread_mailbox_context::mailbox_context_descriptor_component;
use super::thread_mailbox_context::mailbox_payload_context_text;

const MAILBOX_DISPATCH_POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAILBOX_DISPATCH_LEASE_DURATION: Duration = Duration::from_secs(60);
const MAILBOX_DISPATCH_RETRY_DELAY_SECONDS: i64 = 30;
const MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS: usize = 10;
const MAILBOX_DISPATCH_DURABLE_WRITE_RETRY_DELAY: Duration = Duration::from_millis(100);
// Liveness window for treating another owner's local active session as still
// serving a target. A tight window (previously 5s) let a brief GC pause, sleep,
// or debugger stall on a live owner make its target look claimable, so a second
// default-on dispatcher could cold-resume and duplicate work against the same
// rollout. Heartbeats refresh roughly once per second, so 30s tolerates short
// stalls while staying well under the lease (60s) and retention (300s) windows.
const MAILBOX_LOCAL_ACTIVE_SESSION_STALE_AFTER: Duration = Duration::from_secs(30);
const MAILBOX_LOCAL_ACTIVE_SESSION_RETENTION: Duration = Duration::from_secs(300);
const MAX_MAILBOX_DISPATCH_CLAIMS_PER_TICK: usize = 16;
const MAILBOX_DISPATCH_LEASE_OWNER: &str = "app-server-local-mailbox-dispatcher";
const MAX_MAILBOX_DISPATCH_ERROR_CHARS: usize = 1_000;
const MAILBOX_PENDING_WORK_WAKE_ATTEMPTS: usize = 40;
const MAILBOX_PENDING_WORK_WAKE_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub(crate) struct ThreadMailboxDispatcherRuntime {
    active_peer_directory: ActivePeerDirectory,
    active_channel_router: ActiveChannelRouter,
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
    thread_state_manager: ThreadStateManager,
    pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
    thread_watch_manager: ThreadWatchManager,
    thread_list_state_permit: Arc<Semaphore>,
    skills_watcher: Arc<SkillsWatcher>,
    state_db: Option<StateDbHandle>,
    local_active_owner_id: String,
    cancel_token: CancellationToken,
    tasks: TaskTracker,
}

impl ThreadMailboxDispatcherRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
        thread_state_manager: ThreadStateManager,
        pending_thread_unloads: Arc<Mutex<HashSet<ThreadId>>>,
        thread_watch_manager: ThreadWatchManager,
        thread_list_state_permit: Arc<Semaphore>,
        skills_watcher: Arc<SkillsWatcher>,
        state_db: Option<StateDbHandle>,
        local_active_owner_id: String,
    ) -> Self {
        Self {
            active_peer_directory: ActivePeerDirectory::new(
                Arc::clone(&thread_manager),
                Arc::clone(&pending_thread_unloads),
            ),
            active_channel_router: ActiveChannelRouter::new(
                Arc::clone(&thread_manager),
                Arc::clone(&pending_thread_unloads),
            ),
            auth_manager,
            thread_manager,
            outgoing,
            config,
            config_manager,
            thread_state_manager,
            pending_thread_unloads,
            thread_watch_manager,
            thread_list_state_permit,
            skills_watcher,
            state_db,
            local_active_owner_id,
            cancel_token: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }

    pub(crate) fn start(&self) {
        if self.state_db.is_none() {
            return;
        }
        let runtime = self.clone();
        self.tasks.spawn(async move {
            runtime.run().await;
        });
    }

    pub(crate) fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    pub(crate) async fn drain_background_tasks(&self) {
        self.shutdown();
        self.tasks.close();
        if tokio::time::timeout(Duration::from_secs(10), self.tasks.wait())
            .await
            .is_err()
        {
            warn!(
                "timed out waiting for thread mailbox dispatcher runtime to shut down; proceeding"
            );
        }
    }

    async fn run(self) {
        let mut interval = tokio::time::interval(MAILBOX_DISPATCH_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => break,
                _ = interval.tick() => self.tick().await,
            }
        }
    }

    async fn tick(&self) {
        let Some(state_db) = self.state_db.as_ref() else {
            return;
        };
        let now = Utc::now();
        if let Err(err) = self.refresh_local_active_sessions(state_db, now).await {
            warn!("failed to refresh local mailbox active sessions: {err}");
        }
        if !self.config.features.enabled(Feature::MailboxDispatcher) {
            return;
        }
        let local_active_fresh_after = mailbox_local_active_fresh_after(now);
        for _ in 0..MAX_MAILBOX_DISPATCH_CLAIMS_PER_TICK {
            let claim = match state_db
                .mailbox_messages()
                .claim_next_due_message(codex_state::MailboxDispatchClaimParams {
                    lease_owner: MAILBOX_DISPATCH_LEASE_OWNER.to_string(),
                    lease_duration: MAILBOX_DISPATCH_LEASE_DURATION,
                    now: Utc::now(),
                    local_active_owner_id: self.local_active_owner_id.clone(),
                    local_active_fresh_after,
                })
                .await
            {
                Ok(Some(claim)) => claim,
                Ok(None) => break,
                Err(err) => {
                    warn!("failed to claim due mailbox message: {err}");
                    break;
                }
            };
            self.dispatch_claim(state_db, claim).await;
        }
    }

    /// Refreshes this process's local active-session heartbeats and returns the
    /// set of thread ids whose heartbeat was durably written this pass.
    ///
    /// Heartbeats are refreshed per peer so one bad peer (for example a loaded
    /// peer missing its `threads` FK row) cannot hide this process's other live
    /// sessions or, in `dispatch_claim`, block releasing an unrelated target's
    /// dispatch lease. Pruning is best-effort for the same reason: a transient
    /// prune failure keeps stale rows a little longer (the safe direction) rather
    /// than aborting the whole refresh.
    async fn refresh_local_active_sessions(
        &self,
        state_db: &StateDbHandle,
        now: DateTime<Utc>,
    ) -> anyhow::Result<HashSet<ThreadId>> {
        let now_seen = LastSeenAt::from_unix_seconds(now.timestamp());
        let registry = self.active_peer_directory.snapshot(now_seen).await?;
        let freshness = ActivePeerFreshness::new(now_seen, Duration::from_secs(0));
        let mut active_thread_ids = Vec::new();
        let mut heartbeated = HashSet::new();
        for peer in registry.list_active(freshness) {
            // Keep advertising every active peer even when a single heartbeat
            // write fails, so prune below does not drop a still-active session.
            active_thread_ids.push(peer.thread_id);
            match state_db
                .local_active_sessions()
                .heartbeat_session(codex_state::LocalActiveSessionHeartbeatParams {
                    thread_id: peer.thread_id,
                    owner_id: self.local_active_owner_id.clone(),
                    session_id: peer.session_id,
                    pid: Some(std::process::id()),
                    now,
                })
                .await
            {
                Ok(_) => {
                    heartbeated.insert(peer.thread_id);
                }
                Err(err) => {
                    warn!(
                        thread_id = %peer.thread_id,
                        "failed to heartbeat local mailbox active session; continuing with other peers: {err}"
                    );
                }
            }
        }
        if let Err(err) = state_db
            .local_active_sessions()
            .prune_owner_sessions(codex_state::LocalActiveSessionPruneOwnerParams {
                owner_id: self.local_active_owner_id.clone(),
                active_thread_ids,
                observed_at: now,
            })
            .await
        {
            warn!("failed to prune owner mailbox active sessions: {err}");
        }
        if let Err(err) = state_db
            .local_active_sessions()
            .prune_stale_sessions(mailbox_local_active_retention_cutoff(now))
            .await
        {
            warn!("failed to prune stale mailbox active sessions: {err}");
        }
        Ok(heartbeated)
    }

    async fn dispatch_claim(&self, state_db: &StateDbHandle, claim: codex_state::MailboxClaim) {
        let target_thread_id = claim.message.target_thread_id;
        let message_id = claim.message.message_id.clone();
        let lease_id = claim.attempt.lease_id.clone();
        let result = self.deliver_claim(&claim).await;
        let mut wake_thread_id = None;
        let durable_transition_ok = match result {
            MailboxDispatchResult::Delivered {
                receipt,
                wake_thread_id: wake,
            } => {
                wake_thread_id = wake;
                self.ack_dispatch_claim(state_db, &claim, receipt).await
            }
            MailboxDispatchResult::Retry { error, retry_at } => {
                let retry_at = retry_at.unwrap_or_else(|| {
                    Utc::now() + ChronoDuration::seconds(MAILBOX_DISPATCH_RETRY_DELAY_SECONDS)
                });
                self.fail_dispatch_claim_for_retry(state_db, &claim, error, retry_at)
                    .await
            }
            MailboxDispatchResult::Terminal { error } => {
                self.fail_dispatch_claim_terminal(state_db, &claim, error)
                    .await
            }
        };
        if !durable_transition_ok {
            warn!(
                message_id = %message_id,
                "leaving mailbox target dispatch lease to expire because durable transition did not complete"
            );
            return;
        }
        // The durable claim is acked, so it is finally safe to wake the target and
        // let it drain the enqueued mailbox item into model context. Had the process
        // crashed before this point, the row would still be claimable and simply
        // redelivered, without the target ever consuming a duplicate.
        if let Some(thread_id) = wake_thread_id {
            self.spawn_pending_work_wake(thread_id);
        }
        // Refresh is best-effort: a per-peer heartbeat failure elsewhere must not
        // keep this target's dispatch lease alive. Release the lease once this
        // process has advertised the target as a local active session, so future
        // dispatch is handed off promptly instead of waiting out the lease.
        let heartbeated = self
            .refresh_local_active_sessions(state_db, Utc::now())
            .await
            .unwrap_or_else(|err| {
                warn!(
                    message_id = %message_id,
                    "failed to refresh local active sessions after mailbox dispatch: {err}"
                );
                HashSet::new()
            });
        if heartbeated.contains(&target_thread_id) {
            if let Err(err) = state_db
                .mailbox_messages()
                .release_dispatch_target_lease(
                    target_thread_id,
                    self.local_active_owner_id.as_str(),
                    lease_id.as_str(),
                )
                .await
            {
                warn!(
                    message_id = %message_id,
                    "failed to release mailbox target dispatch lease: {err}"
                );
            }
        } else {
            warn!(
                message_id = %message_id,
                "leaving mailbox target dispatch lease to expire because the target's local active session was not advertised"
            );
        }
    }

    async fn ack_dispatch_claim(
        &self,
        state_db: &StateDbHandle,
        claim: &codex_state::MailboxClaim,
        receipt: serde_json::Value,
    ) -> bool {
        let mut last_error = None;
        for attempt in 1..=MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS {
            match state_db
                .mailbox_messages()
                .ack_message(codex_state::MailboxAckParams {
                    message_id: claim.message.message_id.clone(),
                    attempt_id: claim.attempt.attempt_id.clone(),
                    lease_id: claim.attempt.lease_id.clone(),
                    receipt_payload_json: Some(receipt.clone()),
                    now: Utc::now(),
                })
                .await
            {
                Ok(Some(_)) => return true,
                Ok(None) => {
                    warn!(
                        message_id = %claim.message.message_id,
                        "failed to ack delivered mailbox message: lease no longer matches"
                    );
                    return false;
                }
                Err(err) => {
                    last_error = Some(err.to_string());
                    if attempt < MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS {
                        tokio::time::sleep(MAILBOX_DISPATCH_DURABLE_WRITE_RETRY_DELAY).await;
                    }
                }
            }
        }
        warn!(
            message_id = %claim.message.message_id,
            attempts = MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS,
            "failed to ack delivered mailbox message after retries: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        );
        false
    }

    async fn fail_dispatch_claim_for_retry(
        &self,
        state_db: &StateDbHandle,
        claim: &codex_state::MailboxClaim,
        error: String,
        retry_at: DateTime<Utc>,
    ) -> bool {
        self.fail_dispatch_claim(
            state_db,
            claim,
            error,
            |next_attempt_at| codex_state::MailboxFailDisposition::Retry { next_attempt_at },
            Some(retry_at),
            "failed to requeue mailbox message after dispatch miss",
        )
        .await
    }

    async fn fail_dispatch_claim_terminal(
        &self,
        state_db: &StateDbHandle,
        claim: &codex_state::MailboxClaim,
        error: String,
    ) -> bool {
        self.fail_dispatch_claim(
            state_db,
            claim,
            error,
            |_| codex_state::MailboxFailDisposition::Terminal,
            /*retry_at*/ None,
            "failed to mark mailbox message terminal after dispatch miss",
        )
        .await
    }

    async fn fail_dispatch_claim(
        &self,
        state_db: &StateDbHandle,
        claim: &codex_state::MailboxClaim,
        error: String,
        disposition: impl Fn(DateTime<Utc>) -> codex_state::MailboxFailDisposition,
        retry_at: Option<DateTime<Utc>>,
        context: &'static str,
    ) -> bool {
        let mut last_error = None;
        for attempt in 1..=MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS {
            match state_db
                .mailbox_messages()
                .fail_message(codex_state::MailboxFailParams {
                    message_id: claim.message.message_id.clone(),
                    attempt_id: claim.attempt.attempt_id.clone(),
                    lease_id: claim.attempt.lease_id.clone(),
                    error: error.clone(),
                    disposition: disposition(retry_at.unwrap_or_else(Utc::now)),
                    now: Utc::now(),
                })
                .await
            {
                Ok(Some(_)) => return true,
                Ok(None) => {
                    warn!(
                        message_id = %claim.message.message_id,
                        "{context}: lease no longer matches"
                    );
                    return false;
                }
                Err(err) => {
                    last_error = Some(err.to_string());
                    if attempt < MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS {
                        tokio::time::sleep(MAILBOX_DISPATCH_DURABLE_WRITE_RETRY_DELAY).await;
                    }
                }
            }
        }
        warn!(
            message_id = %claim.message.message_id,
            attempts = MAILBOX_DISPATCH_DURABLE_WRITE_ATTEMPTS,
            "{context} after retries: {}",
            last_error.unwrap_or_else(|| "unknown error".to_string())
        );
        false
    }

    async fn deliver_claim(&self, claim: &codex_state::MailboxClaim) -> MailboxDispatchResult {
        let policy = mailbox_local_delivery_policy(&claim.message);
        let freshness = dispatcher_freshness_now();
        let registry = match self.active_peer_directory.snapshot(freshness.now).await {
            Ok(registry) => registry,
            Err(err) => {
                return MailboxDispatchResult::retry(format!(
                    "failed to read active session directory: {err}"
                ));
            }
        };
        let target_peer_id = claim.message.target_thread_id.to_string();
        let mut resumed_target = false;
        let (registry, target_peer) = match registry.get_active(target_peer_id.as_str(), freshness)
        {
            Ok(peer) => (registry, peer),
            Err(err) => match policy {
                MailboxLocalDeliveryPolicy::LiveOnly | MailboxLocalDeliveryPolicy::QueueOnly => {
                    return mailbox_target_not_loaded(err);
                }
                MailboxLocalDeliveryPolicy::ResumeAndTrigger => {
                    if let Err(err) = self
                        .resume_mailbox_target(claim.message.target_thread_id)
                        .await
                    {
                        return err.into_dispatch_result();
                    }
                    resumed_target = true;
                    let registry = match self.active_peer_directory.snapshot(freshness.now).await {
                        Ok(registry) => registry,
                        Err(err) => {
                            return MailboxDispatchResult::retry(format!(
                                "failed to read active session directory after resume: {err}"
                            ));
                        }
                    };
                    let target_peer = match registry.get_active(target_peer_id.as_str(), freshness)
                    {
                        Ok(peer) => peer,
                        Err(err) => return mailbox_target_not_loaded(err),
                    };
                    (registry, target_peer)
                }
            },
        };
        let sender_peer = claim
            .message
            .sender_thread_id
            .map(|thread_id| thread_id.to_string())
            .and_then(|peer_id| registry.get_active(peer_id.as_str(), freshness).ok());
        let delivery = mailbox_delivery_mode(claim.message.kind, policy);
        let envelope = mailbox_active_channel_envelope(
            &claim.message,
            sender_peer.as_ref(),
            &target_peer,
            delivery,
        );
        let communication = active_channel_communication(&envelope);
        let delivery_outcome = self
            .active_channel_router
            .enqueue_for_pending_work(&envelope, &target_peer, communication)
            .await;
        match delivery_outcome {
            // Defer the pending-work wake until dispatch_claim has durably acked
            // this claim. Waking here (before the ack) risked the target draining
            // the item into model context while the durable row was still
            // claimable, so a crash or ack failure could redeliver it.
            Ok(ActiveChannelDeliveryOutcome::Delivered { .. }) => {
                MailboxDispatchResult::Delivered {
                    wake_thread_id: mailbox_dispatch_wake_target(delivery, target_peer.thread_id),
                    receipt: serde_json::json!({
                        "delivery": if resumed_target { "resumed" } else { "live" },
                        "recipientPeerId": target_peer.peer_id,
                        "recipientThreadId": target_peer.thread_id.to_string(),
                        "deliveryMode": mailbox_delivery_mode_name(delivery),
                        "triggerTurn": delivery.trigger_turn(),
                    }),
                }
            }
            Ok(ActiveChannelDeliveryOutcome::NotLoaded { .. }) => MailboxDispatchResult::retry(
                "target thread became unloaded during mailbox dispatch",
            ),
            Ok(ActiveChannelDeliveryOutcome::Unsupported { .. }) => MailboxDispatchResult::retry(
                "target peer is active but does not support local mailbox dispatch",
            ),
            Err(err) => MailboxDispatchResult::retry(format!("mailbox dispatch failed: {err}")),
        }
    }

    fn spawn_pending_work_wake(&self, thread_id: ThreadId) {
        let thread_manager = Arc::clone(&self.thread_manager);
        let cancel_token = self.cancel_token.clone();
        self.tasks.spawn(async move {
            for _ in 0..MAILBOX_PENDING_WORK_WAKE_ATTEMPTS {
                tokio::select! {
                    _ = cancel_token.cancelled() => return,
                    _ = tokio::time::sleep(MAILBOX_PENDING_WORK_WAKE_INTERVAL) => {}
                }
                match thread_manager.get_thread(thread_id).await {
                    Ok(thread) => {
                        if thread.maybe_start_turn_for_pending_work().await {
                            return;
                        }
                    }
                    Err(CodexErr::ThreadNotFound(_)) => return,
                    Err(err) => {
                        warn!("failed to wake mailbox pending work for thread {thread_id}: {err}");
                        return;
                    }
                }
            }
        });
    }

    async fn resume_mailbox_target(&self, thread_id: ThreadId) -> Result<(), MailboxResumeError> {
        match self.thread_manager.get_thread(thread_id).await {
            Ok(thread) => return self.ensure_mailbox_listener(thread_id, thread).await,
            Err(CodexErr::ThreadNotFound(_)) => {}
            Err(err) => {
                return Err(MailboxResumeError::Failed(format!(
                    "failed to inspect loaded mailbox target: {err}"
                )));
            }
        }
        let Some(state_db) = self.state_db.as_ref() else {
            return Err(MailboxResumeError::Failed(
                "sqlite state db unavailable for mailbox resume".to_string(),
            ));
        };
        let rollout_path = codex_rollout::find_thread_path_by_id_str(
            &self.config.codex_home,
            &thread_id.to_string(),
            Some(state_db),
        )
        .await
        .map_err(|err| {
            MailboxResumeError::Failed(format!("failed to resolve mailbox resume target: {err}"))
        })?
        .ok_or_else(|| {
            MailboxResumeError::NotResolvable(format!(
                "mailbox target thread is not resumable: {thread_id}"
            ))
        })?;
        let initial_history = codex_rollout::RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .map_err(|err| {
                MailboxResumeError::Failed(format!(
                    "failed to load rollout {} for mailbox resume: {err}",
                    rollout_path.display()
                ))
            })?;
        let history_cwd = initial_history.session_cwd();
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides::default();
        apply_persisted_mailbox_resume_metadata(
            state_db,
            thread_id,
            &initial_history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;
        let broker_decision = super::usage_profile_broker::resolve_dispatch_auth_profile(
            &self.auth_manager,
            &self.config,
            typesafe_overrides.auth_profile.clone(),
        )
        .await;
        if let Some(profile) = broker_decision.selected_profile.as_ref() {
            tracing::debug!(
                thread_id = %thread_id,
                auth_profile = %profile,
                reason = ?broker_decision.reason,
                "usage profile broker selected auth profile for mailbox resume"
            );
            typesafe_overrides.auth_profile = Some(Some(profile.clone()));
        } else if let Some(retry_at) = broker_decision.retry_at
            && let Some(retry_at) = broker_retry_at_datetime(&self.config, retry_at)
        {
            tracing::debug!(
                thread_id = %thread_id,
                retry_at = %retry_at.to_rfc3339(),
                reason = ?broker_decision.reason,
                "usage profile broker deferred mailbox resume"
            );
            return Err(MailboxResumeError::UsageProfileWait {
                retry_at,
                error: format!(
                    "all eligible auth profiles are exhausted; retrying mailbox resume after {}",
                    retry_at.to_rfc3339()
                ),
            });
        }
        let config = self
            .config_manager
            .load_for_cwd(request_overrides, typesafe_overrides, history_cwd)
            .await
            .map_err(|err| {
                MailboxResumeError::Failed(format!(
                    "failed to load config for mailbox resume: {err}"
                ))
            })?;
        let thread = self
            .thread_manager
            .resume_thread_with_history(
                config,
                initial_history,
                Arc::clone(&self.auth_manager),
                /*parent_trace*/ None,
            )
            .await
            .map(|new_thread| new_thread.thread)
            .map_err(|err| {
                MailboxResumeError::Failed(format!("failed to resume mailbox target: {err}"))
            })?;
        self.ensure_mailbox_listener(thread_id, thread).await
    }

    async fn ensure_mailbox_listener(
        &self,
        thread_id: ThreadId,
        thread: Arc<CodexThread>,
    ) -> Result<(), MailboxResumeError> {
        let thread_state = self.thread_state_manager.thread_state(thread_id).await;
        let context = ListenerTaskContext {
            thread_manager: Arc::clone(&self.thread_manager),
            thread_state_manager: self.thread_state_manager.clone(),
            outgoing: Arc::clone(&self.outgoing),
            pending_thread_unloads: Arc::clone(&self.pending_thread_unloads),
            thread_watch_manager: self.thread_watch_manager.clone(),
            thread_list_state_permit: Arc::clone(&self.thread_list_state_permit),
            fallback_model_provider: self.config.model_provider_id.clone(),
            codex_home: self.config.codex_home.to_path_buf(),
            skills_watcher: Arc::clone(&self.skills_watcher),
            state_db: self.state_db.clone(),
            local_active_owner_id: self.local_active_owner_id.clone(),
        };
        super::thread_lifecycle::ensure_listener_task_running(
            context,
            thread_id,
            thread,
            thread_state,
        )
        .await
        .map_err(|err| MailboxResumeError::Failed(err.message))
    }
}

enum MailboxDispatchResult {
    Delivered {
        receipt: serde_json::Value,
        /// Thread to wake for pending work once the claim is durably acked, or
        /// `None` for queue-only deliveries that must not trigger a turn.
        wake_thread_id: Option<ThreadId>,
    },
    Retry {
        error: String,
        retry_at: Option<DateTime<Utc>>,
    },
    Terminal {
        error: String,
    },
}

impl MailboxDispatchResult {
    fn retry(error: impl Into<String>) -> Self {
        Self::Retry {
            error: truncate_mailbox_dispatch_error(error.into()),
            retry_at: None,
        }
    }

    fn retry_at(error: impl Into<String>, retry_at: DateTime<Utc>) -> Self {
        Self::Retry {
            error: truncate_mailbox_dispatch_error(error.into()),
            retry_at: Some(retry_at),
        }
    }

    fn terminal(error: impl Into<String>) -> Self {
        Self::Terminal {
            error: truncate_mailbox_dispatch_error(error.into()),
        }
    }
}

enum MailboxResumeError {
    NotResolvable(String),
    Failed(String),
    UsageProfileWait {
        retry_at: DateTime<Utc>,
        error: String,
    },
}

impl MailboxResumeError {
    fn into_dispatch_result(self) -> MailboxDispatchResult {
        match self {
            Self::NotResolvable(error) => MailboxDispatchResult::terminal(error),
            Self::Failed(error) => MailboxDispatchResult::retry(error),
            Self::UsageProfileWait { retry_at, error } => {
                MailboxDispatchResult::retry_at(error, retry_at)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MailboxLocalDeliveryPolicy {
    LiveOnly,
    QueueOnly,
    ResumeAndTrigger,
}

fn dispatcher_freshness_now() -> ActivePeerFreshness {
    let now = LastSeenAt::from_unix_seconds(time::OffsetDateTime::now_utc().unix_timestamp());
    ActivePeerFreshness::new(now, Duration::from_secs(0))
}

fn mailbox_local_active_fresh_after(now: DateTime<Utc>) -> DateTime<Utc> {
    now - ChronoDuration::seconds(duration_seconds_i64(
        MAILBOX_LOCAL_ACTIVE_SESSION_STALE_AFTER,
    ))
}

fn mailbox_local_active_retention_cutoff(now: DateTime<Utc>) -> DateTime<Utc> {
    now - ChronoDuration::seconds(duration_seconds_i64(MAILBOX_LOCAL_ACTIVE_SESSION_RETENTION))
}

fn duration_seconds_i64(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

fn mailbox_target_not_loaded(error: ActivePeerLookupError) -> MailboxDispatchResult {
    match error {
        ActivePeerLookupError::Unknown { .. } => MailboxDispatchResult::retry(
            "target thread is not currently loaded for mailbox dispatch",
        ),
        ActivePeerLookupError::Inactive { .. } => {
            MailboxDispatchResult::retry("target thread is inactive for mailbox dispatch")
        }
    }
}

fn mailbox_delivery_mode(
    kind: codex_state::MailboxMessageKind,
    policy: MailboxLocalDeliveryPolicy,
) -> ActiveChannelDeliveryMode {
    match policy {
        MailboxLocalDeliveryPolicy::QueueOnly => ActiveChannelDeliveryMode::QueueOnly,
        MailboxLocalDeliveryPolicy::ResumeAndTrigger => ActiveChannelDeliveryMode::TriggerTurn,
        MailboxLocalDeliveryPolicy::LiveOnly => match kind {
            codex_state::MailboxMessageKind::UserInstruction
            | codex_state::MailboxMessageKind::UserReply => ActiveChannelDeliveryMode::TriggerTurn,
            codex_state::MailboxMessageKind::Control => ActiveChannelDeliveryMode::QueueOnly,
        },
    }
}

fn mailbox_local_delivery_policy(
    message: &codex_state::MailboxMessage,
) -> MailboxLocalDeliveryPolicy {
    mailbox_local_delivery_policy_from_payload(&message.payload_json)
}

fn mailbox_local_delivery_policy_from_payload(
    payload: &serde_json::Value,
) -> MailboxLocalDeliveryPolicy {
    // Mirror the SQL claim filter's OR-any-recognized acceptance (see the
    // Dispatch-scope claim query in codex-state's mailbox runtime): take the
    // first RECOGNIZED marker across the four locations instead of
    // short-circuiting on the first present key. Otherwise a message the
    // dispatcher claimed via a secondary marker (e.g. an unrecognized
    // "delivery" value plus a resumeAndTrigger "dispatch.mode") would be
    // misparsed as LiveOnly and burn its delivery attempts.
    [
        payload.get("delivery"),
        payload.get("deliveryMode"),
        payload.get("localDelivery"),
        payload.pointer("/dispatch/mode"),
    ]
    .into_iter()
    .flatten()
    .filter_map(serde_json::Value::as_str)
    .find_map(|mode| match mode {
        "resumeAndTrigger" | "resume_and_trigger" => {
            Some(MailboxLocalDeliveryPolicy::ResumeAndTrigger)
        }
        "queueOnly" | "queue_only" => Some(MailboxLocalDeliveryPolicy::QueueOnly),
        "liveOnly" | "live_only" => Some(MailboxLocalDeliveryPolicy::LiveOnly),
        _ => None,
    })
    .unwrap_or(MailboxLocalDeliveryPolicy::LiveOnly)
}

/// Thread to wake after a delivered claim is durably acked. Queue-only
/// deliveries never trigger a turn, so they schedule no post-ack wake.
fn mailbox_dispatch_wake_target(
    delivery: ActiveChannelDeliveryMode,
    thread_id: ThreadId,
) -> Option<ThreadId> {
    delivery.trigger_turn().then_some(thread_id)
}

fn mailbox_delivery_mode_name(mode: ActiveChannelDeliveryMode) -> &'static str {
    match mode {
        ActiveChannelDeliveryMode::QueueOnly => "queue_only",
        ActiveChannelDeliveryMode::TriggerTurn => "trigger_turn",
    }
}

fn mailbox_active_channel_envelope(
    message: &codex_state::MailboxMessage,
    sender_peer: Option<&crate::active_session_registry::ActivePeer>,
    target_peer: &crate::active_session_registry::ActivePeer,
    delivery: ActiveChannelDeliveryMode,
) -> ActiveChannelEnvelope {
    let sender = durable_mailbox_sender_endpoint();
    let claimed_sender = sender_peer.map(ActiveChannelEndpoint::from_peer);
    ActiveChannelEnvelope::new(
        message.message_id.clone(),
        sender,
        claimed_sender,
        ActiveChannelEndpoint::from_peer(target_peer),
        format!(
            "Durable mailbox message {} from {}:\n\n{}",
            message.message_id,
            mailbox_sender_descriptor(message),
            mailbox_payload_context_text(&message.payload_json)
        ),
        delivery,
    )
}

fn durable_mailbox_sender_endpoint() -> ActiveChannelEndpoint {
    ActiveChannelEndpoint {
        id: "durable-mailbox".to_string(),
        kind: ActiveChannelEndpointKind::BridgeAdapter,
        label: Some("durable mailbox".to_string()),
        agent_path: None,
    }
}

fn mailbox_sender_descriptor(message: &codex_state::MailboxMessage) -> String {
    match (&message.sender_thread_id, &message.sender_label) {
        (Some(sender_thread_id), Some(sender_label)) => {
            let sender_label = mailbox_context_descriptor_component(sender_label);
            format!("unverified sender thread {sender_thread_id} with label {sender_label:?}")
        }
        (Some(sender_thread_id), None) => format!("unverified sender thread {sender_thread_id}"),
        (None, Some(sender_label)) => {
            let sender_label = mailbox_context_descriptor_component(sender_label);
            format!("external sender {sender_label:?}")
        }
        (None, None) => "external sender".to_string(),
    }
}

fn truncate_mailbox_dispatch_error(error: String) -> String {
    if error.chars().count() <= MAX_MAILBOX_DISPATCH_ERROR_CHARS {
        return error;
    }
    error
        .chars()
        .take(MAX_MAILBOX_DISPATCH_ERROR_CHARS.saturating_sub(3))
        .chain("...".chars())
        .collect()
}

fn broker_retry_at_datetime(config: &Config, retry_at: i64) -> Option<DateTime<Utc>> {
    let retry_at = DateTime::<Utc>::from_timestamp(retry_at, /*nsecs*/ 0)?;
    let buffer_secs = i64::try_from(config.usage_self_heal.reset_retry_buffer_secs).ok()?;
    let retry_at = retry_at + ChronoDuration::seconds(buffer_secs);
    (retry_at > Utc::now()).then_some(retry_at)
}

async fn apply_persisted_mailbox_resume_metadata(
    state_db: &StateDbHandle,
    thread_id: ThreadId,
    initial_history: &InitialHistory,
    request_overrides: &mut Option<HashMap<String, serde_json::Value>>,
    typesafe_overrides: &mut ConfigOverrides,
) {
    super::thread_processor::merge_persisted_auth_profile_from_history(
        typesafe_overrides,
        initial_history,
    );

    let persisted_metadata = match state_db.get_thread(thread_id).await {
        Ok(Some(metadata)) => metadata,
        Ok(None) => return,
        Err(err) => {
            warn!("failed to read persisted metadata for mailbox resume thread {thread_id}: {err}");
            return;
        }
    };
    let permission_cwd = persisted_permission_profile_cwd(&persisted_metadata, initial_history);
    typesafe_overrides.model = persisted_metadata.model;
    typesafe_overrides.model_provider = Some(persisted_metadata.model_provider);
    if let Some(reasoning_effort) = persisted_metadata.reasoning_effort {
        request_overrides.get_or_insert_with(HashMap::new).insert(
            "model_reasoning_effort".to_string(),
            serde_json::Value::String(reasoning_effort.to_string()),
        );
    }
    if let Some(permission_profile) =
        parse_persisted_permission_profile(&persisted_metadata.sandbox_policy, &permission_cwd)
    {
        typesafe_overrides.permission_profile = Some(permission_profile);
    }
    if let Some(approval_policy) = parse_persisted_approval_mode(&persisted_metadata.approval_mode)
    {
        typesafe_overrides.approval_policy = Some(approval_policy);
    }
}

fn persisted_permission_profile_cwd(
    metadata: &codex_state::ThreadMetadata,
    initial_history: &InitialHistory,
) -> PathBuf {
    initial_history.session_cwd().unwrap_or_else(|| {
        if metadata.cwd.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            metadata.cwd.clone()
        }
    })
}

fn parse_persisted_permission_profile(
    stored: &str,
    cwd: &Path,
) -> Option<codex_protocol::models::PermissionProfile> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(permission_profile) =
        serde_json::from_str::<codex_protocol::models::PermissionProfile>(trimmed)
    {
        return Some(permission_profile);
    }
    if let Ok(sandbox_policy) =
        serde_json::from_str::<codex_protocol::protocol::SandboxPolicy>(trimmed)
    {
        return Some(
            codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &sandbox_policy,
                cwd,
            ),
        );
    }
    let owned_bare;
    let bare = match serde_json::from_str::<String>(trimmed) {
        Ok(value) => {
            owned_bare = value;
            owned_bare.as_str()
        }
        Err(_) => trimmed,
    };
    let sandbox_policy = match bare {
        "danger-full-access" => Some(codex_protocol::protocol::SandboxPolicy::DangerFullAccess),
        "external-sandbox" => Some(codex_protocol::protocol::SandboxPolicy::ExternalSandbox {
            network_access: codex_protocol::protocol::NetworkAccess::Restricted,
        }),
        "read-only" => Some(codex_protocol::protocol::SandboxPolicy::new_read_only_policy()),
        "workspace-write" => Some(codex_protocol::protocol::SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        }),
        _ => None,
    }?;
    Some(
        codex_protocol::models::PermissionProfile::from_legacy_sandbox_policy_for_cwd(
            &sandbox_policy,
            cwd,
        ),
    )
}

fn parse_persisted_approval_mode(stored: &str) -> Option<codex_protocol::protocol::AskForApproval> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_value(serde_json::Value::String(trimmed.to_string())).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::PermissionProfile;
    use codex_protocol::protocol::SandboxPolicy;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[test]
    fn usage_profile_wait_resume_error_preserves_retry_at_for_dispatch() {
        let retry_at = Utc::now() + ChronoDuration::seconds(90);
        let dispatch_result = MailboxResumeError::UsageProfileWait {
            retry_at,
            error: "all eligible auth profiles are exhausted".to_string(),
        }
        .into_dispatch_result();

        match dispatch_result {
            MailboxDispatchResult::Retry {
                error,
                retry_at: Some(actual_retry_at),
            } => {
                assert_eq!(error, "all eligible auth profiles are exhausted");
                assert_eq!(actual_retry_at, retry_at);
            }
            _ => panic!("expected retry with retry_at"),
        }
    }

    #[test]
    fn parse_persisted_permission_profile_accepts_current_metadata_format() {
        let profile = PermissionProfile::Disabled;
        let stored = serde_json::to_string(&profile).expect("serialize permission profile");

        assert_eq!(
            Some(profile),
            parse_persisted_permission_profile(&stored, Path::new("/workspace"))
        );
    }

    #[test]
    fn parse_persisted_permission_profile_accepts_legacy_sandbox_policy_metadata() {
        let sandbox_policy = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: true,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };
        let cwd = Path::new("/workspace");
        let stored = serde_json::to_string(&sandbox_policy).expect("serialize sandbox policy");

        assert_eq!(
            Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &sandbox_policy,
                cwd
            )),
            parse_persisted_permission_profile(&stored, cwd)
        );
    }

    #[test]
    fn parse_persisted_permission_profile_accepts_legacy_bare_sandbox_policy_metadata() {
        let cwd = Path::new("/workspace");
        let workspace_write = SandboxPolicy::WorkspaceWrite {
            writable_roots: Vec::new(),
            network_access: false,
            exclude_tmpdir_env_var: false,
            exclude_slash_tmp: false,
        };

        for (stored, expected) in [
            ("danger-full-access", SandboxPolicy::DangerFullAccess),
            ("read-only", SandboxPolicy::new_read_only_policy()),
            ("workspace-write", workspace_write),
        ] {
            assert_eq!(
                Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                    &expected, cwd
                )),
                parse_persisted_permission_profile(stored, cwd)
            );
        }

        assert_eq!(
            Some(PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &SandboxPolicy::new_read_only_policy(),
                cwd
            )),
            parse_persisted_permission_profile("\"read-only\"", cwd)
        );
    }

    #[tokio::test]
    async fn mailbox_resume_metadata_restores_persisted_permission_profile() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let state_db = codex_state::StateRuntime::init(
            temp_dir.path().to_path_buf(),
            "fallback-provider".to_string(),
        )
        .await
        .expect("state db should initialize");
        let thread_id = ThreadId::new();
        let mut builder = ThreadMetadataBuilder::new(
            thread_id,
            temp_dir.path().join("thread.jsonl"),
            DateTime::<Utc>::from_timestamp(1_700_000_000, /*nsecs*/ 0).expect("valid timestamp"),
            SessionSource::Cli,
        );
        builder.cwd = temp_dir.path().join("workspace");
        builder.model_provider = Some("openai".to_string());
        builder.approval_mode = codex_protocol::protocol::AskForApproval::Never;
        let mut metadata = builder.build("fallback-provider");
        metadata.model = Some("gpt-5.5".to_string());
        metadata.sandbox_policy =
            serde_json::to_string(&PermissionProfile::read_only()).expect("serialize permissions");
        state_db
            .upsert_thread(&metadata)
            .await
            .expect("thread metadata should persist");

        let history = InitialHistory::Resumed(ResumedHistory {
            conversation_id: thread_id,
            history: vec![RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_id,
                    cwd: temp_dir.path().join("latest-workspace"),
                    ..SessionMeta::default()
                },
                git: None,
            })],
            rollout_path: None,
        });
        let mut request_overrides = None;
        let mut typesafe_overrides = ConfigOverrides {
            permission_profile: Some(PermissionProfile::Disabled),
            ..ConfigOverrides::default()
        };

        apply_persisted_mailbox_resume_metadata(
            &state_db,
            thread_id,
            &history,
            &mut request_overrides,
            &mut typesafe_overrides,
        )
        .await;

        assert_eq!(
            Some(PermissionProfile::read_only()),
            typesafe_overrides.permission_profile
        );
        assert_eq!(Some("gpt-5.5".to_string()), typesafe_overrides.model);
        assert_eq!(
            Some("openai".to_string()),
            typesafe_overrides.model_provider
        );
        assert_eq!(
            Some(codex_protocol::protocol::AskForApproval::Never),
            typesafe_overrides.approval_policy
        );
    }

    #[test]
    fn trigger_turn_delivery_wakes_target_only_after_ack() {
        let thread_id = ThreadId::new();
        // Queue-only deliveries must never schedule a post-ack wake.
        assert_eq!(
            mailbox_dispatch_wake_target(ActiveChannelDeliveryMode::QueueOnly, thread_id),
            None
        );
        // Trigger-turn deliveries hand the target back to dispatch_claim, which
        // only wakes it once the durable claim is acked.
        assert_eq!(
            mailbox_dispatch_wake_target(ActiveChannelDeliveryMode::TriggerTurn, thread_id),
            Some(thread_id)
        );
    }

    #[test]
    fn local_active_fresh_after_tolerates_short_stalls() {
        let now = DateTime::<Utc>::from_timestamp(1_700_000_000, /*nsecs*/ 0)
            .expect("valid timestamp");
        let window = now - mailbox_local_active_fresh_after(now);
        assert!(
            window.num_seconds() >= 30,
            "liveness window must tolerate brief stalls before a live target looks claimable; got {}s",
            window.num_seconds()
        );
    }

    #[test]
    fn queue_only_policy_does_not_trigger_user_instruction_turn() {
        let policy = mailbox_local_delivery_policy_from_payload(&serde_json::json!({
            "delivery": "queueOnly",
        }));

        assert_eq!(
            mailbox_delivery_mode(codex_state::MailboxMessageKind::UserInstruction, policy),
            ActiveChannelDeliveryMode::QueueOnly
        );
    }

    #[test]
    fn delivery_policy_uses_first_recognized_marker_across_locations() {
        // An unrecognized primary key must not shadow a recognized secondary
        // marker that the SQL claim filter honors.
        assert_eq!(
            mailbox_local_delivery_policy_from_payload(&serde_json::json!({
                "delivery": "expedited",
                "dispatch": { "mode": "resumeAndTrigger" },
            })),
            MailboxLocalDeliveryPolicy::ResumeAndTrigger
        );

        // A non-string primary key is skipped, not treated as LiveOnly.
        assert_eq!(
            mailbox_local_delivery_policy_from_payload(&serde_json::json!({
                "delivery": { "mode": "resumeAndTrigger" },
                "deliveryMode": "resume_and_trigger",
            })),
            MailboxLocalDeliveryPolicy::ResumeAndTrigger
        );

        // Key order still decides between conflicting recognized markers.
        assert_eq!(
            mailbox_local_delivery_policy_from_payload(&serde_json::json!({
                "delivery": "liveOnly",
                "deliveryMode": "resumeAndTrigger",
            })),
            MailboxLocalDeliveryPolicy::LiveOnly
        );

        // No recognized marker anywhere falls back to LiveOnly.
        assert_eq!(
            mailbox_local_delivery_policy_from_payload(&serde_json::json!({
                "delivery": "expedited",
            })),
            MailboxLocalDeliveryPolicy::LiveOnly
        );
    }
}
