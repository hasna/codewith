use super::*;
use codex_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;

#[test]
fn resume_parses_prompt_after_global_flags() {
    const PROMPT: &str = "echo resume-with-global-flags-after-subcommand";
    let cli = Cli::parse_from([
        "codex-exec",
        "resume",
        "--last",
        "--json",
        "--model",
        "gpt-5.2-codex",
        "--auth-profile",
        "account005",
        "--sandbox",
        "read-only",
        "-C",
        "/tmp/resume-cwd",
        "--add-dir",
        "/tmp/resume-add-dir",
        "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check",
        "--ephemeral",
        "--ignore-user-config",
        "--ignore-rules",
        PROMPT,
    ]);

    assert!(cli.ephemeral);
    assert!(cli.ignore_user_config);
    assert!(cli.ignore_rules);
    assert_eq!(cli.shared.model.as_deref(), Some("gpt-5.2-codex"));
    assert_eq!(cli.shared.auth_profile.as_deref(), Some("account005"));
    assert!(matches!(
        cli.shared.sandbox_mode,
        Some(codex_utils_cli::SandboxModeCliArg::ReadOnly)
    ));
    assert_eq!(cli.shared.cwd, Some(PathBuf::from("/tmp/resume-cwd")));
    assert_eq!(
        cli.shared.add_dir,
        vec![PathBuf::from("/tmp/resume-add-dir")]
    );
    let Some(Command::Resume(args)) = cli.command else {
        panic!("expected resume command");
    };
    let effective_prompt = args.prompt.clone().or_else(|| {
        if args.last {
            args.session_id.clone()
        } else {
            None
        }
    });
    assert_eq!(effective_prompt.as_deref(), Some(PROMPT));
}

#[test]
fn durable_parses_as_explicit_persistent_mode() {
    let cli = Cli::parse_from(["codex-exec", "--durable", "summarize"]);

    assert!(cli.durable);
    assert!(!cli.ephemeral);

    let cli = Cli::parse_from(["codex-exec", "--persist", "summarize"]);

    assert!(cli.durable);
    assert!(!cli.ephemeral);
}

#[test]
fn durable_conflicts_with_ephemeral() {
    let error = Cli::try_parse_from(["codex-exec", "--durable", "--ephemeral", "summarize"])
        .expect_err("storage modes should be mutually exclusive");

    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}

#[test]
fn resume_accepts_output_flags_after_subcommand() {
    const PROMPT: &str = "echo resume-with-output-file";
    let cli = Cli::parse_from([
        "codex-exec",
        "resume",
        "session-123",
        "-o",
        "/tmp/resume-output.md",
        "--output-schema",
        "/tmp/schema.json",
        PROMPT,
    ]);

    assert_eq!(
        cli.last_message_file,
        Some(PathBuf::from("/tmp/resume-output.md"))
    );
    assert_eq!(cli.output_schema, Some(PathBuf::from("/tmp/schema.json")));
    let Some(Command::Resume(args)) = cli.command else {
        panic!("expected resume command");
    };
    assert_eq!(args.session_id.as_deref(), Some("session-123"));
    assert_eq!(args.prompt.as_deref(), Some(PROMPT));
}

#[test]
fn parses_config_isolation_flags() {
    let cli = Cli::parse_from([
        "codex-exec",
        "--ignore-user-config",
        "--ignore-rules",
        "summarize",
    ]);

    assert!(cli.ignore_user_config);
    assert!(cli.ignore_rules);
}

#[test]
fn parses_hidden_skill_input() {
    let cli = Cli::parse_from([
        "codex-exec",
        "--skill",
        "codewith-self-heal=/tmp/codewith-self-heal/SKILL.md",
        "diagnose",
    ]);

    assert_eq!(
        cli.skill_inputs,
        vec![UserInput::Skill {
            name: "codewith-self-heal".to_string(),
            path: PathBuf::from("/tmp/codewith-self-heal/SKILL.md"),
        }]
    );
}

#[test]
fn removed_full_auto_flag_reports_migration_path() {
    let cli = Cli::parse_from(["codex-exec", "--full-auto", "summarize"]);

    assert_eq!(
        cli.removed_full_auto_warning(),
        Some("warning: `--full-auto` is deprecated; use `--sandbox workspace-write` instead.")
    );
}
