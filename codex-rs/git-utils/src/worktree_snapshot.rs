use anyhow::Context;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task;

use crate::operations::resolve_head;
use crate::operations::resolve_repository_root;
use crate::operations::run_git_for_status;
use crate::operations::run_git_for_stdout;

/// A Git tree object representing the tracked and non-ignored files in a worktree.
///
/// Capturing a snapshot uses an isolated temporary index and object directory,
/// so it does not change the worktree's real index or persist working file
/// contents in the repository object database.
#[derive(Debug, Clone)]
pub struct GitWorktreeSnapshot {
    repository_root: PathBuf,
    repository_object_directory: PathBuf,
    snapshot_objects: Arc<GitSnapshotObjectDirectory>,
    tree_id: String,
}

#[derive(Debug)]
struct GitSnapshotObjectDirectory {
    _temporary_directory: tempfile::TempDir,
    objects_path: PathBuf,
}

impl PartialEq for GitWorktreeSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.repository_root == other.repository_root && self.tree_id == other.tree_id
    }
}

impl Eq for GitWorktreeSnapshot {}

/// Aggregate text line changes between two worktree snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitWorktreeLineChangeStats {
    pub lines_added: u64,
    pub lines_deleted: u64,
}

/// Captures the current tracked and non-ignored worktree contents as a Git tree.
pub async fn capture_git_worktree_snapshot(path: &Path) -> anyhow::Result<GitWorktreeSnapshot> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || capture_git_worktree_snapshot_sync(path.as_path())).await?
}

/// Resolves the root of the Git worktree containing `path`.
pub async fn resolve_git_worktree_root(path: &Path) -> anyhow::Result<PathBuf> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || {
        resolve_repository_root(path.as_path()).context("resolve worktree repository root")
    })
    .await?
}

/// Returns aggregate text line changes between two snapshots of the same repository.
pub async fn diff_git_worktree_snapshots(
    before: &GitWorktreeSnapshot,
    after: &GitWorktreeSnapshot,
) -> anyhow::Result<GitWorktreeLineChangeStats> {
    let before = before.clone();
    let after = after.clone();
    task::spawn_blocking(move || diff_git_worktree_snapshots_sync(&before, &after)).await?
}

fn capture_git_worktree_snapshot_sync(path: &Path) -> anyhow::Result<GitWorktreeSnapshot> {
    let repository_root =
        resolve_repository_root(path).context("resolve worktree repository root")?;
    let repository_object_directory = PathBuf::from(
        run_git_for_stdout(
            repository_root.as_path(),
            [
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
            /*env*/ None,
        )
        .context("resolve repository object directory")?,
    );
    let temporary_directory =
        tempfile::tempdir().context("create temporary Git snapshot directory")?;
    let index_path = temporary_directory.path().join("index");
    let objects_path = temporary_directory.path().join("objects");
    fs::create_dir(&objects_path).context("create temporary Git object directory")?;
    let alternate_objects = std::env::join_paths([repository_object_directory.as_path()])
        .context("encode repository object directory")?;
    let env = [
        (
            OsString::from("GIT_INDEX_FILE"),
            index_path.as_os_str().to_os_string(),
        ),
        (
            OsString::from("GIT_OBJECT_DIRECTORY"),
            objects_path.as_os_str().to_os_string(),
        ),
        (
            OsString::from("GIT_ALTERNATE_OBJECT_DIRECTORIES"),
            alternate_objects,
        ),
    ];

    if resolve_head(repository_root.as_path())
        .context("resolve worktree HEAD")?
        .is_some()
    {
        run_git_for_status(
            repository_root.as_path(),
            ["read-tree", "--reset", "HEAD"],
            Some(&env),
        )
        .context("initialize temporary Git index from HEAD")?;
    } else {
        run_git_for_status(
            repository_root.as_path(),
            ["read-tree", "--empty"],
            Some(&env),
        )
        .context("initialize empty temporary Git index")?;
    }
    run_git_for_status(
        repository_root.as_path(),
        ["add", "--all", "--", "."],
        Some(&env),
    )
    .context("populate temporary Git index from worktree")?;
    let tree_id = run_git_for_stdout(repository_root.as_path(), ["write-tree"], Some(&env))
        .context("write worktree snapshot tree")?;

    Ok(GitWorktreeSnapshot {
        repository_root,
        repository_object_directory,
        snapshot_objects: Arc::new(GitSnapshotObjectDirectory {
            _temporary_directory: temporary_directory,
            objects_path,
        }),
        tree_id,
    })
}

fn diff_git_worktree_snapshots_sync(
    before: &GitWorktreeSnapshot,
    after: &GitWorktreeSnapshot,
) -> anyhow::Result<GitWorktreeLineChangeStats> {
    if before.repository_root != after.repository_root {
        anyhow::bail!("cannot compare worktree snapshots from different repositories");
    }
    if before.repository_object_directory != after.repository_object_directory {
        anyhow::bail!("cannot compare worktree snapshots with different object databases");
    }
    let temporary_directory =
        tempfile::tempdir().context("create temporary Git comparison directory")?;
    let objects_path = temporary_directory.path().join("objects");
    fs::create_dir(&objects_path).context("create temporary comparison object directory")?;
    let alternate_objects = std::env::join_paths([
        before.repository_object_directory.as_path(),
        before.snapshot_objects.objects_path.as_path(),
        after.snapshot_objects.objects_path.as_path(),
    ])
    .context("encode snapshot object directories")?;
    let env = [
        (
            OsString::from("GIT_OBJECT_DIRECTORY"),
            objects_path.as_os_str().to_os_string(),
        ),
        (
            OsString::from("GIT_ALTERNATE_OBJECT_DIRECTORIES"),
            alternate_objects,
        ),
    ];
    let output = run_git_for_stdout(
        before.repository_root.as_path(),
        [
            "diff",
            "--numstat",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--ignore-submodules=all",
            before.tree_id.as_str(),
            after.tree_id.as_str(),
            "--",
        ],
        Some(&env),
    )
    .context("compare worktree snapshot trees")?;
    Ok(parse_numstat(output.as_str()))
}

fn parse_numstat(output: &str) -> GitWorktreeLineChangeStats {
    let mut stats = GitWorktreeLineChangeStats {
        lines_added: 0,
        lines_deleted: 0,
    };
    for line in output.lines() {
        let mut fields = line.splitn(3, '\t');
        let Some(added) = fields.next() else {
            continue;
        };
        let Some(deleted) = fields.next() else {
            continue;
        };
        let Ok(added) = added.parse::<u64>() else {
            continue;
        };
        let Ok(deleted) = deleted.parse::<u64>() else {
            continue;
        };
        stats.lines_added = stats.lines_added.saturating_add(added);
        stats.lines_deleted = stats.lines_deleted.saturating_add(deleted);
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::fs;

    #[tokio::test]
    async fn snapshots_count_same_file_replacements_and_untracked_files() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let repo = tempdir.path();
        init_repo(repo)?;
        fs::write(repo.join("tracked.txt"), "one\nbefore\nthree\n")?;
        run_git_for_status(repo, ["add", "."], /*env*/ None)?;
        commit(repo, "initial")?;
        let repository_objects = repo.join(".git").join("objects");
        let object_count_before = object_file_count(repository_objects.as_path())?;

        let before = capture_git_worktree_snapshot(repo).await?;
        fs::write(repo.join("tracked.txt"), "one\nafter\nthree\n")?;
        fs::write(repo.join("untracked.txt"), "new\nlines\n")?;
        let after = capture_git_worktree_snapshot(repo).await?;

        assert_eq!(
            GitWorktreeLineChangeStats {
                lines_added: 3,
                lines_deleted: 1,
            },
            diff_git_worktree_snapshots(&before, &after).await?
        );
        assert_eq!(
            object_count_before,
            object_file_count(repository_objects.as_path())?,
            "snapshot contents must stay out of the repository object database"
        );
        Ok(())
    }

    #[tokio::test]
    async fn snapshots_handle_unborn_repositories_with_large_untracked_file_sets()
    -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let repo = tempdir.path();
        init_repo(repo)?;
        let before = capture_git_worktree_snapshot(repo).await?;
        for index in 0..128 {
            fs::write(repo.join(format!("file-{index:03}.txt")), "one\ntwo\n")?;
        }
        let after = capture_git_worktree_snapshot(repo).await?;

        assert_eq!(
            GitWorktreeLineChangeStats {
                lines_added: 256,
                lines_deleted: 0,
            },
            diff_git_worktree_snapshots(&before, &after).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn snapshots_ignore_ignored_and_binary_changes_but_count_deletions() -> anyhow::Result<()>
    {
        let tempdir = tempfile::tempdir()?;
        let repo = tempdir.path();
        init_repo(repo)?;
        fs::write(repo.join(".gitignore"), "ignored.txt\n")?;
        fs::write(repo.join("tracked.txt"), "one\ntwo\n")?;
        fs::write(repo.join("binary.bin"), [0, 1, 2])?;
        run_git_for_status(repo, ["add", "."], /*env*/ None)?;
        commit(repo, "initial")?;
        let before = capture_git_worktree_snapshot(repo).await?;

        fs::remove_file(repo.join("tracked.txt"))?;
        fs::write(repo.join("binary.bin"), [0, 3, 4])?;
        fs::write(repo.join("ignored.txt"), "ignored\nlines\n")?;
        let after = capture_git_worktree_snapshot(repo).await?;

        assert_eq!(
            GitWorktreeLineChangeStats {
                lines_added: 0,
                lines_deleted: 2,
            },
            diff_git_worktree_snapshots(&before, &after).await?
        );
        Ok(())
    }

    #[tokio::test]
    async fn snapshots_detect_edited_renames_in_linked_worktrees() -> anyhow::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let repo = tempdir.path().join("main");
        let linked = tempdir.path().join("linked");
        fs::create_dir(&repo)?;
        init_repo(repo.as_path())?;
        fs::write(repo.join("before.txt"), "one\ntwo\nthree\nfour\n")?;
        run_git_for_status(repo.as_path(), ["add", "."], /*env*/ None)?;
        commit(repo.as_path(), "initial")?;
        let linked_path = linked
            .to_str()
            .context("linked worktree path should be valid UTF-8")?;
        run_git_for_status(
            repo.as_path(),
            ["worktree", "add", "-b", "linked-snapshot-test", linked_path],
            /*env*/ None,
        )?;
        let before = capture_git_worktree_snapshot(linked.as_path()).await?;

        fs::rename(linked.join("before.txt"), linked.join("after.txt"))?;
        fs::write(linked.join("after.txt"), "one\ntwo\nthree\nfour\nfive\n")?;
        let after = capture_git_worktree_snapshot(linked.as_path()).await?;

        assert_eq!(
            GitWorktreeLineChangeStats {
                lines_added: 1,
                lines_deleted: 0,
            },
            diff_git_worktree_snapshots(&before, &after).await?
        );
        Ok(())
    }

    fn init_repo(repo: &Path) -> anyhow::Result<()> {
        run_git_for_status(repo, ["init"], /*env*/ None).context("initialize test repository")
    }

    fn commit(repo: &Path, message: &str) -> anyhow::Result<()> {
        run_git_for_status(
            repo,
            [
                "-c",
                "user.name=Codewith Test",
                "-c",
                "user.email=codewith@example.invalid",
                "-c",
                "commit.gpgSign=false",
                "commit",
                "-m",
                message,
            ],
            /*env*/ None,
        )
        .context("commit test repository")
    }

    fn object_file_count(path: &Path) -> anyhow::Result<usize> {
        let mut count = 0;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                count += object_file_count(entry.path().as_path())?;
            } else {
                count += 1;
            }
        }
        Ok(count)
    }
}
