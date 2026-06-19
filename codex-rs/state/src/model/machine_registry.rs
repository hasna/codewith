use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use serde_json::Value as JsonValue;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::epoch_millis_to_datetime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineTrustState {
    Local,
    Trusted,
    Untrusted,
    Disabled,
    Revoked,
}

impl MachineTrustState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Trusted => "trusted",
            Self::Untrusted => "untrusted",
            Self::Disabled => "disabled",
            Self::Revoked => "revoked",
        }
    }
}

impl TryFrom<&str> for MachineTrustState {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "trusted" => Ok(Self::Trusted),
            "untrusted" => Ok(Self::Untrusted),
            "disabled" => Ok(Self::Disabled),
            "revoked" => Ok(Self::Revoked),
            other => Err(anyhow!("unknown machine trust state `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineEnrollmentState {
    Local,
    Manual,
    Discovered,
    Enrolled,
}

impl MachineEnrollmentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Manual => "manual",
            Self::Discovered => "discovered",
            Self::Enrolled => "enrolled",
        }
    }
}

impl TryFrom<&str> for MachineEnrollmentState {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "manual" => Ok(Self::Manual),
            "discovered" => Ok(Self::Discovered),
            "enrolled" => Ok(Self::Enrolled),
            other => Err(anyhow!("unknown machine enrollment state `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineHealthState {
    Unknown,
    Online,
    Offline,
    Degraded,
}

impl MachineHealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Degraded => "degraded",
        }
    }
}

impl TryFrom<&str> for MachineHealthState {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "unknown" => Ok(Self::Unknown),
            "online" => Ok(Self::Online),
            "offline" => Ok(Self::Offline),
            "degraded" => Ok(Self::Degraded),
            other => Err(anyhow!("unknown machine health state `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineSourceKind {
    Local,
    Manual,
    Adapter,
}

impl MachineSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Manual => "manual",
            Self::Adapter => "adapter",
        }
    }
}

impl TryFrom<&str> for MachineSourceKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "local" => Ok(Self::Local),
            "manual" => Ok(Self::Manual),
            "adapter" => Ok(Self::Adapter),
            other => Err(anyhow!("unknown machine source kind `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MachineEndpointTransport {
    Lan,
    Tailscale,
    Manual,
    RemoteControl,
    Adapter,
}

impl MachineEndpointTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Tailscale => "tailscale",
            Self::Manual => "manual",
            Self::RemoteControl => "remote_control",
            Self::Adapter => "adapter",
        }
    }
}

impl TryFrom<&str> for MachineEndpointTransport {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "lan" => Ok(Self::Lan),
            "tailscale" => Ok(Self::Tailscale),
            "manual" => Ok(Self::Manual),
            "remote_control" => Ok(Self::RemoteControl),
            "adapter" => Ok(Self::Adapter),
            other => Err(anyhow!("unknown machine endpoint transport `{other}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineRecord {
    pub machine_id: String,
    pub installation_id: Option<String>,
    pub display_name: Option<String>,
    pub trust_state: MachineTrustState,
    pub enrollment_state: MachineEnrollmentState,
    pub health_state: MachineHealthState,
    pub source_kind: MachineSourceKind,
    pub adapter_name: Option<String>,
    pub capabilities_json: JsonValue,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub disabled_at: Option<DateTime<Utc>>,
    pub forgotten_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub endpoints: Vec<MachineEndpoint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MachineEndpoint {
    pub endpoint_id: String,
    pub machine_id: String,
    pub transport: MachineEndpointTransport,
    pub normalized_address: String,
    pub display_address: String,
    pub priority: i64,
    pub capabilities_json: JsonValue,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) struct MachineRecordRow {
    pub machine_id: String,
    pub installation_id: Option<String>,
    pub display_name: Option<String>,
    pub trust_state: String,
    pub enrollment_state: String,
    pub health_state: String,
    pub source_kind: String,
    pub adapter_name: Option<String>,
    pub capabilities_json: String,
    pub last_seen_at_ms: Option<i64>,
    pub disabled_at_ms: Option<i64>,
    pub forgotten_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

pub(crate) struct MachineEndpointRow {
    pub endpoint_id: String,
    pub machine_id: String,
    pub transport: String,
    pub normalized_address: String,
    pub display_address: String,
    pub priority: i64,
    pub capabilities_json: String,
    pub last_success_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl MachineRecordRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            machine_id: row.try_get("machine_id")?,
            installation_id: row.try_get("installation_id")?,
            display_name: row.try_get("display_name")?,
            trust_state: row.try_get("trust_state")?,
            enrollment_state: row.try_get("enrollment_state")?,
            health_state: row.try_get("health_state")?,
            source_kind: row.try_get("source_kind")?,
            adapter_name: row.try_get("adapter_name")?,
            capabilities_json: row.try_get("capabilities_json")?,
            last_seen_at_ms: row.try_get("last_seen_at_ms")?,
            disabled_at_ms: row.try_get("disabled_at_ms")?,
            forgotten_at_ms: row.try_get("forgotten_at_ms")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl MachineEndpointRow {
    pub(crate) fn try_from_row(row: &SqliteRow) -> Result<Self> {
        Ok(Self {
            endpoint_id: row.try_get("endpoint_id")?,
            machine_id: row.try_get("machine_id")?,
            transport: row.try_get("transport")?,
            normalized_address: row.try_get("normalized_address")?,
            display_address: row.try_get("display_address")?,
            priority: row.try_get("priority")?,
            capabilities_json: row.try_get("capabilities_json")?,
            last_success_at_ms: row.try_get("last_success_at_ms")?,
            last_error: row.try_get("last_error")?,
            created_at_ms: row.try_get("created_at_ms")?,
            updated_at_ms: row.try_get("updated_at_ms")?,
        })
    }
}

impl TryFrom<MachineRecordRow> for MachineRecord {
    type Error = anyhow::Error;

    fn try_from(row: MachineRecordRow) -> Result<Self> {
        Ok(Self {
            machine_id: row.machine_id,
            installation_id: row.installation_id,
            display_name: row.display_name,
            trust_state: MachineTrustState::try_from(row.trust_state.as_str())?,
            enrollment_state: MachineEnrollmentState::try_from(row.enrollment_state.as_str())?,
            health_state: MachineHealthState::try_from(row.health_state.as_str())?,
            source_kind: MachineSourceKind::try_from(row.source_kind.as_str())?,
            adapter_name: row.adapter_name,
            capabilities_json: serde_json::from_str(row.capabilities_json.as_str())?,
            last_seen_at: optional_epoch_millis_to_datetime(row.last_seen_at_ms)?,
            disabled_at: optional_epoch_millis_to_datetime(row.disabled_at_ms)?,
            forgotten_at: optional_epoch_millis_to_datetime(row.forgotten_at_ms)?,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
            endpoints: Vec::new(),
        })
    }
}

impl TryFrom<MachineEndpointRow> for MachineEndpoint {
    type Error = anyhow::Error;

    fn try_from(row: MachineEndpointRow) -> Result<Self> {
        Ok(Self {
            endpoint_id: row.endpoint_id,
            machine_id: row.machine_id,
            transport: MachineEndpointTransport::try_from(row.transport.as_str())?,
            normalized_address: row.normalized_address,
            display_address: row.display_address,
            priority: row.priority,
            capabilities_json: serde_json::from_str(row.capabilities_json.as_str())?,
            last_success_at: optional_epoch_millis_to_datetime(row.last_success_at_ms)?,
            last_error: row.last_error,
            created_at: epoch_millis_to_datetime(row.created_at_ms)?,
            updated_at: epoch_millis_to_datetime(row.updated_at_ms)?,
        })
    }
}

fn optional_epoch_millis_to_datetime(value: Option<i64>) -> Result<Option<DateTime<Utc>>> {
    value.map(epoch_millis_to_datetime).transpose()
}
