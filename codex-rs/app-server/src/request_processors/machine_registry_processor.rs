use super::sqlite_retry::retry_transient_sqlite_busy;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use chrono::DateTime;
use chrono::Utc;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::MachineRegistryDisableParams;
use codex_app_server_protocol::MachineRegistryDisableResponse;
use codex_app_server_protocol::MachineRegistryEndpoint;
use codex_app_server_protocol::MachineRegistryEndpointTransport;
use codex_app_server_protocol::MachineRegistryEndpointUpsert;
use codex_app_server_protocol::MachineRegistryEnrollmentState;
use codex_app_server_protocol::MachineRegistryForgetParams;
use codex_app_server_protocol::MachineRegistryForgetResponse;
use codex_app_server_protocol::MachineRegistryHealthState;
use codex_app_server_protocol::MachineRegistryListParams;
use codex_app_server_protocol::MachineRegistryListResponse;
use codex_app_server_protocol::MachineRegistryMachine;
use codex_app_server_protocol::MachineRegistryReadParams;
use codex_app_server_protocol::MachineRegistryReadResponse;
use codex_app_server_protocol::MachineRegistryRedaction;
use codex_app_server_protocol::MachineRegistrySourceKind;
use codex_app_server_protocol::MachineRegistryTrustState;
use codex_app_server_protocol::MachineRegistryUpdateTrustParams;
use codex_app_server_protocol::MachineRegistryUpdateTrustResponse;
use codex_app_server_protocol::MachineRegistryUpsertParams;
use codex_app_server_protocol::MachineRegistryUpsertResponse;
use codex_rollout::StateDbHandle;

#[derive(Clone)]
pub(crate) struct MachineRegistryRequestProcessor {
    state_db: Option<StateDbHandle>,
}

impl MachineRegistryRequestProcessor {
    pub(crate) fn new(state_db: Option<StateDbHandle>) -> Self {
        Self { state_db }
    }

    pub(crate) async fn list(
        &self,
        params: MachineRegistryListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let limit = params
            .limit
            .unwrap_or(codex_state::DEFAULT_MACHINE_REGISTRY_LIST_LIMIT);
        if limit == 0 || limit > codex_state::MAX_MACHINE_REGISTRY_LIST_LIMIT {
            return Err(invalid_request(format!(
                "machine registry list limit must be between 1 and {}",
                codex_state::MAX_MACHINE_REGISTRY_LIST_LIMIT
            )));
        }
        let list_params = codex_state::MachineRegistryListParams {
            include_disabled: params.include_disabled,
            include_forgotten: params.include_forgotten,
            cursor: params.cursor,
            limit,
        };
        let page = retry_transient_sqlite_busy("list machine registry", || {
            state_db
                .machine_registry()
                .list_machines(list_params.clone())
        })
        .await
        .map_err(machine_registry_store_error)?;
        Ok(Some(
            MachineRegistryListResponse {
                data: page.data.into_iter().map(api_machine_from_state).collect(),
                next_cursor: page.next_cursor,
            }
            .into(),
        ))
    }

    pub(crate) async fn read(
        &self,
        params: MachineRegistryReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let machine_id = normalize_required_text("machineId", params.machine_id)?;
        let machine = retry_transient_sqlite_busy("read machine registry", || {
            state_db.machine_registry().get_machine(machine_id.as_str())
        })
        .await
        .map_err(|err| internal_error(format!("failed to read machine registry: {err}")))?
        .map(api_machine_from_state);
        Ok(Some(MachineRegistryReadResponse { machine }.into()))
    }

    pub(crate) async fn upsert(
        &self,
        params: MachineRegistryUpsertParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let endpoints = params
            .endpoints
            .into_iter()
            .map(state_endpoint_upsert_from_api)
            .collect::<Result<Vec<_>, _>>()?;
        let upsert_params = codex_state::MachineRegistryUpsertParams {
            machine_id: params.machine_id,
            installation_id: params.installation_id,
            display_name: params.display_name,
            trust_state: codex_state::MachineTrustState::Untrusted,
            enrollment_state: codex_state::MachineEnrollmentState::Manual,
            health_state: codex_state::MachineHealthState::Unknown,
            source_kind: codex_state::MachineSourceKind::Manual,
            adapter_name: None,
            capabilities_json: params.capabilities,
            endpoints,
            last_seen_at: params
                .last_seen_at
                .map(|timestamp| timestamp_to_datetime(timestamp, "lastSeenAt"))
                .transpose()?,
        };
        let machine = state_db
            .machine_registry()
            .upsert_machine(upsert_params)
            .await
            .map_err(machine_registry_store_error)?;
        Ok(Some(
            MachineRegistryUpsertResponse {
                machine: api_machine_from_state(machine),
            }
            .into(),
        ))
    }

    pub(crate) async fn disable(
        &self,
        params: MachineRegistryDisableParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let machine_id = normalize_required_text("machineId", params.machine_id)?;
        let found = state_db
            .machine_registry()
            .disable_machine(machine_id.as_str())
            .await
            .map_err(machine_registry_store_error)?;
        let machine = if found {
            retry_transient_sqlite_busy("read disabled machine registry row", || {
                state_db.machine_registry().get_machine(machine_id.as_str())
            })
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to read disabled machine registry row: {err}"
                ))
            })?
            .map(api_machine_from_state)
        } else {
            None
        };
        Ok(Some(MachineRegistryDisableResponse { machine }.into()))
    }

    pub(crate) async fn update_trust(
        &self,
        params: MachineRegistryUpdateTrustParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        if params.trust_state == MachineRegistryTrustState::Local {
            return Err(invalid_request(
                "machineRegistry/updateTrust cannot assign the local trust state",
            ));
        }
        let state_db = self.state_db()?;
        let machine_id = normalize_required_text("machineId", params.machine_id)?;
        let trust_state = state_trust_from_api(params.trust_state);
        let machine = state_db
            .machine_registry()
            .update_machine_trust(machine_id.as_str(), trust_state)
            .await
            .map_err(machine_registry_store_error)?
            .map(api_machine_from_state);
        Ok(Some(MachineRegistryUpdateTrustResponse { machine }.into()))
    }

    pub(crate) async fn forget(
        &self,
        params: MachineRegistryForgetParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let state_db = self.state_db()?;
        let machine_id = normalize_required_text("machineId", params.machine_id)?;
        let found = state_db
            .machine_registry()
            .forget_machine(machine_id.as_str())
            .await
            .map_err(machine_registry_store_error)?;
        Ok(Some(MachineRegistryForgetResponse { found }.into()))
    }

    fn state_db(&self) -> Result<StateDbHandle, JSONRPCErrorError> {
        self.state_db
            .clone()
            .ok_or_else(|| internal_error("machine registry state store is unavailable"))
    }
}

fn state_endpoint_upsert_from_api(
    endpoint: MachineRegistryEndpointUpsert,
) -> Result<codex_state::MachineEndpointUpsertParams, JSONRPCErrorError> {
    if endpoint.transport == MachineRegistryEndpointTransport::Adapter {
        return Err(invalid_request(
            "machineRegistry/upsert does not accept adapter endpoints",
        ));
    }
    Ok(codex_state::MachineEndpointUpsertParams {
        endpoint_id: endpoint.endpoint_id,
        transport: state_transport_from_api(endpoint.transport),
        address: normalize_required_text("endpoint address", endpoint.address)?,
        display_address: endpoint.display_address,
        priority: endpoint.priority.unwrap_or(0),
        capabilities_json: endpoint.capabilities,
        last_success_at: endpoint
            .last_success_at
            .map(|timestamp| timestamp_to_datetime(timestamp, "lastSuccessAt"))
            .transpose()?,
        last_error: endpoint.last_error,
    })
}

fn api_machine_from_state(machine: codex_state::MachineRecord) -> MachineRegistryMachine {
    MachineRegistryMachine {
        machine_id: machine.machine_id,
        installation_id: machine.installation_id,
        display_name: machine.display_name,
        trust_state: api_trust_from_state(machine.trust_state),
        enrollment_state: api_enrollment_from_state(machine.enrollment_state),
        health_state: api_health_from_state(machine.health_state),
        source_kind: api_source_from_state(machine.source_kind),
        adapter_name: machine.adapter_name,
        capabilities: machine.capabilities_json,
        last_seen_at: machine.last_seen_at.map(|timestamp| timestamp.timestamp()),
        disabled_at: machine.disabled_at.map(|timestamp| timestamp.timestamp()),
        forgotten_at: machine.forgotten_at.map(|timestamp| timestamp.timestamp()),
        created_at: machine.created_at.timestamp(),
        updated_at: machine.updated_at.timestamp(),
        endpoints: machine
            .endpoints
            .into_iter()
            .map(api_endpoint_from_state)
            .collect(),
    }
}

fn api_endpoint_from_state(endpoint: codex_state::MachineEndpoint) -> MachineRegistryEndpoint {
    MachineRegistryEndpoint {
        endpoint_id: endpoint.endpoint_id,
        machine_id: endpoint.machine_id,
        transport: api_transport_from_state(endpoint.transport),
        display_address: endpoint.display_address,
        redactions: vec![MachineRegistryRedaction::EndpointAddress],
        priority: endpoint.priority,
        capabilities: endpoint.capabilities_json,
        last_success_at: endpoint
            .last_success_at
            .map(|timestamp| timestamp.timestamp()),
        last_error: endpoint.last_error,
        created_at: endpoint.created_at.timestamp(),
        updated_at: endpoint.updated_at.timestamp(),
    }
}

fn state_trust_from_api(value: MachineRegistryTrustState) -> codex_state::MachineTrustState {
    match value {
        MachineRegistryTrustState::Local => codex_state::MachineTrustState::Local,
        MachineRegistryTrustState::Trusted => codex_state::MachineTrustState::Trusted,
        MachineRegistryTrustState::Untrusted => codex_state::MachineTrustState::Untrusted,
        MachineRegistryTrustState::Disabled => codex_state::MachineTrustState::Disabled,
        MachineRegistryTrustState::Revoked => codex_state::MachineTrustState::Revoked,
    }
}

fn api_trust_from_state(value: codex_state::MachineTrustState) -> MachineRegistryTrustState {
    match value {
        codex_state::MachineTrustState::Local => MachineRegistryTrustState::Local,
        codex_state::MachineTrustState::Trusted => MachineRegistryTrustState::Trusted,
        codex_state::MachineTrustState::Untrusted => MachineRegistryTrustState::Untrusted,
        codex_state::MachineTrustState::Disabled => MachineRegistryTrustState::Disabled,
        codex_state::MachineTrustState::Revoked => MachineRegistryTrustState::Revoked,
    }
}

fn api_enrollment_from_state(
    value: codex_state::MachineEnrollmentState,
) -> MachineRegistryEnrollmentState {
    match value {
        codex_state::MachineEnrollmentState::Local => MachineRegistryEnrollmentState::Local,
        codex_state::MachineEnrollmentState::Manual => MachineRegistryEnrollmentState::Manual,
        codex_state::MachineEnrollmentState::Discovered => {
            MachineRegistryEnrollmentState::Discovered
        }
        codex_state::MachineEnrollmentState::Enrolled => MachineRegistryEnrollmentState::Enrolled,
    }
}

fn api_health_from_state(value: codex_state::MachineHealthState) -> MachineRegistryHealthState {
    match value {
        codex_state::MachineHealthState::Unknown => MachineRegistryHealthState::Unknown,
        codex_state::MachineHealthState::Online => MachineRegistryHealthState::Online,
        codex_state::MachineHealthState::Offline => MachineRegistryHealthState::Offline,
        codex_state::MachineHealthState::Degraded => MachineRegistryHealthState::Degraded,
    }
}

fn api_source_from_state(value: codex_state::MachineSourceKind) -> MachineRegistrySourceKind {
    match value {
        codex_state::MachineSourceKind::Local => MachineRegistrySourceKind::Local,
        codex_state::MachineSourceKind::Manual => MachineRegistrySourceKind::Manual,
        codex_state::MachineSourceKind::Adapter => MachineRegistrySourceKind::Adapter,
    }
}

fn state_transport_from_api(
    value: MachineRegistryEndpointTransport,
) -> codex_state::MachineEndpointTransport {
    match value {
        MachineRegistryEndpointTransport::Lan => codex_state::MachineEndpointTransport::Lan,
        MachineRegistryEndpointTransport::Tailscale => {
            codex_state::MachineEndpointTransport::Tailscale
        }
        MachineRegistryEndpointTransport::Manual => codex_state::MachineEndpointTransport::Manual,
        MachineRegistryEndpointTransport::RemoteControl => {
            codex_state::MachineEndpointTransport::RemoteControl
        }
        MachineRegistryEndpointTransport::Adapter => codex_state::MachineEndpointTransport::Adapter,
    }
}

fn api_transport_from_state(
    value: codex_state::MachineEndpointTransport,
) -> MachineRegistryEndpointTransport {
    match value {
        codex_state::MachineEndpointTransport::Lan => MachineRegistryEndpointTransport::Lan,
        codex_state::MachineEndpointTransport::Tailscale => {
            MachineRegistryEndpointTransport::Tailscale
        }
        codex_state::MachineEndpointTransport::Manual => MachineRegistryEndpointTransport::Manual,
        codex_state::MachineEndpointTransport::RemoteControl => {
            MachineRegistryEndpointTransport::RemoteControl
        }
        codex_state::MachineEndpointTransport::Adapter => MachineRegistryEndpointTransport::Adapter,
    }
}

fn normalize_required_text(field_name: &str, value: String) -> Result<String, JSONRPCErrorError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request(format!("{field_name} must not be empty")));
    }
    Ok(value.to_string())
}

fn timestamp_to_datetime(value: i64, field_name: &str) -> Result<DateTime<Utc>, JSONRPCErrorError> {
    DateTime::<Utc>::from_timestamp(value, 0)
        .ok_or_else(|| invalid_request(format!("{field_name} must be a valid Unix timestamp")))
}

fn machine_registry_store_error(err: anyhow::Error) -> JSONRPCErrorError {
    let message = err.to_string();
    if message.contains("invalid machine registry cursor")
        || message.contains("must not be empty")
        || message.contains("requires")
        || message.contains("adapter_name")
        || message.contains("matched multiple existing machines")
    {
        invalid_request(message)
    } else {
        internal_error(format!("failed to access machine registry: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::MachineRegistryRedaction;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn upsert_and_list_redacts_endpoint_addresses() {
        let tempdir = TempDir::new().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let processor = MachineRegistryRequestProcessor::new(Some(state_db));
        let response = processor
            .upsert(test_upsert_params("install-api"))
            .await
            .expect("upsert should succeed")
            .expect("response should be present");
        let response = expect_upsert_response(response);
        let endpoint = &response.machine.endpoints[0];

        assert_eq!(
            MachineRegistryTrustState::Untrusted,
            response.machine.trust_state
        );
        assert_eq!(
            MachineRegistryEnrollmentState::Manual,
            response.machine.enrollment_state
        );
        assert_eq!(
            MachineRegistryHealthState::Unknown,
            response.machine.health_state
        );
        assert_eq!(
            MachineRegistrySourceKind::Manual,
            response.machine.source_kind
        );
        assert_eq!("Tailscale endpoint", endpoint.display_address);
        assert_eq!(
            vec![MachineRegistryRedaction::EndpointAddress],
            endpoint.redactions
        );
        let payload = serde_json::to_value(&response).expect("response should serialize");
        assert!(
            payload["machine"]["endpoints"][0]
                .get("normalizedAddress")
                .is_none()
        );

        let response = processor
            .list(MachineRegistryListParams::default())
            .await
            .expect("list should succeed")
            .expect("response should be present");
        let response = expect_list_response(response);
        let endpoint = &response.data[0].endpoints[0];
        assert_eq!("Tailscale endpoint", endpoint.display_address);
        assert_eq!(
            vec![MachineRegistryRedaction::EndpointAddress],
            endpoint.redactions
        );
        let payload = serde_json::to_value(&response).expect("response should serialize");
        assert!(
            payload["data"][0]["endpoints"][0]
                .get("normalizedAddress")
                .is_none()
        );
    }

    #[tokio::test]
    async fn adapter_sourced_rows_read_and_list_with_public_redaction() {
        let tempdir = TempDir::new().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let machine = state_db
            .machine_registry()
            .upsert_machine(codex_state::MachineRegistryUpsertParams {
                machine_id: None,
                installation_id: Some("adapter-install".to_string()),
                display_name: Some("Adapter machine".to_string()),
                trust_state: codex_state::MachineTrustState::Untrusted,
                enrollment_state: codex_state::MachineEnrollmentState::Discovered,
                health_state: codex_state::MachineHealthState::Online,
                source_kind: codex_state::MachineSourceKind::Adapter,
                adapter_name: Some("generic-network-discovery".to_string()),
                capabilities_json: serde_json::json!({"appServer": true}),
                endpoints: vec![codex_state::MachineEndpointUpsertParams {
                    endpoint_id: None,
                    transport: codex_state::MachineEndpointTransport::Adapter,
                    address: "adapter://spark02".to_string(),
                    display_address: None,
                    priority: 0,
                    capabilities_json: serde_json::json!({"dispatch": true}),
                    last_success_at: Some(Utc::now()),
                    last_error: None,
                }],
                last_seen_at: Some(Utc::now()),
            })
            .await
            .expect("adapter-sourced machine should persist");
        let processor = MachineRegistryRequestProcessor::new(Some(state_db));

        let list = processor
            .list(MachineRegistryListParams::default())
            .await
            .expect("list should succeed")
            .expect("response should be present");
        let list = expect_list_response(list);
        assert_eq!(1, list.data.len());
        assert_adapter_machine(&list.data[0], machine.machine_id.as_str());
        let list_payload = serde_json::to_value(&list).expect("list response should serialize");
        assert_endpoint_addresses_redacted(&list_payload["data"][0]["endpoints"][0]);

        let read = processor
            .read(MachineRegistryReadParams {
                machine_id: machine.machine_id.clone(),
            })
            .await
            .expect("read should succeed")
            .expect("response should be present");
        let read = expect_read_response(read);
        let read_machine = read.machine.as_ref().expect("machine should be returned");
        assert_adapter_machine(read_machine, machine.machine_id.as_str());
        let read_payload = serde_json::to_value(&read).expect("read response should serialize");
        assert_endpoint_addresses_redacted(&read_payload["machine"]["endpoints"][0]);
    }

    #[tokio::test]
    async fn update_trust_is_explicit_and_local_is_internal_only() {
        let tempdir = TempDir::new().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let processor = MachineRegistryRequestProcessor::new(Some(state_db));
        let response = processor
            .upsert(test_upsert_params("trust-install"))
            .await
            .expect("upsert should succeed")
            .expect("response should be present");
        let response = expect_upsert_response(response);
        let machine_id = response.machine.machine_id;

        let err = processor
            .update_trust(MachineRegistryUpdateTrustParams {
                machine_id: machine_id.clone(),
                trust_state: MachineRegistryTrustState::Local,
            })
            .await
            .expect_err("local trust should be rejected");
        assert!(err.message.contains("local trust state"));

        let response = processor
            .update_trust(MachineRegistryUpdateTrustParams {
                machine_id,
                trust_state: MachineRegistryTrustState::Trusted,
            })
            .await
            .expect("trust update should succeed")
            .expect("response should be present");
        let response = expect_update_trust_response(response);
        assert_eq!(
            Some(MachineRegistryTrustState::Trusted),
            response.machine.map(|machine| machine.trust_state)
        );
    }

    #[tokio::test]
    async fn public_upsert_rejects_adapter_endpoints() {
        let tempdir = TempDir::new().expect("tempdir");
        let state_db = codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize");
        let processor = MachineRegistryRequestProcessor::new(Some(state_db));
        let err = processor
            .upsert(MachineRegistryUpsertParams {
                machine_id: None,
                installation_id: Some("adapter-install".to_string()),
                display_name: Some("Adapter machine".to_string()),
                capabilities: serde_json::json!({}),
                endpoints: vec![MachineRegistryEndpointUpsert {
                    endpoint_id: None,
                    transport: MachineRegistryEndpointTransport::Adapter,
                    address: "adapter://spark02".to_string(),
                    display_address: None,
                    priority: None,
                    capabilities: serde_json::json!({}),
                    last_success_at: None,
                    last_error: None,
                }],
                last_seen_at: None,
            })
            .await
            .expect_err("adapter endpoints should be rejected");
        assert!(err.message.contains("does not accept adapter endpoints"));
    }

    fn test_upsert_params(installation_id: &str) -> MachineRegistryUpsertParams {
        MachineRegistryUpsertParams {
            machine_id: None,
            installation_id: Some(installation_id.to_string()),
            display_name: Some("API machine".to_string()),
            capabilities: serde_json::json!({"dispatch": false}),
            endpoints: vec![MachineRegistryEndpointUpsert {
                endpoint_id: None,
                transport: MachineRegistryEndpointTransport::Tailscale,
                address: "https://spark02.tailnet:1455/secret".to_string(),
                display_address: None,
                priority: Some(10),
                capabilities: serde_json::json!({"appServer": true}),
                last_success_at: None,
                last_error: None,
            }],
            last_seen_at: None,
        }
    }

    fn expect_upsert_response(payload: ClientResponsePayload) -> MachineRegistryUpsertResponse {
        let ClientResponsePayload::MachineRegistryUpsert(response) = payload else {
            panic!("expected machine registry upsert response");
        };
        response
    }

    fn expect_read_response(payload: ClientResponsePayload) -> MachineRegistryReadResponse {
        let ClientResponsePayload::MachineRegistryRead(response) = payload else {
            panic!("expected machine registry read response");
        };
        response
    }

    fn expect_list_response(payload: ClientResponsePayload) -> MachineRegistryListResponse {
        let ClientResponsePayload::MachineRegistryList(response) = payload else {
            panic!("expected machine registry list response");
        };
        response
    }

    fn expect_update_trust_response(
        payload: ClientResponsePayload,
    ) -> MachineRegistryUpdateTrustResponse {
        let ClientResponsePayload::MachineRegistryUpdateTrust(response) = payload else {
            panic!("expected machine registry update trust response");
        };
        response
    }

    fn assert_adapter_machine(machine: &MachineRegistryMachine, machine_id: &str) {
        assert_eq!(machine_id, machine.machine_id.as_str());
        assert_eq!(Some("adapter-install"), machine.installation_id.as_deref());
        assert_eq!(Some("Adapter machine"), machine.display_name.as_deref());
        assert_eq!(MachineRegistryTrustState::Untrusted, machine.trust_state);
        assert_eq!(
            MachineRegistryEnrollmentState::Discovered,
            machine.enrollment_state
        );
        assert_eq!(MachineRegistryHealthState::Online, machine.health_state);
        assert_eq!(MachineRegistrySourceKind::Adapter, machine.source_kind);
        assert_eq!(
            Some("generic-network-discovery"),
            machine.adapter_name.as_deref()
        );
        assert_eq!(serde_json::json!({"appServer": true}), machine.capabilities);
        assert_eq!(1, machine.endpoints.len());
        let endpoint = &machine.endpoints[0];
        assert_eq!(
            MachineRegistryEndpointTransport::Adapter,
            endpoint.transport
        );
        assert_eq!("Adapter endpoint", endpoint.display_address);
        assert_eq!(
            vec![MachineRegistryRedaction::EndpointAddress],
            endpoint.redactions
        );
        assert_eq!(0, endpoint.priority);
        assert_eq!(serde_json::json!({"dispatch": true}), endpoint.capabilities);
    }

    fn assert_endpoint_addresses_redacted(endpoint: &serde_json::Value) {
        let endpoint = endpoint
            .as_object()
            .expect("endpoint should serialize as object");
        assert!(!endpoint.contains_key("normalizedAddress"));
        assert!(!endpoint.contains_key("address"));
    }
}
