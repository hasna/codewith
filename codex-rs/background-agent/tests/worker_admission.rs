use codex_background_agent::process_lifecycle::WorkerProcessCommand;
use codex_background_agent::worker_admission::CONVERSATIONS_AGENT_ID_ENV;
use codex_background_agent::worker_admission::WorkerAdmission;
use codex_background_agent::worker_admission::WorkerAdmissionCommand;
use codex_background_agent::worker_admission::WorkerAdmissionCommandOutput;
use codex_background_agent::worker_admission::WorkerAdmissionCommandRunner;
use codex_background_agent::worker_admission::WorkerAdmissionInput;
use codex_background_agent::worker_admission::WorkerAdmissionPrograms;
use codex_background_agent::worker_admission::apply_worker_identity;
use codex_background_agent::worker_admission::revalidate_worker_admission;
use codex_background_agent::worker_admission::verify_worker_admission;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

const WORKER: &str = "worker-one";
const PARENT: &str = "parent-one";
const TASK_ID: &str = "85553dae-7c24-4777-a281-b335f24b74ef";
const ARTIFACT_TYPE: &str = "git-branch";
const ARTIFACT_ID: &str = "github:hasna/codewith:branch:feature";
const WORKER_TODOS_ID: &str = "worker-todos-id";
const PARENT_TODOS_ID: &str = "parent-todos-id";

#[derive(Debug)]
struct ExpectedCommand {
    program: &'static str,
    args: Vec<String>,
    output: WorkerAdmissionCommandOutput,
}

#[derive(Debug, Default)]
struct ScriptedRunner {
    expected: Mutex<VecDeque<ExpectedCommand>>,
}

impl ScriptedRunner {
    fn new(expected: Vec<ExpectedCommand>) -> Self {
        Self {
            expected: Mutex::new(expected.into()),
        }
    }

    fn assert_exhausted(&self) {
        assert_eq!(
            self.expected.lock().expect("runner lock").len(),
            0,
            "all expected direct-argv commands should be consumed"
        );
    }
}

impl WorkerAdmissionCommandRunner for ScriptedRunner {
    fn run(
        &self,
        command: WorkerAdmissionCommand,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<WorkerAdmissionCommandOutput>> + Send + '_>>
    {
        Box::pin(async move {
            let expected = self
                .expected
                .lock()
                .expect("runner lock")
                .pop_front()
                .expect("unexpected worker-admission command");
            assert_eq!(
                command.program.to_string_lossy(),
                expected.program,
                "program must be invoked directly"
            );
            assert_eq!(
                command
                    .args
                    .iter()
                    .map(|arg| arg.to_string_lossy().to_string())
                    .collect::<Vec<_>>(),
                expected.args,
                "argv must preserve field boundaries without shell interpolation"
            );
            assert_eq!(command.env, Vec::<(OsString, OsString)>::new());
            Ok(expected.output)
        })
    }
}

fn output(exit_code: i32, stdout: Value) -> WorkerAdmissionCommandOutput {
    WorkerAdmissionCommandOutput {
        exit_code,
        stdout: serde_json::to_vec(&stdout).expect("serialize command output"),
        stderr: Vec::new(),
    }
}

fn failed_output(stderr: &str) -> WorkerAdmissionCommandOutput {
    WorkerAdmissionCommandOutput {
        exit_code: 1,
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

fn input() -> WorkerAdmissionInput {
    WorkerAdmissionInput {
        worker: Some(WORKER.to_string()),
        parent: Some(PARENT.to_string()),
        task_id: Some(TASK_ID.to_string()),
        artifact_type: Some(ARTIFACT_TYPE.to_string()),
        artifact_id: Some(ARTIFACT_ID.to_string()),
    }
}

fn request() -> codex_background_agent::worker_admission::WorkerAdmissionRequest {
    input()
        .into_request()
        .expect("complete worker admission")
        .expect("worker admission enabled")
}

fn identity_command(exit_code: i32) -> ExpectedCommand {
    ExpectedCommand {
        program: "identities",
        args: vec![
            "--json".to_string(),
            "show".to_string(),
            format!("agent:{WORKER}"),
        ],
        output: if exit_code == 0 {
            output(
                0,
                json!({
                    "id": "worker-identities-id",
                    "fullName": WORKER,
                    "uniqueIdentifier": {
                        "scheme": "agent",
                        "value": WORKER
                    }
                }),
            )
        } else {
            failed_output("identity not found")
        },
    }
}

fn todos_agent_command(name: &str, id: &str, reports_to: Option<&str>) -> ExpectedCommand {
    ExpectedCommand {
        program: "todos",
        args: vec!["--json".to_string(), "agent".to_string(), name.to_string()],
        output: output(
            0,
            json!({
                "agent": {
                    "id": id,
                    "name": name,
                    "status": "active",
                    "reports_to": reports_to
                },
                "tasks": {},
                "all_tasks": []
            }),
        ),
    }
}

fn task_command(assigned_to: &str, status: &str) -> ExpectedCommand {
    ExpectedCommand {
        program: "todos",
        args: vec![
            "--json".to_string(),
            "show".to_string(),
            TASK_ID.to_string(),
        ],
        output: output(
            0,
            json!({
                "id": TASK_ID,
                "status": status,
                "assigned_to": assigned_to
            }),
        ),
    }
}

fn roster_command(cursor: usize, agents: Value) -> ExpectedCommand {
    ExpectedCommand {
        program: "conversations",
        args: vec![
            "agents".to_string(),
            "list".to_string(),
            "--json".to_string(),
            "--limit".to_string(),
            "100".to_string(),
            "--cursor".to_string(),
            cursor.to_string(),
        ],
        output: output(0, agents),
    }
}

fn whoami_command(agent: &str) -> ExpectedCommand {
    ExpectedCommand {
        program: "conversations",
        args: vec!["whoami".to_string(), "--json".to_string()],
        output: output(0, json!({"agent": agent, "source": "env var"})),
    }
}

fn lock_command(
    resource_type: &str,
    resource_id: &str,
    locked: bool,
    holder: &str,
) -> ExpectedCommand {
    ExpectedCommand {
        program: "conversations",
        args: vec![
            "locks".to_string(),
            "check".to_string(),
            resource_id.to_string(),
            "--type".to_string(),
            resource_type.to_string(),
            "--json".to_string(),
        ],
        output: output(
            if locked { 2 } else { 0 },
            json!({
                "locked": locked,
                "resource_type": resource_type,
                "resource_id": resource_id,
                "agent_id": holder,
                "lock_type": "exclusive"
            }),
        ),
    }
}

fn valid_commands(roster_pages: Vec<Value>) -> Vec<ExpectedCommand> {
    let mut commands = vec![
        identity_command(0),
        todos_agent_command(WORKER, WORKER_TODOS_ID, Some(PARENT_TODOS_ID)),
        todos_agent_command(PARENT, PARENT_TODOS_ID, None),
        task_command(PARENT, "in_progress"),
    ];
    commands.extend(
        roster_pages
            .into_iter()
            .enumerate()
            .map(|(page, agents)| roster_command(page * 100, agents)),
    );
    commands.extend([
        whoami_command(PARENT),
        lock_command(ARTIFACT_TYPE, ARTIFACT_ID, true, WORKER),
    ]);
    commands
}

fn one_page_roster() -> Vec<Value> {
    vec![json!([
        {"id": "parent-conversations-id", "agent": PARENT},
        {"id": "worker-conversations-id", "agent": WORKER}
    ])]
}

#[test]
fn worker_admission_is_disabled_only_when_every_field_is_omitted() {
    assert_eq!(
        WorkerAdmissionInput::default()
            .into_request()
            .expect("omitted worker admission"),
        None
    );
}

#[test]
fn worker_admission_refuses_each_missing_identity_or_artifact_field() {
    let missing_fields = [
        (
            "worker",
            WorkerAdmissionInput {
                worker: None,
                ..input()
            },
        ),
        (
            "parent",
            WorkerAdmissionInput {
                parent: None,
                ..input()
            },
        ),
        (
            "task-id",
            WorkerAdmissionInput {
                task_id: None,
                ..input()
            },
        ),
        (
            "artifact-type",
            WorkerAdmissionInput {
                artifact_type: None,
                ..input()
            },
        ),
        (
            "artifact-id",
            WorkerAdmissionInput {
                artifact_id: None,
                ..input()
            },
        ),
    ];
    for (field, candidate) in missing_fields {
        let error = candidate
            .into_request()
            .expect_err("partial worker admission must fail closed");
        assert!(
            error.to_string().contains(field),
            "missing {field} error should name the field: {error:#}"
        );
    }
}

#[tokio::test]
async fn unregistered_worker_identity_is_rejected_before_other_queries() {
    let runner = ScriptedRunner::new(vec![identity_command(1)]);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("unregistered identity must fail");

    assert!(error.to_string().contains("not registered in Identities"));
    runner.assert_exhausted();
}

#[tokio::test]
async fn worker_without_parent_lineage_is_rejected() {
    let runner = ScriptedRunner::new(vec![
        identity_command(0),
        todos_agent_command(WORKER, WORKER_TODOS_ID, None),
        todos_agent_command(PARENT, PARENT_TODOS_ID, None),
        task_command(PARENT, "in_progress"),
    ]);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("missing lineage must fail");

    assert!(error.to_string().contains("reports_to"));
    runner.assert_exhausted();
}

#[tokio::test]
async fn task_assigned_to_a_different_parent_is_rejected() {
    let runner = ScriptedRunner::new(vec![
        identity_command(0),
        todos_agent_command(WORKER, WORKER_TODOS_ID, Some(PARENT_TODOS_ID)),
        todos_agent_command(PARENT, PARENT_TODOS_ID, None),
        task_command("another-parent", "in_progress"),
    ]);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("task-parent mismatch must fail");

    assert!(error.to_string().contains("assigned_to"));
    runner.assert_exhausted();
}

#[tokio::test]
async fn effective_conversations_parent_mismatch_is_rejected() {
    let mut commands = valid_commands(one_page_roster());
    commands[5] = whoami_command("another-parent");
    let runner = ScriptedRunner::new(commands);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("actual parent mismatch must fail");

    assert!(
        error
            .to_string()
            .contains("effective Conversations identity")
    );
    runner.assert_exhausted();
}

#[tokio::test]
async fn missing_artifact_lock_is_rejected() {
    let mut commands = valid_commands(one_page_roster());
    commands[6] = lock_command(ARTIFACT_TYPE, ARTIFACT_ID, false, "");
    let runner = ScriptedRunner::new(commands);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("missing artifact lock must fail");

    assert!(error.to_string().contains("is not actively locked"));
    runner.assert_exhausted();
}

#[tokio::test]
async fn lock_in_a_different_resource_namespace_does_not_satisfy_admission() {
    let mut commands = valid_commands(one_page_roster());
    commands[6] = lock_command(ARTIFACT_TYPE, ARTIFACT_ID, false, "");
    let runner = ScriptedRunner::new(commands);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("wrong lock namespace must fail");

    assert!(error.to_string().contains(ARTIFACT_TYPE));
    assert!(error.to_string().contains(ARTIFACT_ID));
    runner.assert_exhausted();
}

#[tokio::test]
async fn worker_beyond_first_roster_page_is_registered() {
    let first_page = (0..100)
        .map(|index| json!({"id": format!("other-{index}"), "agent": format!("other-{index}")}))
        .collect::<Vec<_>>();
    let second_page = json!([
        {"id": "worker-conversations-id", "agent": WORKER}
    ]);
    let runner = ScriptedRunner::new(valid_commands(vec![json!(first_page), second_page]));

    let admission =
        verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
            .await
            .expect("worker on the second page must pass");

    assert_eq!(admission.evidence.roster_pages_scanned, 2);
    assert_eq!(
        admission.evidence.conversations_worker_id,
        "worker-conversations-id"
    );
    runner.assert_exhausted();
}

#[tokio::test]
async fn roster_page_error_fails_closed() {
    let first_page = (0..100)
        .map(|index| json!({"id": format!("other-{index}"), "agent": format!("other-{index}")}))
        .collect::<Vec<_>>();
    let mut commands = vec![
        identity_command(0),
        todos_agent_command(WORKER, WORKER_TODOS_ID, Some(PARENT_TODOS_ID)),
        todos_agent_command(PARENT, PARENT_TODOS_ID, None),
        task_command(PARENT, "in_progress"),
        roster_command(0, json!(first_page)),
    ];
    commands.push(ExpectedCommand {
        program: "conversations",
        args: vec![
            "agents".to_string(),
            "list".to_string(),
            "--json".to_string(),
            "--limit".to_string(),
            "100".to_string(),
            "--cursor".to_string(),
            "100".to_string(),
        ],
        output: failed_output("registry unavailable"),
    });
    let runner = ScriptedRunner::new(commands);

    let error = verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
        .await
        .expect_err("roster page failure must fail closed");

    assert!(error.to_string().contains("cursor 100"));
    runner.assert_exhausted();
}

#[tokio::test]
async fn complete_worker_admission_persists_structured_evidence() {
    let runner = ScriptedRunner::new(valid_commands(one_page_roster()));

    let admission =
        verify_worker_admission(&runner, &WorkerAdmissionPrograms::default(), &request())
            .await
            .expect("complete worker admission");

    assert_eq!(
        admission,
        WorkerAdmission {
            worker: WORKER.to_string(),
            parent: PARENT.to_string(),
            task_id: TASK_ID.to_string(),
            artifact_type: ARTIFACT_TYPE.to_string(),
            artifact_id: ARTIFACT_ID.to_string(),
            task_assignee: PARENT.to_string(),
            worker_reports_to: PARENT_TODOS_ID.to_string(),
            evidence: codex_background_agent::worker_admission::WorkerAdmissionEvidence {
                identities_worker_id: "worker-identities-id".to_string(),
                todos_worker_id: WORKER_TODOS_ID.to_string(),
                todos_parent_id: PARENT_TODOS_ID.to_string(),
                conversations_worker_id: "worker-conversations-id".to_string(),
                effective_parent: PARENT.to_string(),
                lock_holder: WORKER.to_string(),
                roster_pages_scanned: 1,
            },
        }
    );
    assert_eq!(
        serde_json::from_value::<WorkerAdmission>(
            serde_json::to_value(&admission).expect("serialize admission")
        )
        .expect("deserialize admission"),
        admission
    );
    runner.assert_exhausted();
}

#[tokio::test]
async fn pre_spawn_revalidation_detects_a_lock_lost_after_admission() {
    let admission_runner = ScriptedRunner::new(valid_commands(one_page_roster()));
    let admission = verify_worker_admission(
        &admission_runner,
        &WorkerAdmissionPrograms::default(),
        &request(),
    )
    .await
    .expect("initial admission");
    admission_runner.assert_exhausted();

    let mut revalidation_commands = valid_commands(one_page_roster());
    revalidation_commands[6] = lock_command(ARTIFACT_TYPE, ARTIFACT_ID, false, "");
    let revalidation_runner = ScriptedRunner::new(revalidation_commands);
    let error = revalidate_worker_admission(
        &revalidation_runner,
        &WorkerAdmissionPrograms::default(),
        &admission,
    )
    .await
    .expect_err("lost lock must prevent spawn");

    assert!(error.to_string().contains("is not actively locked"));
    revalidation_runner.assert_exhausted();
}

#[test]
fn worker_process_command_sets_explicit_conversations_identity() {
    let admission = WorkerAdmission {
        worker: WORKER.to_string(),
        parent: PARENT.to_string(),
        task_id: TASK_ID.to_string(),
        artifact_type: ARTIFACT_TYPE.to_string(),
        artifact_id: ARTIFACT_ID.to_string(),
        task_assignee: PARENT.to_string(),
        worker_reports_to: PARENT_TODOS_ID.to_string(),
        evidence: codex_background_agent::worker_admission::WorkerAdmissionEvidence {
            identities_worker_id: "worker-identities-id".to_string(),
            todos_worker_id: WORKER_TODOS_ID.to_string(),
            todos_parent_id: PARENT_TODOS_ID.to_string(),
            conversations_worker_id: "worker-conversations-id".to_string(),
            effective_parent: PARENT.to_string(),
            lock_holder: WORKER.to_string(),
            roster_pages_scanned: 1,
        },
    };
    let command = apply_worker_identity(
        WorkerProcessCommand::new("codewith", "worker.stderr.log"),
        &admission,
    );

    assert_eq!(
        command.env,
        vec![(
            OsString::from(CONVERSATIONS_AGENT_ID_ENV),
            OsString::from(WORKER)
        )]
    );
}
