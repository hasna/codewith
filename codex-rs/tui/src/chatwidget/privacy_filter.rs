//! `/privacy-filter` (alias `/redaction`): local readiness check and opt-in
//! toggle for the optional Privacy Filter secret-redaction layer.
//!
//! This slice is intentionally limited to a **local readiness report** plus a
//! persisted on/off toggle. It performs no network I/O, downloads no model, and
//! handles no secret material. The on-device classifier install and composer
//! redaction are tracked as follow-up work.

use codex_privacy_filter::HardwareSupport;
use codex_privacy_filter::InstallStatus;
use codex_privacy_filter::ReadinessReport;
use codex_privacy_filter::collect_readiness;

use super::*;

const PRIVACY_FILTER_CONFIG_KEY: &str = "privacy_filter.enabled";
const PRIVACY_FILTER_LABEL: &str = "Privacy Filter";
const PRIVACY_FILTER_USAGE: &str = "Usage: /privacy-filter [on|off|status]";

/// Parsed `/privacy-filter` sub-command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrivacyFilterCommand {
    Status,
    Enable,
    Disable,
}

impl PrivacyFilterCommand {
    pub(super) fn parse(args: &str) -> Result<Self, &'static str> {
        match args.trim().to_ascii_lowercase().as_str() {
            "" | "status" => Ok(Self::Status),
            "on" | "enable" | "enabled" => Ok(Self::Enable),
            "off" | "disable" | "disabled" => Ok(Self::Disable),
            _ => Err(PRIVACY_FILTER_USAGE),
        }
    }
}

impl ChatWidget {
    pub(super) fn apply_privacy_filter_command(&mut self, command: PrivacyFilterCommand) {
        match command {
            PrivacyFilterCommand::Status => self.render_privacy_filter_report(),
            PrivacyFilterCommand::Enable => self.set_privacy_filter_enabled(/*enabled*/ true),
            PrivacyFilterCommand::Disable => {
                self.set_privacy_filter_enabled(/*enabled*/ false)
            }
        }
    }

    /// Render the full readiness + status report shown for `/privacy-filter` and
    /// `/privacy-filter status`.
    pub(super) fn render_privacy_filter_report(&mut self) {
        let enabled = self.config.privacy_filter.enabled;
        let report = collect_readiness(self.config.codex_home.as_path());
        let message = render_privacy_filter_message(enabled, &report);
        self.add_info_message(
            message,
            /*hint*/ Some(PRIVACY_FILTER_USAGE.to_string()),
        );
    }

    fn set_privacy_filter_enabled(&mut self, enabled: bool) {
        // Reflect immediately in the in-memory config so a follow-up `status`
        // in the same session is accurate...
        self.config.privacy_filter.enabled = enabled;
        // ...and persist to config.toml via the app server.
        self.app_event_tx.send(AppEvent::UpdateConfigValue {
            key_path: PRIVACY_FILTER_CONFIG_KEY.to_string(),
            value: serde_json::Value::Bool(enabled),
            label: PRIVACY_FILTER_LABEL.to_string(),
        });

        let notice = if enabled {
            "Privacy Filter enabled (opt-in)."
        } else {
            "Privacy Filter disabled."
        };
        let hint = if enabled {
            // Be explicit that enabling the toggle does not yet redact anything:
            // the classifier install/inference is a separate, not-yet-shipped step.
            "The local classifier is not installed yet, so secrets are not redacted. \
             Run /privacy-filter status to check local readiness. Everything stays on-device."
        } else {
            "Secrets will not be classified locally. Re-enable any time with /privacy-filter on."
        };
        self.add_info_message(notice.to_string(), /*hint*/ Some(hint.to_string()));
    }
}

fn render_privacy_filter_message(enabled: bool, report: &ReadinessReport) -> String {
    let mut lines: Vec<String> = Vec::new();

    lines.push(format!(
        "Privacy Filter is currently {}.",
        if enabled { "ENABLED" } else { "DISABLED" }
    ));
    lines.push(String::new());

    lines.push("Local readiness".to_string());
    lines.push(format!(
        "  [{}] RAM: {} total, {} available (needs ~{})",
        readiness_label(report.meets_min_ram()),
        format_bytes(report.total_ram_bytes),
        format_bytes(report.available_ram_bytes),
        format_bytes(report.model.min_recommended_ram_bytes),
    ));
    lines.push(format!(
        "  [{}] Disk: {} free (needs ~{})",
        readiness_label(report.meets_min_disk()),
        report
            .free_disk_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "unknown".to_string()),
        format_bytes(report.model.approx_on_disk_bytes),
    ));
    lines.push(format!("  - CPU: {} logical cores", report.logical_cpus));
    lines.push(format!("  - GPU: {}", hardware_label(report.gpu)));
    lines.push(format!("  - WebGPU: {}", hardware_label(report.webgpu)));
    lines.push(format!(
        "  - Runtime dependencies: {}",
        install_label(report.runtime_dependencies)
    ));
    lines.push(format!(
        "  - Classifier weights: {}",
        install_label(report.install_status)
    ));
    lines.push(String::new());

    lines.push("Model".to_string());
    lines.push(format!(
        "  - {} ({})",
        report.model.display_name, report.model.repo_id
    ));
    lines.push(format!(
        "  - {} params, {} license, {}k token context",
        report.model.approx_parameters,
        report.model.license,
        report.model.context_window_tokens / 1000,
    ));
    lines.push(String::new());

    if report.meets_hardware_requirements() {
        lines
            .push("This machine meets the minimum hardware bar to run the classifier.".to_string());
    } else {
        lines.push(
            "This machine may not meet the minimum hardware bar; enabling is still allowed."
                .to_string(),
        );
    }
    lines.push(String::new());

    lines.push("Local-only guarantee".to_string());
    lines.push(
        "  Readiness checks read host stats only. No secrets, no network I/O, no model download."
            .to_string(),
    );
    lines.push(String::new());

    lines.push("Limitations (this build)".to_string());
    lines.push(
        "  - The on-device classifier is not installed yet, so nothing is redacted.".to_string(),
    );
    lines.push(
        "  - Composer paste classification and reversible placeholders ship in a later update."
            .to_string(),
    );
    lines.push(
        "  - A classifier can miss secrets or flag false positives; never rely on it alone."
            .to_string(),
    );
    lines.push(String::new());

    lines.push("Controls".to_string());
    lines.push("  - /privacy-filter on    enable (opt-in)".to_string());
    lines.push("  - /privacy-filter off   disable".to_string());
    lines.push("  - /privacy-filter status  re-show this report".to_string());

    lines.join("\n")
}

fn readiness_label(ok: bool) -> &'static str {
    if ok { "ok" } else { "warn" }
}

fn hardware_label(support: HardwareSupport) -> &'static str {
    match support {
        HardwareSupport::Detected => "detected",
        HardwareSupport::Unavailable => "unavailable",
        HardwareSupport::Unknown => "unknown (not probed in this build)",
    }
}

fn install_label(status: InstallStatus) -> &'static str {
    match status {
        InstallStatus::Installed => "installed",
        InstallStatus::NotInstalled => "not installed",
        InstallStatus::Unknown => "unknown",
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_privacy_filter::PRIVACY_FILTER_MODEL;
    use pretty_assertions::assert_eq;

    fn sample_report() -> ReadinessReport {
        ReadinessReport {
            total_ram_bytes: 16 * 1024 * 1024 * 1024,
            available_ram_bytes: 8 * 1024 * 1024 * 1024,
            free_disk_bytes: Some(200 * 1024 * 1024 * 1024),
            logical_cpus: 10,
            gpu: HardwareSupport::Unknown,
            webgpu: HardwareSupport::Unknown,
            runtime_dependencies: InstallStatus::NotInstalled,
            install_status: InstallStatus::NotInstalled,
            model: PRIVACY_FILTER_MODEL.into(),
        }
    }

    #[test]
    fn parse_accepts_on_off_status_and_aliases() {
        assert_eq!(
            PrivacyFilterCommand::parse(""),
            Ok(PrivacyFilterCommand::Status)
        );
        assert_eq!(
            PrivacyFilterCommand::parse("status"),
            Ok(PrivacyFilterCommand::Status)
        );
        assert_eq!(
            PrivacyFilterCommand::parse("on"),
            Ok(PrivacyFilterCommand::Enable)
        );
        assert_eq!(
            PrivacyFilterCommand::parse("ENABLE"),
            Ok(PrivacyFilterCommand::Enable)
        );
        assert_eq!(
            PrivacyFilterCommand::parse("off"),
            Ok(PrivacyFilterCommand::Disable)
        );
        assert!(PrivacyFilterCommand::parse("bogus").is_err());
    }

    #[test]
    fn report_states_local_only_and_not_installed() {
        let message = render_privacy_filter_message(false, &sample_report());
        insta::assert_snapshot!(
            message,
            @r###"
Privacy Filter is currently DISABLED.

Local readiness
  [ok] RAM: 16.0 GiB total, 8.0 GiB available (needs ~4.0 GiB)
  [ok] Disk: 200.0 GiB free (needs ~3.0 GiB)
  - CPU: 10 logical cores
  - GPU: unknown (not probed in this build)
  - WebGPU: unknown (not probed in this build)
  - Runtime dependencies: not installed
  - Classifier weights: not installed

Model
  - OpenAI Privacy Filter (openai/privacy-filter)
  - ~1.5B total (~50M active per token) params, Apache-2.0 license, 128k token context

This machine meets the minimum hardware bar to run the classifier.

Local-only guarantee
  Readiness checks read host stats only. No secrets, no network I/O, no model download.

Limitations (this build)
  - The on-device classifier is not installed yet, so nothing is redacted.
  - Composer paste classification and reversible placeholders ship in a later update.
  - A classifier can miss secrets or flag false positives; never rely on it alone.

Controls
  - /privacy-filter on    enable (opt-in)
  - /privacy-filter off   disable
  - /privacy-filter status  re-show this report
"###
        );
    }

    #[test]
    fn format_bytes_uses_binary_units() {
        assert_eq!(format_bytes(4 * 1024 * 1024 * 1024), "4.0 GiB");
        assert_eq!(format_bytes(512), "512 B");
    }
}
