use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fs;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_config::CONFIG_TOML_FILE;
use codex_core::config::edit::ConfigEditsBuilder;
use codex_core::config::find_codex_home;
use codex_exec::Cli as ExecCli;
use codex_tui::Cli as TuiCli;
use codex_utils_cli::CliConfigOverrides;

use super::CheckStatus;
use super::DoctorCommand;
use super::DoctorReport;
use super::human_output_options;
use super::mcp_check_from_servers;
use super::redacted_json_report;
use super::render_human_report;

const SELF_HEAL_SKILL_NAME: &str = "codewith-self-heal";

pub(super) async fn run_self_heal(
    command: DoctorCommand,
    report: DoctorReport,
    root_config_overrides: CliConfigOverrides,
    interactive: &TuiCli,
    arg0_paths: &Arg0DispatchPaths,
) -> anyhow::Result<()> {
    print!(
        "{}",
        render_human_report(&report, human_output_options(&command))
    );

    if !report_has_self_heal_issue(&report) {
        eprintln!("No config.toml or MCP issue was found for self-heal.");
        return Ok(());
    }

    let codex_home = find_codex_home().context("find CODEWITH_HOME for self-heal")?;
    let plan = propose_repairs(&report, codex_home.as_path())
        .await
        .context("build self-heal repair plan")?;

    if command.self_heal_apply {
        print_plan(&plan);
        let confirmed = command.yes || prompt_for_confirmation()?;
        let outcome = apply_repairs(&plan, confirmed)
            .await
            .context("apply self-heal repairs")?;
        print_apply_outcome(&outcome);
    }

    let prompt = build_self_heal_prompt(&report, &plan.config_path)?;
    eprintln!("Starting Codewith self-heal turn with redacted diagnostics.");
    run_self_heal_exec(
        prompt,
        codex_home.as_path(),
        root_config_overrides,
        interactive,
        arg0_paths,
    )
    .await
}

pub(super) fn report_has_self_heal_issue(report: &DoctorReport) -> bool {
    report.checks.iter().any(|check| {
        check.status != CheckStatus::Ok
            && (check.category == "mcp"
                || (check.id == "config.load" && check.status == CheckStatus::Fail)
                || check
                    .details
                    .iter()
                    .any(|detail| detail.starts_with("startup warning MCP: ")))
    })
}

async fn run_self_heal_exec(
    prompt: String,
    codex_home: &Path,
    root_config_overrides: CliConfigOverrides,
    interactive: &TuiCli,
    arg0_paths: &Arg0DispatchPaths,
) -> anyhow::Result<()> {
    let args = build_self_heal_exec_args(prompt, codex_home, root_config_overrides, interactive);
    let exec_cli = ExecCli::try_parse_from(args)?;
    codex_exec::run_main(exec_cli, arg0_paths.clone()).await
}

fn build_self_heal_exec_args(
    prompt: String,
    codex_home: &Path,
    root_config_overrides: CliConfigOverrides,
    interactive: &TuiCli,
) -> Vec<String> {
    let skill_path = system_skill_path(codex_home);
    let mut args = vec![
        "codewith".to_string(),
        "--ignore-user-config".to_string(),
        "--skip-git-repo-check".to_string(),
        "--sandbox".to_string(),
        "workspace-write".to_string(),
        "--skill".to_string(),
        format!("{SELF_HEAL_SKILL_NAME}={}", skill_path.display()),
    ];
    if let Some(cwd) = interactive.cwd.as_ref() {
        args.push("--cd".to_string());
        args.push(cwd.display().to_string());
    }
    for raw_override in root_config_overrides.raw_overrides {
        args.push("-c".to_string());
        args.push(raw_override);
    }
    args.push(prompt);
    args
}

fn system_skill_path(codex_home: &Path) -> PathBuf {
    codex_home
        .join("skills")
        .join(".system")
        .join(SELF_HEAL_SKILL_NAME)
        .join("SKILL.md")
}

fn build_self_heal_prompt(report: &DoctorReport, config_path: &Path) -> anyhow::Result<String> {
    let redacted_report = serde_json::to_string_pretty(&redacted_json_report(report))?;
    Ok(format!(
        "Run the Codewith self-heal workflow for config.toml and MCP breakage.\n\
         \n\
         Use the attached `{SELF_HEAL_SKILL_NAME}` skill. Diagnose first, propose changes before \
         writing, ask for confirmation before editing config, create a timestamped backup before \
         any write, avoid exposing secrets, and rerun validation afterward.\n\
         \n\
         Config path: {}\n\
         \n\
         Redacted doctor report:\n```json\n{redacted_report}\n```",
        config_path.display()
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RepairPlan {
    codex_home: PathBuf,
    config_path: PathBuf,
    repairs: Vec<ProposedRepair>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ProposedRepair {
    ReplaceMalformedConfig,
    DisableOptionalMcpServers { names: Vec<String> },
}

impl ProposedRepair {
    fn description(&self) -> String {
        match self {
            ProposedRepair::ReplaceMalformedConfig => {
                "Back up malformed config.toml and replace it with a minimal valid config."
                    .to_string()
            }
            ProposedRepair::DisableOptionalMcpServers { names } => format!(
                "Disable optional MCP server(s) with doctor failures: {}.",
                names.join(", ")
            ),
        }
    }
}

async fn propose_repairs(report: &DoctorReport, codex_home: &Path) -> anyhow::Result<RepairPlan> {
    let config_path = codex_home.join(CONFIG_TOML_FILE);
    let mut repairs = Vec::new();

    if report_has_config_load_failure(report)
        && config_path.exists()
        && config_file_is_malformed(&config_path)?
    {
        repairs.push(ProposedRepair::ReplaceMalformedConfig);
        return Ok(RepairPlan {
            codex_home: codex_home.to_path_buf(),
            config_path,
            repairs,
        });
    }

    let names = optional_mcp_servers_to_disable(report, codex_home).await?;
    if !names.is_empty() {
        repairs.push(ProposedRepair::DisableOptionalMcpServers { names });
    }

    Ok(RepairPlan {
        codex_home: codex_home.to_path_buf(),
        config_path,
        repairs,
    })
}

fn report_has_config_load_failure(report: &DoctorReport) -> bool {
    report.checks.iter().any(|check| {
        check.id == "config.load" && check.category == "config" && check.status == CheckStatus::Fail
    })
}

fn config_file_is_malformed(config_path: &Path) -> anyhow::Result<bool> {
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("read {}", config_path.display()))?;
    Ok(toml::from_str::<toml::Value>(&raw).is_err())
}

async fn optional_mcp_servers_to_disable(
    report: &DoctorReport,
    codex_home: &Path,
) -> anyhow::Result<Vec<String>> {
    let problem_names = mcp_problem_server_names(report);
    if problem_names.is_empty() {
        return Ok(Vec::new());
    }

    let servers = codex_core::config::load_global_mcp_servers(codex_home)
        .await
        .with_context(|| format!("load MCP servers from {}", codex_home.display()))?;
    let mut names = problem_names
        .into_iter()
        .filter(|name| {
            servers
                .get(name)
                .is_some_and(|server| !server.required && server.enabled)
        })
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

fn mcp_problem_server_names(report: &DoctorReport) -> BTreeSet<String> {
    report
        .checks
        .iter()
        .filter(|check| check.id == "mcp.config" && check.status != CheckStatus::Ok)
        .flat_map(|check| check.details.iter())
        .filter_map(|detail| {
            let detail = detail
                .strip_prefix("optional reachability failed: ")
                .unwrap_or(detail);
            let (name, _) = detail.split_once(':')?;
            let name = name.trim();
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum ApplyOutcome {
    NoRepairs,
    ConfirmationRequired,
    Applied {
        backup_path: PathBuf,
        applied: Vec<String>,
        validation: Vec<String>,
    },
}

async fn apply_repairs(plan: &RepairPlan, confirmed: bool) -> anyhow::Result<ApplyOutcome> {
    if plan.repairs.is_empty() {
        return Ok(ApplyOutcome::NoRepairs);
    }
    if !confirmed {
        return Ok(ApplyOutcome::ConfirmationRequired);
    }

    let backup_path = backup_config(&plan.config_path)?;
    let mut applied = Vec::new();
    let mut validation = Vec::new();

    for repair in &plan.repairs {
        match repair {
            ProposedRepair::ReplaceMalformedConfig => {
                fs::write(&plan.config_path, recovered_config_contents())
                    .with_context(|| format!("write {}", plan.config_path.display()))?;
                let raw = fs::read_to_string(&plan.config_path)
                    .with_context(|| format!("read {}", plan.config_path.display()))?;
                toml::from_str::<toml::Value>(&raw)
                    .context("recovered config.toml did not parse")?;
                applied.push(repair.description());
                validation.push("config.toml parses after recovery".to_string());
            }
            ProposedRepair::DisableOptionalMcpServers { names } => {
                let mut servers = codex_core::config::load_global_mcp_servers(&plan.codex_home)
                    .await
                    .with_context(|| {
                        format!("load MCP servers from {}", plan.codex_home.display())
                    })?;
                for name in names {
                    if let Some(server) = servers.get_mut(name) {
                        server.enabled = false;
                    }
                }
                ConfigEditsBuilder::new(&plan.codex_home)
                    .replace_mcp_servers(&servers)
                    .apply()
                    .await?;
                let validation_servers = servers.into_iter().collect::<HashMap<_, _>>();
                let check = mcp_check_from_servers(&validation_servers).await;
                validation.push(format!("doctor MCP check after repair: {}", check.summary));
                applied.push(repair.description());
            }
        }
    }

    Ok(ApplyOutcome::Applied {
        backup_path,
        applied,
        validation,
    })
}

fn recovered_config_contents() -> &'static str {
    "# Recovered by `codewith doctor --self-heal`.\n\
     # The original config.toml was backed up next to this file.\n"
}

fn backup_config(config_path: &Path) -> anyhow::Result<PathBuf> {
    let parent = config_path
        .parent()
        .with_context(|| format!("{} has no parent directory", config_path.display()))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let file_name = config_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("config path has no UTF-8 file name")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_secs();
    for sequence in 0..1000 {
        let suffix = if sequence == 0 {
            format!(".self-heal-{timestamp}.bak")
        } else {
            format!(".self-heal-{timestamp}.{sequence}.bak")
        };
        let backup_path = parent.join(format!("{file_name}{suffix}"));
        if !backup_path.exists() {
            fs::copy(config_path, &backup_path).with_context(|| {
                format!(
                    "backup {} to {}",
                    config_path.display(),
                    backup_path.display()
                )
            })?;
            return Ok(backup_path);
        }
    }
    anyhow::bail!(
        "could not create a unique backup path for {}",
        config_path.display()
    )
}

fn print_plan(plan: &RepairPlan) {
    if plan.repairs.is_empty() {
        eprintln!("No built-in self-heal repairs are available for this report.");
        return;
    }
    eprintln!("Built-in self-heal repairs proposed:");
    for repair in &plan.repairs {
        eprintln!("  - {}", repair.description());
    }
}

fn prompt_for_confirmation() -> anyhow::Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }

    eprint!("Apply the proposed built-in repairs now? [y/N] ");
    std::io::stderr().flush()?;
    let mut response = String::new();
    std::io::stdin().read_line(&mut response)?;
    Ok(matches!(response.trim(), "y" | "Y" | "yes" | "YES"))
}

fn print_apply_outcome(outcome: &ApplyOutcome) {
    match outcome {
        ApplyOutcome::NoRepairs => eprintln!("No built-in repairs were applied."),
        ApplyOutcome::ConfirmationRequired => {
            eprintln!("No repairs applied. Re-run with `--self-heal-apply --yes` to confirm.")
        }
        ApplyOutcome::Applied {
            backup_path,
            applied,
            validation,
        } => {
            eprintln!("Backed up config before repair: {}", backup_path.display());
            for item in applied {
                eprintln!("Applied: {item}");
            }
            for item in validation {
                eprintln!("Validation: {item}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn report_with_check(check: super::super::DoctorCheck) -> DoctorReport {
        DoctorReport {
            schema_version: 1,
            generated_at: "0s since unix epoch".to_string(),
            overall_status: check.status,
            codex_version: "0.0.0".to_string(),
            checks: vec![check],
        }
    }

    fn failing_config_report() -> DoctorReport {
        report_with_check(super::super::DoctorCheck::new(
            "config.load",
            "config",
            CheckStatus::Fail,
            "config could not be loaded",
        ))
    }

    #[test]
    fn self_heal_prompt_uses_redacted_diagnostics() {
        let report = report_with_check(
            super::super::DoctorCheck::new(
                "mcp.config",
                "mcp",
                CheckStatus::Warning,
                "MCP configuration has optional issues",
            )
            .detail("optional reachability failed: docs: https://user:pass@example.com/mcp?x=abc")
            .detail("OPENAI_API_KEY: sk-test-secret"),
        );

        let prompt = build_self_heal_prompt(&report, Path::new("/tmp/config.toml"))
            .expect("prompt should render");

        assert!(prompt.contains(SELF_HEAL_SKILL_NAME));
        assert!(prompt.contains("Redacted doctor report"));
        assert!(!prompt.contains("user:pass"));
        assert!(!prompt.contains("x=abc"));
        assert!(!prompt.contains("sk-test-secret"));
        assert!(prompt.contains("https://example.com/mcp"));
    }

    #[test]
    fn self_heal_trigger_ignores_unrelated_config_warnings() {
        let report = report_with_check(
            super::super::DoctorCheck::new(
                "config.load",
                "config",
                CheckStatus::Warning,
                "config loaded",
            )
            .detail("startup warning skills: 1"),
        );

        assert!(!report_has_self_heal_issue(&report));
    }

    #[tokio::test]
    async fn no_confirmation_does_not_write_or_backup() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let config_path = tmp.path().join(CONFIG_TOML_FILE);
        fs::write(&config_path, "model = \"gpt-5\"\n").expect("write config");
        let plan = RepairPlan {
            codex_home: tmp.path().to_path_buf(),
            config_path: config_path.clone(),
            repairs: vec![ProposedRepair::ReplaceMalformedConfig],
        };

        let outcome = apply_repairs(&plan, /*confirmed*/ false)
            .await
            .expect("apply should not fail");

        assert_eq!(outcome, ApplyOutcome::ConfirmationRequired);
        assert_eq!(
            fs::read_to_string(&config_path).expect("read config"),
            "model = \"gpt-5\"\n"
        );
        let backup_count = fs::read_dir(tmp.path())
            .expect("read temp dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".self-heal-"))
            .count();
        assert_eq!(backup_count, 0);
    }

    #[tokio::test]
    async fn malformed_config_recovery_writes_backup_and_valid_config() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let config_path = tmp.path().join(CONFIG_TOML_FILE);
        fs::write(&config_path, "model = \"unterminated\n").expect("write malformed config");

        let plan = propose_repairs(&failing_config_report(), tmp.path())
            .await
            .expect("plan should build");
        assert_eq!(plan.repairs, vec![ProposedRepair::ReplaceMalformedConfig]);

        let outcome = apply_repairs(&plan, /*confirmed*/ true)
            .await
            .expect("repair should apply");
        let ApplyOutcome::Applied {
            backup_path,
            applied,
            validation,
        } = outcome
        else {
            panic!("expected applied repair");
        };

        assert_eq!(
            fs::read_to_string(&backup_path).expect("read backup"),
            "model = \"unterminated\n"
        );
        let recovered = fs::read_to_string(&config_path).expect("read recovered config");
        toml::from_str::<toml::Value>(&recovered).expect("recovered config parses");
        assert_eq!(applied.len(), 1);
        assert_eq!(
            validation,
            vec!["config.toml parses after recovery".to_string()]
        );
    }

    #[tokio::test]
    async fn optional_mcp_repair_disables_server_and_writes_backup() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let config_path = tmp.path().join(CONFIG_TOML_FILE);
        fs::write(
            &config_path,
            r#"[mcp_servers.docs]
command = "definitely-missing-codewith-self-heal-test"
"#,
        )
        .expect("write config");
        let report = report_with_check(
            super::super::DoctorCheck::new(
                "mcp.config",
                "mcp",
                CheckStatus::Warning,
                "MCP configuration has optional issues",
            )
            .detail(
                "docs: stdio command \"definitely-missing-codewith-self-heal-test\" is not resolvable",
            ),
        );

        let plan = propose_repairs(&report, tmp.path())
            .await
            .expect("plan should build");
        assert_eq!(
            plan.repairs,
            vec![ProposedRepair::DisableOptionalMcpServers {
                names: vec!["docs".to_string()],
            }]
        );

        let outcome = apply_repairs(&plan, /*confirmed*/ true)
            .await
            .expect("repair should apply");
        let ApplyOutcome::Applied { backup_path, .. } = outcome else {
            panic!("expected applied repair");
        };
        assert!(backup_path.exists());
        let repaired = fs::read_to_string(&config_path).expect("read repaired config");
        assert!(repaired.contains("enabled = false"));
        assert!(
            fs::read_to_string(backup_path)
                .expect("read backup")
                .contains("definitely-missing-codewith-self-heal-test")
        );
    }

    #[test]
    fn self_heal_exec_args_include_structured_skill_input() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let interactive = TuiCli::parse_from(["codewith"]);
        let args = build_self_heal_exec_args(
            "diagnose".to_string(),
            tmp.path(),
            CliConfigOverrides::default(),
            &interactive,
        );
        let cli = ExecCli::try_parse_from(args).expect("exec args should parse");

        assert_eq!(cli.skill_inputs.len(), 1);
        assert_eq!(
            cli.skill_inputs[0],
            codex_protocol::user_input::UserInput::Skill {
                name: SELF_HEAL_SKILL_NAME.to_string(),
                path: tmp
                    .path()
                    .join("skills")
                    .join(".system")
                    .join(SELF_HEAL_SKILL_NAME)
                    .join("SKILL.md"),
            }
        );
    }
}
