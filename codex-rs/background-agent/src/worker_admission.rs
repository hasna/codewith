use crate::process_lifecycle::WorkerProcessCommand;
use anyhow::Context;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;

pub const CONVERSATIONS_AGENT_ID_ENV: &str = "CONVERSATIONS_AGENT_ID";
pub const WORKER_ADMISSION_SNAPSHOT_FIELD: &str = "workerAdmission";
const ROSTER_PAGE_SIZE: usize = 100;
const MAX_ROSTER_PAGES: usize = 1_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkerAdmissionInput {
    pub worker: Option<String>,
    pub parent: Option<String>,
    pub task_id: Option<String>,
    pub artifact_type: Option<String>,
    pub artifact_id: Option<String>,
}

impl WorkerAdmissionInput {
    pub fn into_request(self) -> anyhow::Result<Option<WorkerAdmissionRequest>> {
        if self == Self::default() {
            return Ok(None);
        }
        Ok(Some(WorkerAdmissionRequest {
            worker: required_field(self.worker, "worker")?,
            parent: required_field(self.parent, "parent")?,
            task_id: required_field(self.task_id, "task-id")?,
            artifact_type: required_field(self.artifact_type, "artifact-type")?,
            artifact_id: required_field(self.artifact_id, "artifact-id")?,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAdmissionRequest {
    pub worker: String,
    pub parent: String,
    pub task_id: String,
    pub artifact_type: String,
    pub artifact_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAdmission {
    pub worker: String,
    pub parent: String,
    pub task_id: String,
    pub artifact_type: String,
    pub artifact_id: String,
    pub task_assignee: String,
    pub worker_reports_to: String,
    pub evidence: WorkerAdmissionEvidence,
}

impl WorkerAdmission {
    pub fn request(&self) -> WorkerAdmissionRequest {
        WorkerAdmissionRequest {
            worker: self.worker.clone(),
            parent: self.parent.clone(),
            task_id: self.task_id.clone(),
            artifact_type: self.artifact_type.clone(),
            artifact_id: self.artifact_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAdmissionEvidence {
    pub identities_worker_id: String,
    pub todos_worker_id: String,
    pub todos_parent_id: String,
    pub conversations_worker_id: String,
    pub effective_parent: String,
    pub lock_holder: String,
    pub roster_pages_scanned: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAdmissionPrograms {
    pub identities: PathBuf,
    pub todos: PathBuf,
    pub conversations: PathBuf,
}

impl Default for WorkerAdmissionPrograms {
    fn default() -> Self {
        Self {
            identities: PathBuf::from("identities"),
            todos: PathBuf::from("todos"),
            conversations: PathBuf::from("conversations"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAdmissionCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerAdmissionCommandOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs one worker-admission dependency command without a shell.
///
/// Implementations must preserve argv and environment field boundaries and
/// return stdout separately from stderr so JSON parsing never consumes a human
/// pagination footer.
pub trait WorkerAdmissionCommandRunner {
    fn run(
        &self,
        command: WorkerAdmissionCommand,
    ) -> impl Future<Output = anyhow::Result<WorkerAdmissionCommandOutput>> + Send;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessWorkerAdmissionCommandRunner;

impl WorkerAdmissionCommandRunner for ProcessWorkerAdmissionCommandRunner {
    async fn run(
        &self,
        command: WorkerAdmissionCommand,
    ) -> anyhow::Result<WorkerAdmissionCommandOutput> {
        let output = Command::new(&command.program)
            .args(&command.args)
            .envs(command.env)
            .stdin(Stdio::null())
            .output()
            .await
            .with_context(|| {
                format!(
                    "failed to execute worker-admission dependency {}",
                    command.program.display()
                )
            })?;
        Ok(WorkerAdmissionCommandOutput {
            exit_code: output.status.code().unwrap_or(1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

pub async fn verify_worker_admission(
    runner: &impl WorkerAdmissionCommandRunner,
    programs: &WorkerAdmissionPrograms,
    request: &WorkerAdmissionRequest,
) -> anyhow::Result<WorkerAdmission> {
    verify_worker_admission_inner(
        runner,
        programs,
        request,
        EffectiveParentCheck::CurrentProcess,
    )
    .await
}

pub async fn revalidate_worker_admission(
    runner: &impl WorkerAdmissionCommandRunner,
    programs: &WorkerAdmissionPrograms,
    admitted: &WorkerAdmission,
) -> anyhow::Result<WorkerAdmission> {
    let current = verify_worker_admission_inner(
        runner,
        programs,
        &admitted.request(),
        EffectiveParentCheck::Persisted {
            identity: admitted.evidence.effective_parent.as_str(),
        },
    )
    .await?;
    ensure_stable_evidence(admitted, &current)?;
    Ok(current)
}

pub fn worker_admission_from_snapshot(payload: &Value) -> anyhow::Result<Option<WorkerAdmission>> {
    payload
        .get(WORKER_ADMISSION_SNAPSHOT_FIELD)
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("invalid persisted worker-admission evidence")
}

pub fn apply_worker_identity(
    command: WorkerProcessCommand,
    admission: &WorkerAdmission,
) -> WorkerProcessCommand {
    command.env(CONVERSATIONS_AGENT_ID_ENV, admission.worker.as_str())
}

enum EffectiveParentCheck<'a> {
    CurrentProcess,
    Persisted { identity: &'a str },
}

async fn verify_worker_admission_inner(
    runner: &impl WorkerAdmissionCommandRunner,
    programs: &WorkerAdmissionPrograms,
    request: &WorkerAdmissionRequest,
    effective_parent_check: EffectiveParentCheck<'_>,
) -> anyhow::Result<WorkerAdmission> {
    validate_request(request)?;
    let identities_worker = run_json(
        runner,
        WorkerAdmissionCommand {
            program: programs.identities.clone(),
            args: argv([
                "--json",
                "show",
                format!("agent:{}", request.worker).as_str(),
            ]),
            env: Vec::new(),
        },
        &[0],
        "read worker identity from Identities",
    )
    .await
    .with_context(|| {
        format!(
            "worker `{}` is not registered in Identities",
            request.worker
        )
    })?;
    let identities_worker_id = identities_worker
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("worker Identities record has no id")?
        .to_string();
    let identifier_matches = identities_worker
        .pointer("/uniqueIdentifier/value")
        .and_then(Value::as_str)
        .is_some_and(|value| same_identity(value, request.worker.as_str()));
    if !identifier_matches {
        anyhow::bail!(
            "worker Identities record does not resolve exact agent identity `{}`",
            request.worker
        );
    }

    let worker_agent = read_todos_agent(runner, programs, request.worker.as_str()).await?;
    let parent_agent = read_todos_agent(runner, programs, request.parent.as_str()).await?;
    let todos_worker_id = required_json_string(&worker_agent, "id", "worker Todos agent")?;
    let todos_parent_id = required_json_string(&parent_agent, "id", "parent Todos agent")?;
    let worker_name = required_json_string(&worker_agent, "name", "worker Todos agent")?;
    let parent_name = required_json_string(&parent_agent, "name", "parent Todos agent")?;
    if !same_identity(worker_name.as_str(), request.worker.as_str()) {
        anyhow::bail!(
            "Todos worker record resolved `{worker_name}` instead of `{}`",
            request.worker
        );
    }
    if !same_identity(parent_name.as_str(), request.parent.as_str()) {
        anyhow::bail!(
            "Todos parent record resolved `{parent_name}` instead of `{}`",
            request.parent
        );
    }
    let worker_reports_to =
        required_json_string(&worker_agent, "reports_to", "worker Todos agent")?;

    let task = run_json(
        runner,
        WorkerAdmissionCommand {
            program: programs.todos.clone(),
            args: argv(["--json", "show", request.task_id.as_str()]),
            env: Vec::new(),
        },
        &[0],
        "read declared Todos task",
    )
    .await?;
    let task_id = required_json_string(&task, "id", "Todos task")?;
    if task_id != request.task_id {
        anyhow::bail!(
            "Todos task lookup returned `{task_id}` instead of `{}`",
            request.task_id
        );
    }
    let task_status = required_json_string(&task, "status", "Todos task")?;
    if !matches!(task_status.as_str(), "active" | "in_progress") {
        anyhow::bail!(
            "Todos task `{}` is not active/in_progress (status `{task_status}`)",
            request.task_id
        );
    }
    let task_assignee = required_json_string(&task, "assigned_to", "Todos task")?;
    if !same_identity(task_assignee.as_str(), request.parent.as_str()) {
        anyhow::bail!(
            "Todos task `{}` assigned_to `{task_assignee}` does not match parent `{}`",
            request.task_id,
            request.parent
        );
    }
    if worker_reports_to != todos_parent_id {
        anyhow::bail!(
            "worker `{}` reports_to `{worker_reports_to}` instead of parent Todos id `{todos_parent_id}`",
            request.worker
        );
    }

    let (conversations_worker_id, roster_pages_scanned) =
        find_conversations_worker(runner, programs, request.worker.as_str()).await?;
    let effective_parent = match effective_parent_check {
        EffectiveParentCheck::CurrentProcess => {
            let whoami = run_json(
                runner,
                WorkerAdmissionCommand {
                    program: programs.conversations.clone(),
                    args: argv(["whoami", "--json"]),
                    env: Vec::new(),
                },
                &[0],
                "read effective Conversations identity",
            )
            .await?;
            let effective_parent = required_json_string(&whoami, "agent", "Conversations whoami")?;
            if !same_identity(effective_parent.as_str(), request.parent.as_str()) {
                anyhow::bail!(
                    "effective Conversations identity `{effective_parent}` does not match parent `{}`",
                    request.parent
                );
            }
            effective_parent
        }
        EffectiveParentCheck::Persisted { identity } => identity.to_string(),
    };

    let lock = run_json(
        runner,
        WorkerAdmissionCommand {
            program: programs.conversations.clone(),
            args: argv([
                "locks",
                "check",
                request.artifact_id.as_str(),
                "--type",
                request.artifact_type.as_str(),
                "--json",
            ]),
            env: Vec::new(),
        },
        &[0, 2],
        "check worker artifact lock",
    )
    .await?;
    let locked = lock
        .get("locked")
        .and_then(Value::as_bool)
        .context("Conversations lock check returned no locked boolean")?;
    if !locked {
        anyhow::bail!(
            "artifact {} `{}` is not actively locked",
            request.artifact_type,
            request.artifact_id
        );
    }
    let returned_artifact_type =
        required_json_string(&lock, "resource_type", "Conversations lock")?;
    let returned_artifact_id = required_json_string(&lock, "resource_id", "Conversations lock")?;
    if returned_artifact_type != request.artifact_type
        || returned_artifact_id != request.artifact_id
    {
        anyhow::bail!(
            "Conversations lock evidence names {} `{}` instead of {} `{}`",
            returned_artifact_type,
            returned_artifact_id,
            request.artifact_type,
            request.artifact_id
        );
    }
    let lock_holder = required_json_string(&lock, "agent_id", "Conversations lock")?;
    if !same_identity(lock_holder.as_str(), request.worker.as_str()) {
        anyhow::bail!(
            "artifact {} `{}` is held by `{lock_holder}`, not worker `{}`",
            request.artifact_type,
            request.artifact_id,
            request.worker
        );
    }

    Ok(WorkerAdmission {
        worker: request.worker.clone(),
        parent: request.parent.clone(),
        task_id: request.task_id.clone(),
        artifact_type: request.artifact_type.clone(),
        artifact_id: request.artifact_id.clone(),
        task_assignee,
        worker_reports_to,
        evidence: WorkerAdmissionEvidence {
            identities_worker_id,
            todos_worker_id,
            todos_parent_id,
            conversations_worker_id,
            effective_parent,
            lock_holder,
            roster_pages_scanned,
        },
    })
}

async fn read_todos_agent(
    runner: &impl WorkerAdmissionCommandRunner,
    programs: &WorkerAdmissionPrograms,
    name: &str,
) -> anyhow::Result<Value> {
    let response = run_json(
        runner,
        WorkerAdmissionCommand {
            program: programs.todos.clone(),
            args: argv(["--json", "agent", name]),
            env: Vec::new(),
        },
        &[0],
        format!("read Todos agent `{name}`").as_str(),
    )
    .await?;
    response
        .get("agent")
        .filter(|value| value.is_object())
        .cloned()
        .with_context(|| format!("Todos has no registered agent `{name}`"))
}

async fn find_conversations_worker(
    runner: &impl WorkerAdmissionCommandRunner,
    programs: &WorkerAdmissionPrograms,
    worker: &str,
) -> anyhow::Result<(String, usize)> {
    let mut cursor = 0;
    let mut pages_scanned = 0;
    let mut matches = Vec::new();
    loop {
        if pages_scanned >= MAX_ROSTER_PAGES {
            anyhow::bail!("Conversations roster did not terminate after {MAX_ROSTER_PAGES} pages");
        }
        let page = run_json(
            runner,
            WorkerAdmissionCommand {
                program: programs.conversations.clone(),
                args: argv([
                    "agents",
                    "list",
                    "--json",
                    "--limit",
                    "100",
                    "--cursor",
                    cursor.to_string().as_str(),
                ]),
                env: Vec::new(),
            },
            &[0],
            format!("read Conversations roster page at cursor {cursor}").as_str(),
        )
        .await
        .with_context(|| format!("failed Conversations roster page at cursor {cursor}"))?;
        let agents = page.as_array().with_context(|| {
            format!("Conversations roster page at cursor {cursor} is not a JSON array")
        })?;
        pages_scanned += 1;
        matches.extend(agents.iter().filter_map(|agent| {
            let name = agent.get("agent").and_then(Value::as_str)?;
            if same_identity(name, worker) {
                agent.get("id").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        }));
        if agents.len() < ROSTER_PAGE_SIZE {
            break;
        }
        cursor += ROSTER_PAGE_SIZE;
    }
    match matches.as_slice() {
        [] => anyhow::bail!(
            "worker `{worker}` is not registered in the exhaustive Conversations roster"
        ),
        [id] => Ok((id.clone(), pages_scanned)),
        _ => anyhow::bail!(
            "worker `{worker}` has {} ambiguous Conversations registrations",
            matches.len()
        ),
    }
}

async fn run_json(
    runner: &impl WorkerAdmissionCommandRunner,
    command: WorkerAdmissionCommand,
    accepted_exit_codes: &[i32],
    action: &str,
) -> anyhow::Result<Value> {
    let output = runner.run(command).await?;
    if !accepted_exit_codes.contains(&output.exit_code) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "{action} failed with exit {}: {}",
            output.exit_code,
            stderr.trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{action} returned invalid JSON on stdout"))
}

fn ensure_stable_evidence(
    admitted: &WorkerAdmission,
    current: &WorkerAdmission,
) -> anyhow::Result<()> {
    let admitted_stable = (
        admitted.worker.as_str(),
        admitted.parent.as_str(),
        admitted.task_id.as_str(),
        admitted.artifact_type.as_str(),
        admitted.artifact_id.as_str(),
        admitted.task_assignee.as_str(),
        admitted.worker_reports_to.as_str(),
        admitted.evidence.identities_worker_id.as_str(),
        admitted.evidence.todos_worker_id.as_str(),
        admitted.evidence.todos_parent_id.as_str(),
        admitted.evidence.conversations_worker_id.as_str(),
        admitted.evidence.effective_parent.as_str(),
        admitted.evidence.lock_holder.as_str(),
    );
    let current_stable = (
        current.worker.as_str(),
        current.parent.as_str(),
        current.task_id.as_str(),
        current.artifact_type.as_str(),
        current.artifact_id.as_str(),
        current.task_assignee.as_str(),
        current.worker_reports_to.as_str(),
        current.evidence.identities_worker_id.as_str(),
        current.evidence.todos_worker_id.as_str(),
        current.evidence.todos_parent_id.as_str(),
        current.evidence.conversations_worker_id.as_str(),
        current.evidence.effective_parent.as_str(),
        current.evidence.lock_holder.as_str(),
    );
    if admitted_stable != current_stable {
        anyhow::bail!("worker-admission evidence changed before process spawn");
    }
    Ok(())
}

fn validate_request(request: &WorkerAdmissionRequest) -> anyhow::Result<()> {
    for (name, value) in [
        ("worker", request.worker.as_str()),
        ("parent", request.parent.as_str()),
        ("task-id", request.task_id.as_str()),
        ("artifact-type", request.artifact_type.as_str()),
        ("artifact-id", request.artifact_id.as_str()),
    ] {
        if value.trim().is_empty() {
            anyhow::bail!("worker admission requires non-empty {name}");
        }
    }
    Ok(())
}

fn required_field(value: Option<String>, name: &str) -> anyhow::Result<String> {
    let value = value.with_context(|| format!("worker admission requires --{name}"))?;
    if value.trim().is_empty() {
        anyhow::bail!("worker admission requires non-empty --{name}");
    }
    Ok(value)
}

fn required_json_string(value: &Value, field: &str, record: &str) -> anyhow::Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .with_context(|| format!("{record} has no {field}"))
}

fn same_identity(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
}

fn argv<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}
