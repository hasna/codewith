//! Local host readiness inspection for the Privacy Filter classifier.
//!
//! Everything in this module is read-only host inspection. No network calls, no
//! model download, no secret handling.

use std::path::Path;

use serde::Deserialize;
use serde::Serialize;
use sysinfo::Disks;
use sysinfo::System;

use crate::model::ModelSummary;
use crate::model::PRIVACY_FILTER_MODEL;

/// Whether an install/runtime component is present locally.
///
/// In this slice no install path exists yet, so anything install-related
/// reports [`InstallStatus::NotInstalled`]. The [`InstallStatus::Installed`] and
/// [`InstallStatus::Unknown`] variants exist for the follow-up install slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    NotInstalled,
    Installed,
    Unknown,
}

/// Best-effort accelerator support classification.
///
/// Detection is intentionally conservative: without an inference runtime we
/// cannot reliably probe the accelerator, so we degrade to
/// [`HardwareSupport::Unknown`] rather than guessing. It never errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareSupport {
    Detected,
    Unavailable,
    Unknown,
}

/// A point-in-time snapshot of whether this machine could host the classifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadinessReport {
    /// Total physical RAM in bytes.
    pub total_ram_bytes: u64,
    /// Currently available RAM in bytes.
    pub available_ram_bytes: u64,
    /// Free disk space for the intended install directory, in bytes. `None`
    /// when the containing filesystem could not be determined.
    pub free_disk_bytes: Option<u64>,
    /// Number of logical CPUs.
    pub logical_cpus: usize,
    /// Best-effort GPU support.
    pub gpu: HardwareSupport,
    /// Best-effort in-process WebGPU support.
    pub webgpu: HardwareSupport,
    /// Whether the model runtime dependencies are present locally.
    pub runtime_dependencies: InstallStatus,
    /// Whether the classifier weights are installed locally.
    pub install_status: InstallStatus,
    /// Model metadata this report was evaluated against.
    pub model: ModelSummary,
}

impl ReadinessReport {
    /// Whether total RAM meets the model's minimum recommendation.
    pub fn meets_min_ram(&self) -> bool {
        self.total_ram_bytes >= self.model.min_recommended_ram_bytes
    }

    /// Whether free disk (when known) can hold the model's footprint. Unknown
    /// disk space is treated as "not proven to fit" so the UX can warn.
    pub fn meets_min_disk(&self) -> bool {
        match self.free_disk_bytes {
            Some(free) => free >= self.model.approx_on_disk_bytes,
            None => false,
        }
    }

    /// Whether the host clears the minimum hardware bar to *attempt* an install.
    /// This says nothing about whether the model is actually installed.
    pub fn meets_hardware_requirements(&self) -> bool {
        self.meets_min_ram() && self.meets_min_disk()
    }
}

/// Inspect the local host and produce a [`ReadinessReport`].
///
/// `install_dir` is the directory the model would eventually live in; it is used
/// only to pick the filesystem whose free space is reported. The directory does
/// not need to exist.
pub fn collect_readiness(install_dir: &Path) -> ReadinessReport {
    let mut system = System::new();
    system.refresh_memory();

    let total_ram_bytes = system.total_memory();
    let available_ram_bytes = system.available_memory();
    let logical_cpus = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(0);

    ReadinessReport {
        total_ram_bytes,
        available_ram_bytes,
        free_disk_bytes: free_disk_for_path(install_dir),
        logical_cpus,
        // Accelerator probing is deferred to the inference slice; report Unknown
        // rather than guessing.
        gpu: HardwareSupport::Unknown,
        webgpu: HardwareSupport::Unknown,
        runtime_dependencies: InstallStatus::NotInstalled,
        install_status: InstallStatus::NotInstalled,
        model: PRIVACY_FILTER_MODEL.into(),
    }
}

/// Free space on the filesystem that contains `path`, chosen by the mount point
/// that is the longest prefix of `path`. Returns `None` when no disk matches.
fn free_disk_for_path(path: &Path) -> Option<u64> {
    let disks = Disks::new_with_refreshed_list();
    let mut best: Option<(usize, u64)> = None;
    for disk in disks.list() {
        let mount = disk.mount_point();
        if path.starts_with(mount) {
            let depth = mount.components().count();
            let replace = match best {
                Some((best_depth, _)) => depth > best_depth,
                None => true,
            };
            if replace {
                best = Some((depth, disk.available_space()));
            }
        }
    }
    best.map(|(_, free)| free)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn readiness_report_serde_roundtrips() {
        let report = ReadinessReport {
            total_ram_bytes: 16 * 1024 * 1024 * 1024,
            available_ram_bytes: 8 * 1024 * 1024 * 1024,
            free_disk_bytes: Some(100 * 1024 * 1024 * 1024),
            logical_cpus: 10,
            gpu: HardwareSupport::Unknown,
            webgpu: HardwareSupport::Detected,
            runtime_dependencies: InstallStatus::NotInstalled,
            install_status: InstallStatus::NotInstalled,
            model: PRIVACY_FILTER_MODEL.into(),
        };

        let json = serde_json::to_string(&report).expect("serialize");
        let decoded: ReadinessReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(report, decoded);
    }

    #[test]
    fn install_status_serializes_snake_case() {
        let json = serde_json::to_string(&InstallStatus::NotInstalled).expect("serialize");
        assert_eq!(json, "\"not_installed\"");
    }

    #[test]
    fn hardware_thresholds_use_model_minimums() {
        let mut report = ReadinessReport {
            total_ram_bytes: PRIVACY_FILTER_MODEL.min_recommended_ram_bytes,
            available_ram_bytes: 0,
            free_disk_bytes: Some(PRIVACY_FILTER_MODEL.approx_on_disk_bytes),
            logical_cpus: 4,
            gpu: HardwareSupport::Unknown,
            webgpu: HardwareSupport::Unknown,
            runtime_dependencies: InstallStatus::NotInstalled,
            install_status: InstallStatus::NotInstalled,
            model: PRIVACY_FILTER_MODEL.into(),
        };
        assert!(report.meets_min_ram());
        assert!(report.meets_min_disk());
        assert!(report.meets_hardware_requirements());

        report.total_ram_bytes = PRIVACY_FILTER_MODEL.min_recommended_ram_bytes - 1;
        assert!(!report.meets_min_ram());
        assert!(!report.meets_hardware_requirements());

        report.total_ram_bytes = PRIVACY_FILTER_MODEL.min_recommended_ram_bytes;
        report.free_disk_bytes = None;
        assert!(!report.meets_min_disk());
        assert!(!report.meets_hardware_requirements());
    }

    #[test]
    fn collect_readiness_reports_not_installed_and_no_network() {
        // Uses a path guaranteed to have a filesystem root prefix.
        let report = collect_readiness(Path::new("/"));
        assert_eq!(report.install_status, InstallStatus::NotInstalled);
        assert_eq!(report.runtime_dependencies, InstallStatus::NotInstalled);
        assert_eq!(report.model.repo_id, "openai/privacy-filter");
    }
}
