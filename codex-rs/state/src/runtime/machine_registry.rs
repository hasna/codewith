use super::*;
use crate::model::MachineEndpointRow;
use crate::model::MachineRecordRow;
use std::collections::BTreeSet;
use std::future::Future;
use uuid::Uuid;

pub const DEFAULT_MACHINE_REGISTRY_LIST_LIMIT: u32 = 50;
pub const MAX_MACHINE_REGISTRY_LIST_LIMIT: u32 = 200;

#[derive(Clone)]
pub struct MachineRegistryStore {
    pool: Arc<SqlitePool>,
}

impl MachineRegistryStore {
    pub(crate) fn new(pool: Arc<SqlitePool>) -> Self {
        Self { pool }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineRegistryUpsertParams {
    pub machine_id: Option<String>,
    pub installation_id: Option<String>,
    pub display_name: Option<String>,
    pub trust_state: crate::MachineTrustState,
    pub enrollment_state: crate::MachineEnrollmentState,
    pub health_state: crate::MachineHealthState,
    pub source_kind: crate::MachineSourceKind,
    pub adapter_name: Option<String>,
    pub capabilities_json: Value,
    pub endpoints: Vec<MachineEndpointUpsertParams>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineEndpointUpsertParams {
    pub endpoint_id: Option<String>,
    pub transport: crate::MachineEndpointTransport,
    pub address: String,
    pub display_address: Option<String>,
    pub priority: i64,
    pub capabilities_json: Value,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineRegistryListParams {
    pub include_disabled: bool,
    pub include_forgotten: bool,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineRegistryListPage {
    pub data: Vec<crate::MachineRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct NormalizedEndpoint {
    endpoint_id: Option<String>,
    transport: crate::MachineEndpointTransport,
    normalized_address: String,
    display_address: String,
    priority: i64,
    capabilities_json: Value,
    last_success_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

impl MachineRegistryStore {
    pub async fn upsert_machine(
        &self,
        params: MachineRegistryUpsertParams,
    ) -> anyhow::Result<crate::MachineRecord> {
        retry_transient_sqlite_busy("upsert machine registry", || {
            let params = params.clone();
            async move { self.upsert_machine_once(params).await }
        })
        .await
    }

    async fn upsert_machine_once(
        &self,
        params: MachineRegistryUpsertParams,
    ) -> anyhow::Result<crate::MachineRecord> {
        let machine_id_hint = normalize_optional_token("machine_id", params.machine_id)?;
        let installation_id = normalize_optional_token("installation_id", params.installation_id)?;
        let display_name = normalize_optional_token("display_name", params.display_name)?;
        let adapter_name = normalize_optional_token("adapter_name", params.adapter_name)?;
        if params.source_kind == crate::MachineSourceKind::Adapter && adapter_name.is_none() {
            anyhow::bail!("adapter_name is required for adapter-sourced machines");
        }
        let endpoints = params
            .endpoints
            .into_iter()
            .map(normalize_endpoint)
            .collect::<anyhow::Result<Vec<_>>>()?;
        if machine_id_hint.is_none() && installation_id.is_none() && endpoints.is_empty() {
            anyhow::bail!(
                "machine upsert requires machine_id, installation_id, or at least one endpoint"
            );
        }
        let capabilities_json = serde_json::to_string(&params.capabilities_json)?;
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let last_seen_at_ms = params.last_seen_at.map(datetime_to_epoch_millis);
        let mut tx = self.pool.begin().await?;
        let existing_machine_id = resolve_existing_machine_id(
            &mut tx,
            machine_id_hint.as_deref(),
            installation_id.as_deref(),
            &endpoints,
        )
        .await?;
        let machine_id = existing_machine_id
            .or(machine_id_hint)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        sqlx::query(
            r#"
INSERT INTO machine_registry_machines (
    machine_id,
    installation_id,
    display_name,
    trust_state,
    enrollment_state,
    health_state,
    source_kind,
    adapter_name,
    capabilities_json,
    last_seen_at_ms,
    disabled_at_ms,
    forgotten_at_ms,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(machine_id) DO UPDATE SET
    installation_id = COALESCE(excluded.installation_id, machine_registry_machines.installation_id),
    display_name = COALESCE(excluded.display_name, machine_registry_machines.display_name),
    trust_state = excluded.trust_state,
    enrollment_state = excluded.enrollment_state,
    health_state = excluded.health_state,
    source_kind = excluded.source_kind,
    adapter_name = excluded.adapter_name,
    capabilities_json = excluded.capabilities_json,
    last_seen_at_ms = COALESCE(excluded.last_seen_at_ms, machine_registry_machines.last_seen_at_ms),
    disabled_at_ms = CASE
        WHEN excluded.trust_state = 'disabled' THEN COALESCE(machine_registry_machines.disabled_at_ms, excluded.updated_at_ms)
        WHEN machine_registry_machines.trust_state = 'disabled' AND excluded.trust_state != 'disabled' THEN NULL
        ELSE machine_registry_machines.disabled_at_ms
    END,
    forgotten_at_ms = NULL,
    updated_at_ms = excluded.updated_at_ms
            "#,
        )
        .bind(machine_id.as_str())
        .bind(installation_id.as_deref())
        .bind(display_name.as_deref())
        .bind(params.trust_state.as_str())
        .bind(params.enrollment_state.as_str())
        .bind(params.health_state.as_str())
        .bind(params.source_kind.as_str())
        .bind(adapter_name.as_deref())
        .bind(capabilities_json.as_str())
        .bind(last_seen_at_ms)
        .bind(disabled_at_ms(params.trust_state, now_ms))
        .bind(Option::<i64>::None)
        .bind(now_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;

        for endpoint in endpoints {
            upsert_endpoint(&mut tx, machine_id.as_str(), endpoint, now_ms).await?;
        }

        tx.commit().await?;
        retry_transient_sqlite_busy("read machine registry upsert result", || {
            self.get_machine(machine_id.as_str())
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("machine registry upsert did not return a row"))
    }

    pub async fn get_machine(
        &self,
        machine_id: &str,
    ) -> anyhow::Result<Option<crate::MachineRecord>> {
        let row = sqlx::query(
            r#"
SELECT
    machine_id,
    installation_id,
    display_name,
    trust_state,
    enrollment_state,
    health_state,
    source_kind,
    adapter_name,
    capabilities_json,
    last_seen_at_ms,
    disabled_at_ms,
    forgotten_at_ms,
    created_at_ms,
    updated_at_ms
FROM machine_registry_machines
WHERE machine_id = ?
            "#,
        )
        .bind(machine_id)
        .fetch_optional(self.pool.as_ref())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut record = machine_record_from_row(&row)?;
        record.endpoints = self
            .list_endpoints_for_machine(record.machine_id.as_str())
            .await?;
        Ok(Some(record))
    }

    pub async fn list_machines(
        &self,
        params: MachineRegistryListParams,
    ) -> anyhow::Result<MachineRegistryListPage> {
        let offset = parse_cursor(params.cursor.as_deref())?;
        let limit = params.limit.clamp(1, MAX_MACHINE_REGISTRY_LIST_LIMIT);
        let mut query = QueryBuilder::<Sqlite>::new(machine_record_select_sql("WHERE 1 = 1"));
        if !params.include_forgotten {
            query.push(" AND forgotten_at_ms IS NULL");
        }
        if !params.include_disabled {
            query.push(" AND trust_state NOT IN ('disabled', 'revoked')");
        }
        query.push(" ORDER BY updated_at_ms DESC, machine_id DESC LIMIT ");
        query.push_bind(i64::from(limit) + 1);
        query.push(" OFFSET ");
        query.push_bind(i64::from(offset));

        let rows = query.build().fetch_all(self.pool.as_ref()).await?;
        let has_more = rows.len() > limit as usize;
        let mut data = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            let mut record = machine_record_from_row(&row)?;
            record.endpoints = self
                .list_endpoints_for_machine(record.machine_id.as_str())
                .await?;
            data.push(record);
        }
        let next_cursor = has_more.then(|| (offset + limit).to_string());
        Ok(MachineRegistryListPage { data, next_cursor })
    }

    pub async fn disable_machine(&self, machine_id: &str) -> anyhow::Result<bool> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let result = sqlx::query(
            r#"
UPDATE machine_registry_machines
SET
    trust_state = ?,
    health_state = ?,
    disabled_at_ms = COALESCE(disabled_at_ms, ?),
    updated_at_ms = ?
WHERE machine_id = ? AND forgotten_at_ms IS NULL
            "#,
        )
        .bind(crate::MachineTrustState::Disabled.as_str())
        .bind(crate::MachineHealthState::Offline.as_str())
        .bind(now_ms)
        .bind(now_ms)
        .bind(machine_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn update_machine_trust(
        &self,
        machine_id: &str,
        trust_state: crate::MachineTrustState,
    ) -> anyhow::Result<Option<crate::MachineRecord>> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let result = sqlx::query(
            r#"
UPDATE machine_registry_machines
SET
    trust_state = ?,
    health_state = CASE
        WHEN ? = 'disabled' THEN 'offline'
        ELSE health_state
    END,
    disabled_at_ms = CASE
        WHEN ? = 'disabled' THEN COALESCE(disabled_at_ms, ?)
        WHEN trust_state = 'disabled' AND ? != 'disabled' THEN NULL
        ELSE disabled_at_ms
    END,
    updated_at_ms = ?
WHERE machine_id = ? AND forgotten_at_ms IS NULL
            "#,
        )
        .bind(trust_state.as_str())
        .bind(trust_state.as_str())
        .bind(trust_state.as_str())
        .bind(now_ms)
        .bind(trust_state.as_str())
        .bind(now_ms)
        .bind(machine_id)
        .execute(self.pool.as_ref())
        .await?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        retry_transient_sqlite_busy("read machine registry trust update result", || {
            self.get_machine(machine_id)
        })
        .await
    }

    pub async fn forget_machine(&self, machine_id: &str) -> anyhow::Result<bool> {
        let now_ms = datetime_to_epoch_millis(Utc::now());
        let result = sqlx::query(
            r#"
UPDATE machine_registry_machines
SET
    forgotten_at_ms = COALESCE(forgotten_at_ms, ?),
    updated_at_ms = ?
WHERE machine_id = ?
            "#,
        )
        .bind(now_ms)
        .bind(now_ms)
        .bind(machine_id)
        .execute(self.pool.as_ref())
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_endpoints_for_machine(
        &self,
        machine_id: &str,
    ) -> anyhow::Result<Vec<crate::MachineEndpoint>> {
        let rows = sqlx::query(
            r#"
SELECT
    endpoint_id,
    machine_id,
    transport,
    normalized_address,
    display_address,
    priority,
    capabilities_json,
    last_success_at_ms,
    last_error,
    created_at_ms,
    updated_at_ms
FROM machine_registry_endpoints
WHERE machine_id = ?
ORDER BY priority DESC, updated_at_ms DESC, endpoint_id
            "#,
        )
        .bind(machine_id)
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.iter()
            .map(machine_endpoint_from_row)
            .collect::<anyhow::Result<Vec<_>>>()
    }
}

async fn resolve_existing_machine_id(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    machine_id: Option<&str>,
    installation_id: Option<&str>,
    endpoints: &[NormalizedEndpoint],
) -> anyhow::Result<Option<String>> {
    let mut candidates = BTreeSet::new();
    if let Some(machine_id) = machine_id
        && machine_exists(tx, machine_id).await?
    {
        candidates.insert(machine_id.to_string());
    }
    if let Some(installation_id) = installation_id
        && let Some(machine_id) = sqlx::query_scalar::<_, String>(
            "SELECT machine_id FROM machine_registry_machines WHERE installation_id = ?",
        )
        .bind(installation_id)
        .fetch_optional(&mut **tx)
        .await?
    {
        candidates.insert(machine_id);
    }
    for endpoint in endpoints {
        if let Some(machine_id) = sqlx::query_scalar::<_, String>(
            r#"
SELECT machine_id
FROM machine_registry_endpoints
WHERE transport = ? AND normalized_address = ?
            "#,
        )
        .bind(endpoint.transport.as_str())
        .bind(endpoint.normalized_address.as_str())
        .fetch_optional(&mut **tx)
        .await?
        {
            candidates.insert(machine_id);
        }
    }
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.into_iter().next()),
        _ => anyhow::bail!("machine identity signals matched multiple existing machines"),
    }
}

async fn machine_exists(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    machine_id: &str,
) -> anyhow::Result<bool> {
    let found = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM machine_registry_machines WHERE machine_id = ?",
    )
    .bind(machine_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(found > 0)
}

async fn upsert_endpoint(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    machine_id: &str,
    endpoint: NormalizedEndpoint,
    now_ms: i64,
) -> anyhow::Result<()> {
    let endpoint_id = endpoint
        .endpoint_id
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let capabilities_json = serde_json::to_string(&endpoint.capabilities_json)?;
    let last_success_at_ms = endpoint.last_success_at.map(datetime_to_epoch_millis);
    sqlx::query(
        r#"
INSERT INTO machine_registry_endpoints (
    endpoint_id,
    machine_id,
    transport,
    normalized_address,
    display_address,
    priority,
    capabilities_json,
    last_success_at_ms,
    last_error,
    created_at_ms,
    updated_at_ms
) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(transport, normalized_address) DO UPDATE SET
    machine_id = excluded.machine_id,
    display_address = excluded.display_address,
    priority = excluded.priority,
    capabilities_json = excluded.capabilities_json,
    last_success_at_ms = COALESCE(excluded.last_success_at_ms, machine_registry_endpoints.last_success_at_ms),
    last_error = excluded.last_error,
    updated_at_ms = excluded.updated_at_ms
            "#,
    )
    .bind(endpoint_id)
    .bind(machine_id)
    .bind(endpoint.transport.as_str())
    .bind(endpoint.normalized_address.as_str())
    .bind(endpoint.display_address.as_str())
    .bind(endpoint.priority)
    .bind(capabilities_json)
    .bind(last_success_at_ms)
    .bind(endpoint.last_error)
    .bind(now_ms)
    .bind(now_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn normalize_endpoint(params: MachineEndpointUpsertParams) -> anyhow::Result<NormalizedEndpoint> {
    let normalized_address = normalize_address(params.address.as_str())?;
    let display_address = params
        .display_address
        .map(|value| normalize_display_text("display_address", value))
        .transpose()?
        .unwrap_or_else(|| redact_address(params.transport));
    Ok(NormalizedEndpoint {
        endpoint_id: normalize_optional_token("endpoint_id", params.endpoint_id)?,
        transport: params.transport,
        normalized_address,
        display_address,
        priority: params.priority,
        capabilities_json: params.capabilities_json,
        last_success_at: params.last_success_at,
        last_error: params.last_error,
    })
}

fn normalize_address(address: &str) -> anyhow::Result<String> {
    let address = address.trim().trim_end_matches('/').to_ascii_lowercase();
    if address.is_empty() {
        anyhow::bail!("machine endpoint address must not be empty");
    }
    Ok(address)
}

fn redact_address(transport: crate::MachineEndpointTransport) -> String {
    match transport {
        crate::MachineEndpointTransport::Lan => "LAN endpoint",
        crate::MachineEndpointTransport::Tailscale => "Tailscale endpoint",
        crate::MachineEndpointTransport::Manual => "Manual endpoint",
        crate::MachineEndpointTransport::RemoteControl => "Remote-control endpoint",
        crate::MachineEndpointTransport::Adapter => "Adapter endpoint",
    }
    .to_string()
}

fn normalize_optional_token(
    field_name: &str,
    value: Option<String>,
) -> anyhow::Result<Option<String>> {
    value
        .map(|value| normalize_display_text(field_name, value))
        .transpose()
}

fn normalize_display_text(field_name: &str, value: String) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{field_name} must not be empty");
    }
    Ok(value.to_string())
}

fn disabled_at_ms(trust_state: crate::MachineTrustState, now_ms: i64) -> Option<i64> {
    (trust_state == crate::MachineTrustState::Disabled).then_some(now_ms)
}

fn parse_cursor(cursor: Option<&str>) -> anyhow::Result<u32> {
    cursor
        .map(str::parse::<u32>)
        .transpose()
        .map_err(|err| anyhow::anyhow!("invalid machine registry cursor: {err}"))
        .map(Option::unwrap_or_default)
}

fn machine_record_select_sql(where_clause: &str) -> String {
    format!(
        r#"
SELECT
    machine_id,
    installation_id,
    display_name,
    trust_state,
    enrollment_state,
    health_state,
    source_kind,
    adapter_name,
    capabilities_json,
    last_seen_at_ms,
    disabled_at_ms,
    forgotten_at_ms,
    created_at_ms,
    updated_at_ms
FROM machine_registry_machines
{where_clause}
        "#
    )
}

fn machine_record_from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<crate::MachineRecord> {
    MachineRecordRow::try_from_row(row)?.try_into()
}

fn machine_endpoint_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> anyhow::Result<crate::MachineEndpoint> {
    MachineEndpointRow::try_from_row(row)?.try_into()
}

async fn retry_transient_sqlite_busy<T, F, Fut>(operation: &str, mut f: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut delay = std::time::Duration::from_millis(25);
    for attempt in 0..5 {
        match f().await {
            Ok(value) => return Ok(value),
            Err(err) if is_transient_sqlite_busy(&err) && attempt < 4 => {
                tracing::debug!(
                    operation,
                    attempt = attempt + 1,
                    "retrying machine registry operation after SQLite busy: {err}"
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!("retry loop should return on success or final error")
}

fn is_transient_sqlite_busy(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<sqlx::Error>()
            .is_some_and(is_transient_sqlx_busy)
    })
}

fn is_transient_sqlx_busy(err: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database_err) = err else {
        return false;
    };
    let code = database_err.code();
    matches!(code.as_deref(), Some("5" | "517"))
        || matches!(
            database_err.message(),
            "database is locked" | "database is busy"
        )
}

#[cfg(test)]
mod tests {
    use super::super::StateRuntime;
    use super::super::test_support::unique_temp_dir;
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn upsert_machine_deduplicates_by_normalized_endpoint() {
        let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("runtime should initialize");
        let first = runtime
            .machine_registry()
            .upsert_machine(test_machine_params(
                "First",
                "HTTPS://Spark02.tailnet:1455/",
            ))
            .await
            .expect("first machine should upsert");
        let second = runtime
            .machine_registry()
            .upsert_machine(test_machine_params(
                "Second",
                "https://spark02.tailnet:1455",
            ))
            .await
            .expect("second upsert should dedupe");

        assert_eq!(first.machine_id, second.machine_id);
        assert_eq!(Some("Second".to_string()), second.display_name);
        assert_eq!(1, second.endpoints.len());
        assert_eq!(
            "https://spark02.tailnet:1455",
            second.endpoints[0].normalized_address
        );
        assert_eq!("Tailscale endpoint", second.endpoints[0].display_address);
    }

    #[tokio::test]
    async fn list_disable_and_forget_machine_without_optional_adapter() {
        let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("runtime should initialize");
        let machine = runtime
            .machine_registry()
            .upsert_machine(MachineRegistryUpsertParams {
                machine_id: None,
                installation_id: Some("install-local".to_string()),
                display_name: Some("Local machine".to_string()),
                trust_state: crate::MachineTrustState::Local,
                enrollment_state: crate::MachineEnrollmentState::Local,
                health_state: crate::MachineHealthState::Online,
                source_kind: crate::MachineSourceKind::Local,
                adapter_name: None,
                capabilities_json: serde_json::json!({"sessions": true}),
                endpoints: Vec::new(),
                last_seen_at: Some(Utc::now()),
            })
            .await
            .expect("local machine should upsert without adapter");

        let page = runtime
            .machine_registry()
            .list_machines(MachineRegistryListParams {
                include_disabled: false,
                include_forgotten: false,
                cursor: None,
                limit: 10,
            })
            .await
            .expect("machines should list");
        assert_eq!(vec![machine.clone()], page.data);

        assert!(
            runtime
                .machine_registry()
                .disable_machine(machine.machine_id.as_str())
                .await
                .expect("disable should succeed")
        );
        let visible = runtime
            .machine_registry()
            .list_machines(MachineRegistryListParams {
                include_disabled: false,
                include_forgotten: false,
                cursor: None,
                limit: 10,
            })
            .await
            .expect("enabled machines should list");
        assert_eq!(Vec::<crate::MachineRecord>::new(), visible.data);

        let disabled = runtime
            .machine_registry()
            .get_machine(machine.machine_id.as_str())
            .await
            .expect("disabled machine should load")
            .expect("disabled machine should exist");
        assert_eq!(crate::MachineTrustState::Disabled, disabled.trust_state);
        assert_eq!(crate::MachineHealthState::Offline, disabled.health_state);
        assert!(disabled.disabled_at.is_some());

        assert!(
            runtime
                .machine_registry()
                .forget_machine(machine.machine_id.as_str())
                .await
                .expect("forget should succeed")
        );
        let active = runtime
            .machine_registry()
            .list_machines(MachineRegistryListParams {
                include_disabled: true,
                include_forgotten: false,
                cursor: None,
                limit: 10,
            })
            .await
            .expect("non-forgotten machines should list");
        assert_eq!(Vec::<crate::MachineRecord>::new(), active.data);
    }

    #[tokio::test]
    async fn adapter_source_requires_generic_adapter_name_only() {
        let runtime = StateRuntime::init(unique_temp_dir(), "test-provider".to_string())
            .await
            .expect("runtime should initialize");
        let err = runtime
            .machine_registry()
            .upsert_machine(MachineRegistryUpsertParams {
                machine_id: None,
                installation_id: Some("adapter-install".to_string()),
                display_name: Some("Adapter machine".to_string()),
                trust_state: crate::MachineTrustState::Untrusted,
                enrollment_state: crate::MachineEnrollmentState::Discovered,
                health_state: crate::MachineHealthState::Unknown,
                source_kind: crate::MachineSourceKind::Adapter,
                adapter_name: None,
                capabilities_json: serde_json::json!({}),
                endpoints: vec![MachineEndpointUpsertParams {
                    endpoint_id: None,
                    transport: crate::MachineEndpointTransport::Adapter,
                    address: "adapter://spark02".to_string(),
                    display_address: None,
                    priority: 0,
                    capabilities_json: serde_json::json!({}),
                    last_success_at: None,
                    last_error: None,
                }],
                last_seen_at: None,
            })
            .await
            .expect_err("missing adapter name should be rejected");
        assert!(err.to_string().contains("adapter_name is required"));

        let machine = runtime
            .machine_registry()
            .upsert_machine(MachineRegistryUpsertParams {
                machine_id: None,
                installation_id: Some("adapter-install".to_string()),
                display_name: Some("Adapter machine".to_string()),
                trust_state: crate::MachineTrustState::Untrusted,
                enrollment_state: crate::MachineEnrollmentState::Discovered,
                health_state: crate::MachineHealthState::Unknown,
                source_kind: crate::MachineSourceKind::Adapter,
                adapter_name: Some("generic-local-discovery".to_string()),
                capabilities_json: serde_json::json!({}),
                endpoints: vec![MachineEndpointUpsertParams {
                    endpoint_id: None,
                    transport: crate::MachineEndpointTransport::Adapter,
                    address: "adapter://spark02".to_string(),
                    display_address: None,
                    priority: 0,
                    capabilities_json: serde_json::json!({}),
                    last_success_at: None,
                    last_error: None,
                }],
                last_seen_at: None,
            })
            .await
            .expect("generic adapter should persist");

        assert_eq!(crate::MachineSourceKind::Adapter, machine.source_kind);
        assert_eq!(
            Some("generic-local-discovery".to_string()),
            machine.adapter_name
        );
        assert_eq!(
            crate::MachineEndpointTransport::Adapter,
            machine.endpoints[0].transport
        );
        assert_eq!("Adapter endpoint", machine.endpoints[0].display_address);
    }

    fn test_machine_params(display_name: &str, address: &str) -> MachineRegistryUpsertParams {
        MachineRegistryUpsertParams {
            machine_id: None,
            installation_id: None,
            display_name: Some(display_name.to_string()),
            trust_state: crate::MachineTrustState::Trusted,
            enrollment_state: crate::MachineEnrollmentState::Manual,
            health_state: crate::MachineHealthState::Online,
            source_kind: crate::MachineSourceKind::Manual,
            adapter_name: None,
            capabilities_json: serde_json::json!({"remoteDispatch": false}),
            endpoints: vec![MachineEndpointUpsertParams {
                endpoint_id: None,
                transport: crate::MachineEndpointTransport::Tailscale,
                address: address.to_string(),
                display_address: None,
                priority: 10,
                capabilities_json: serde_json::json!({"appServer": true}),
                last_success_at: Some(Utc::now()),
                last_error: None,
            }],
            last_seen_at: Some(Utc::now()),
        }
    }
}
