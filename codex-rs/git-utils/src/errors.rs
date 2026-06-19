use std::path::PathBuf;
use std::process::ExitStatus;
use std::string::FromUtf8Error;

use thiserror::Error;
use walkdir::Error as WalkdirError;

/// Errors returned while managing git worktree snapshots.
#[derive(Debug, Error)]
pub enum GitToolingError {
    #[error("git command `{command}` failed with status {status}: {stderr}")]
    GitCommand {
        command: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("git command `{command}` produced non-UTF-8 output")]
    GitOutputUtf8 {
        command: String,
        #[source]
        source: FromUtf8Error,
    },
    #[error("{path:?} is not a git repository")]
    NotAGitRepository { path: PathBuf },
    #[error("path {path:?} must be relative to the repository root")]
    NonRelativePath { path: PathBuf },
    #[error("path {path:?} escapes the repository root")]
    PathEscapesRepository { path: PathBuf },
    #[error("path {worktree_path:?} is not a linked worktree of repository {base_repo_path:?}")]
    WorktreeNotLinked {
        base_repo_path: PathBuf,
        worktree_path: PathBuf,
    },
    #[error("refusing to remove the main worktree {path:?}")]
    MainWorktreeRemovalRefused { path: PathBuf },
    #[error("failed to process path inside worktree")]
    PathPrefix(#[from] std::path::StripPrefixError),
    #[error(transparent)]
    Walkdir(#[from] WalkdirError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
