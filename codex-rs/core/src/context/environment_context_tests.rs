use crate::shell::ShellType;

use super::*;
use codex_protocol::SessionId;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::permissions::project_roots_glob_pattern;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::TurnContextItem;
use codex_utils_absolute_path::test_support::PathBufExt;
use core_test_support::test_path_buf;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::path::PathBuf;

fn fake_shell_name() -> String {
    let shell = crate::shell::Shell {
        shell_type: ShellType::Bash,
        shell_path: PathBuf::from("/bin/bash"),
        shell_snapshot: crate::shell::empty_shell_snapshot_receiver(),
    };
    shell.name().to_string()
}

fn test_abs_path(unix_path: &str) -> AbsolutePathBuf {
    test_path_buf(unix_path).abs()
}

#[test]
fn serialize_workspace_write_environment_context() {
    let cwd = test_path_buf("/repo");
    let context = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: cwd.abs(),
            shell: fake_shell_name(),
        }],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{cwd}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
</environment_context>"#,
        cwd = cwd.display(),
    );

    assert_eq!(context.render(), expected);
}

#[test]
fn serialize_environment_context_with_network() {
    let network = NetworkContext::new(
        vec!["api.example.com".to_string(), "*.openai.com".to_string()],
        vec!["blocked.example.com".to_string()],
    );
    let context = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_path_buf("/repo").abs(),
            shell: fake_shell_name(),
        }],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        Some(network),
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
  <network enabled="true"><allowed>api.example.com,*.openai.com</allowed><denied>blocked.example.com</denied></network>
</environment_context>"#,
        test_path_buf("/repo").display()
    );

    assert_eq!(context.render(), expected);
}

#[test]
fn serialize_environment_context_with_machine() {
    let mut context = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_path_buf("/repo").abs(),
            shell: fake_shell_name(),
        }],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );
    context.machine = MachineContext::from_id_and_name(
        "machine-<&\"'>".to_string(),
        Some("spark<&\"'>".to_string()),
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
  <machine><id>machine-&lt;&amp;&quot;&apos;&gt;</id><name>spark&lt;&amp;&quot;&apos;&gt;</name></machine>
</environment_context>"#,
        test_path_buf("/repo").display()
    );

    assert_eq!(context.render(), expected);
}

#[test]
fn serialize_environment_context_with_xml_safe_session() {
    let mut context = EnvironmentContext::new(
        Vec::new(),
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    context.session = SessionContext::new(
        Some("profile-<&\"'>".to_string()),
        Some("tmux-<&\"'>".to_string()),
        Some("window-<&\"'>".to_string()),
    );

    let expected = r#"<environment_context>
  <session><profile_id>profile-&lt;&amp;&quot;&apos;&gt;</profile_id><tmux_session>tmux-&lt;&amp;&quot;&apos;&gt;</tmux_session><tmux_window>window-&lt;&amp;&quot;&apos;&gt;</tmux_window></session>
</environment_context>"#;

    assert_eq!(context.render(), expected);
}

fn workspace_write_permission_profile_with_private_denials() -> PermissionProfile {
    PermissionProfile::from_runtime_permissions(
        &FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
                },
                access: FileSystemAccessMode::Write,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: FileSystemSpecialPath::project_roots(Some(PathBuf::from("private"))),
                },
                access: FileSystemAccessMode::Deny,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::GlobPattern {
                    pattern: project_roots_glob_pattern(Path::new("private/**")),
                },
                access: FileSystemAccessMode::Deny,
            },
        ]),
        NetworkSandboxPolicy::Restricted,
    )
}

#[test]
fn serialize_environment_context_with_full_filesystem_profile() {
    let repo = test_abs_path("/repo");
    let other_repo = test_abs_path("/other-repo");
    let repo_private = repo.join("private");
    let other_repo_private = other_repo.join("private");
    let repo_private_glob =
        AbsolutePathBuf::resolve_path_against_base(Path::new("private/**"), repo.as_path());
    let other_repo_private_glob =
        AbsolutePathBuf::resolve_path_against_base(Path::new("private/**"), other_repo.as_path());
    let mut context = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_path_buf("/repo").abs(),
            shell: fake_shell_name(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    context.filesystem = Some(FileSystemContext::from_permission_profile(
        &workspace_write_permission_profile_with_private_denials(),
        &[repo.clone(), other_repo.clone()],
    ));

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <filesystem><workspace_roots><root>{repo}</root><root>{other_repo}</root></workspace_roots><permission_profile type="managed"><file_system type="restricted"><entry access="write"><path>{repo}</path></entry><entry access="write"><path>{other_repo}</path></entry><entry access="deny" escalatable="false"><path>{repo_private}</path></entry><entry access="deny" escalatable="false"><path>{other_repo_private}</path></entry><entry access="deny" escalatable="false"><glob>{repo_private_glob}</glob></entry><entry access="deny" escalatable="false"><glob>{other_repo_private_glob}</glob></entry></file_system></permission_profile></filesystem>
</environment_context>"#,
        test_path_buf("/repo").display(),
        repo = repo.to_string_lossy(),
        other_repo = other_repo.to_string_lossy(),
        repo_private = repo_private.to_string_lossy(),
        other_repo_private = other_repo_private.to_string_lossy(),
        repo_private_glob = repo_private_glob.to_string_lossy(),
        other_repo_private_glob = other_repo_private_glob.to_string_lossy(),
    );

    assert_eq!(context.render(), expected);
}

#[test]
fn turn_context_item_filesystem_uses_workspace_roots_instead_of_cwd() {
    let repo = test_abs_path("/repo");
    let other_repo = test_abs_path("/other-repo");
    let repo_private = repo.join("private");
    let item = TurnContextItem {
        thread_id: None,
        turn_id: None,
        session_id: Some(
            SessionId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")
                .expect("valid session id"),
        ),
        profile_id: Some("workspace".to_string()),
        tmux_session: Some("codewith".to_string()),
        tmux_window: Some("implementation".to_string()),
        cwd: test_path_buf("/not-the-workspace"),
        workspace_roots: Some(vec![repo.clone(), other_repo.clone()]),
        current_date: None,
        timezone: None,
        machine_id: Some("machine-123".to_string()),
        machine_name: Some("spark01".to_string()),
        approval_policy: AskForApproval::Never,
        sandbox_policy: SandboxPolicy::new_read_only_policy(),
        permission_profile: Some(workspace_write_permission_profile_with_private_denials()),
        network: None,
        file_system_sandbox_policy: None,
        model: "gpt-5".to_string(),
        model_provider_id: None,
        personality: None,
        collaboration_mode: None,
        session_prompt: None,
        worktree_mode: codex_protocol::protocol::SessionWorktreeMode::Manual,
        multi_agent_version: None,
        auth_profile: None,
        realtime_active: None,
        effort: None,
        summary: codex_protocol::config_types::ReasoningSummary::Auto,
    };

    let context = EnvironmentContext::from_turn_context_item(&item, fake_shell_name()).render();

    assert!(
        context.contains("<machine><id>machine-123</id><name>spark01</name></machine>"),
        "{context}"
    );
    assert!(
        context.contains(
            "<session><profile_id>workspace</profile_id><tmux_session>codewith</tmux_session><tmux_window>implementation</tmux_window></session>"
        ),
        "{context}"
    );
    assert!(
        !context.contains("67e55044-10b1-426f-9247-bb680e5fe0c8"),
        "session id must stay out of the model-visible context: {context}"
    );
    assert!(
        context.contains(&format!(
            "<root>{}</root><root>{}</root>",
            repo.to_string_lossy(),
            other_repo.to_string_lossy()
        )),
        "{context}"
    );
    assert!(
        context.contains(&format!("<path>{}</path>", repo_private.to_string_lossy())),
        "{context}"
    );
    assert!(
        !context.contains(
            test_abs_path("/not-the-workspace")
                .join("private")
                .to_string_lossy()
                .as_ref()
        ),
        "{context}"
    );
}

#[test]
fn serialize_read_only_environment_context() {
    let context = EnvironmentContext::new(
        Vec::new(),
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = r#"<environment_context>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
</environment_context>"#;

    assert_eq!(context.render(), expected);
}

#[test]
fn equals_except_shell_compares_cwd() {
    let context1 = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_abs_path("/repo"),
            shell: fake_shell_name(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    let context2 = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_abs_path("/repo"),
            shell: fake_shell_name(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    assert!(context1.equals_except_shell(&context2));
}

#[test]
fn equals_except_shell_compares_cwd_differences() {
    let context1 = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_abs_path("/repo1"),
            shell: fake_shell_name(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    let context2 = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_abs_path("/repo2"),
            shell: fake_shell_name(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );

    assert!(!context1.equals_except_shell(&context2));
}

#[test]
fn equals_except_shell_ignores_shell() {
    let context1 = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_abs_path("/repo"),
            shell: "bash".to_string(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    let context2 = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "other".to_string(),
            cwd: test_abs_path("/repo"),
            shell: "zsh".to_string(),
        }],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );

    assert!(context1.equals_except_shell(&context2));
}

#[test]
fn equals_except_shell_detects_tmux_window_rename() {
    let mut before = EnvironmentContext::new(
        Vec::new(),
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );
    before.session = SessionContext::new(
        Some("workspace".to_string()),
        Some("codewith".to_string()),
        Some("before".to_string()),
    );
    let mut after = before.clone();
    after.session.as_mut().expect("session context").tmux_window = Some("after".to_string());

    assert!(!before.equals_except_shell(&after));
}

#[test]
fn turn_context_item_session_id_alone_is_not_model_visible() {
    let item = TurnContextItem {
        thread_id: None,
        turn_id: None,
        session_id: Some(
            SessionId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")
                .expect("valid session id"),
        ),
        profile_id: None,
        tmux_session: None,
        tmux_window: None,
        cwd: test_path_buf("/workspace"),
        workspace_roots: None,
        current_date: None,
        timezone: None,
        machine_id: None,
        machine_name: None,
        approval_policy: AskForApproval::Never,
        sandbox_policy: SandboxPolicy::new_read_only_policy(),
        permission_profile: None,
        network: None,
        file_system_sandbox_policy: None,
        model: "gpt-5".to_string(),
        model_provider_id: None,
        personality: None,
        collaboration_mode: None,
        session_prompt: None,
        worktree_mode: codex_protocol::protocol::SessionWorktreeMode::Manual,
        multi_agent_version: None,
        auth_profile: None,
        realtime_active: None,
        effort: None,
        summary: codex_protocol::config_types::ReasoningSummary::Auto,
    };

    let context = EnvironmentContext::from_turn_context_item(&item, fake_shell_name()).render();

    assert!(!context.contains("<session>"), "{context}");
    assert!(
        !context.contains("67e55044-10b1-426f-9247-bb680e5fe0c8"),
        "{context}"
    );
}

#[test]
fn serialize_environment_context_with_subagents() {
    let context = EnvironmentContext::new(
        vec![EnvironmentContextEnvironment {
            id: "local".to_string(),
            cwd: test_path_buf("/repo").abs(),
            shell: fake_shell_name(),
        }],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        Some("- agent-1: atlas\n- agent-2".to_string()),
    );

    let expected = format!(
        r#"<environment_context>
  <cwd>{}</cwd>
  <shell>bash</shell>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
  <subagents>
    - agent-1: atlas
    - agent-2
  </subagents>
</environment_context>"#,
        test_path_buf("/repo").display()
    );

    assert_eq!(context.render(), expected);
}

#[test]
fn serialize_environment_context_with_multiple_selected_environments() {
    let local_cwd = test_path_buf("/repo/local");
    let remote_cwd = test_path_buf("/repo/remote");
    let context = EnvironmentContext::new(
        vec![
            EnvironmentContextEnvironment {
                id: "local".to_string(),
                cwd: local_cwd.abs(),
                shell: "bash".to_string(),
            },
            EnvironmentContextEnvironment {
                id: "remote".to_string(),
                cwd: remote_cwd.abs(),
                shell: "bash".to_string(),
            },
        ],
        Some("2026-02-26".to_string()),
        Some("America/Los_Angeles".to_string()),
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <environments>
    <environment id="local">
      <cwd>{}</cwd>
      <shell>bash</shell>
    </environment>
    <environment id="remote">
      <cwd>{}</cwd>
      <shell>bash</shell>
    </environment>
  </environments>
  <current_date>2026-02-26</current_date>
  <timezone>America/Los_Angeles</timezone>
</environment_context>"#,
        local_cwd.display(),
        remote_cwd.display()
    );

    assert_eq!(context.render(), expected);
}

#[test]
fn serialize_environment_context_prefers_environment_shell_when_present() {
    let local_cwd = test_path_buf("/repo/local");
    let remote_cwd = test_path_buf("/repo/remote");
    let context = EnvironmentContext::new(
        vec![
            EnvironmentContextEnvironment {
                id: "local".to_string(),
                cwd: local_cwd.abs(),
                shell: "powershell".to_string(),
            },
            EnvironmentContextEnvironment {
                id: "remote".to_string(),
                cwd: remote_cwd.abs(),
                shell: "cmd".to_string(),
            },
        ],
        /*current_date*/ None,
        /*timezone*/ None,
        /*network*/ None,
        /*subagents*/ None,
    );

    let expected = format!(
        r#"<environment_context>
  <environments>
    <environment id="local">
      <cwd>{}</cwd>
      <shell>powershell</shell>
    </environment>
    <environment id="remote">
      <cwd>{}</cwd>
      <shell>cmd</shell>
    </environment>
  </environments>
</environment_context>"#,
        local_cwd.display(),
        remote_cwd.display()
    );

    assert_eq!(context.render(), expected);
}
