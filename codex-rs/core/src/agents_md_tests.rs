use super::*;
use crate::config::ConfigBuilder;
use async_trait::async_trait;
use codex_exec_server::CopyOptions;
use codex_exec_server::CreateDirectoryOptions;
use codex_exec_server::FileMetadata;
use codex_exec_server::FileSystemResult;
use codex_exec_server::FileSystemSandboxContext;
use codex_exec_server::LOCAL_FS;
use codex_exec_server::ReadDirectoryEntry;
use codex_exec_server::RemoveOptions;
use codex_features::Feature;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::PathBufExt;
use core_test_support::TempDirExt;
use core_test_support::create_directory_symlink;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

fn write_doc(root: &Path, relative_path: &str, contents: &str) {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

#[cfg(unix)]
fn create_file_symlink(source: &Path, link: &Path) {
    std::os::unix::fs::symlink(source, link).expect("create file symlink");
}

#[cfg(windows)]
fn create_file_symlink(source: &Path, link: &Path) {
    std::os::windows::fs::symlink_file(source, link)
        .expect("create file symlink; enable Developer Mode or run the test elevated");
}

async fn get_user_instructions(config: &Config) -> Option<String> {
    let mut warnings = Vec::new();
    AgentsMdManager::new(config)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .map(|loaded| loaded.text())
}

async fn load_user_instructions(config: &Config) -> (Option<LoadedAgentsMd>, Vec<String>) {
    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(config)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await;
    (loaded, warnings)
}

async fn agents_md_paths(config: &Config) -> std::io::Result<Vec<AbsolutePathBuf>> {
    AgentsMdManager::new(config)
        .agents_md_paths(LOCAL_FS.as_ref())
        .await
}

fn assert_invalid_utf8_warning(warnings: &[String], source: &str, path: &Path) {
    let path_display = path.display().to_string();
    assert_eq!(warnings.len(), 1, "expected one warning, got {warnings:?}");
    let warning = &warnings[0];
    assert!(
        warning.contains(&format!("{source} project instructions"))
            && warning.contains(&path_display)
            && warning.contains("invalid UTF-8")
            && warning.contains("Invalid byte sequences were replaced."),
        "unexpected invalid UTF-8 warning: {warning:?}"
    );
}

fn assert_symlink_warning(
    warnings: &[String],
    source: &str,
    path: &Path,
    target: &Path,
    secret_contents: &str,
) {
    let path_display = path.display().to_string();
    let target_display = target.display().to_string();
    assert_eq!(warnings.len(), 1, "expected one warning, got {warnings:?}");
    let warning = &warnings[0];
    assert!(
        warning.contains(&format!("{source} project instructions"))
            && warning.contains(&path_display)
            && warning.contains("symlinked instruction files are not allowed"),
        "unexpected symlink warning: {warning:?}"
    );
    assert!(
        !warning.contains(&target_display) && !warning.contains(secret_contents),
        "symlink warning should not expose target path or contents: {warning:?}"
    );
}

fn assert_symlink_safe_read_unsupported_warning(
    warnings: &[String],
    source: &str,
    path: &Path,
    secret_contents: &str,
) {
    let path_display = path.display().to_string();
    assert_eq!(warnings.len(), 1, "expected one warning, got {warnings:?}");
    let warning = &warnings[0];
    assert!(
        warning.contains(&format!("{source} project instructions"))
            && warning.contains(&path_display)
            && warning.contains("does not support symlink-safe instruction reads"),
        "unexpected symlink-safe read unsupported warning: {warning:?}"
    );
    assert!(
        !warning.contains(secret_contents),
        "warning should not expose file contents: {warning:?}"
    );
}

/// Helper that returns a `Config` pointing at `root` and using `limit` as
/// the maximum number of bytes to embed from AGENTS.md. The caller can
/// optionally specify a custom `instructions` string – when `None` the
/// value is cleared to mimic a scenario where no system instructions have
/// been configured.
async fn make_config(root: &TempDir, limit: usize, instructions: Option<&str>) -> Config {
    let codex_home = TempDir::new().unwrap();
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await
        .expect("defaults for test should always succeed");

    config.cwd = root.abs();
    config.project_doc_max_bytes = limit;

    config.user_instructions = instructions.map(|text| {
        LoadedAgentsMd::new_user(
            text.to_owned(),
            config.codex_home.join(DEFAULT_AGENTS_MD_FILENAME),
        )
    });
    config
}

async fn make_config_with_fallback(
    root: &TempDir,
    limit: usize,
    instructions: Option<&str>,
    fallbacks: &[&str],
) -> Config {
    let mut config = make_config(root, limit, instructions).await;
    config.project_doc_fallback_filenames = fallbacks
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    config
}

async fn make_config_with_project_root_markers(
    root: &TempDir,
    limit: usize,
    instructions: Option<&str>,
    markers: &[&str],
) -> Config {
    let codex_home = TempDir::new().unwrap();
    let cli_overrides = vec![(
        "project_root_markers".to_string(),
        TomlValue::Array(
            markers
                .iter()
                .map(|marker| TomlValue::String((*marker).to_string()))
                .collect(),
        ),
    )];
    let mut config = ConfigBuilder::default()
        .codex_home(codex_home.path().to_path_buf())
        .cli_overrides(cli_overrides)
        .build()
        .await
        .expect("defaults for test should always succeed");

    config.cwd = root.abs();
    config.project_doc_max_bytes = limit;
    config.user_instructions = instructions.map(|text| {
        LoadedAgentsMd::new_user(
            text.to_owned(),
            config.codex_home.join(DEFAULT_AGENTS_MD_FILENAME),
        )
    });
    config
}

/// AGENTS.md missing – should yield `None`.
#[tokio::test]
async fn no_doc_file_returns_none() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let res =
        get_user_instructions(&make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await)
            .await;
    assert!(
        res.is_none(),
        "Expected None when AGENTS.md is absent and no system instructions provided"
    );
    assert!(res.is_none(), "Expected None when AGENTS.md is absent");
}

#[test]
fn empty_loaded_instructions_are_empty() {
    let source =
        AbsolutePathBuf::from_absolute_path("/tmp/AGENTS.md").expect("absolute source path");

    assert_eq!(
        LoadedAgentsMd::new_user(String::new(), source.clone()),
        LoadedAgentsMd::default()
    );
    assert_eq!(
        LoadedAgentsMd::new_user(" \n\t".to_string(), source),
        LoadedAgentsMd::default()
    );
    assert_eq!(
        LoadedAgentsMd::from_text_for_testing(String::new()),
        LoadedAgentsMd::default()
    );
    assert_eq!(
        LoadedAgentsMd::from_text_for_testing(" \n\t"),
        LoadedAgentsMd::default()
    );
}

#[test]
fn loaded_instructions_with_only_empty_or_whitespace_entries_are_empty() {
    let empty = LoadedAgentsMd {
        entries: vec![InstructionEntry {
            contents: String::new(),
            provenance: InstructionProvenance::Internal,
        }],
    };
    let whitespace = LoadedAgentsMd {
        entries: vec![InstructionEntry {
            contents: " \n\t".to_string(),
            provenance: InstructionProvenance::Internal,
        }],
    };

    assert!(empty.is_empty());
    assert!(whitespace.is_empty());
}

/// Small file within the byte-limit is returned unmodified.
#[tokio::test]
async fn doc_smaller_than_limit_is_returned() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "hello world").unwrap();

    let res =
        get_user_instructions(&make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await)
            .await
            .expect("doc expected");

    assert_eq!(
        res, "hello world",
        "The document should be returned verbatim when it is smaller than the limit and there are no existing instructions"
    );
}

#[tokio::test]
async fn global_doc_invalid_utf8_warns_and_uses_lossy_text() {
    let codex_home = tempfile::tempdir().expect("tempdir");
    let codex_home_abs = codex_home.abs();
    let path = codex_home_abs.join(DEFAULT_AGENTS_MD_FILENAME);
    fs::write(&path, b"global\xFF doc").unwrap();

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::load_global_instructions(
        LOCAL_FS.as_ref(),
        Some(&codex_home_abs),
        &mut warnings,
    )
    .await
    .expect("global doc expected");

    assert_eq!(
        loaded,
        LoadedAgentsMd::new_user("global\u{FFFD} doc".to_string(), path.clone())
    );
    assert_invalid_utf8_warning(&warnings, "Global", path.as_path());
}

#[tokio::test]
async fn project_doc_invalid_utf8_warns_and_uses_lossy_text() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("AGENTS.md");
    fs::write(&path, b"project\xFF doc").unwrap();

    let config = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let mut warnings = Vec::new();
    let res = AgentsMdManager::new(&config)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("doc expected")
        .text();

    assert_eq!(res, "project\u{FFFD} doc");
    assert_invalid_utf8_warning(&warnings, "Project", config.cwd.join("AGENTS.md").as_path());
}

#[tokio::test]
async fn global_doc_symlink_is_skipped_with_redacted_warning() {
    let codex_home = tempfile::tempdir().expect("tempdir");
    let codex_home_abs = codex_home.abs();
    let outside = tempfile::tempdir().expect("outside tempdir");
    let secret_path = outside.path().join("private-instructions.txt");
    let secret_contents = "external secret instructions";
    fs::write(&secret_path, secret_contents).unwrap();
    let link_path = codex_home.path().join(DEFAULT_AGENTS_MD_FILENAME);
    create_file_symlink(&secret_path, &link_path);

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::load_global_instructions(
        LOCAL_FS.as_ref(),
        Some(&codex_home_abs),
        &mut warnings,
    )
    .await;

    assert_eq!(loaded, None);
    assert_symlink_warning(
        &warnings,
        "Global",
        codex_home_abs.join(DEFAULT_AGENTS_MD_FILENAME).as_path(),
        &secret_path,
        secret_contents,
    );
}

#[tokio::test]
async fn project_doc_symlink_is_skipped_with_redacted_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let secret_path = outside.path().join("private-instructions.txt");
    let secret_contents = "external project secret";
    fs::write(&secret_path, secret_contents).unwrap();
    let link_path = tmp.path().join("AGENTS.md");
    create_file_symlink(&secret_path, &link_path);

    let config = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&config)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await;

    assert_eq!(loaded, None);
    assert_eq!(
        agents_md_paths(&config).await.expect("discover paths"),
        Vec::<AbsolutePathBuf>::new()
    );
    assert_symlink_warning(
        &warnings,
        "Project",
        config.cwd.join("AGENTS.md").as_path(),
        &secret_path,
        secret_contents,
    );
}

#[tokio::test]
async fn symlinked_project_doc_candidate_falls_back_to_regular_candidate() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let secret_path = outside.path().join("private-instructions.txt");
    let secret_contents = "external preferred secret";
    fs::write(&secret_path, secret_contents).unwrap();
    let link_path = tmp.path().join(DEFAULT_PROJECT_AGENTS_MD_PATH);
    fs::create_dir_all(link_path.parent().expect("link parent")).unwrap();
    create_file_symlink(&secret_path, &link_path);
    write_doc(
        tmp.path(),
        DEFAULT_AGENTS_MD_FILENAME,
        "regular project doc",
    );

    let config = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&config)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("fallback doc expected");
    let fallback_path = config.cwd.join(DEFAULT_AGENTS_MD_FILENAME);

    assert_eq!(loaded.text(), "regular project doc");
    assert_eq!(loaded.sources().collect::<Vec<_>>(), vec![&fallback_path]);
    assert_eq!(
        agents_md_paths(&config).await.expect("discover paths"),
        vec![fallback_path]
    );
    assert_symlink_warning(
        &warnings,
        "Project",
        config.cwd.join(DEFAULT_PROJECT_AGENTS_MD_PATH).as_path(),
        &secret_path,
        secret_contents,
    );
}

#[tokio::test]
async fn project_doc_read_uses_no_following_symlink_guard() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let instruction_path = config.cwd.join("AGENTS.md");
    let secret_contents = "external race secret";
    let fs = GuardedInstructionFileSystem {
        instruction_path: instruction_path.clone(),
        secret_contents: secret_contents.as_bytes().to_vec(),
        guarded_read_failure: GuardedReadFailure::Symlink,
    };

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&config)
        .user_instructions_with_fs(&fs, &mut warnings)
        .await;

    assert_eq!(loaded, None);
    assert_symlink_warning(
        &warnings,
        "Project",
        instruction_path.as_path(),
        tmp.path().join("external-target").as_path(),
        secret_contents,
    );
}

#[tokio::test]
async fn project_doc_read_skips_unsupported_no_following_filesystem() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let instruction_path = config.cwd.join("AGENTS.md");
    let secret_contents = "external unsupported secret";
    let fs = GuardedInstructionFileSystem {
        instruction_path: instruction_path.clone(),
        secret_contents: secret_contents.as_bytes().to_vec(),
        guarded_read_failure: GuardedReadFailure::Unsupported,
    };

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&config)
        .user_instructions_with_fs(&fs, &mut warnings)
        .await;

    assert_eq!(loaded, None);
    assert_symlink_safe_read_unsupported_warning(
        &warnings,
        "Project",
        instruction_path.as_path(),
        secret_contents,
    );
}

/// Oversize file is truncated to `project_doc_max_bytes`.
#[tokio::test]
async fn doc_larger_than_limit_is_truncated() {
    const LIMIT: usize = 1024;
    let tmp = tempfile::tempdir().expect("tempdir");

    let huge = "A".repeat(LIMIT * 2); // 2 KiB
    fs::write(tmp.path().join("AGENTS.md"), &huge).unwrap();

    let res = get_user_instructions(&make_config(&tmp, LIMIT, /*instructions*/ None).await)
        .await
        .expect("doc expected");

    assert_eq!(res.len(), LIMIT, "doc should be truncated to LIMIT bytes");
    assert_eq!(res, huge[..LIMIT]);
}

/// When `cwd` is nested inside a repo, the search should locate AGENTS.md
/// placed at the repository root (identified by `.git`).
#[tokio::test]
async fn finds_doc_in_repo_root() {
    let repo = tempfile::tempdir().expect("tempdir");

    // Simulate a git repository. Note .git can be a file or a directory.
    std::fs::write(
        repo.path().join(".git"),
        "gitdir: /path/to/actual/git/dir\n",
    )
    .unwrap();

    // Put the doc at the repo root.
    fs::write(repo.path().join("AGENTS.md"), "root level doc").unwrap();

    // Now create a nested working directory: repo/workspace/crate_a
    let nested = repo.path().join("workspace/crate_a");
    std::fs::create_dir_all(&nested).unwrap();

    // Build config pointing at the nested dir.
    let mut cfg = make_config(&repo, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = nested.abs();

    let res = get_user_instructions(&cfg).await.expect("doc expected");
    assert_eq!(res, "root level doc");
}

/// Explicitly setting the byte-limit to zero disables project docs.
#[tokio::test]
async fn zero_byte_limit_disables_docs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "something").unwrap();

    let res =
        get_user_instructions(&make_config(&tmp, /*limit*/ 0, /*instructions*/ None).await).await;
    assert!(
        res.is_none(),
        "With limit 0 the function should return None"
    );
}

#[tokio::test]
async fn zero_byte_limit_disables_discovery() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "something").unwrap();

    let discovery = agents_md_paths(&make_config(&tmp, /*limit*/ 0, /*instructions*/ None).await)
        .await
        .expect("discover paths");
    assert_eq!(discovery, Vec::<AbsolutePathBuf>::new());
}

/// When both system instructions and AGENTS.md docs are present the two
/// should be concatenated with the separator.
#[tokio::test]
async fn merges_existing_instructions_with_agents_md() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "proj doc").unwrap();

    const INSTRUCTIONS: &str = "base instructions";

    let res = get_user_instructions(&make_config(&tmp, /*limit*/ 4096, Some(INSTRUCTIONS)).await)
        .await
        .expect("should produce a combined instruction string");

    let expected = format!("{INSTRUCTIONS}{AGENTS_MD_SEPARATOR}{}", "proj doc");

    assert_eq!(res, expected);
}

#[tokio::test]
async fn sourceless_user_instructions_preserve_separator_without_reporting_a_source() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "project doc").unwrap();

    let mut cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.user_instructions = Some(LoadedAgentsMd::from_text_for_testing(
        "user instructions".to_string(),
    ));

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("instructions expected");
    let project_agents = cfg.cwd.join("AGENTS.md");

    assert_eq!(
        loaded.text(),
        format!("user instructions{AGENTS_MD_SEPARATOR}project doc")
    );
    assert_eq!(loaded.sources().collect::<Vec<_>>(), vec![&project_agents]);
}

/// If there are existing system instructions but AGENTS.md docs are
/// missing we expect the original instructions to be returned unchanged.
#[tokio::test]
async fn keeps_existing_instructions_when_doc_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");

    const INSTRUCTIONS: &str = "some instructions";

    let res =
        get_user_instructions(&make_config(&tmp, /*limit*/ 4096, Some(INSTRUCTIONS)).await).await;

    assert_eq!(res, Some(INSTRUCTIONS.to_string()));
}

/// When both the repository root and the working directory contain
/// AGENTS.md files, their contents are concatenated from root to cwd.
#[tokio::test]
async fn concatenates_root_and_cwd_docs() {
    let repo = tempfile::tempdir().expect("tempdir");

    // Simulate a git repository.
    std::fs::write(
        repo.path().join(".git"),
        "gitdir: /path/to/actual/git/dir\n",
    )
    .unwrap();

    // Repo root doc.
    fs::write(repo.path().join("AGENTS.md"), "root doc").unwrap();

    // Nested working directory with its own doc.
    let nested = repo.path().join("workspace/crate_a");
    std::fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("AGENTS.md"), "crate doc").unwrap();

    let mut cfg = make_config(&repo, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = nested.abs();

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("doc expected");
    let root_agents = repo.path().join("AGENTS.md").abs();
    let crate_agents = cfg.cwd.join("AGENTS.md");
    let expected = LoadedAgentsMd {
        entries: vec![
            InstructionEntry {
                contents: "root doc".to_string(),
                provenance: InstructionProvenance::Project(root_agents.clone()),
            },
            InstructionEntry {
                contents: "crate doc".to_string(),
                provenance: InstructionProvenance::Project(crate_agents.clone()),
            },
        ],
    };

    assert_eq!(loaded, expected);
    assert_eq!(loaded.text(), "root doc\n\ncrate doc");
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&root_agents, &crate_agents]
    );
}

#[tokio::test]
async fn project_root_markers_are_honored_for_agents_discovery() {
    let root = tempfile::tempdir().expect("tempdir");
    fs::write(root.path().join(".codex-root"), "").unwrap();
    fs::write(root.path().join("AGENTS.md"), "parent doc").unwrap();

    let nested = root.path().join("dir1");
    fs::create_dir_all(nested.join(".git")).unwrap();
    fs::write(nested.join("AGENTS.md"), "child doc").unwrap();

    let mut cfg = make_config_with_project_root_markers(
        &root,
        /*limit*/ 4096,
        /*instructions*/ None,
        &[".codex-root"],
    )
    .await;
    cfg.cwd = nested.abs();

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    let expected_parent = root.path().join("AGENTS.md").abs();
    let expected_child = cfg.cwd.join("AGENTS.md");
    assert_eq!(discovery.len(), 2);
    assert_eq!(discovery[0], expected_parent);
    assert_eq!(discovery[1], expected_child);

    let res = get_user_instructions(&cfg).await.expect("doc expected");
    assert_eq!(res, "parent doc\n\nchild doc");
}

#[tokio::test]
async fn agents_md_paths_preserve_symlinked_cwd() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("AGENTS.md"), "project doc").unwrap();

    let linked_cwd = tmp.path().join("linked");
    create_directory_symlink(&target, &linked_cwd);

    let mut cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = linked_cwd.abs();

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery, vec![cfg.cwd.join("AGENTS.md")]);

    let res = get_user_instructions(&cfg).await.expect("doc expected");
    assert_eq!(res, "project doc");
}

#[tokio::test]
async fn child_agents_message_after_global_instructions_uses_plain_separator() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = make_config(&tmp, /*limit*/ 4096, Some("global doc")).await;
    cfg.features.enable(Feature::ChildAgentsMd).unwrap();

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("instructions expected");
    let global_agents = cfg.codex_home.join(DEFAULT_AGENTS_MD_FILENAME);
    let expected = LoadedAgentsMd {
        entries: vec![
            InstructionEntry {
                contents: "global doc".to_string(),
                provenance: InstructionProvenance::User(global_agents),
            },
            InstructionEntry {
                contents: HIERARCHICAL_AGENTS_MESSAGE.to_string(),
                provenance: InstructionProvenance::Internal,
            },
        ],
    };

    assert_eq!(loaded, expected);
    assert_eq!(
        loaded.text(),
        format!("global doc\n\n{HIERARCHICAL_AGENTS_MESSAGE}")
    );
}

#[tokio::test]
async fn instruction_sources_include_global_before_agents_md_docs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "project doc").unwrap();

    let cfg = make_config(&tmp, /*limit*/ 4096, Some("global doc")).await;
    let global_agents = cfg.codex_home.join(DEFAULT_AGENTS_MD_FILENAME);
    fs::create_dir_all(&cfg.codex_home).unwrap();
    fs::write(&global_agents, "global doc").unwrap();

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("instructions expected");
    let project_agents = cfg.cwd.join("AGENTS.md");

    let expected = LoadedAgentsMd {
        entries: vec![
            InstructionEntry {
                contents: "global doc".to_string(),
                provenance: InstructionProvenance::User(global_agents.clone()),
            },
            InstructionEntry {
                contents: "project doc".to_string(),
                provenance: InstructionProvenance::Project(project_agents.clone()),
            },
        ],
    };
    assert_eq!(loaded, expected);
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&global_agents, &project_agents]
    );
    assert_eq!(
        loaded.text(),
        format!("global doc{AGENTS_MD_SEPARATOR}project doc")
    );
}

#[tokio::test]
async fn child_agents_message_after_project_docs_is_not_an_instruction_source() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "project doc").unwrap();

    let mut cfg = make_config(&tmp, /*limit*/ 4096, Some("global doc")).await;
    cfg.features.enable(Feature::ChildAgentsMd).unwrap();
    let global_agents = cfg.codex_home.join(DEFAULT_AGENTS_MD_FILENAME);
    fs::create_dir_all(&cfg.codex_home).unwrap();
    fs::write(&global_agents, "global doc").unwrap();

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("instructions expected");
    let project_agents = cfg.cwd.join("AGENTS.md");

    let expected = LoadedAgentsMd {
        entries: vec![
            InstructionEntry {
                contents: "global doc".to_string(),
                provenance: InstructionProvenance::User(global_agents.clone()),
            },
            InstructionEntry {
                contents: "project doc".to_string(),
                provenance: InstructionProvenance::Project(project_agents.clone()),
            },
            InstructionEntry {
                contents: HIERARCHICAL_AGENTS_MESSAGE.to_string(),
                provenance: InstructionProvenance::Internal,
            },
        ],
    };
    assert_eq!(loaded, expected);
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&global_agents, &project_agents]
    );
    assert_eq!(
        loaded.text(),
        format!("global doc{AGENTS_MD_SEPARATOR}project doc\n\n{HIERARCHICAL_AGENTS_MESSAGE}")
    );
}

/// AGENTS.override.md is preferred over AGENTS.md when both are present.
#[tokio::test]
async fn agents_local_md_preferred() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join(DEFAULT_AGENTS_MD_FILENAME), "versioned").unwrap();
    fs::write(tmp.path().join(LOCAL_AGENTS_MD_FILENAME), "local").unwrap();

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;

    let res = get_user_instructions(&cfg)
        .await
        .expect("local doc expected");

    assert_eq!(res, "local");

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery.len(), 1);
    assert_eq!(
        discovery[0].file_name().unwrap().to_string_lossy(),
        LOCAL_AGENTS_MD_FILENAME
    );
}

#[tokio::test]
async fn project_codewith_dir_doc_is_preferred_over_root_codewith_md() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), DEFAULT_AGENTS_MD_FILENAME, "root");
    write_doc(tmp.path(), DEFAULT_PROJECT_AGENTS_MD_PATH, "project");

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;

    let res = get_user_instructions(&cfg)
        .await
        .expect("project doc expected");
    assert_eq!(res, "project");

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery.len(), 1);
    assert!(discovery[0].ends_with(Path::new(DEFAULT_PROJECT_AGENTS_MD_PATH)));
}

#[tokio::test]
async fn project_codewith_dir_override_is_preferred() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), DEFAULT_PROJECT_AGENTS_MD_PATH, "versioned");
    write_doc(tmp.path(), LOCAL_PROJECT_AGENTS_MD_PATH, "local");

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;

    let res = get_user_instructions(&cfg)
        .await
        .expect("local project doc expected");
    assert_eq!(res, "local");

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery.len(), 1);
    assert!(discovery[0].ends_with(Path::new(LOCAL_PROJECT_AGENTS_MD_PATH)));
}

#[tokio::test]
async fn project_rules_are_loaded_without_codewith_md() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), ".codewith/rules/main.md", "rule");

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut Vec::new())
        .await
        .expect("rule expected");
    let rule_path = tmp.path().join(".codewith/rules/main.md").abs();

    assert_eq!(loaded.text(), "rule");
    assert_eq!(loaded.sources().collect::<Vec<_>>(), vec![&rule_path]);
}

#[tokio::test]
async fn project_rules_are_loaded_recursively_after_project_docs() {
    let repo = tempfile::tempdir().expect("tempdir");
    std::fs::write(repo.path().join(".git"), "").unwrap();
    write_doc(repo.path(), DEFAULT_PROJECT_AGENTS_MD_PATH, "root doc");
    write_doc(repo.path(), ".codewith/rules/z.md", "z rule");
    write_doc(repo.path(), ".codewith/rules/a.md", "a rule");

    let nested = repo.path().join("workspace/crate_a");
    fs::create_dir_all(&nested).unwrap();
    write_doc(&nested, ".codewith/rules/deep/b.md", "nested rule");

    let mut cfg = make_config(&repo, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = nested.abs();

    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut Vec::new())
        .await
        .expect("docs expected");
    let root_doc = repo.path().join(DEFAULT_PROJECT_AGENTS_MD_PATH).abs();
    let root_rule_a = repo.path().join(".codewith/rules/a.md").abs();
    let root_rule_z = repo.path().join(".codewith/rules/z.md").abs();
    let nested_rule = nested.join(".codewith/rules/deep/b.md").abs();

    assert_eq!(loaded.text(), "root doc\n\na rule\n\nz rule\n\nnested rule");
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&root_doc, &root_rule_a, &root_rule_z, &nested_rule]
    );
}

#[tokio::test]
async fn project_rules_skip_directory_symlink_cycles() {
    let repo = tempfile::tempdir().expect("tempdir");
    write_doc(repo.path(), DEFAULT_PROJECT_AGENTS_MD_PATH, "root doc");
    write_doc(repo.path(), ".codewith/rules/real.md", "real rule");
    create_directory_symlink(
        &repo.path().join(".codewith/rules"),
        &repo.path().join(".codewith/rules/loop"),
    );

    let cfg = make_config(&repo, /*limit*/ 4096, /*instructions*/ None).await;
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut Vec::new())
        .await
        .expect("docs expected");
    let root_doc = repo.path().join(DEFAULT_PROJECT_AGENTS_MD_PATH).abs();
    let real_rule = repo.path().join(".codewith/rules/real.md").abs();

    assert_eq!(loaded.text(), "root doc\n\nreal rule");
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&root_doc, &real_rule]
    );
}

#[tokio::test]
async fn project_rules_skip_symlinked_files() {
    let parent = tempfile::tempdir().expect("tempdir");
    let repo = parent.path().join("repo");
    fs::create_dir(&repo).unwrap();
    write_doc(&repo, DEFAULT_PROJECT_AGENTS_MD_PATH, "root doc");
    write_doc(&repo, ".codewith/rules/real.md", "real rule");
    fs::write(parent.path().join("secret.md"), "secret rule").unwrap();
    create_file_symlink(
        &parent.path().join("secret.md"),
        &repo.join(".codewith/rules/leak.md"),
    );

    let mut warnings = Vec::new();
    let mut cfg = make_config(&parent, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = repo.abs();
    let loaded = AgentsMdManager::new(&cfg)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await
        .expect("docs expected");
    let root_doc = repo.join(DEFAULT_PROJECT_AGENTS_MD_PATH).abs();
    let real_rule = repo.join(".codewith/rules/real.md").abs();

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(loaded.text(), "root doc\n\nreal rule");
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&root_doc, &real_rule]
    );
    assert!(!loaded.text().contains("secret"));
}

#[tokio::test]
async fn project_codewith_imports_relative_files_and_reports_sources() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join(".git"), "gitdir: /path/to/actual/git/dir\n").unwrap();
    write_doc(
        tmp.path(),
        DEFAULT_PROJECT_AGENTS_MD_PATH,
        "root before\n@fragments/base.md\nroot after",
    );
    write_doc(tmp.path(), ".codewith/fragments/base.md", "base fragment");

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");
    let root_doc = tmp.path().join(DEFAULT_PROJECT_AGENTS_MD_PATH).abs();
    let imported_doc = tmp.path().join(".codewith/fragments/base.md").abs();

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(
        loaded.entries,
        vec![
            InstructionEntry {
                contents: "root before\n".to_string(),
                provenance: InstructionProvenance::Project(root_doc.clone()),
            },
            InstructionEntry {
                contents: "base fragment".to_string(),
                provenance: InstructionProvenance::Project(imported_doc.clone()),
            },
            InstructionEntry {
                contents: "root after".to_string(),
                provenance: InstructionProvenance::Project(root_doc.clone()),
            },
        ]
    );
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&root_doc, &imported_doc]
    );
}

#[tokio::test]
async fn global_codewith_imports_profile_scoped_fragments() {
    let codex_home = tempfile::tempdir().expect("tempdir");
    let codex_home_abs = codex_home.abs();
    let global_path = codex_home_abs.join(DEFAULT_AGENTS_MD_FILENAME);
    let profile_path = codex_home_abs.join("profiles/marcus.md");
    write_doc(
        codex_home.path(),
        DEFAULT_AGENTS_MD_FILENAME,
        "@profiles/marcus.md",
    );
    write_doc(codex_home.path(), "profiles/marcus.md", "profile identity");

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::load_global_instructions(
        LOCAL_FS.as_ref(),
        Some(&codex_home_abs),
        &mut warnings,
    )
    .await
    .expect("global instructions expected");

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(loaded.text(), "profile identity");
    assert_eq!(loaded.sources().collect::<Vec<_>>(), vec![&profile_path]);
    assert!(
        !loaded.sources().any(|source| source == &global_path),
        "import-only parent with no text should not be reported as a content source"
    );
}

#[tokio::test]
async fn global_codewith_import_outside_home_is_blocked() {
    let parent = tempfile::tempdir().expect("tempdir");
    let codex_home = parent.path().join("codewith-home");
    fs::create_dir(&codex_home).unwrap();
    let codex_home_abs = codex_home.abs();
    fs::write(parent.path().join("outside.md"), "secret").unwrap();
    write_doc(
        &codex_home,
        DEFAULT_AGENTS_MD_FILENAME,
        "global\n@../outside.md",
    );

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::load_global_instructions(
        LOCAL_FS.as_ref(),
        Some(&codex_home_abs),
        &mut warnings,
    )
    .await
    .expect("global instructions expected");

    assert_eq!(loaded.text(), "global");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("outside import root"));
    assert!(!loaded.text().contains("secret"));
}

#[tokio::test]
async fn global_codewith_root_file_keeps_existing_unbounded_behavior() {
    let codex_home = tempfile::tempdir().expect("tempdir");
    let codex_home_abs = codex_home.abs();
    let global_path = codex_home_abs.join(DEFAULT_AGENTS_MD_FILENAME);
    let large_global = "G".repeat(DEFAULT_PROJECT_DOC_MAX_BYTES + 20);
    write_doc(codex_home.path(), DEFAULT_AGENTS_MD_FILENAME, &large_global);

    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::load_global_instructions(
        LOCAL_FS.as_ref(),
        Some(&codex_home_abs),
        &mut warnings,
    )
    .await
    .expect("global instructions expected");

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(loaded.text().len(), large_global.len());
    assert_eq!(loaded.sources().collect::<Vec<_>>(), vec![&global_path]);
}

#[tokio::test]
async fn imported_child_doc_is_still_loaded_when_discovered_later() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join(".git"), "gitdir: /path/to/actual/git/dir\n").unwrap();
    write_doc(
        tmp.path(),
        DEFAULT_AGENTS_MD_FILENAME,
        "root\n@sub/CODEWITH.md",
    );
    write_doc(tmp.path(), "sub/CODEWITH.md", "child");

    let mut cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = tmp.path().join("sub").abs();
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");
    let root_doc = tmp.path().join(DEFAULT_AGENTS_MD_FILENAME).abs();
    let child_doc = tmp.path().join("sub/CODEWITH.md").abs();

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(
        loaded.entries,
        vec![
            InstructionEntry {
                contents: "root\n".to_string(),
                provenance: InstructionProvenance::Project(root_doc.clone()),
            },
            InstructionEntry {
                contents: "child".to_string(),
                provenance: InstructionProvenance::Project(child_doc.clone()),
            },
            InstructionEntry {
                contents: "child".to_string(),
                provenance: InstructionProvenance::Project(child_doc.clone()),
            },
        ]
    );
    assert_eq!(
        loaded.sources().collect::<Vec<_>>(),
        vec![&root_doc, &child_doc]
    );
}

#[tokio::test]
async fn import_directory_loads_rule_files_in_name_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), DEFAULT_AGENTS_MD_FILENAME, "@rules");
    write_doc(tmp.path(), "rules/b.mdc", "bravo");
    write_doc(tmp.path(), "rules/a.md", "alpha");
    write_doc(tmp.path(), "rules/c.txt", "charlie");
    write_doc(tmp.path(), "rules/nested.md/ignored.md", "ignored nested");
    write_doc(tmp.path(), "rules/nested/ignored.md", "ignored nested");
    write_doc(tmp.path(), "rules/z.json", "ignored");

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(warnings, Vec::<String>::new());
    assert_eq!(loaded.text(), "alpha\n\nbravo\n\ncharlie");
    assert_eq!(
        loaded.sources().cloned().collect::<Vec<_>>(),
        vec![
            tmp.path().join("rules/a.md").abs(),
            tmp.path().join("rules/b.mdc").abs(),
            tmp.path().join("rules/c.txt").abs(),
        ]
    );
}

#[tokio::test]
async fn import_outside_project_root_is_blocked() {
    let parent = tempfile::tempdir().expect("tempdir");
    let repo = parent.path().join("repo");
    fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join(".git"), "gitdir: /path/to/actual/git/dir\n").unwrap();
    fs::write(parent.path().join("outside.md"), "secret").unwrap();
    write_doc(
        &repo,
        DEFAULT_AGENTS_MD_FILENAME,
        "before\n@../outside.md\nafter",
    );

    let mut cfg = make_config(&parent, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = AbsolutePathBuf::from_absolute_path(&repo).expect("absolute repo");
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text(), "before\n\n\nafter");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("outside import root"));
    assert!(!loaded.text().contains("secret"));
}

#[tokio::test]
async fn absolute_import_outside_project_root_is_blocked() {
    let parent = tempfile::tempdir().expect("tempdir");
    let repo = parent.path().join("repo");
    fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join(".git"), "gitdir: /path/to/actual/git/dir\n").unwrap();
    let outside_path = parent.path().join("outside.md").abs();
    fs::write(outside_path.as_path(), "secret").unwrap();
    write_doc(
        &repo,
        DEFAULT_AGENTS_MD_FILENAME,
        &format!("before\n@{}\nafter", outside_path.display()),
    );

    let mut cfg = make_config(&parent, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = AbsolutePathBuf::from_absolute_path(&repo).expect("absolute repo");
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text(), "before\n\n\nafter");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("absolute import paths are not supported"));
    assert!(!loaded.text().contains("secret"));
}

#[tokio::test]
async fn direct_import_of_non_instruction_file_is_blocked() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(
        tmp.path(),
        DEFAULT_AGENTS_MD_FILENAME,
        "before\n@.env\nafter",
    );
    fs::write(tmp.path().join(".env"), "SECRET=value").unwrap();

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text(), "before\n\n\nafter");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("only .md, .mdc, and .txt"));
    assert!(!loaded.text().contains("SECRET"));
}

#[tokio::test]
async fn symlinked_directory_import_is_blocked() {
    let parent = tempfile::tempdir().expect("tempdir");
    let repo = parent.path().join("repo");
    let outside_rules = parent.path().join("outside-rules");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&outside_rules).unwrap();
    std::fs::write(repo.join(".git"), "gitdir: /path/to/actual/git/dir\n").unwrap();
    fs::write(outside_rules.join("secret.md"), "secret").unwrap();
    create_directory_symlink(&outside_rules, &repo.join("rules"));
    write_doc(&repo, DEFAULT_AGENTS_MD_FILENAME, "before\n@rules\nafter");

    let mut cfg = make_config(&parent, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = AbsolutePathBuf::from_absolute_path(&repo).expect("absolute repo");
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text(), "before\n\n\nafter");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("symlink imports are not followed"));
    assert!(!loaded.text().contains("secret"));
}

#[tokio::test]
async fn import_through_symlinked_ancestor_is_blocked() {
    let parent = tempfile::tempdir().expect("tempdir");
    let repo = parent.path().join("repo");
    let outside_rules = parent.path().join("outside-rules");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&outside_rules).unwrap();
    std::fs::write(repo.join(".git"), "gitdir: /path/to/actual/git/dir\n").unwrap();
    fs::write(outside_rules.join("secret.md"), "secret").unwrap();
    create_directory_symlink(&outside_rules, &repo.join("rules"));
    write_doc(
        &repo,
        DEFAULT_AGENTS_MD_FILENAME,
        "before\n@rules/secret.md\nafter",
    );

    let mut cfg = make_config(&parent, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.cwd = AbsolutePathBuf::from_absolute_path(&repo).expect("absolute repo");
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text(), "before\n\n\nafter");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("outside resolved import root"));
    assert!(!loaded.text().contains("secret"));
}

#[tokio::test]
async fn import_cycle_is_skipped_and_warned() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), DEFAULT_AGENTS_MD_FILENAME, "root\n@a.md");
    write_doc(tmp.path(), "a.md", "a\n@CODEWITH.md");

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text().matches("root").count(), 1);
    assert!(loaded.text().contains("a"));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("prevents cycles"));
}

#[tokio::test]
async fn import_depth_limit_is_enforced() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), DEFAULT_AGENTS_MD_FILENAME, "@0.md");
    for index in 0..=AGENTS_MD_IMPORT_MAX_DEPTH + 1 {
        let next = index + 1;
        let contents = if index <= AGENTS_MD_IMPORT_MAX_DEPTH {
            format!("depth {index}\n@{next}.md")
        } else {
            format!("depth {index}")
        };
        write_doc(tmp.path(), format!("{index}.md").as_str(), &contents);
    }

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    let deepest_loaded = AGENTS_MD_IMPORT_MAX_DEPTH - 1;
    let first_skipped = AGENTS_MD_IMPORT_MAX_DEPTH;
    assert!(loaded.text().contains("depth 0"));
    assert!(loaded.text().contains(&format!("depth {deepest_loaded}")));
    assert!(!loaded.text().contains(&format!("depth {first_skipped}")));
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("maximum import depth"));
}

#[tokio::test]
async fn imported_file_size_limit_is_enforced() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_doc(tmp.path(), DEFAULT_AGENTS_MD_FILENAME, "@big.md");
    write_doc(
        tmp.path(),
        "big.md",
        "A".repeat(AGENTS_MD_IMPORT_MAX_FILE_BYTES + 20).as_str(),
    );

    let cfg = make_config(
        &tmp,
        AGENTS_MD_IMPORT_MAX_FILE_BYTES + 100,
        /*instructions*/ None,
    )
    .await;
    let (loaded, warnings) = load_user_instructions(&cfg).await;
    let loaded = loaded.expect("instructions expected");

    assert_eq!(loaded.text().len(), AGENTS_MD_IMPORT_MAX_FILE_BYTES);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("exceeds"));
    assert!(warnings[0].contains("truncating"));
}
/// When AGENTS.md is absent but a configured fallback exists, the fallback is used.
#[tokio::test]
async fn uses_configured_fallback_when_agents_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("EXAMPLE.md"), "example instructions").unwrap();

    let cfg = make_config_with_fallback(
        &tmp,
        /*limit*/ 4096,
        /*instructions*/ None,
        &["EXAMPLE.md"],
    )
    .await;

    let res = get_user_instructions(&cfg)
        .await
        .expect("fallback doc expected");

    assert_eq!(res, "example instructions");
}

#[tokio::test]
async fn configured_fallback_symlink_is_skipped_with_redacted_warning() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let secret_path = outside.path().join("fallback-secret.txt");
    let secret_contents = "external fallback secret";
    fs::write(&secret_path, secret_contents).unwrap();
    let link_path = tmp.path().join("EXAMPLE.md");
    create_file_symlink(&secret_path, &link_path);

    let config = make_config_with_fallback(
        &tmp,
        /*limit*/ 4096,
        /*instructions*/ None,
        &["EXAMPLE.md"],
    )
    .await;
    let mut warnings = Vec::new();
    let loaded = AgentsMdManager::new(&config)
        .user_instructions_with_fs(LOCAL_FS.as_ref(), &mut warnings)
        .await;

    assert_eq!(loaded, None);
    assert_eq!(
        agents_md_paths(&config).await.expect("discover paths"),
        Vec::<AbsolutePathBuf>::new()
    );
    assert_symlink_warning(
        &warnings,
        "Project",
        config.cwd.join("EXAMPLE.md").as_path(),
        &secret_path,
        secret_contents,
    );
}

/// Legacy AGENTS.md remains preferred over configured fallback filenames.
#[tokio::test]
async fn agents_md_preferred_over_fallbacks() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "primary").unwrap();
    fs::write(tmp.path().join("EXAMPLE.md"), "secondary").unwrap();

    let cfg = make_config_with_fallback(
        &tmp,
        /*limit*/ 4096,
        /*instructions*/ None,
        &["EXAMPLE.md", ".example.md"],
    )
    .await;

    let res = get_user_instructions(&cfg)
        .await
        .expect("legacy AGENTS.md should win");

    assert_eq!(res, "primary");

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery.len(), 1);
    assert!(
        discovery[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .eq(LEGACY_DEFAULT_AGENTS_MD_FILENAME)
    );
}

#[tokio::test]
async fn agents_md_directory_is_ignored() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(tmp.path().join("AGENTS.md")).unwrap();

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;

    let res = get_user_instructions(&cfg).await;
    assert_eq!(res, None);

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery, Vec::<AbsolutePathBuf>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn agents_md_special_file_is_ignored() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("AGENTS.md");
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path without nul");
    // SAFETY: `c_path` is a valid, nul-terminated path and `mkfifo` does not
    // retain the pointer after the call.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) };
    assert_eq!(rc, 0);

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;

    let res = get_user_instructions(&cfg).await;
    assert_eq!(res, None);

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery, Vec::<AbsolutePathBuf>::new());
}

#[tokio::test]
async fn override_directory_falls_back_to_agents_md_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(tmp.path().join(LOCAL_AGENTS_MD_FILENAME)).unwrap();
    fs::write(tmp.path().join(DEFAULT_AGENTS_MD_FILENAME), "primary").unwrap();

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;

    let res = get_user_instructions(&cfg)
        .await
        .expect("AGENTS.md should be used when override is a directory");
    assert_eq!(res, "primary");

    let discovery = agents_md_paths(&cfg).await.expect("discover paths");
    assert_eq!(discovery.len(), 1);
    assert_eq!(
        discovery[0]
            .file_name()
            .expect("file name")
            .to_string_lossy(),
        DEFAULT_AGENTS_MD_FILENAME
    );
}

#[tokio::test]
async fn skills_are_not_appended_to_agents_md() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "base doc").unwrap();

    let cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    create_skill(
        cfg.codex_home.to_path_buf(),
        "pdf-processing",
        "extract from pdfs",
    );

    let res = get_user_instructions(&cfg)
        .await
        .expect("instructions expected");
    assert_eq!(res, "base doc");
}

#[tokio::test]
async fn apps_feature_does_not_emit_user_instructions_by_itself() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.features
        .enable(Feature::Apps)
        .expect("test config should allow apps");

    let res = get_user_instructions(&cfg).await;
    assert_eq!(res, None);
}

#[tokio::test]
async fn apps_feature_does_not_append_to_agents_md_user_instructions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::write(tmp.path().join("AGENTS.md"), "base doc").unwrap();

    let mut cfg = make_config(&tmp, /*limit*/ 4096, /*instructions*/ None).await;
    cfg.features
        .enable(Feature::Apps)
        .expect("test config should allow apps");

    let res = get_user_instructions(&cfg)
        .await
        .expect("instructions expected");
    assert_eq!(res, "base doc");
}

fn create_skill(codex_home: PathBuf, name: &str, description: &str) {
    let skill_dir = codex_home.join(format!("skills/{name}"));
    fs::create_dir_all(&skill_dir).unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n");
    fs::write(skill_dir.join("SKILL.md"), content).unwrap();
}

enum GuardedReadFailure {
    Symlink,
    Unsupported,
}

struct GuardedInstructionFileSystem {
    instruction_path: AbsolutePathBuf,
    secret_contents: Vec<u8>,
    guarded_read_failure: GuardedReadFailure,
}

#[async_trait]
impl ExecutorFileSystem for GuardedInstructionFileSystem {
    async fn canonicalize(
        &self,
        path: &AbsolutePathBuf,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<AbsolutePathBuf> {
        Ok(path.clone())
    }

    async fn join(
        &self,
        base_path: &AbsolutePathBuf,
        path: &Path,
    ) -> FileSystemResult<AbsolutePathBuf> {
        Ok(base_path.join(path))
    }

    async fn parent(&self, path: &AbsolutePathBuf) -> FileSystemResult<Option<AbsolutePathBuf>> {
        Ok(path.parent())
    }

    async fn read_file(
        &self,
        path: &AbsolutePathBuf,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        if path == &self.instruction_path {
            return Ok(self.secret_contents.clone());
        }
        Err(io::ErrorKind::NotFound.into())
    }

    async fn read_file_without_following_symlinks(
        &self,
        path: &AbsolutePathBuf,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        if path == &self.instruction_path {
            return match self.guarded_read_failure {
                GuardedReadFailure::Symlink => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    SYMLINKED_FILE_ERROR,
                )),
                GuardedReadFailure::Unsupported => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    SYMLINK_SAFE_READ_UNSUPPORTED_ERROR,
                )),
            };
        }
        Err(io::ErrorKind::NotFound.into())
    }

    async fn write_file(
        &self,
        _path: &AbsolutePathBuf,
        _contents: Vec<u8>,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        unimplemented!("test filesystem only supports instruction reads")
    }

    async fn create_directory(
        &self,
        _path: &AbsolutePathBuf,
        _create_directory_options: CreateDirectoryOptions,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        unimplemented!("test filesystem only supports instruction reads")
    }

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata> {
        if path == &self.instruction_path {
            return Ok(FileMetadata {
                is_directory: false,
                is_file: true,
                is_symlink: false,
                created_at_ms: 0,
                modified_at_ms: 0,
            });
        }
        Err(io::ErrorKind::NotFound.into())
    }

    async fn read_directory(
        &self,
        _path: &AbsolutePathBuf,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        unimplemented!("test filesystem only supports instruction reads")
    }

    async fn remove(
        &self,
        _path: &AbsolutePathBuf,
        _remove_options: RemoveOptions,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        unimplemented!("test filesystem only supports instruction reads")
    }

    async fn copy(
        &self,
        _source_path: &AbsolutePathBuf,
        _destination_path: &AbsolutePathBuf,
        _copy_options: CopyOptions,
        _sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        unimplemented!("test filesystem only supports instruction reads")
    }
}
