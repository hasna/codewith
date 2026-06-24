use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::GitToolingError;
use crate::operations::resolve_head;
use crate::operations::run_git_for_output;
use crate::operations::run_git_for_status;
use crate::operations::run_git_for_stdout;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktreeEntry {
    pub path: PathBuf,
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub is_main: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktreeStatusSnapshot {
    pub dirty: bool,
    pub branch: Option<String>,
    pub head_sha: Option<String>,
    pub records: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitMergeTreeDryRun {
    pub clean: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub conflicted_paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitWorktreeAddOptions {
    pub worktree_path: PathBuf,
    pub branch: String,
    pub start_point: String,
}

pub fn list_git_worktrees(base_repo_path: &Path) -> Result<Vec<GitWorktreeEntry>, GitToolingError> {
    let output = run_git_for_stdout(
        base_repo_path,
        [
            OsString::from("worktree"),
            OsString::from("list"),
            OsString::from("--porcelain"),
            OsString::from("-z"),
        ],
        /*env*/ None,
    )?;
    Ok(parse_worktree_list_porcelain(output.as_str()))
}

pub fn add_linked_git_worktree(
    base_repo_path: &Path,
    options: GitWorktreeAddOptions,
) -> Result<GitWorktreeEntry, GitToolingError> {
    run_git_for_status(
        base_repo_path,
        [
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("-b"),
            OsString::from(options.branch),
            options.worktree_path.as_os_str().to_os_string(),
            OsString::from(options.start_point),
        ],
        /*env*/ None,
    )?;
    let worktrees = list_git_worktrees(base_repo_path)?;
    worktrees
        .into_iter()
        .find(|entry| paths_match(entry.path.as_path(), options.worktree_path.as_path()))
        .ok_or_else(|| GitToolingError::WorktreeNotLinked {
            base_repo_path: base_repo_path.to_path_buf(),
            worktree_path: options.worktree_path,
        })
}

pub fn get_git_worktree_status_snapshot(
    worktree_path: &Path,
) -> Result<GitWorktreeStatusSnapshot, GitToolingError> {
    let output = run_git_for_stdout(
        worktree_path,
        [
            OsString::from("status"),
            OsString::from("--porcelain=v2"),
            OsString::from("-z"),
            OsString::from("--branch"),
            OsString::from("--untracked-files=all"),
        ],
        /*env*/ None,
    )?;
    Ok(parse_status_porcelain_v2(output.as_str()))
}

pub fn resolve_git_ref(repo_path: &Path, git_ref: &str) -> Result<Option<String>, GitToolingError> {
    if git_ref.trim().is_empty() || git_ref == "HEAD" {
        return resolve_head(repo_path);
    }
    match run_git_for_stdout(
        repo_path,
        [
            OsString::from("rev-parse"),
            OsString::from("--verify"),
            OsString::from(git_ref),
        ],
        /*env*/ None,
    ) {
        Ok(sha) => Ok(Some(sha)),
        Err(GitToolingError::GitCommand { status, .. }) if status.code() == Some(128) => Ok(None),
        Err(err) => Err(err),
    }
}

pub fn validate_git_branch_name(repo_path: &Path, branch: &str) -> Result<bool, GitToolingError> {
    match run_git_for_status(
        repo_path,
        [
            OsString::from("check-ref-format"),
            OsString::from("--branch"),
            OsString::from(branch),
        ],
        /*env*/ None,
    ) {
        Ok(()) => Ok(true),
        Err(GitToolingError::GitCommand { .. }) => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn remove_linked_git_worktree(
    base_repo_path: &Path,
    worktree_path: &Path,
    force: bool,
) -> Result<(), GitToolingError> {
    let worktrees = list_git_worktrees(base_repo_path)?;
    let Some(entry) = worktrees
        .iter()
        .find(|entry| paths_match(entry.path.as_path(), worktree_path))
    else {
        return Err(GitToolingError::WorktreeNotLinked {
            base_repo_path: base_repo_path.to_path_buf(),
            worktree_path: worktree_path.to_path_buf(),
        });
    };
    if entry.is_main {
        return Err(GitToolingError::MainWorktreeRemovalRefused {
            path: worktree_path.to_path_buf(),
        });
    }

    let mut args = vec![OsString::from("worktree"), OsString::from("remove")];
    if force {
        args.push(OsString::from("--force"));
    }
    args.push(worktree_path.as_os_str().to_os_string());
    run_git_for_status(base_repo_path, args, /*env*/ None)
}

pub fn merge_tree_dry_run(
    base_repo_path: &Path,
    target_ref: &str,
    head_ref: &str,
) -> Result<GitMergeTreeDryRun, GitToolingError> {
    let run = run_git_for_output(
        base_repo_path,
        [
            OsString::from("merge-tree"),
            OsString::from("--write-tree"),
            OsString::from("--name-only"),
            OsString::from(target_ref),
            OsString::from(head_ref),
        ],
        /*env*/ None,
    )?;
    let command = run.command;
    let output = run.output;
    let status = output.status;
    let stdout =
        String::from_utf8(output.stdout).map_err(|source| GitToolingError::GitOutputUtf8 {
            command: command.clone(),
            source,
        })?;
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let exit_code = status.code();
    if !status.success() && exit_code != Some(1) {
        return Err(GitToolingError::GitCommand {
            command,
            status,
            stderr,
        });
    }
    let conflicted_paths = if exit_code == Some(1) {
        stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    Ok(GitMergeTreeDryRun {
        clean: status.success(),
        exit_code,
        stdout,
        stderr,
        conflicted_paths,
    })
}

pub fn fast_forward_merge_ref(
    base_repo_path: &Path,
    head_ref: &str,
) -> Result<(), GitToolingError> {
    run_git_for_status(
        base_repo_path,
        [
            OsString::from("merge"),
            OsString::from("--ff-only"),
            OsString::from(head_ref),
        ],
        /*env*/ None,
    )
}

pub fn worktree_has_commits_after(
    worktree_path: &Path,
    base_ref: &str,
) -> Result<bool, GitToolingError> {
    let output = run_git_for_stdout(
        worktree_path,
        [
            OsString::from("rev-list"),
            OsString::from("--count"),
            OsString::from(format!("{base_ref}..HEAD")),
        ],
        /*env*/ None,
    )?;
    Ok(output.trim().parse::<u32>().unwrap_or(1) > 0)
}

fn parse_worktree_list_porcelain(output: &str) -> Vec<GitWorktreeEntry> {
    let mut entries = Vec::new();
    let mut current = None::<GitWorktreeEntry>;
    for field in output.split('\0').filter(|field| !field.is_empty()) {
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(GitWorktreeEntry {
                path: PathBuf::from(path),
                head_sha: None,
                branch: None,
                detached: false,
                bare: false,
                is_main: entries.is_empty(),
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(head_sha) = field.strip_prefix("HEAD ") {
                entry.head_sha = Some(head_sha.to_string());
            } else if let Some(branch) = field.strip_prefix("branch ") {
                entry.branch = Some(branch.to_string());
            } else if field == "detached" {
                entry.detached = true;
            } else if field == "bare" {
                entry.bare = true;
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

fn parse_status_porcelain_v2(output: &str) -> GitWorktreeStatusSnapshot {
    let records = output
        .split('\0')
        .filter(|record| !record.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let dirty = records
        .iter()
        .any(|record| !record.starts_with("# ") && !record.trim().is_empty());
    let branch = records
        .iter()
        .find_map(|record| record.strip_prefix("# branch.head "))
        .filter(|branch| *branch != "(detached)")
        .map(str::to_string);
    let head_sha = records
        .iter()
        .find_map(|record| record.strip_prefix("# branch.oid "))
        .filter(|head_sha| *head_sha != "(initial)")
        .map(str::to_string);
    GitWorktreeStatusSnapshot {
        dirty,
        branch,
        head_sha,
        records,
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::process::Command;
    use tempfile::tempdir;

    fn run_git(repo_path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .status()
            .expect("git command should run");
        assert!(status.success(), "git command failed: {args:?}");
    }

    fn git_stdout(repo_path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo_path)
            .args(args)
            .output()
            .expect("git command should run");
        assert!(output.status.success(), "git command failed: {args:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(repo_path: &Path) {
        run_git(repo_path, &["init", "--initial-branch=main"]);
        run_git(repo_path, &["config", "user.email", "codewith@example.com"]);
        run_git(repo_path, &["config", "user.name", "Codewith Test"]);
        std::fs::write(repo_path.join("README.md"), "worktree test\n").expect("write README");
        run_git(repo_path, &["add", "README.md"]);
        run_git(repo_path, &["commit", "-m", "initial"]);
    }

    #[test]
    fn parses_worktree_list_porcelain_z() {
        let output = "\
worktree /repo\0\
HEAD abc123\0\
branch refs/heads/main\0\
worktree /repo/.codewith/worktrees/run-1\0\
HEAD def456\0\
branch refs/heads/codewith/run-1\0\
worktree /repo/.codewith/worktrees/detached\0\
HEAD feedbeef\0\
detached\0";

        assert_eq!(
            parse_worktree_list_porcelain(output),
            vec![
                GitWorktreeEntry {
                    path: PathBuf::from("/repo"),
                    head_sha: Some("abc123".to_string()),
                    branch: Some("refs/heads/main".to_string()),
                    detached: false,
                    bare: false,
                    is_main: true,
                },
                GitWorktreeEntry {
                    path: PathBuf::from("/repo/.codewith/worktrees/run-1"),
                    head_sha: Some("def456".to_string()),
                    branch: Some("refs/heads/codewith/run-1".to_string()),
                    detached: false,
                    bare: false,
                    is_main: false,
                },
                GitWorktreeEntry {
                    path: PathBuf::from("/repo/.codewith/worktrees/detached"),
                    head_sha: Some("feedbeef".to_string()),
                    branch: None,
                    detached: true,
                    bare: false,
                    is_main: false,
                },
            ]
        );
    }

    #[test]
    fn parses_status_porcelain_v2_snapshot() {
        let output = "\
# branch.oid abc123\0\
# branch.head feature\0\
1 .M N... 100644 100644 100644 a b file.txt\0\
? notes.txt\0";

        assert_eq!(
            parse_status_porcelain_v2(output),
            GitWorktreeStatusSnapshot {
                dirty: true,
                branch: Some("feature".to_string()),
                head_sha: Some("abc123".to_string()),
                records: vec![
                    "# branch.oid abc123".to_string(),
                    "# branch.head feature".to_string(),
                    "1 .M N... 100644 100644 100644 a b file.txt".to_string(),
                    "? notes.txt".to_string(),
                ],
            }
        );
    }

    #[test]
    fn validates_git_branch_names() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path());

        assert!(validate_git_branch_name(temp.path(), "codewith/valid-branch").expect("valid"));
        assert!(!validate_git_branch_name(temp.path(), "bad branch").expect("invalid"));
        assert!(!validate_git_branch_name(temp.path(), "-bad").expect("invalid"));
    }

    #[test]
    fn detects_commits_after_base_ref() {
        let temp = tempdir().expect("tempdir");
        init_repo(temp.path());
        let base_sha = git_stdout(temp.path(), &["rev-parse", "HEAD"]);
        assert!(!worktree_has_commits_after(temp.path(), base_sha.as_str()).expect("check"));

        std::fs::write(temp.path().join("feature.txt"), "feature\n").expect("write feature");
        run_git(temp.path(), &["add", "feature.txt"]);
        run_git(temp.path(), &["commit", "-m", "feature"]);

        assert!(worktree_has_commits_after(temp.path(), base_sha.as_str()).expect("check"));
    }
}
