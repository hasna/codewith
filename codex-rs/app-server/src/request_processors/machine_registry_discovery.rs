use chrono::DateTime;
use chrono::Utc;
use codex_rollout::StateDbHandle;
use serde_json::Value;
use std::future::Future;

/// Boundary for optional machine discovery integrations.
///
/// Implementations keep their dependency edges outside the core registry and
/// return generic machine facts. The importer intentionally controls trust,
/// enrollment, and source fields so a discovery adapter cannot claim local or
/// trusted identity.
pub(crate) trait MachineDiscoveryAdapter {
    fn adapter_name(&self) -> &str;

    fn discover_machines(
        &self,
    ) -> impl Future<Output = anyhow::Result<Vec<DiscoveredMachine>>> + Send;
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DiscoveredMachine {
    pub(crate) machine_id: Option<String>,
    pub(crate) installation_id: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) health_state: codex_state::MachineHealthState,
    pub(crate) capabilities_json: Value,
    pub(crate) endpoints: Vec<DiscoveredMachineEndpoint>,
    pub(crate) last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DiscoveredMachineEndpoint {
    pub(crate) endpoint_id: Option<String>,
    pub(crate) transport: codex_state::MachineEndpointTransport,
    pub(crate) address: String,
    pub(crate) display_address: Option<String>,
    pub(crate) priority: i64,
    pub(crate) capabilities_json: Value,
    pub(crate) last_success_at: Option<DateTime<Utc>>,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MachineDiscoveryImportReport {
    pub(crate) adapter_name: String,
    pub(crate) machines: Vec<codex_state::MachineRecord>,
}

#[derive(Clone)]
pub(crate) struct MachineRegistryDiscoveryImporter {
    state_db: StateDbHandle,
}

impl MachineRegistryDiscoveryImporter {
    pub(crate) fn new(state_db: StateDbHandle) -> Self {
        Self { state_db }
    }

    pub(crate) async fn import_from_adapter<A>(
        &self,
        adapter: &A,
    ) -> anyhow::Result<MachineDiscoveryImportReport>
    where
        A: MachineDiscoveryAdapter + Sync,
    {
        let adapter_name = adapter.adapter_name().trim();
        if adapter_name.is_empty() {
            anyhow::bail!("machine discovery adapter name must not be empty");
        }
        let adapter_name = adapter_name.to_string();
        let machines = adapter.discover_machines().await?;
        let mut imported = Vec::with_capacity(machines.len());
        for machine in machines {
            imported.push(
                self.state_db
                    .machine_registry()
                    .upsert_machine(codex_state::MachineRegistryUpsertParams {
                        machine_id: machine.machine_id,
                        installation_id: machine.installation_id,
                        display_name: machine.display_name,
                        trust_state: codex_state::MachineTrustState::Untrusted,
                        enrollment_state: codex_state::MachineEnrollmentState::Discovered,
                        health_state: machine.health_state,
                        source_kind: codex_state::MachineSourceKind::Adapter,
                        adapter_name: Some(adapter_name.clone()),
                        capabilities_json: machine.capabilities_json,
                        endpoints: machine
                            .endpoints
                            .into_iter()
                            .map(|endpoint| codex_state::MachineEndpointUpsertParams {
                                endpoint_id: endpoint.endpoint_id,
                                transport: endpoint.transport,
                                address: endpoint.address,
                                display_address: endpoint.display_address,
                                priority: endpoint.priority,
                                capabilities_json: endpoint.capabilities_json,
                                last_success_at: endpoint.last_success_at,
                                last_error: endpoint.last_error,
                            })
                            .collect(),
                        last_seen_at: machine.last_seen_at,
                    })
                    .await?,
            );
        }
        Ok(MachineDiscoveryImportReport {
            adapter_name,
            machines: imported,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ClientResponsePayload;
    use codex_app_server_protocol::MachineRegistryListParams;
    use codex_app_server_protocol::MachineRegistryRedaction;
    use codex_app_server_protocol::MachineRegistrySourceKind;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    struct FakeDiscoveryAdapter {
        adapter_name: &'static str,
        machines: Vec<DiscoveredMachine>,
    }

    impl MachineDiscoveryAdapter for FakeDiscoveryAdapter {
        fn adapter_name(&self) -> &str {
            self.adapter_name
        }

        async fn discover_machines(&self) -> anyhow::Result<Vec<DiscoveredMachine>> {
            Ok(self.machines.clone())
        }
    }

    #[tokio::test]
    async fn imports_adapter_machines_without_granting_trust() {
        let state_db = test_state_db().await;
        let importer = MachineRegistryDiscoveryImporter::new(state_db);
        let report = importer
            .import_from_adapter(&FakeDiscoveryAdapter {
                adapter_name: "generic-network-discovery",
                machines: vec![test_machine("adapter-install")],
            })
            .await
            .expect("adapter discovery should import");

        let machine = &report.machines[0];
        assert_eq!("generic-network-discovery", report.adapter_name);
        assert_eq!(
            codex_state::MachineTrustState::Untrusted,
            machine.trust_state
        );
        assert_eq!(
            codex_state::MachineEnrollmentState::Discovered,
            machine.enrollment_state
        );
        assert_eq!(codex_state::MachineSourceKind::Adapter, machine.source_kind);
        assert_eq!(
            Some("generic-network-discovery".to_string()),
            machine.adapter_name
        );
        assert_eq!(
            codex_state::MachineEndpointTransport::Adapter,
            machine.endpoints[0].transport
        );
        assert_eq!("Adapter endpoint", machine.endpoints[0].display_address);
    }

    #[tokio::test]
    async fn empty_adapter_name_is_rejected_before_discovery_runs() {
        let state_db = test_state_db().await;
        let importer = MachineRegistryDiscoveryImporter::new(state_db);
        let err = importer
            .import_from_adapter(&FakeDiscoveryAdapter {
                adapter_name: " ",
                machines: vec![test_machine("adapter-install")],
            })
            .await
            .expect_err("blank adapter names should be rejected");

        assert!(
            err.to_string()
                .contains("machine discovery adapter name must not be empty")
        );
    }

    #[tokio::test]
    async fn imported_adapter_endpoint_lists_with_public_redaction() {
        let state_db = test_state_db().await;
        let importer = MachineRegistryDiscoveryImporter::new(state_db.clone());
        importer
            .import_from_adapter(&FakeDiscoveryAdapter {
                adapter_name: "generic-network-discovery",
                machines: vec![test_machine("adapter-install")],
            })
            .await
            .expect("adapter discovery should import");

        let processor =
            super::super::machine_registry_processor::MachineRegistryRequestProcessor::new(Some(
                state_db,
            ));
        let payload = processor
            .list(MachineRegistryListParams::default())
            .await
            .expect("list should succeed")
            .expect("list response should be present");
        let ClientResponsePayload::MachineRegistryList(response) = payload else {
            panic!("expected machine registry list response");
        };
        let machine = &response.data[0];
        let endpoint = &machine.endpoints[0];

        assert_eq!(MachineRegistrySourceKind::Adapter, machine.source_kind);
        assert_eq!(
            Some("generic-network-discovery".to_string()),
            machine.adapter_name
        );
        assert_eq!("Adapter endpoint", endpoint.display_address);
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
        assert!(payload["data"][0]["endpoints"][0].get("address").is_none());
    }

    async fn test_state_db() -> StateDbHandle {
        let tempdir = TempDir::new().expect("tempdir");
        codex_state::StateRuntime::init(tempdir.keep(), "test-provider".to_string())
            .await
            .expect("state db should initialize")
    }

    fn test_machine(installation_id: &str) -> DiscoveredMachine {
        DiscoveredMachine {
            machine_id: None,
            installation_id: Some(installation_id.to_string()),
            display_name: Some("Adapter machine".to_string()),
            health_state: codex_state::MachineHealthState::Online,
            capabilities_json: serde_json::json!({"appServer": true}),
            endpoints: vec![DiscoveredMachineEndpoint {
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
        }
    }
}
