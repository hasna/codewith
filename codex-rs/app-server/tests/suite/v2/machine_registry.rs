use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::MachineRegistryDisableResponse;
use codex_app_server_protocol::MachineRegistryForgetResponse;
use codex_app_server_protocol::MachineRegistryListResponse;
use codex_app_server_protocol::MachineRegistryReadResponse;
use codex_app_server_protocol::MachineRegistryRedaction;
use codex_app_server_protocol::MachineRegistryTrustState;
use codex_app_server_protocol::MachineRegistryUpdateTrustResponse;
use codex_app_server_protocol::MachineRegistryUpsertResponse;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn machine_registry_jsonrpc_lifecycle_redacts_endpoint_addresses() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let upsert_id = mcp
        .send_raw_request(
            "machineRegistry/upsert",
            Some(json!({
                "machineId": null,
                "installationId": "install-jsonrpc",
                "displayName": "JSON-RPC machine",
                "capabilities": {
                    "dispatch": true,
                },
                "endpoints": [{
                    "endpointId": null,
                    "transport": "tailscale",
                    "address": "https://spark02.tailnet:1455/secret",
                    "displayAddress": "Tailscale endpoint",
                    "priority": 10,
                    "capabilities": {
                        "appServer": true,
                    },
                    "lastSuccessAt": null,
                    "lastError": null,
                }],
                "lastSeenAt": null,
            })),
        )
        .await?;
    let upsert_response = read_json_response(&mut mcp, upsert_id).await?;
    let serialized = serde_json::to_string(&upsert_response)?;
    assert!(!serialized.contains("spark02.tailnet"));
    assert!(!serialized.contains("/secret"));
    let upsert: MachineRegistryUpsertResponse = to_response(upsert_response)?;
    assert_eq!(
        MachineRegistryTrustState::Untrusted,
        upsert.machine.trust_state
    );
    assert_eq!(1, upsert.machine.endpoints.len());
    assert_eq!(
        vec![MachineRegistryRedaction::EndpointAddress],
        upsert.machine.endpoints[0].redactions
    );
    assert_eq!(
        "Tailscale endpoint",
        upsert.machine.endpoints[0].display_address
    );

    let machine_id = upsert.machine.machine_id;
    let read: MachineRegistryReadResponse = send_and_read(
        &mut mcp,
        "machineRegistry/read",
        json!({
            "machineId": machine_id,
        }),
    )
    .await?;
    assert_eq!(
        Some(machine_id.as_str()),
        read.machine
            .as_ref()
            .map(|machine| machine.machine_id.as_str())
    );

    let trusted: MachineRegistryUpdateTrustResponse = send_and_read(
        &mut mcp,
        "machineRegistry/updateTrust",
        json!({
            "machineId": machine_id,
            "trustState": "trusted",
        }),
    )
    .await?;
    assert_eq!(
        Some(MachineRegistryTrustState::Trusted),
        trusted.machine.map(|machine| machine.trust_state)
    );

    let local_update_id = mcp
        .send_raw_request(
            "machineRegistry/updateTrust",
            Some(json!({
                "machineId": machine_id,
                "trustState": "local",
            })),
        )
        .await?;
    let local_update_error = read_error(&mut mcp, local_update_id).await?;
    assert_eq!(-32600, local_update_error.error.code);
    assert!(
        local_update_error
            .error
            .message
            .contains("cannot assign the local trust state")
    );

    let disabled: MachineRegistryDisableResponse = send_and_read(
        &mut mcp,
        "machineRegistry/disable",
        json!({
            "machineId": machine_id,
        }),
    )
    .await?;
    let disabled = disabled
        .machine
        .expect("disabled machine should be returned");
    assert_eq!(MachineRegistryTrustState::Disabled, disabled.trust_state);
    assert!(disabled.disabled_at.is_some());

    let visible: MachineRegistryListResponse = send_and_read(
        &mut mcp,
        "machineRegistry/list",
        json!({
            "includeDisabled": false,
            "includeForgotten": false,
            "cursor": null,
            "limit": 10,
        }),
    )
    .await?;
    assert_eq!(Vec::<String>::new(), machine_ids(&visible));

    let include_disabled: MachineRegistryListResponse = send_and_read(
        &mut mcp,
        "machineRegistry/list",
        json!({
            "includeDisabled": true,
            "includeForgotten": false,
            "cursor": null,
            "limit": 10,
        }),
    )
    .await?;
    assert_eq!(vec![machine_id.clone()], machine_ids(&include_disabled));

    let forgotten: MachineRegistryForgetResponse = send_and_read(
        &mut mcp,
        "machineRegistry/forget",
        json!({
            "machineId": machine_id,
        }),
    )
    .await?;
    assert!(forgotten.found);

    let include_forgotten: MachineRegistryListResponse = send_and_read(
        &mut mcp,
        "machineRegistry/list",
        json!({
            "includeDisabled": true,
            "includeForgotten": true,
            "cursor": null,
            "limit": 10,
        }),
    )
    .await?;
    assert_eq!(vec![machine_id], machine_ids(&include_forgotten));
    assert!(include_forgotten.data[0].forgotten_at.is_some());

    Ok(())
}

#[tokio::test]
async fn machine_registry_jsonrpc_rejects_adapter_endpoints() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "machineRegistry/upsert",
            Some(json!({
                "machineId": null,
                "installationId": "adapter-install-jsonrpc",
                "displayName": "Adapter machine",
                "capabilities": {},
                "endpoints": [{
                    "endpointId": null,
                    "transport": "adapter",
                    "address": "adapter://spark02",
                    "displayAddress": null,
                    "priority": null,
                    "capabilities": {},
                    "lastSuccessAt": null,
                    "lastError": null,
                }],
                "lastSeenAt": null,
            })),
        )
        .await?;
    let error = read_error(&mut mcp, request_id).await?;
    assert_eq!(-32600, error.error.code);
    assert!(
        error
            .error
            .message
            .contains("does not accept adapter endpoints")
    );

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
    let response = read_json_response(mcp, request_id).await?;
    to_response(response)
}

async fn read_json_response(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCResponse> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await?
}

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCError> {
    timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await?
}

fn machine_ids(response: &MachineRegistryListResponse) -> Vec<String> {
    response
        .data
        .iter()
        .map(|machine| machine.machine_id.clone())
        .collect()
}
