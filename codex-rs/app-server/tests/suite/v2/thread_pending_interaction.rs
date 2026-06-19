use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadPendingInteractionKind;
use codex_app_server_protocol::ThreadPendingInteractionListResponse;
use codex_app_server_protocol::ThreadPendingInteractionReadResponse;
use codex_app_server_protocol::ThreadPendingInteractionRespondResponse;
use codex_app_server_protocol::ThreadPendingInteractionStatus;
use codex_protocol::ThreadId;
use codex_state::StateRuntime;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const THREAD_ID: &str = "00000000-0000-4000-8000-000000000101";
const OTHER_THREAD_ID: &str = "00000000-0000-4000-8000-000000000102";

#[tokio::test]
async fn thread_pending_interaction_jsonrpc_lists_reads_and_responds() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let state_db =
        StateRuntime::init(codex_home.path().to_path_buf(), "test-provider".to_string()).await?;
    upsert_thread(&state_db, THREAD_ID, "pending interaction thread").await?;
    upsert_thread(
        &state_db,
        OTHER_THREAD_ID,
        "other pending interaction thread",
    )
    .await?;
    create_user_input_interaction(&state_db, "interaction-jsonrpc").await?;

    let page: ThreadPendingInteractionListResponse = send_and_read(
        &mut mcp,
        "thread/pendingInteraction/list",
        json!({
            "threadId": THREAD_ID,
            "statuses": ["pending"],
            "kinds": ["userInput"],
            "cursor": null,
            "limit": 10,
        }),
    )
    .await?;
    assert_eq!(None, page.next_cursor);
    assert_eq!(1, page.data.len());
    assert_eq!("interaction-jsonrpc", page.data[0].interaction_id);
    assert_eq!(ThreadPendingInteractionKind::UserInput, page.data[0].kind);
    assert_eq!(ThreadPendingInteractionStatus::Pending, page.data[0].status);

    let read: ThreadPendingInteractionReadResponse = send_and_read(
        &mut mcp,
        "thread/pendingInteraction/read",
        json!({
            "interactionId": "interaction-jsonrpc",
            "threadId": THREAD_ID,
        }),
    )
    .await?;
    assert_eq!("interaction-jsonrpc", read.interaction.interaction_id);
    assert_eq!(1, read.events.len());
    assert_eq!(
        ThreadPendingInteractionStatus::Pending,
        read.events[0].status
    );

    let wrong_thread_id = mcp
        .send_raw_request(
            "thread/pendingInteraction/read",
            Some(json!({
                "interactionId": "interaction-jsonrpc",
                "threadId": OTHER_THREAD_ID,
            })),
        )
        .await?;
    let wrong_thread = read_error(&mut mcp, wrong_thread_id).await?;
    assert_eq!(-32600, wrong_thread.error.code);
    assert!(
        wrong_thread
            .error
            .message
            .contains("pending interaction not found")
    );

    let responded: ThreadPendingInteractionRespondResponse = send_and_read(
        &mut mcp,
        "thread/pendingInteraction/respond",
        json!({
            "interactionId": "interaction-jsonrpc",
            "threadId": THREAD_ID,
            "terminalStatus": "responded",
            "response": {
                "type": "requestUserInput",
                "answers": {
                    "decision": {
                        "answers": ["ship it"],
                    },
                },
            },
        }),
    )
    .await?;
    assert_eq!(true, responded.updated);
    let interaction = responded
        .interaction
        .expect("responded interaction should be returned");
    assert_eq!(
        ThreadPendingInteractionStatus::NoLongerWaiting,
        interaction.status
    );
    assert_eq!(
        Some("1 user input answer(s)".to_string()),
        interaction.response_payload_preview
    );
    assert_eq!(
        vec!["responsePayload".to_string()],
        interaction.response_redactions
    );

    Ok(())
}

async fn upsert_thread(state_db: &StateRuntime, thread_id: &str, preview: &str) -> Result<()> {
    let thread_id = ThreadId::from_string(thread_id)?;
    let now = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
        .ok_or_else(|| anyhow::anyhow!("timestamp should parse"))?;
    let codex_home = state_db.codex_home();
    state_db
        .upsert_thread(&codex_state::ThreadMetadata {
            id: thread_id,
            rollout_path: codex_home.join(format!("rollout-{thread_id}.jsonl")),
            created_at: now,
            updated_at: now,
            source: "cli".to_string(),
            thread_source: None,
            agent_nickname: None,
            agent_role: None,
            agent_path: None,
            model_provider: "test-provider".to_string(),
            model: Some("gpt-5".to_string()),
            reasoning_effort: None,
            cwd: codex_home.join("workspace"),
            cli_version: "0.0.0".to_string(),
            title: String::new(),
            preview: Some(preview.to_string()),
            sandbox_policy: "read-only".to_string(),
            approval_mode: "on-request".to_string(),
            tokens_used: 0,
            first_user_message: Some(preview.to_string()),
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
        })
        .await?;
    Ok(())
}

async fn create_user_input_interaction(
    state_db: &StateRuntime,
    interaction_id: &str,
) -> Result<()> {
    state_db
        .create_thread_pending_interaction(&codex_state::PendingInteractionCreateParams {
            interaction_id: interaction_id.to_string(),
            thread_id: ThreadId::from_string(THREAD_ID)?,
            source_kind: codex_state::PendingInteractionSourceKind::Thread,
            source_id: None,
            turn_id: Some("turn-1".to_string()),
            worker_request_id: Some(interaction_id.to_string()),
            server_request_id_json: None,
            kind: codex_state::PendingInteractionKind::UserInput,
            request_payload_json: json!({
                "type": "requestUserInput",
                "questions": [{
                    "id": "decision",
                    "question": "Proceed?",
                }],
            }),
            request_payload_preview: "Proceed?".to_string(),
            request_redactions_json: json!([]),
            no_client_policy: "record-and-wait-for-coordinator".to_string(),
            timeout_at: None,
        })
        .await?;
    Ok(())
}

async fn send_and_read<T>(
    mcp: &mut TestAppServer,
    method: &str,
    params: serde_json::Value,
) -> Result<T>
where
    T: DeserializeOwned,
{
    let request_id = mcp.send_raw_request(method, Some(params)).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(response)
}

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCError> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}
