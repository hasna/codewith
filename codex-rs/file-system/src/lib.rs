use async_trait::async_trait;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::SandboxEnforcement;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxKind;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::FileSystemSpecialPath;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::permissions::ReadDenyMatcher;
use codex_protocol::protocol::SandboxPolicy;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreateDirectoryOptions {
    pub recursive: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoveOptions {
    pub recursive: bool,
    pub force: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CopyOptions {
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileMetadata {
    pub is_directory: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    pub created_at_ms: i64,
    pub modified_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadDirectoryEntry {
    pub file_name: String,
    pub is_directory: bool,
    pub is_file: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemSandboxContext {
    pub permissions: PermissionProfile,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<AbsolutePathBuf>,
    pub windows_sandbox_level: WindowsSandboxLevel,
    #[serde(default)]
    pub windows_sandbox_private_desktop: bool,
    #[serde(default)]
    pub use_legacy_landlock: bool,
}

impl FileSystemSandboxContext {
    pub fn from_legacy_sandbox_policy(sandbox_policy: SandboxPolicy, cwd: AbsolutePathBuf) -> Self {
        let file_system_sandbox_policy =
            FileSystemSandboxPolicy::from_legacy_sandbox_policy_for_cwd(&sandbox_policy, &cwd);
        let permissions = PermissionProfile::from_runtime_permissions_with_enforcement(
            SandboxEnforcement::from_legacy_sandbox_policy(&sandbox_policy),
            &file_system_sandbox_policy,
            NetworkSandboxPolicy::from(&sandbox_policy),
        );
        Self::from_permission_profile_with_cwd(permissions, cwd)
    }

    pub fn from_permission_profile(permissions: PermissionProfile) -> Self {
        Self::from_permissions_and_cwd(permissions, /*cwd*/ None)
    }

    pub fn from_permission_profile_with_cwd(
        permissions: PermissionProfile,
        cwd: AbsolutePathBuf,
    ) -> Self {
        Self::from_permissions_and_cwd(permissions, Some(cwd))
    }

    fn from_permissions_and_cwd(
        permissions: PermissionProfile,
        cwd: Option<AbsolutePathBuf>,
    ) -> Self {
        Self {
            permissions,
            cwd,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
            windows_sandbox_private_desktop: false,
            use_legacy_landlock: false,
        }
    }

    pub fn should_run_in_sandbox(&self) -> bool {
        let file_system_policy = self.permissions.file_system_sandbox_policy();
        matches!(file_system_policy.kind, FileSystemSandboxKind::Restricted)
            && !file_system_policy.has_full_disk_write_access()
    }

    /// Whether the model's file/shell tools are denied read access to `path`
    /// under this context's policy.
    ///
    /// This is the fail-closed, defense-in-depth read guard the tool
    /// file-access layer consults before every read/metadata/list, independent
    /// of whether the platform sandbox is engaged (see the `LocalFileSystem`
    /// guard in `exec-server`). The underlying [`ReadDenyMatcher`] canonicalizes
    /// candidate paths, so symlinks, `..`, and `/proc/self/root` spellings that
    /// resolve into a denied root are all caught. Codewith's own credential
    /// loading uses `std::fs` directly and never flows through this path, so
    /// authentication keeps working while model-driven tools are blocked.
    pub fn is_read_denied(&self, path: &Path) -> bool {
        let file_system_policy = self.permissions.file_system_sandbox_policy();
        let cwd = self
            .cwd
            .as_ref()
            .map_or_else(|| Path::new("/"), AbsolutePathBuf::as_path);
        ReadDenyMatcher::new(&file_system_policy, cwd)
            .is_some_and(|matcher| matcher.is_read_denied(path))
    }

    pub fn has_cwd_dependent_permissions(&self) -> bool {
        let file_system_policy = self.permissions.file_system_sandbox_policy();
        file_system_policy_has_cwd_dependent_entries(&file_system_policy)
    }

    pub fn drop_cwd_if_unused(mut self) -> Self {
        if !self.has_cwd_dependent_permissions() {
            self.cwd = None;
        }
        self
    }
}

fn file_system_policy_has_cwd_dependent_entries(
    file_system_policy: &FileSystemSandboxPolicy,
) -> bool {
    file_system_policy
        .entries
        .iter()
        .any(|entry| match &entry.path {
            FileSystemPath::GlobPattern { pattern } => !Path::new(pattern).is_absolute(),
            FileSystemPath::Special {
                value: FileSystemSpecialPath::ProjectRoots { .. },
            } => true,
            FileSystemPath::Path { .. } | FileSystemPath::Special { .. } => false,
        })
}

pub type FileSystemResult<T> = io::Result<T>;

pub const SYMLINKED_FILE_ERROR: &str = "symlinked files are not allowed";
pub const FILE_CHANGED_DURING_OPEN_ERROR: &str = "file changed while opening";
pub const SYMLINK_SAFE_READ_UNSUPPORTED_ERROR: &str =
    "filesystem does not support symlink-safe reads";

/// Abstract filesystem access used by components that may operate locally or via
/// a remote environment.
#[async_trait]
pub trait ExecutorFileSystem: Send + Sync {
    /// Resolves a path within this filesystem.
    async fn canonicalize(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<AbsolutePathBuf>;

    /// Lexically joins a path onto an existing bound path.
    async fn join(
        &self,
        base_path: &AbsolutePathBuf,
        path: &Path,
    ) -> FileSystemResult<AbsolutePathBuf>;

    /// Returns the parent directory of a bound path.
    async fn parent(&self, path: &AbsolutePathBuf) -> FileSystemResult<Option<AbsolutePathBuf>>;

    async fn read_file(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>>;

    async fn read_file_without_following_symlinks(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        let _ = (path, sandbox);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            SYMLINK_SAFE_READ_UNSUPPORTED_ERROR,
        ))
    }

    /// Reads a file and decodes it as UTF-8 text.
    async fn read_file_text(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<String> {
        let bytes = self.read_file(path, sandbox).await?;
        String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    async fn write_file(
        &self,
        path: &AbsolutePathBuf,
        contents: Vec<u8>,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()>;

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        create_directory_options: CreateDirectoryOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()>;

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata>;

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>>;

    async fn remove(
        &self,
        path: &AbsolutePathBuf,
        remove_options: RemoveOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()>;

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        copy_options: CopyOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()>;
}
