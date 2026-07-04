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
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.read_file(path, sandbox).await
    }

    async fn read_file_without_following_symlinks(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<u8>> {
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
        let (file_system, sandbox) = self.file_system_for(sandbox)?;
        file_system.get_metadata(path, sandbox).await
    }

    async fn read_directory(
        &self,
        path: &AbsolutePathBuf,
        sandbox: Option<&FileSystemSandboxContext>,
    ) -> FileSystemResult<Vec<ReadDirectoryEntry>> {
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
        let initial_path_snapshot = path_snapshot_without_following_symlinks(path).await?;
        initial_path_snapshot.reject_non_regular_or_symlink()?;

        #[cfg(all(test, windows))]
        replace_regular_file_before_open_for_test(path)?;

        let mut file = tokio::fs::File::open(path).await?;
        let file_metadata = file.metadata().await?;
        let current_path_snapshot = path_snapshot_without_following_symlinks(path).await?;
        current_path_snapshot.reject_non_regular_or_symlink()?;

        if !file_metadata.is_file() {
            return Err(not_regular_file_error());
        }
        verify_same_file_during_open(
            &initial_path_snapshot,
            &file,
            &file_metadata,
            &current_path_snapshot,
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

#[cfg(unix)]
struct PathSnapshot {
    metadata: std::fs::Metadata,
}

#[cfg(unix)]
async fn path_snapshot_without_following_symlinks(path: &Path) -> io::Result<PathSnapshot> {
    Ok(PathSnapshot {
        metadata: tokio::fs::symlink_metadata(path).await?,
    })
}

#[cfg(unix)]
impl PathSnapshot {
    fn reject_non_regular_or_symlink(&self) -> io::Result<()> {
        reject_non_regular_or_symlink(&self.metadata)
    }
}

#[cfg(windows)]
struct PathSnapshot {
    identity: WindowsFileIdentity,
    file_attributes: u32,
}

#[cfg(windows)]
async fn path_snapshot_without_following_symlinks(path: &Path) -> io::Result<PathSnapshot> {
    windows_path_snapshot(path)
}

#[cfg(windows)]
impl PathSnapshot {
    fn reject_non_regular_or_symlink(&self) -> io::Result<()> {
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

        if self.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(symlinked_file_error());
        }
        if self.file_attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err(not_regular_file_error());
        }
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
struct PathSnapshot;

#[cfg(not(any(unix, windows)))]
async fn path_snapshot_without_following_symlinks(_path: &Path) -> io::Result<PathSnapshot> {
    Err(file_changed_during_open_error())
}

#[cfg(not(any(unix, windows)))]
impl PathSnapshot {
    fn reject_non_regular_or_symlink(&self) -> io::Result<()> {
        Err(file_changed_during_open_error())
    }
}

/// Guard against a TOCTOU swap of `path` while it is being opened: confirm that
/// the file we actually opened is the same on-disk object as the initial and
/// current no-follow path snapshots. On success returns `Ok(())`; otherwise a
/// "file changed during open" error.
#[cfg(unix)]
fn verify_same_file_during_open(
    initial_path_snapshot: &PathSnapshot,
    _file: &tokio::fs::File,
    file_metadata: &std::fs::Metadata,
    current_path_snapshot: &PathSnapshot,
) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    fn same(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
        a.dev() == b.dev() && a.ino() == b.ino()
    }

    if same(&initial_path_snapshot.metadata, file_metadata)
        && same(file_metadata, &current_path_snapshot.metadata)
    {
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
/// `GetFileInformationByHandle`. Path snapshots are captured by opening the
/// path without following the final reparse point, so the initial snapshot, the
/// file we actually opened, and the current path snapshot must all match.
#[cfg(windows)]
fn verify_same_file_during_open(
    initial_path_snapshot: &PathSnapshot,
    file: &tokio::fs::File,
    _file_metadata: &std::fs::Metadata,
    current_path_snapshot: &PathSnapshot,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;

    let opened_identity = windows_file_identity(file.as_raw_handle())?;

    if initial_path_snapshot.identity == opened_identity
        && opened_identity == current_path_snapshot.identity
    {
        Ok(())
    } else {
        Err(file_changed_during_open_error())
    }
}

#[cfg(not(any(unix, windows)))]
fn verify_same_file_during_open(
    _initial_path_snapshot: &PathSnapshot,
    _file: &tokio::fs::File,
    _file_metadata: &std::fs::Metadata,
    _current_path_snapshot: &PathSnapshot,
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

#[cfg(windows)]
struct WindowsFileInformation {
    identity: WindowsFileIdentity,
    file_attributes: u32,
}

/// Query the volume serial number and file index for an open handle via
/// `GetFileInformationByHandle`, the stable replacement for the nightly-only
/// `Metadata::volume_serial_number`/`file_index` accessors.
#[cfg(windows)]
fn windows_file_identity(
    handle: std::os::windows::io::RawHandle,
) -> io::Result<WindowsFileIdentity> {
    Ok(windows_file_information(handle)?.identity)
}

#[cfg(windows)]
fn windows_file_information(
    handle: std::os::windows::io::RawHandle,
) -> io::Result<WindowsFileInformation> {
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

    Ok(WindowsFileInformation {
        identity: WindowsFileIdentity {
            volume_serial_number: info.dwVolumeSerialNumber,
            file_index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        },
        file_attributes: info.dwFileAttributes,
    })
}

/// Open `path` for attribute queries only, without following the final reparse
/// point, and read its identity plus file attributes as one path snapshot.
#[cfg(windows)]
fn windows_path_snapshot(path: &Path) -> io::Result<PathSnapshot> {
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

    let info = windows_file_information(file.as_raw_handle())?;
    Ok(PathSnapshot {
        identity: info.identity,
        file_attributes: info.file_attributes,
    })
}

#[cfg(all(test, windows))]
static REPLACE_REGULAR_FILE_BEFORE_OPEN_FOR_TEST: std::sync::Mutex<Option<PathBuf>> =
    std::sync::Mutex::new(None);

#[cfg(all(test, windows))]
fn replace_regular_file_before_open_for_test(path: &Path) -> io::Result<()> {
    let mut pending_path = REPLACE_REGULAR_FILE_BEFORE_OPEN_FOR_TEST
        .lock()
        .map_err(|_| io::Error::other("test replacement hook poisoned"))?;
    let Some(path_to_replace) = pending_path.take() else {
        return Ok(());
    };

    if path != path_to_replace {
        *pending_path = Some(path_to_replace);
        return Ok(());
    }
    drop(pending_path);

    let original_path = path.with_extension("original-before-open");
    std::fs::rename(path, original_path)?;
    std::fs::write(path, b"replacement contents")?;
    Ok(())
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

    #[tokio::test]
    async fn read_without_following_symlinks_rejects_regular_file_replacement_during_open()
    -> io::Result<()> {
        let temp_dir = tempfile::TempDir::new()?;
        let file_path = temp_dir.path().join("note.txt");
        std::fs::write(&file_path, b"initial contents")?;
        let absolute_path =
            AbsolutePathBuf::try_from(file_path.clone()).expect("temp file path is absolute");

        *REPLACE_REGULAR_FILE_BEFORE_OPEN_FOR_TEST
            .lock()
            .expect("test replacement hook should not be poisoned") = Some(file_path.clone());
        let error = LocalFileSystem::unsandboxed()
            .read_file_without_following_symlinks(&absolute_path, /*sandbox*/ None)
            .await
            .expect_err("regular-file replacement during open must be rejected");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), FILE_CHANGED_DURING_OPEN_ERROR);
        assert_eq!(std::fs::read(file_path)?, b"replacement contents");
        Ok(())
    }
}
