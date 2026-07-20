use async_trait::async_trait;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::io;
use tokio::io::AsyncReadExt;

use crate::CopyOptions;
use crate::CreateDirectoryOptions;
use crate::ExecServerRuntimePaths;
use crate::ExecutorFileSystem;
use crate::FILE_CHANGED_DURING_OPEN_ERROR;
use crate::FileMetadata;
use crate::FileSystemResult;
use crate::FileSystemSandboxContext;
use crate::ReadDirectoryEntry;
use crate::RemoveOptions;
use crate::SYMLINKED_FILE_ERROR;
use crate::sandboxed_file_system::SandboxedFileSystem;

const MAX_READ_FILE_BYTES: u64 = 512 * 1024 * 1024;

pub static LOCAL_FS: LazyLock<Arc<dyn ExecutorFileSystem>> =
    LazyLock::new(|| -> Arc<dyn ExecutorFileSystem> { Arc::new(LocalFileSystem::unsandboxed()) });

#[derive(Clone, Default)]
pub(crate) struct DirectFileSystem;

#[derive(Clone, Default)]
pub(crate) struct UnsandboxedFileSystem {
    file_system: DirectFileSystem,
}

#[derive(Clone, Default)]
pub struct LocalFileSystem {
    unsandboxed: UnsandboxedFileSystem,
    sandboxed: Option<SandboxedFileSystem>,
}

impl LocalFileSystem {
    pub fn unsandboxed() -> Self {
        Self {
            unsandboxed: UnsandboxedFileSystem::default(),
            sandboxed: None,
        }
    }

    pub fn with_runtime_paths(runtime_paths: ExecServerRuntimePaths) -> Self {
        Self {
            unsandboxed: UnsandboxedFileSystem::default(),
            sandboxed: Some(SandboxedFileSystem::new(runtime_paths)),
        }
    }

    fn sandboxed(&self) -> io::Result<&SandboxedFileSystem> {
        self.sandboxed.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "sandboxed filesystem operations require configured runtime paths",
            )
        })
    }

    fn file_system_for<'a>(
        &'a self,
        sandbox: Option<&'a FileSystemSandboxContext>,
    ) -> io::Result<(
        &'a dyn ExecutorFileSystem,
        Option<&'a FileSystemSandboxContext>,
    )> {
        if sandbox.is_some_and(FileSystemSandboxContext::should_run_in_sandbox) {
            Ok((self.sandboxed()?, sandbox))
        } else {
            Ok((&self.unsandboxed, sandbox))
        }
    }
}

#[async_trait]
impl ExecutorFileSystem for LocalFileSystem {
    async fn canonicalize(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<AbsolutePathBuf> {
        enforce_read_not_denied(path, sandbox)?;
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.canonicalize(path, sandbox).await
    }

    async fn join(
        &self,
        base_path: &AbsolutePathBuf,
        path: &Path,
    ) -> FileSystemResult<AbsolutePathBuf> {
        self.unsandboxed.join(base_path, path).await
    }

    async fn parent(&self, path: &AbsolutePathBuf) -> FileSystemResult<Option<AbsolutePathBuf>> {
        self.unsandboxed.parent(path).await
    }

    async fn read_file(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        enforce_read_not_denied(path, sandbox)?;
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.read_file(path, sandbox).await
    }

    async fn read_file_without_following_symlinks(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        enforce_read_not_denied(path, sandbox)?;
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system
            .read_file_without_following_symlinks(path, sandbox)
            .await
    }

    async fn write_file(
        &self,
        path: &AbsolutePathBuf,
        contents: Vec<u8>,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.write_file(path, contents, sandbox).await
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: CreateDirectoryOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.create_directory(path, options, sandbox).await
    }

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata> {
        enforce_read_not_denied(path, sandbox)?;
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.get_metadata(path, sandbox).await
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        enforce_read_not_denied(path, sandbox)?;
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.read_directory(path, sandbox).await
    }

    async fn remove(
        &self,
        path: &AbsolutePathBuf,
        options: RemoveOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.remove(path, options, sandbox).await
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: CopyOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        enforce_read_not_denied(source_path, sandbox)?;
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system
            .copy(source_path, destination_path, options, sandbox)
            .await
    }
}

#[async_trait]
impl ExecutorFileSystem for UnsandboxedFileSystem {
    async fn canonicalize(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<AbsolutePathBuf> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system.canonicalize(path, /*sandbox*/ None).await
    }

    async fn join(
        &self,
        base_path: &AbsolutePathBuf,
        path: &Path,
    ) -> FileSystemResult<AbsolutePathBuf> {
        self.file_system.join(base_path, path).await
    }

    async fn parent(&self, path: &AbsolutePathBuf) -> FileSystemResult<Option<AbsolutePathBuf>> {
        self.file_system.parent(path).await
    }

    async fn read_file(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system.read_file(path, /*sandbox*/ None).await
    }

    async fn read_file_without_following_symlinks(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system
            .read_file_without_following_symlinks(path, /*sandbox*/ None)
            .await
    }

    async fn write_file(
        &self,
        path: &AbsolutePathBuf,
        contents: Vec<u8>,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system
            .write_file(path, contents, /*sandbox*/ None)
            .await
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: CreateDirectoryOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system
            .create_directory(path, options, /*sandbox*/ None)
            .await
    }

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system.get_metadata(path, /*sandbox*/ None).await
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system
            .read_directory(path, /*sandbox*/ None)
            .await
    }

    async fn remove(
        &self,
        path: &AbsolutePathBuf,
        options: RemoveOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system
            .remove(path, options, /*sandbox*/ None)
            .await
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: CopyOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_platform_sandbox_context(sandbox)?;
        self.file_system
            .copy(
                source_path,
                destination_path,
                options,
                /*sandbox*/ None,
            )
            .await
    }
}

#[async_trait]
impl ExecutorFileSystem for DirectFileSystem {
    async fn canonicalize(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<AbsolutePathBuf> {
        reject_sandbox_context(sandbox)?;
        AbsolutePathBuf::from_absolute_path(tokio::fs::canonicalize(path.as_path()).await?)
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
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        reject_sandbox_context(sandbox)?;
        let metadata = tokio::fs::metadata(path.as_path()).await?;
        if metadata.len() > MAX_READ_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("file is too large to read: limit is {MAX_READ_FILE_BYTES} bytes"),
            ));
        }
        tokio::fs::read(path.as_path()).await
    }

    async fn read_file_without_following_symlinks(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
        reject_sandbox_context(sandbox)?;
        let path = path.as_path();
        let initial_path_metadata = tokio::fs::symlink_metadata(path).await?;
        reject_non_regular_or_symlink(&initial_path_metadata)?;

        let mut file = tokio::fs::File::open(path).await?;
        let file_metadata = file.metadata().await?;
        let current_path_metadata = tokio::fs::symlink_metadata(path).await?;
        reject_non_regular_or_symlink(&current_path_metadata)?;

        if !file_metadata.is_file() {
            return Err(not_regular_file_error());
        }
        verify_same_file_during_open(
            path,
            &file,
            &initial_path_metadata,
            &file_metadata,
            &current_path_metadata,
        )?;
        if file_metadata.len() > MAX_READ_FILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("file is too large to read: limit is {MAX_READ_FILE_BYTES} bytes"),
            ));
        }

        let mut data = Vec::new();
        file.read_to_end(&mut data).await?;
        Ok(data)
    }

    async fn write_file(
        &self,
        path: &AbsolutePathBuf,
        contents: Vec<u8>,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_sandbox_context(sandbox)?;
        tokio::fs::write(path.as_path(), contents).await
    }

    async fn create_directory(
        &self,
        path: &AbsolutePathBuf,
        options: CreateDirectoryOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_sandbox_context(sandbox)?;
        if options.recursive {
            tokio::fs::create_dir_all(path.as_path()).await?;
        } else {
            tokio::fs::create_dir(path.as_path()).await?;
        }
        Ok(())
    }

    async fn get_metadata(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<FileMetadata> {
        reject_sandbox_context(sandbox)?;
        let metadata = tokio::fs::metadata(path.as_path()).await?;
        let symlink_metadata = tokio::fs::symlink_metadata(path.as_path()).await?;
        Ok(FileMetadata {
            is_directory: metadata.is_dir(),
            is_file: metadata.is_file(),
            is_symlink: symlink_metadata.file_type().is_symlink(),
            created_at_ms: metadata.created().ok().map_or(0, system_time_to_unix_ms),
            modified_at_ms: metadata.modified().ok().map_or(0, system_time_to_unix_ms),
        })
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
        reject_sandbox_context(sandbox)?;
        let mut entries = Vec::new();
        let mut read_dir = tokio::fs::read_dir(path.as_path()).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let Ok(metadata) = tokio::fs::metadata(entry.path()).await else {
                continue;
            };
            entries.push(ReadDirectoryEntry {
                file_name: entry.file_name().to_string_lossy().into_owned(),
                is_directory: metadata.is_dir(),
                is_file: metadata.is_file(),
            });
        }
        Ok(entries)
    }

    async fn remove(
        &self,
        path: &AbsolutePathBuf,
        options: RemoveOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_sandbox_context(sandbox)?;
        match tokio::fs::symlink_metadata(path.as_path()).await {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_dir() {
                    if options.recursive {
                        tokio::fs::remove_dir_all(path.as_path()).await?;
                    } else {
                        tokio::fs::remove_dir(path.as_path()).await?;
                    }
                } else {
                    tokio::fs::remove_file(path.as_path()).await?;
                }
                Ok(())
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound && options.force => Ok(()),
            Err(err) => Err(err),
        }
    }

    async fn copy(
        &self,
        source_path: &AbsolutePathBuf,
        destination_path: &AbsolutePathBuf,
        options: CopyOptions,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<()> {
        reject_sandbox_context(sandbox)?;
        let source_path = source_path.to_path_buf();
        let destination_path = destination_path.to_path_buf();
        tokio::task::spawn_blocking(move || -> FileSystemResult<()> {
            let metadata = std::fs::symlink_metadata(source_path.as_path())?;
            let file_type = metadata.file_type();

            if file_type.is_dir() {
                if !options.recursive {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "fs/copy requires recursive: true when sourcePath is a directory",
                    ));
                }
                if destination_is_same_or_descendant_of_source(
                    source_path.as_path(),
                    destination_path.as_path(),
                )? {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "fs/copy cannot copy a directory to itself or one of its descendants",
                    ));
                }
                copy_dir_recursive(source_path.as_path(), destination_path.as_path())?;
                return Ok(());
            }

            if file_type.is_symlink() {
                copy_symlink(source_path.as_path(), destination_path.as_path())?;
                return Ok(());
            }

            if file_type.is_file() {
                std::fs::copy(source_path.as_path(), destination_path.as_path())?;
                return Ok(());
            }

            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fs/copy only supports regular files, directories, and symlinks",
            ))
        })
        .await
        .map_err(|err| io::Error::other(format!("filesystem task failed: {err}")))?
    }
}

/// Fail-closed, defense-in-depth read guard for the tool file-access layer.
///
/// Blocks the model's file tools from reading any path the sandbox context's
/// policy denies (`deny_read`), independent of whether the platform sandbox is
/// engaged. It runs before dispatch to either the sandboxed or the unsandboxed
/// backend, so a `deny_read` on the auth-profile credential store is honored on
/// the unsandboxed path too (where bwrap masking would not run). Codewith's own
/// credential loading uses `std::fs` directly and never flows through here, so
/// authentication keeps working while model-driven tool reads are blocked.
fn enforce_read_not_denied(
    path: &AbsolutePathBuf,
    sandbox: Option<&FileSystemSandboxContext>,
) -> io::Result<()> {
    if let Some(sandbox) = sandbox
        && sandbox.is_read_denied(path.as_path())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "read denied by filesystem sandbox policy (deny_read)",
        ));
    }
    Ok(())
}

fn reject_sandbox_context(sandbox: Option<&FileSystemSandboxContext>) -> io::Result<()> {
    if sandbox.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "direct filesystem operations do not accept sandbox context",
        ));
    }
    Ok(())
}

fn reject_non_regular_or_symlink(metadata: &std::fs::Metadata) -> io::Result<()> {
    if metadata.file_type().is_symlink() {
        return Err(symlinked_file_error());
    }
    if !metadata.is_file() {
        return Err(not_regular_file_error());
    }
    Ok(())
}

fn symlinked_file_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, SYMLINKED_FILE_ERROR)
}

fn not_regular_file_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "not a regular file")
}

fn file_changed_during_open_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, FILE_CHANGED_DURING_OPEN_ERROR)
}

/// Guard against a TOCTOU swap of `path` while it is being opened: confirm that
/// the file we actually opened is the same on-disk object that `symlink_metadata`
/// observed. On success returns `Ok(())`; otherwise a "file changed during open"
/// error.
#[cfg(unix)]
fn verify_same_file_during_open(
    _path: &Path,
    _file: &tokio::fs::File,
    initial_path_metadata: &std::fs::Metadata,
    file_metadata: &std::fs::Metadata,
    current_path_metadata: &std::fs::Metadata,
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    fn same(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
        a.dev() == b.dev() && a.ino() == b.ino()
    }

    if same(initial_path_metadata, file_metadata) && same(file_metadata, current_path_metadata) {
        Ok(())
    } else {
        Err(file_changed_during_open_error())
    }
}

/// Windows equivalent of the Unix same-file guard.
///
/// `std::fs::Metadata` does not expose the volume serial number or file index on
/// stable Rust (that lives behind the nightly-only `windows_by_handle` feature),
/// so identity is queried directly from open handles via
/// `GetFileInformationByHandle`. The path metadata was already verified to be a
/// regular file, so we re-open the path (without following the final reparse
/// point) and confirm its identity matches the file we actually opened.
#[cfg(windows)]
fn verify_same_file_during_open(
    path: &Path,
    file: &tokio::fs::File,
    _initial_path_metadata: &std::fs::Metadata,
    _file_metadata: &std::fs::Metadata,
    _current_path_metadata: &std::fs::Metadata,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;

    let opened_identity = windows_file_identity(file.as_raw_handle())?;
    let path_identity = windows_path_identity(path)?;

    if opened_identity == path_identity {
        Ok(())
    } else {
        Err(file_changed_during_open_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn verify_same_file_during_open(
    _path: &Path,
    _file: &tokio::fs::File,
    _initial_path_metadata: &std::fs::Metadata,
    _file_metadata: &std::fs::Metadata,
    _current_path_metadata: &std::fs::Metadata,
) -> io::Result<()> {
    // Fail closed on platforms where we cannot establish file identity.
    Err(file_changed_during_open_error())
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index: u64,
}

/// Query the volume serial number and file index for an open handle via
/// `GetFileInformationByHandle`, the stable replacement for the nightly-only
/// `Metadata::volume_serial_number`/`file_index` accessors.
#[cfg(windows)]
fn windows_file_identity(
    handle: std::os::windows::io::RawHandle,
) -> io::Result<WindowsFileIdentity> {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::GetFileInformationByHandle;

    // SAFETY: `handle` is a live file handle borrowed from an open `File`, and
    // `info` is owned, correctly aligned, writable storage for the call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle as HANDLE, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(WindowsFileIdentity {
        volume_serial_number: info.dwVolumeSerialNumber,
        file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// Open `path` for attribute queries only, without following the final reparse
/// point, and read its identity. Mirrors the `symlink_metadata` semantics used
/// to capture the path snapshots.
#[cfg(windows)]
fn windows_path_identity(path: &Path) -> io::Result<WindowsFileIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_DELETE;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE;

    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;

    windows_file_identity(file.as_raw_handle())
}

fn reject_platform_sandbox_context(sandbox: Option<&FileSystemSandboxContext>) -> io::Result<()> {
    if sandbox.is_some_and(FileSystemSandboxContext::should_run_in_sandbox) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandboxed filesystem operations require configured runtime paths",
        ));
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &target_path)?;
        } else if file_type.is_symlink() {
            copy_symlink(&source_path, &target_path)?;
        }
    }
    Ok(())
}

fn destination_is_same_or_descendant_of_source(
    source: &Path,
    destination: &Path,
) -> io::Result<bool> {
    let source = std::fs::canonicalize(source)?;
    let destination = resolve_existing_path(destination)?;
    Ok(destination.starts_with(&source))
}

pub(crate) fn resolve_existing_path(path: &Path) -> io::Result<PathBuf> {
    let mut unresolved_suffix = Vec::new();
    let mut existing_path = path;
    while !existing_path.exists() {
        let Some(file_name) = existing_path.file_name() else {
            break;
        };
        unresolved_suffix.push(file_name.to_os_string());
        let Some(parent) = existing_path.parent() else {
            break;
        };
        existing_path = parent;
    }

    let mut resolved = std::fs::canonicalize(existing_path)?;
    for file_name in unresolved_suffix.iter().rev() {
        resolved.push(file_name);
    }
    Ok(resolved)
}

pub(crate) fn current_sandbox_cwd() -> io::Result<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| io::Error::other(format!("failed to read current dir: {err}")))?;
    resolve_existing_path(cwd.as_path())
}

fn copy_symlink(source: &Path, target: &Path) -> io::Result<()> {
    let link_target = std::fs::read_link(source)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link_target, target)
    }
    #[cfg(windows)]
    {
        if symlink_points_to_directory(source)? {
            std::os::windows::fs::symlink_dir(&link_target, target)
        } else {
            std::os::windows::fs::symlink_file(&link_target, target)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = link_target;
        let _ = target;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "copying symlinks is unsupported on this platform",
        ))
    }
}

#[cfg(windows)]
fn symlink_points_to_directory(source: &Path) -> io::Result<bool> {
    use std::os::windows::fs::FileTypeExt;

    Ok(std::fs::symlink_metadata(source)?
        .file_type()
        .is_symlink_dir())
}

fn system_time_to_unix_ms(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::os::unix::fs::symlink;

    #[test]
    fn resolve_existing_path_handles_symlink_parent_dotdot_escape() -> io::Result<()> {
        let temp_dir = tempfile::TempDir::new()?;
        let allowed_dir = temp_dir.path().join("allowed");
        let outside_dir = temp_dir.path().join("outside");
        std::fs::create_dir_all(&allowed_dir)?;
        std::fs::create_dir_all(&outside_dir)?;
        symlink(&outside_dir, allowed_dir.join("link"))?;

        let resolved = resolve_existing_path(
            allowed_dir
                .join("link")
                .join("..")
                .join("secret.txt")
                .as_path(),
        )?;

        assert_eq!(
            resolved,
            resolve_existing_path(temp_dir.path())?.join("secret.txt")
        );
        Ok(())
    }

    /// SECURITY: the `deny_read` guard must prevent a model task from reading the
    /// AuthCapsule credential store (`$CODEWITH_HOME/auth_profiles`) through the
    /// tool file-access layer — including symlink, `..`, and directory spellings —
    /// while Codewith's own credential loading (plain `std::fs`, outside this
    /// layer) keeps working so it can still authenticate.
    #[tokio::test]
    async fn deny_read_blocks_auth_profiles_while_direct_read_still_works() -> io::Result<()> {
        use codex_protocol::models::PermissionProfile;
        use codex_protocol::permissions::FileSystemAccessMode;
        use codex_protocol::permissions::FileSystemPath;
        use codex_protocol::permissions::FileSystemSandboxEntry;
        use codex_protocol::permissions::FileSystemSandboxPolicy;
        use codex_protocol::permissions::NetworkSandboxPolicy;

        // Simulate CODEWITH_HOME with an auth-profile credential store.
        let codex_home = tempfile::TempDir::new()?;
        let auth_profiles_dir = codex_home.path().join("auth_profiles");
        let profile_dir = auth_profiles_dir.join("main");
        std::fs::create_dir_all(&profile_dir)?;
        let token_file = profile_dir.join("auth.json");
        let secret = b"SUBSCRIPTION-TOKEN-PLACEHOLDER".to_vec();
        std::fs::write(&token_file, &secret)?;

        // A workspace file the task IS allowed to read.
        let workspace = tempfile::TempDir::new()?;
        let allowed_file = workspace.path().join("notes.txt");
        std::fs::write(&allowed_file, b"ok")?;

        // Build the infinity-agent-equivalent policy: read-only + deny_read on the
        // credential store, exactly as `apply_infinity_agent_credential_deny` emits.
        let auth_profiles_abs = AbsolutePathBuf::from_absolute_path(&auth_profiles_dir)?;
        let mut fs_policy = FileSystemSandboxPolicy::read_only();
        fs_policy.entries.push(FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: auth_profiles_abs,
            },
            access: FileSystemAccessMode::Deny,
        });
        let profile =
            PermissionProfile::from_runtime_permissions(&fs_policy, NetworkSandboxPolicy::Restricted);
        let cwd = AbsolutePathBuf::from_absolute_path(workspace.path())?;
        let sandbox = FileSystemSandboxContext::from_permission_profile_with_cwd(profile, cwd);

        let fs = LocalFileSystem::unsandboxed();
        let token_abs = AbsolutePathBuf::from_absolute_path(&token_file)?;

        // 1) End-to-end: the model's file tool cannot read the subscription token.
        let denied = fs.read_file(&token_abs, Some(&sandbox)).await;
        assert_eq!(
            denied.as_ref().err().map(io::Error::kind),
            Some(io::ErrorKind::PermissionDenied),
            "expected deny_read to block the token read, got {denied:?}"
        );
        // The tool-layer guard itself denies the read directly.
        assert_eq!(
            enforce_read_not_denied(&token_abs, Some(&sandbox))
                .err()
                .map(|err| err.kind()),
            Some(io::ErrorKind::PermissionDenied),
        );

        // 2) Bypass attempts are also blocked (the matcher canonicalizes):
        //    a) a symlink that points into the credential store,
        let link = workspace.path().join("sneaky");
        symlink(&auth_profiles_dir, &link)?;
        let via_symlink = AbsolutePathBuf::from_absolute_path(link.join("main").join("auth.json"))?;
        assert!(
            sandbox.is_read_denied(via_symlink.as_path()),
            "symlink into the credential store must be denied"
        );
        //    b) a `..` traversal that resolves back into the store,
        let via_dotdot =
            AbsolutePathBuf::from_absolute_path(profile_dir.join("..").join("main").join("auth.json"))?;
        assert!(
            sandbox.is_read_denied(via_dotdot.as_path()),
            "`..` traversal into the credential store must be denied"
        );
        //    c) the credential directory itself.
        let dir_abs = AbsolutePathBuf::from_absolute_path(&auth_profiles_dir)?;
        assert!(sandbox.is_read_denied(dir_abs.as_path()));

        // 3) An allowed workspace path is NOT denied by the guard.
        let allowed_abs = AbsolutePathBuf::from_absolute_path(&allowed_file)?;
        assert!(!sandbox.is_read_denied(allowed_abs.as_path()));
        assert!(enforce_read_not_denied(&allowed_abs, Some(&sandbox)).is_ok());

        // 4) Codewith's own auth loader reads via `std::fs` (outside the tool
        //    layer) and is unaffected: the token is still readable, so
        //    authentication keeps working.
        assert_eq!(std::fs::read(&token_file)?, secret);

        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn symlink_points_to_directory_handles_dangling_directory_symlinks() -> io::Result<()> {
        use std::os::windows::fs::symlink_dir;

        let temp_dir = tempfile::TempDir::new()?;
        let source_dir = temp_dir.path().join("source");
        let link_path = temp_dir.path().join("source-link");
        std::fs::create_dir(&source_dir)?;

        if symlink_dir(&source_dir, &link_path).is_err() {
            return Ok(());
        }

        std::fs::remove_dir(&source_dir)?;

        assert_eq!(symlink_points_to_directory(&link_path)?, true);
        Ok(())
    }
}
