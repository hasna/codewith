use super::*;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use chrono::Utc;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::ReviewPublisherContext;
use codex_app_server_protocol::ReviewPublisherEventKind as ApiEventKind;
use codex_app_server_protocol::ReviewPublisherEventStatus as ApiEventStatus;
use codex_app_server_protocol::ReviewPublisherOutboxEvent as ApiOutboxEvent;
use codex_app_server_protocol::ReviewPublisherReplayParams;
use codex_app_server_protocol::ReviewPublisherReplayResponse;
use codex_app_server_protocol::ReviewPublisherRun as ApiRun;
use codex_app_server_protocol::ReviewPublisherRunStatus as ApiRunStatus;
use codex_app_server_protocol::ReviewPublisherStatusReadParams;
use codex_app_server_protocol::ReviewPublisherStatusReadResponse;
use codex_app_server_protocol::ReviewPublisherVerdict as ApiVerdict;
use codex_git_utils::canonicalize_git_remote_url;
use codex_protocol::protocol::ReviewEnvelope;
use codex_protocol::protocol::ReviewImplementerProvenance;
use codex_protocol::protocol::ReviewImplementerProvenanceSource;
use codex_rollout::StateDbHandle;
use codex_state::REVIEW_ENVELOPE_SCHEMA_VERSION;
use codex_state::ReviewPublisherClaimParams;
use codex_state::ReviewPublisherDeliveryAckParams;
use codex_state::ReviewPublisherDeliveryFailParams;
use codex_state::ReviewPublisherFailureDisposition;
use reqwest::StatusCode;
use std::path::Path;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

const GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const DISPATCH_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DISPATCH_LEASE_DURATION: Duration = Duration::from_secs(30);
const DISPATCH_HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const DISPATCH_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);
const DISPATCH_MAX_ATTEMPTS: u32 = 6;
const DISPATCH_LEASE_OWNER: &str = "app-server-review-publisher";
const REVIEW_PUBLISHER_URL_ENV: &str = "CODEWITH_REVIEW_PUBLISHER_URL";
const REVIEW_PUBLISHER_CREDENTIAL_ENV_ENV: &str = "CODEWITH_REVIEW_PUBLISHER_CREDENTIAL_ENV";

#[derive(Clone)]
pub(crate) struct ReviewPublisherRequestProcessor {
    state_db: Option<StateDbHandle>,
}

impl ReviewPublisherRequestProcessor {
    pub(crate) fn new(state_db: Option<StateDbHandle>) -> Self {
        Self { state_db }
    }

    pub(crate) async fn status_read(
        &self,
        params: ReviewPublisherStatusReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let review_run_id = normalize_digest_bound_id("reviewRunId", params.review_run_id)?;
        let run = state_db
            .review_publisher()
            .get_review_run(review_run_id.as_str())
            .await
            .map_err(|err| {
                internal_error(format!("failed to read review publisher status: {err}"))
            })?
            .map(api_run);
        Ok(Some(ReviewPublisherStatusReadResponse { run }.into()))
    }

    pub(crate) async fn replay(
        &self,
        params: ReviewPublisherReplayParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let event_id = normalize_digest_bound_id("eventId", params.event_id)?;
        let payload_sha256 = normalize_sha256("payloadSha256", params.payload_sha256)?;
        let event = state_db
            .review_publisher()
            .exact_replay(event_id.as_str(), payload_sha256.as_str(), Utc::now())
            .await
            .map_err(|err| {
                internal_error(format!("failed to replay review publisher event: {err}"))
            })?
            .map(api_event);
        Ok(Some(ReviewPublisherReplayResponse { event }.into()))
    }

    fn state_db(&self) -> Result<&StateDbHandle, JSONRPCErrorError> {
        self.state_db
            .as_ref()
            .ok_or_else(|| internal_error("review publisher state is unavailable".to_string()))
    }
}

#[derive(Clone)]
pub(crate) struct ReviewPublisherDispatcherRuntime {
    state_db: Option<StateDbHandle>,
    config: Option<ReviewPublisherDispatcherConfig>,
    client: reqwest::Client,
    cancel_token: CancellationToken,
    tasks: TaskTracker,
}

#[derive(Clone)]
struct ReviewPublisherDispatcherConfig {
    endpoint: reqwest::Url,
    credential_env: String,
}

impl ReviewPublisherDispatcherRuntime {
    pub(crate) fn new(state_db: Option<StateDbHandle>) -> Self {
        let mut config = dispatcher_config_from_env();
        let client = match build_dispatch_client() {
            Ok(client) => client,
            Err(err) => {
                warn!(
                    "failed to build review publisher HTTP client; dispatcher is disabled: {err}"
                );
                config = None;
                reqwest::Client::new()
            }
        };
        Self {
            state_db,
            config,
            client,
            cancel_token: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }

    pub(crate) fn start(&self) {
        if self.state_db.is_none() || self.config.is_none() {
            return;
        }
        let runtime = self.clone();
        self.tasks.spawn(async move { runtime.run().await });
    }

    pub(crate) fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    pub(crate) async fn drain_background_tasks(&self) {
        self.shutdown();
        self.tasks.close();
        if tokio::time::timeout(DISPATCH_DRAIN_TIMEOUT, self.tasks.wait())
            .await
            .is_err()
        {
            warn!("timed out waiting for review publisher dispatcher to drain");
        }
    }

    async fn run(self) {
        let mut interval = tokio::time::interval(DISPATCH_POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => break,
                _ = interval.tick() => self.dispatch_one().await,
            }
        }
    }

    async fn dispatch_one(&self) {
        let (Some(state_db), Some(config)) = (self.state_db.as_ref(), self.config.as_ref()) else {
            return;
        };
        let claim = match state_db
            .review_publisher()
            .claim_next_due_event(ReviewPublisherClaimParams {
                lease_owner: DISPATCH_LEASE_OWNER.to_string(),
                lease_duration: DISPATCH_LEASE_DURATION,
                now: Utc::now(),
            })
            .await
        {
            Ok(Some(claim)) => claim,
            Ok(None) => return,
            Err(err) => {
                warn!("failed to claim review publisher event: {err}");
                return;
            }
        };

        let result = self.send_event(config, &claim.event.payload).await;
        let requested_dead_letter = matches!(&result, DispatchResult::DeadLetter { .. });
        let now = Utc::now();
        match result {
            DispatchResult::Delivered { receipt_id } => {
                if let Err(err) = state_db
                    .review_publisher()
                    .acknowledge_delivery(ReviewPublisherDeliveryAckParams {
                        event_id: claim.event.event_id,
                        lease_owner: DISPATCH_LEASE_OWNER.to_string(),
                        receipt_id,
                        now,
                    })
                    .await
                {
                    warn!("failed to acknowledge review publisher delivery: {err}");
                }
            }
            DispatchResult::Retry { error_code } | DispatchResult::DeadLetter { error_code } => {
                let exhausted = claim.event.attempt_count >= DISPATCH_MAX_ATTEMPTS;
                let disposition = if requested_dead_letter || exhausted {
                    ReviewPublisherFailureDisposition::DeadLetter
                } else {
                    ReviewPublisherFailureDisposition::Retry
                };
                let retry_seconds =
                    i64::from(2_u32.saturating_pow(claim.event.attempt_count.min(8))).clamp(2, 300);
                if let Err(err) = state_db
                    .review_publisher()
                    .fail_delivery(ReviewPublisherDeliveryFailParams {
                        event_id: claim.event.event_id,
                        lease_owner: DISPATCH_LEASE_OWNER.to_string(),
                        error_code,
                        disposition,
                        retry_at: now + chrono::Duration::seconds(retry_seconds),
                        now,
                    })
                    .await
                {
                    warn!("failed to record review publisher delivery failure: {err}");
                }
            }
        }
    }

    async fn send_event(
        &self,
        config: &ReviewPublisherDispatcherConfig,
        event: &codex_protocol::protocol::ReviewPublisherEvent,
    ) -> DispatchResult {
        let token = match std::env::var(config.credential_env.as_str()) {
            Ok(token) if !token.is_empty() => token,
            _ => {
                return DispatchResult::DeadLetter {
                    error_code: "credential_unavailable".to_string(),
                };
            }
        };
        let response = self
            .client
            .post(config.endpoint.clone())
            .bearer_auth(token)
            .json(event)
            .send()
            .await;
        match response {
            Ok(response) => classify_http_response(&response),
            Err(err) => classify_transport_error(&err),
        }
    }
}

fn build_dispatch_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(DISPATCH_HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DispatchResult {
    Delivered { receipt_id: Option<String> },
    Retry { error_code: String },
    DeadLetter { error_code: String },
}

fn classify_http_response(response: &reqwest::Response) -> DispatchResult {
    let status = response.status();
    match classify_status(status) {
        StatusDisposition::Delivered => {
            let receipt_id = response
                .headers()
                .get("x-review-receipt-id")
                .or_else(|| response.headers().get("x-github-request-id"))
                .and_then(|value| value.to_str().ok())
                .and_then(normalize_receipt_id);
            DispatchResult::Delivered { receipt_id }
        }
        StatusDisposition::Retry => DispatchResult::Retry {
            error_code: format!("http_{}", status.as_u16()),
        },
        StatusDisposition::DeadLetter => DispatchResult::DeadLetter {
            error_code: format!("http_{}", status.as_u16()),
        },
    }
}

fn classify_transport_error(error: &reqwest::Error) -> DispatchResult {
    DispatchResult::Retry {
        error_code: if error.is_timeout() {
            "http_timeout".to_string()
        } else {
            "http_transport".to_string()
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusDisposition {
    Delivered,
    Retry,
    DeadLetter,
}

fn classify_status(status: StatusCode) -> StatusDisposition {
    if status.is_success() || status == StatusCode::CONFLICT {
        StatusDisposition::Delivered
    } else if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        StatusDisposition::Retry
    } else {
        StatusDisposition::DeadLetter
    }
}

fn normalize_receipt_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return None;
    }
    Some(value.to_string())
}

fn dispatcher_config_from_env() -> Option<ReviewPublisherDispatcherConfig> {
    let endpoint = std::env::var(REVIEW_PUBLISHER_URL_ENV).ok()?;
    let credential_env = std::env::var(REVIEW_PUBLISHER_CREDENTIAL_ENV_ENV).ok()?;
    let endpoint = reqwest::Url::parse(endpoint.trim()).ok()?;
    if !valid_publisher_endpoint(&endpoint) || !valid_env_name(credential_env.trim()) {
        warn!("review publisher configuration is invalid; dispatcher is disabled");
        return None;
    }
    Some(ReviewPublisherDispatcherConfig {
        endpoint,
        credential_env: credential_env.trim().to_string(),
    })
}

fn valid_publisher_endpoint(endpoint: &reqwest::Url) -> bool {
    let transport_is_safe = match endpoint.scheme() {
        "https" => endpoint.host_str().is_some(),
        "http" => endpoint.host_str().is_some_and(|host| {
            let host = host
                .strip_prefix('[')
                .and_then(|host| host.strip_suffix(']'))
                .unwrap_or(host);
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        }),
        _ => false,
    };
    transport_is_safe
        && endpoint.username().is_empty()
        && endpoint.password().is_none()
        && endpoint.query().is_none()
        && endpoint.fragment().is_none()
}

fn valid_env_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(ch) if ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) async fn build_review_envelope(
    cwd: &Path,
    context: ReviewPublisherContext,
) -> Result<ReviewEnvelope, JSONRPCErrorError> {
    if context.pull_request_number == 0 {
        return Err(invalid_request(
            "publisherContext.pullRequestNumber must be positive".to_string(),
        ));
    }
    let base_ref = normalize_ref(context.base_ref)?;
    let reviewed_base_sha = normalize_git_sha("reviewedBaseSha", context.reviewed_base_sha)?;
    let head_sha = normalize_git_sha("headSha", context.head_sha)?;
    let acceptance_scope_id = normalize_scope_id(context.acceptance_scope_id)?;
    let acceptance_scope_sha256 = normalize_sha256(
        "publisherContext.acceptanceScopeSha256",
        context.acceptance_scope_sha256,
    )?;

    let status = git_stdout(cwd, &["status", "--porcelain=v1", "--untracked-files=all"]).await?;
    if !status.is_empty() {
        return Err(invalid_request(
            "review publisher requires a clean working tree".to_string(),
        ));
    }
    let actual_head = git_stdout(cwd, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
    if actual_head != head_sha {
        return Err(invalid_request(
            "publisherContext.headSha does not match local HEAD".to_string(),
        ));
    }
    let base_commit_expr = format!("{base_ref}^{{commit}}");
    let actual_base = git_stdout(
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            base_commit_expr.as_str(),
        ],
    )
    .await?;
    if actual_base != reviewed_base_sha {
        return Err(invalid_request(
            "publisherContext.reviewedBaseSha does not match baseRef".to_string(),
        ));
    }
    let remote = git_stdout(cwd, &["remote", "get-url", "origin"]).await?;
    let repository_origin = canonicalize_git_remote_url(remote.as_str()).ok_or_else(|| {
        invalid_request("origin remote is not a canonical repository URL".to_string())
    })?;
    let merge_tree = git_stdout(
        cwd,
        &[
            "merge-tree",
            "--write-tree",
            reviewed_base_sha.as_str(),
            head_sha.as_str(),
        ],
    )
    .await?;
    let merge_result_tree_sha = merge_tree
        .lines()
        .next()
        .map(str::trim)
        .filter(|value| is_lower_hex(value, 40))
        .ok_or_else(|| invalid_request("git merge-tree returned no clean result tree".to_string()))?
        .to_string();
    let message = git_stdout(cwd, &["show", "-s", "--format=%B", head_sha.as_str()]).await?;
    let implementer_agent = parse_single_agent_trailer(message.as_str())?;
    let candidate_sha256 = codex_state::review_candidate_sha256(
        repository_origin.as_str(),
        context.pull_request_number,
        reviewed_base_sha.as_str(),
        head_sha.as_str(),
        merge_result_tree_sha.as_str(),
    )
    .map_err(|err| internal_error(format!("failed to digest review candidate: {err}")))?;
    let mut envelope = ReviewEnvelope {
        schema_version: REVIEW_ENVELOPE_SCHEMA_VERSION.to_string(),
        repository_origin,
        pull_request_number: context.pull_request_number,
        base_ref,
        reviewed_base_sha,
        head_sha: head_sha.clone(),
        merge_result_tree_sha,
        candidate_sha256,
        acceptance_scope_id,
        acceptance_scope_sha256,
        implementer: ReviewImplementerProvenance {
            source: ReviewImplementerProvenanceSource::GitAgentTrailer,
            agent: implementer_agent,
            commit_sha: head_sha,
        },
        envelope_sha256: String::new(),
    };
    envelope.envelope_sha256 = codex_state::review_envelope_sha256(&envelope)
        .map_err(|err| internal_error(format!("failed to digest review envelope: {err}")))?;
    Ok(envelope)
}

async fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String, JSONRPCErrorError> {
    let output = tokio::time::timeout(
        GIT_PROBE_TIMEOUT,
        Command::new("git").args(args).current_dir(cwd).output(),
    )
    .await
    .map_err(|_| invalid_request("timed out while verifying review candidate".to_string()))?
    .map_err(|_| {
        invalid_request("failed to execute git while verifying review candidate".to_string())
    })?;
    if !output.status.success() {
        return Err(invalid_request(format!(
            "git {} failed while verifying review candidate",
            args.first().copied().unwrap_or("command")
        )));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .map_err(|_| invalid_request("git returned non-UTF-8 candidate metadata".to_string()))
}

fn parse_single_agent_trailer(message: &str) -> Result<String, JSONRPCErrorError> {
    let trailer_block = message
        .rsplit_once("\n\n")
        .map_or(message, |(_, tail)| tail);
    let agents = trailer_block
        .lines()
        .filter_map(|line| line.strip_prefix("Agent:"))
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .collect::<Vec<_>>();
    if agents.len() != 1
        || agents[0].len() > 64
        || !agents[0]
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(invalid_request(
            "reviewed head commit must contain exactly one valid Agent trailer".to_string(),
        ));
    }
    Ok(agents[0].to_string())
}

fn normalize_ref(value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.len() > 256
        || !value.starts_with("refs/")
        || value.contains("..")
        || value.contains("@{")
        || value.chars().any(|ch| {
            ch.is_ascii_control()
                || ch.is_whitespace()
                || matches!(ch, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
        })
    {
        return Err(invalid_request(
            "publisherContext.baseRef must be a full, safe Git ref".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn normalize_scope_id(value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '/'))
    {
        return Err(invalid_request(
            "publisherContext.acceptanceScopeId is invalid".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn normalize_git_sha(field: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if !is_lower_hex(value, 40) {
        return Err(invalid_request(format!(
            "publisherContext.{field} must be a full lowercase Git SHA"
        )));
    }
    Ok(value.to_string())
}

fn normalize_sha256(field: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if !is_lower_hex(value, 64) {
        return Err(invalid_request(format!(
            "{field} must be a lowercase SHA-256"
        )));
    }
    Ok(value.to_string())
}

fn normalize_digest_bound_id(field: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(invalid_request(format!("{field} is invalid")));
    }
    Ok(value.to_string())
}

fn is_lower_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn api_run(snapshot: codex_state::ReviewPublisherRunSnapshot) -> ApiRun {
    ApiRun {
        review_run_id: snapshot.run.review_run_id,
        envelope_sha256: snapshot.run.envelope_sha256,
        status: match snapshot.run.status {
            codex_state::ReviewPublisherRunStatus::Started => ApiRunStatus::Started,
            codex_state::ReviewPublisherRunStatus::Completed => ApiRunStatus::Completed,
        },
        verdict: snapshot.run.verdict.map(|verdict| match verdict {
            codex_protocol::protocol::ReviewPublisherVerdict::Go => ApiVerdict::Go,
            codex_protocol::protocol::ReviewPublisherVerdict::NoGo => ApiVerdict::NoGo,
        }),
        created_at: api_timestamp(snapshot.run.created_at),
        completed_at: snapshot.run.completed_at.map(api_timestamp),
        events: snapshot.events.into_iter().map(api_event).collect(),
    }
}

fn api_event(event: codex_state::ReviewPublisherOutboxEvent) -> ApiOutboxEvent {
    ApiOutboxEvent {
        event_id: event.event_id,
        event_kind: match event.event_kind {
            codex_protocol::protocol::ReviewPublisherEventKind::Started => ApiEventKind::Started,
            codex_protocol::protocol::ReviewPublisherEventKind::Completed => {
                ApiEventKind::Completed
            }
        },
        sequence: event.sequence,
        status: match event.status {
            codex_state::ReviewPublisherOutboxStatus::Pending => ApiEventStatus::Pending,
            codex_state::ReviewPublisherOutboxStatus::InFlight => ApiEventStatus::InFlight,
            codex_state::ReviewPublisherOutboxStatus::Delivered => ApiEventStatus::Delivered,
            codex_state::ReviewPublisherOutboxStatus::DeadLetter => ApiEventStatus::DeadLetter,
        },
        payload_sha256: event.payload_sha256,
        attempt_count: event.attempt_count,
        next_attempt_at: api_timestamp(event.next_attempt_at),
        lease_expires_at: event.lease_expires_at.map(api_timestamp),
        receipt_id: event.receipt_id,
        last_error_code: event.last_error_code,
        created_at: api_timestamp(event.created_at),
        delivered_at: event.delivered_at.map(api_timestamp),
    }
}

fn api_timestamp(value: chrono::DateTime<Utc>) -> i64 {
    value.timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command as StdCommand;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;

    #[test]
    fn publisher_endpoint_requires_https_except_for_loopback_http() {
        for endpoint in [
            "https://publisher.example.com/reviews",
            "http://localhost:8080/reviews",
            "http://127.0.0.1:8080/reviews",
            "http://[::1]:8080/reviews",
        ] {
            assert!(valid_publisher_endpoint(
                &reqwest::Url::parse(endpoint).expect("valid URL")
            ));
        }
        for endpoint in [
            "http://publisher.example.com/reviews",
            "http://10.0.0.8/reviews",
            "https://user@publisher.example.com/reviews",
            "https://publisher.example.com/reviews?key=value",
            "https://publisher.example.com/reviews#fragment",
        ] {
            assert!(!valid_publisher_endpoint(
                &reqwest::Url::parse(endpoint).expect("valid URL")
            ));
        }
    }

    #[test]
    fn publisher_api_timestamps_are_unix_seconds() {
        let timestamp = chrono::DateTime::<Utc>::from_timestamp_millis(1_700_000_000_123)
            .expect("valid timestamp");
        assert_eq!(api_timestamp(timestamp), 1_700_000_000);
    }

    #[test]
    fn http_statuses_are_classified_fail_closed() {
        assert_eq!(
            classify_status(StatusCode::CONFLICT),
            StatusDisposition::Delivered
        );
        assert_eq!(
            classify_status(StatusCode::UNAUTHORIZED),
            StatusDisposition::DeadLetter
        );
        assert_eq!(
            classify_status(StatusCode::FORBIDDEN),
            StatusDisposition::DeadLetter
        );
        assert_eq!(
            classify_status(StatusCode::TOO_MANY_REQUESTS),
            StatusDisposition::Retry
        );
        assert_eq!(
            classify_status(StatusCode::INTERNAL_SERVER_ERROR),
            StatusDisposition::Retry
        );
    }

    #[tokio::test]
    async fn http_timeout_is_retryable_without_persisting_response_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(100)))
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(5))
            .build()
            .expect("client");
        let error = client
            .post(server.uri())
            .send()
            .await
            .expect_err("request must time out");
        assert_eq!(
            classify_transport_error(&error),
            DispatchResult::Retry {
                error_code: "http_timeout".to_string()
            }
        );
    }

    #[tokio::test]
    async fn publisher_client_does_not_follow_redirects() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(StatusCode::FOUND.as_u16())
                    .append_header("location", server.uri()),
            )
            .expect(1)
            .mount(&server)
            .await;

        let response = build_dispatch_client()
            .expect("client")
            .post(server.uri())
            .send()
            .await
            .expect("redirect response");

        assert_eq!(response.status(), StatusCode::FOUND);
    }

    #[test]
    fn agent_provenance_comes_only_from_one_trailer() {
        assert_eq!(
            parse_single_agent_trailer("subject\n\nbody\n\nAgent: Herminia").unwrap(),
            "Herminia"
        );
        assert!(parse_single_agent_trailer("Agent: One\nAgent: Two").is_err());
        assert!(parse_single_agent_trailer("body names Agent: Caller").is_err());
    }

    #[tokio::test]
    async fn envelope_rejects_candidate_mismatch_and_base_ref_movement() {
        let repo = tempfile::tempdir().expect("temp repo");
        git(repo.path(), &["init", "-b", "main"]);
        git(repo.path(), &["config", "user.name", "Test"]);
        git(repo.path(), &["config", "user.email", "test@example.com"]);
        git(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/hasna/codewith.git",
            ],
        );
        fs::write(repo.path().join("file.txt"), "base\n").expect("base file");
        git(repo.path(), &["add", "file.txt"]);
        git(repo.path(), &["commit", "-m", "base", "-m", "Agent: Base"]);
        let base = git(repo.path(), &["rev-parse", "HEAD"]);
        git(repo.path(), &["switch", "-c", "feature"]);
        fs::write(repo.path().join("file.txt"), "base\nfeature\n").expect("feature file");
        git(repo.path(), &["add", "file.txt"]);
        git(
            repo.path(),
            &["commit", "-m", "feature", "-m", "Agent: Herminia"],
        );
        let head = git(repo.path(), &["rev-parse", "HEAD"]);
        let context = publisher_context(base.clone(), head.clone());
        let envelope = build_review_envelope(repo.path(), context.clone())
            .await
            .expect("verified envelope");
        assert_eq!(envelope.reviewed_base_sha, base);
        assert_eq!(envelope.head_sha, head);
        assert_eq!(envelope.implementer.agent, "Herminia");

        let mut mismatched = context.clone();
        mismatched.head_sha = "c".repeat(40);
        assert!(
            build_review_envelope(repo.path(), mismatched)
                .await
                .is_err()
        );

        let base_tree = git(repo.path(), &["rev-parse", "refs/heads/main^{tree}"]);
        let moved_base = git(
            repo.path(),
            &[
                "commit-tree",
                base_tree.as_str(),
                "-p",
                base.as_str(),
                "-m",
                "moved base\n\nAgent: Base",
            ],
        );
        git(
            repo.path(),
            &["update-ref", "refs/heads/main", moved_base.as_str()],
        );
        assert!(build_review_envelope(repo.path(), context).await.is_err());
    }

    fn publisher_context(reviewed_base_sha: String, head_sha: String) -> ReviewPublisherContext {
        ReviewPublisherContext {
            pull_request_number: 17,
            base_ref: "refs/heads/main".to_string(),
            reviewed_base_sha,
            head_sha,
            acceptance_scope_id: "codewith-review-envelope-v1".to_string(),
            acceptance_scope_sha256: "d".repeat(64),
        }
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let output = StdCommand::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git command failed: {}",
            args.join(" ")
        );
        String::from_utf8(output.stdout)
            .expect("git stdout")
            .trim()
            .to_string()
    }
}
