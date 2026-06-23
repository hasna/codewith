use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use crate::workspace_command::WorkspaceCommand;
use crate::workspace_command::WorkspaceCommandError;
use crate::workspace_command::WorkspaceCommandExecutor;
use crate::workspace_command::WorkspaceCommandOutput;

const PR_JSON_FIELDS: &str = concat!(
    "number,title,url,state,isDraft,author,headRefName,baseRefName,",
    "reviewDecision,mergeStateStatus,statusCheckRollup"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PullRequestOverview {
    pub(crate) cwd: PathBuf,
    pub(crate) current: PullRequestQuery,
    pub(crate) open: PullRequestQuery,
}

impl PullRequestOverview {
    pub(crate) fn runner_unavailable(cwd: PathBuf) -> Self {
        Self {
            cwd,
            current: PullRequestQuery::from_status(PullRequestQueryStatus::RunnerUnavailable),
            open: PullRequestQuery::from_status(PullRequestQueryStatus::RunnerUnavailable),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PullRequestQuery {
    pub(crate) status: PullRequestQueryStatus,
    pub(crate) items: Vec<PullRequestSummary>,
}

impl PullRequestQuery {
    fn ready(items: Vec<PullRequestSummary>) -> Self {
        Self {
            status: PullRequestQueryStatus::Ready,
            items,
        }
    }

    fn from_status(status: PullRequestQueryStatus) -> Self {
        Self {
            status,
            items: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PullRequestQueryStatus {
    Ready,
    RunnerUnavailable,
    NotGitRepository,
    GhUnavailable(String),
    AuthRequired(String),
    NoCurrentPullRequest,
    RateLimited(String),
    CommandFailed(String),
    ParseFailed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PullRequestSummary {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) url: String,
    pub(crate) state: String,
    pub(crate) is_draft: bool,
    pub(crate) author: Option<String>,
    pub(crate) head_ref: String,
    pub(crate) base_ref: String,
    pub(crate) review_decision: Option<String>,
    pub(crate) merge_state: Option<String>,
    pub(crate) checks: PullRequestCheckSummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PullRequestCheckSummary {
    pub(crate) total: u64,
    pub(crate) success: u64,
    pub(crate) failing: u64,
    pub(crate) pending: u64,
    pub(crate) skipped: u64,
    pub(crate) unknown: u64,
}

#[derive(Deserialize)]
struct GhPullRequest {
    number: u64,
    title: String,
    url: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    author: Option<GhAuthor>,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Value>,
}

#[derive(Deserialize)]
struct GhAuthor {
    login: String,
}

pub(crate) async fn load_pull_request_overview(
    runner: &dyn WorkspaceCommandExecutor,
    cwd: &Path,
) -> PullRequestOverview {
    if let Some(status) = git_repo_status(runner, cwd).await {
        return PullRequestOverview {
            cwd: cwd.to_path_buf(),
            current: PullRequestQuery::from_status(status.clone()),
            open: PullRequestQuery::from_status(status),
        };
    }

    let current = load_current_pull_request(runner, cwd).await;
    let open = load_open_pull_requests(runner, cwd).await;
    PullRequestOverview {
        cwd: cwd.to_path_buf(),
        current,
        open,
    }
}

async fn git_repo_status(
    runner: &dyn WorkspaceCommandExecutor,
    cwd: &Path,
) -> Option<PullRequestQueryStatus> {
    let output = match run_git_command(runner, cwd, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(output) => output,
        Err(err) => return Some(status_from_command_error(err)),
    };
    if output.success() && output.stdout.trim() == "true" {
        None
    } else {
        Some(PullRequestQueryStatus::NotGitRepository)
    }
}

async fn load_current_pull_request(
    runner: &dyn WorkspaceCommandExecutor,
    cwd: &Path,
) -> PullRequestQuery {
    let output = match run_gh_command(runner, cwd, &["pr", "view", "--json", PR_JSON_FIELDS]).await
    {
        Ok(output) => output,
        Err(err) => {
            return PullRequestQuery::from_status(status_from_gh_command_error(err));
        }
    };
    if !output.success() {
        return PullRequestQuery::from_status(classify_gh_failure(
            &output,
            GhFailureContext::CurrentPullRequest,
        ));
    }

    match serde_json::from_str::<GhPullRequest>(&output.stdout) {
        Ok(pull_request) => PullRequestQuery::ready(vec![pull_request.into_summary()]),
        Err(err) => PullRequestQuery::from_status(PullRequestQueryStatus::ParseFailed(format!(
            "Failed to parse gh pr view JSON: {err}"
        ))),
    }
}

async fn load_open_pull_requests(
    runner: &dyn WorkspaceCommandExecutor,
    cwd: &Path,
) -> PullRequestQuery {
    let output = match run_gh_command(
        runner,
        cwd,
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            "30",
            "--json",
            PR_JSON_FIELDS,
        ],
    )
    .await
    {
        Ok(output) => output,
        Err(err) => {
            return PullRequestQuery::from_status(status_from_gh_command_error(err));
        }
    };
    if !output.success() {
        return PullRequestQuery::from_status(classify_gh_failure(
            &output,
            GhFailureContext::PullRequestList,
        ));
    }

    match serde_json::from_str::<Vec<GhPullRequest>>(&output.stdout) {
        Ok(pull_requests) => PullRequestQuery::ready(
            pull_requests
                .into_iter()
                .map(GhPullRequest::into_summary)
                .collect(),
        ),
        Err(err) => PullRequestQuery::from_status(PullRequestQueryStatus::ParseFailed(format!(
            "Failed to parse gh pr list JSON: {err}"
        ))),
    }
}

impl GhPullRequest {
    fn into_summary(self) -> PullRequestSummary {
        PullRequestSummary {
            number: self.number,
            title: sanitize_remote_text(&self.title, 160),
            url: sanitize_remote_text(&self.url, 2048),
            state: sanitize_remote_text(&self.state, 80),
            is_draft: self.is_draft,
            author: self
                .author
                .map(|author| sanitize_remote_text(&author.login, 80)),
            head_ref: sanitize_remote_text(&self.head_ref_name, 120),
            base_ref: sanitize_remote_text(&self.base_ref_name, 120),
            review_decision: self
                .review_decision
                .as_ref()
                .map(|status| sanitize_remote_text(status, 80)),
            merge_state: self
                .merge_state_status
                .as_ref()
                .map(|status| sanitize_remote_text(status, 80)),
            checks: check_summary_from_rollup(self.status_check_rollup.as_ref()),
        }
    }
}

fn sanitize_remote_text(text: &str, max_chars: usize) -> String {
    let mut sanitized = String::new();
    let mut truncated = false;
    for (count, ch) in text.chars().enumerate() {
        if count >= max_chars {
            truncated = true;
            break;
        }
        if ch == '\r' || ch == '\n' || ch == '\t' || ch.is_control() || is_bidi_format_control(ch) {
            sanitized.push(' ');
        } else {
            sanitized.push(ch);
        }
    }
    let sanitized = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if truncated {
        format!("{sanitized}...")
    } else {
        sanitized
    }
}

fn is_bidi_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[derive(Clone, Copy)]
enum GhFailureContext {
    CurrentPullRequest,
    PullRequestList,
}

fn classify_gh_failure(
    output: &WorkspaceCommandOutput,
    context: GhFailureContext,
) -> PullRequestQueryStatus {
    let message = command_message(output);
    let lower = message.to_ascii_lowercase();
    if lower.contains("rate limit") || lower.contains("secondary rate") {
        return PullRequestQueryStatus::RateLimited(clean_failure_message(&message));
    }
    if lower.contains("gh auth login")
        || lower.contains("not logged in")
        || lower.contains("authentication")
        || lower.contains("could not resolve to a repository")
    {
        return PullRequestQueryStatus::AuthRequired(clean_failure_message(&message));
    }
    if output.exit_code == 127
        || lower.contains("no such file or directory")
        || lower.contains("command not found")
    {
        return PullRequestQueryStatus::GhUnavailable(clean_failure_message(&message));
    }
    if matches!(context, GhFailureContext::CurrentPullRequest)
        && (lower.contains("no pull requests")
            || lower.contains("no open pull requests")
            || lower.contains("could not find any pull requests"))
    {
        return PullRequestQueryStatus::NoCurrentPullRequest;
    }

    PullRequestQueryStatus::CommandFailed(clean_failure_message(&message))
}

fn command_message(output: &WorkspaceCommandOutput) -> String {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        output.stdout.trim().to_string()
    } else {
        stderr.to_string()
    }
}

fn clean_failure_message(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        "command failed without output".to_string()
    } else {
        let first_line = trimmed.lines().next().unwrap_or(trimmed);
        let sanitized = sanitize_remote_text(first_line, 240);
        if sanitized.is_empty() {
            "command failed without output".to_string()
        } else {
            sanitized
        }
    }
}

fn status_from_command_error(err: WorkspaceCommandError) -> PullRequestQueryStatus {
    let message = clean_failure_message(&err.to_string());
    if message
        .to_ascii_lowercase()
        .contains("local environment is not configured")
    {
        PullRequestQueryStatus::RunnerUnavailable
    } else {
        PullRequestQueryStatus::CommandFailed(message)
    }
}

fn status_from_gh_command_error(err: WorkspaceCommandError) -> PullRequestQueryStatus {
    let message = clean_failure_message(&err.to_string());
    let lower = message.to_ascii_lowercase();
    if lower.contains("local environment is not configured") {
        PullRequestQueryStatus::RunnerUnavailable
    } else if lower.contains("no such file or directory")
        || lower.contains("command not found")
        || lower.contains("not found")
    {
        PullRequestQueryStatus::GhUnavailable(message)
    } else {
        PullRequestQueryStatus::CommandFailed(message)
    }
}

fn check_summary_from_rollup(value: Option<&Value>) -> PullRequestCheckSummary {
    let Some(value) = value else {
        return PullRequestCheckSummary::default();
    };
    let checks = match value {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(map) => map
            .get("nodes")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect::<Vec<_>>())
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let mut summary = PullRequestCheckSummary {
        total: checks.len() as u64,
        ..Default::default()
    };
    for check in checks {
        match normalized_check_state(check).as_deref() {
            Some("SUCCESS") => summary.success += 1,
            Some("FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED") => {
                summary.failing += 1
            }
            Some("PENDING" | "QUEUED" | "IN_PROGRESS" | "REQUESTED" | "WAITING") => {
                summary.pending += 1
            }
            Some("SKIPPED" | "NEUTRAL") => summary.skipped += 1,
            Some(_) | None => summary.unknown += 1,
        }
    }
    summary
}

fn normalized_check_state(value: &Value) -> Option<String> {
    ["conclusion", "state", "status"]
        .into_iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::to_ascii_uppercase)
}

async fn run_git_command(
    runner: &dyn WorkspaceCommandExecutor,
    cwd: &Path,
    args: &[&str],
) -> Result<WorkspaceCommandOutput, WorkspaceCommandError> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("git".to_string());
    argv.extend(args.iter().map(|arg| (*arg).to_string()));
    runner
        .run(
            WorkspaceCommand::new(argv)
                .cwd(cwd.to_path_buf())
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env("GIT_TERMINAL_PROMPT", "0"),
        )
        .await
}

async fn run_gh_command(
    runner: &dyn WorkspaceCommandExecutor,
    cwd: &Path,
    args: &[&str],
) -> Result<WorkspaceCommandOutput, WorkspaceCommandError> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("gh".to_string());
    argv.extend(args.iter().map(|arg| (*arg).to_string()));
    runner
        .run(
            WorkspaceCommand::new(argv)
                .cwd(cwd.to_path_buf())
                .env("GH_PROMPT_DISABLED", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GH_PAGER", "cat")
                .env("PAGER", "cat")
                .env("NO_COLOR", "1")
                .env("CLICOLOR", "0")
                .env("GH_NO_UPDATE_NOTIFIER", "1")
                .env("GCM_INTERACTIVE", "never"),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    #[test]
    fn parses_pull_request_without_fetching_body_fields() {
        let parsed = serde_json::from_str::<GhPullRequest>(
            r##"{
                "number": 42,
                "title": "Add PR dashboard",
                "url": "https://github.com/acme/repo/pull/42",
                "state": "OPEN",
                "isDraft": false,
                "author": {"login": "octo"},
                "headRefName": "feature",
                "baseRefName": "main",
                "reviewDecision": "REVIEW_REQUIRED",
                "mergeStateStatus": "CLEAN",
                "statusCheckRollup": [
                    {"conclusion": "SUCCESS"},
                    {"status": "IN_PROGRESS"},
                    {"conclusion": "FAILURE"}
                ]
            }"##,
        )
        .expect("valid gh pr JSON");

        assert_eq!(
            parsed.into_summary(),
            PullRequestSummary {
                number: 42,
                title: "Add PR dashboard".to_string(),
                url: "https://github.com/acme/repo/pull/42".to_string(),
                state: "OPEN".to_string(),
                is_draft: false,
                author: Some("octo".to_string()),
                head_ref: "feature".to_string(),
                base_ref: "main".to_string(),
                review_decision: Some("REVIEW_REQUIRED".to_string()),
                merge_state: Some("CLEAN".to_string()),
                checks: PullRequestCheckSummary {
                    total: 3,
                    success: 1,
                    failing: 1,
                    pending: 1,
                    skipped: 0,
                    unknown: 0,
                },
            }
        );
    }

    #[test]
    fn classifies_current_pr_not_found_separately() {
        let output = WorkspaceCommandOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: "no pull requests found for branch".to_string(),
        };

        assert_eq!(
            classify_gh_failure(&output, GhFailureContext::CurrentPullRequest),
            PullRequestQueryStatus::NoCurrentPullRequest
        );
    }

    #[test]
    fn classifies_auth_and_missing_gh_failures() {
        let auth = WorkspaceCommandOutput {
            exit_code: 4,
            stdout: String::new(),
            stderr: "To get started with GitHub CLI, run: gh auth login".to_string(),
        };
        let missing = WorkspaceCommandOutput {
            exit_code: 127,
            stdout: String::new(),
            stderr: "gh: command not found".to_string(),
        };

        assert_eq!(
            classify_gh_failure(&auth, GhFailureContext::PullRequestList),
            PullRequestQueryStatus::AuthRequired(
                "To get started with GitHub CLI, run: gh auth login".to_string()
            )
        );
        assert_eq!(
            classify_gh_failure(&missing, GhFailureContext::PullRequestList),
            PullRequestQueryStatus::GhUnavailable("gh: command not found".to_string())
        );
    }

    #[test]
    fn classifies_workspace_runner_errors_without_leaking_control_text() {
        assert_eq!(
            status_from_command_error(WorkspaceCommandError::new(
                "local environment is not configured"
            )),
            PullRequestQueryStatus::RunnerUnavailable
        );
        assert_eq!(
            status_from_command_error(WorkspaceCommandError::new(
                "failed\u{001b}[31m\nsecond line with extra detail"
            )),
            PullRequestQueryStatus::CommandFailed("failed [31m".to_string())
        );
        assert_eq!(
            status_from_gh_command_error(WorkspaceCommandError::new(
                "No such file or directory (os error 2)"
            )),
            PullRequestQueryStatus::GhUnavailable(
                "No such file or directory (os error 2)".to_string()
            )
        );
    }

    #[tokio::test]
    async fn overview_uses_read_only_gh_pr_commands_with_hardened_env() {
        let runner = FakeRunner::new(vec![
            ok_output("true\n"),
            ok_output(
                r##"{
                    "number": 7,
                    "title": "Current PR",
                    "url": "https://github.com/acme/repo/pull/7",
                    "state": "OPEN",
                    "isDraft": false,
                    "author": {"login": "octo"},
                    "headRefName": "feature",
                    "baseRefName": "main",
                    "reviewDecision": null,
                    "mergeStateStatus": null,
                    "statusCheckRollup": []
                }"##,
            ),
            ok_output("[]"),
        ]);

        let overview = load_pull_request_overview(&runner, Path::new("/repo")).await;

        assert_eq!(overview.current.items.len(), 1);
        assert_eq!(overview.open.items, Vec::<PullRequestSummary>::new());
        let commands = runner.commands.lock().expect("commands lock").clone();
        assert_eq!(
            commands
                .iter()
                .map(|command| command.argv.clone())
                .collect::<Vec<_>>(),
            vec![
                vec!["git", "rev-parse", "--is-inside-work-tree"],
                vec!["gh", "pr", "view", "--json", PR_JSON_FIELDS],
                vec![
                    "gh",
                    "pr",
                    "list",
                    "--state",
                    "open",
                    "--limit",
                    "30",
                    "--json",
                    PR_JSON_FIELDS,
                ],
            ]
        );
        let gh_command = &commands[1];
        for key in [
            "GH_PROMPT_DISABLED",
            "GIT_TERMINAL_PROMPT",
            "GH_PAGER",
            "PAGER",
            "NO_COLOR",
            "CLICOLOR",
            "GH_NO_UPDATE_NOTIFIER",
            "GCM_INTERACTIVE",
        ] {
            assert!(
                gh_command.env.contains_key(key),
                "missing hardened gh env key {key}"
            );
        }
    }

    #[test]
    fn sanitizes_remote_control_characters() {
        assert_eq!(
            sanitize_remote_text("hello\u{001b}[31m\r\nwor\u{202e}ld", 80),
            "hello [31m wor ld"
        );
        assert_eq!(sanitize_remote_text("abcdef", 3), "abc...");
    }

    struct FakeRunner {
        outputs: Mutex<VecDeque<WorkspaceCommandOutput>>,
        commands: Mutex<Vec<WorkspaceCommand>>,
    }

    impl FakeRunner {
        fn new(outputs: Vec<WorkspaceCommandOutput>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into()),
                commands: Mutex::new(Vec::new()),
            }
        }
    }

    impl WorkspaceCommandExecutor for FakeRunner {
        fn run(
            &self,
            command: WorkspaceCommand,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<WorkspaceCommandOutput, WorkspaceCommandError>>
                    + Send
                    + '_,
            >,
        > {
            self.commands.lock().expect("commands lock").push(command);
            let output = self
                .outputs
                .lock()
                .expect("outputs lock")
                .pop_front()
                .expect("fake output");
            Box::pin(async move { Ok(output) })
        }
    }

    fn ok_output(stdout: &str) -> WorkspaceCommandOutput {
        WorkspaceCommandOutput {
            exit_code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }
}
