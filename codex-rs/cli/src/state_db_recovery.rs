//! CLI recovery for local state database startup failures.
//!
//! This keeps user-facing repair and lock-contention handling out of the main
//! CLI dispatch path while preserving the TUI startup error as the boundary type.

use codex_tui::LocalStateDbStartupError;
#[cfg(target_os = "linux")]
use std::collections::BTreeMap;
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const MAX_LOCK_HOLDERS_TO_PRINT: usize = 12;

pub(crate) fn startup_error(err: &std::io::Error) -> Option<&LocalStateDbStartupError> {
    err.get_ref()
        .and_then(|err| err.downcast_ref::<LocalStateDbStartupError>())
}

pub(crate) fn is_locked(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("database is locked")
        || detail.contains("database is busy")
        || detail.contains("timed out waiting for codewith state startup lock")
}

pub(crate) fn confirm_repair(startup_error: &LocalStateDbStartupError) -> std::io::Result<bool> {
    eprintln!("Codewith couldn't start because its local database appears to be damaged.");
    eprintln!("Codewith can try a safe repair by backing up those files and rebuilding them.");
    print_technical_details(startup_error);
    super::confirm("Repair Codewith local data now? [y/N]: ")
}

pub(crate) async fn repair_files(
    startup_error: &LocalStateDbStartupError,
) -> std::io::Result<Vec<PathBuf>> {
    let state_db_path = startup_error.state_db_path();
    let sqlite_home = state_db_path.parent().ok_or_else(|| {
        std::io::Error::other("state database path does not have a parent directory")
    })?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let repair_suffix = format!("codex-repair-{timestamp}");
    let mut backups = Vec::new();

    match tokio::fs::metadata(sqlite_home).await {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            backups.push(backup_path(sqlite_home, &repair_suffix).await?);
            tokio::fs::create_dir_all(sqlite_home).await?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(sqlite_home).await?;
        }
        Err(err) => return Err(err),
    }

    for path in repair_candidate_paths(sqlite_home, startup_error.detail()).await? {
        if tokio::fs::try_exists(path.as_path()).await? {
            backups.push(backup_path(path.as_path(), &repair_suffix).await?);
        }
    }

    if backups.is_empty() {
        return Err(std::io::Error::other(
            "no repairable Codewith local data files were found",
        ));
    }

    Ok(backups)
}

async fn repair_candidate_paths(sqlite_home: &Path, detail: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = codex_state::runtime_db_paths(sqlite_home)
        .into_iter()
        .flat_map(|db| sqlite_paths(db.path.as_path()))
        .collect();
    if detail.contains("failed to acquire state runtime startup lock") {
        let startup_lock_path = codex_state::state_runtime_startup_lock_path(sqlite_home);
        match tokio::fs::metadata(startup_lock_path.as_path()).await {
            Ok(metadata) if !metadata.is_file() => paths.push(startup_lock_path),
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(paths)
}

pub(crate) fn print_repair_backups(backups: &[PathBuf]) {
    eprintln!("Backed up Codewith local data before repair:");
    for backup in backups {
        eprintln!("  {}", backup.display());
    }
    eprintln!("Retrying startup with rebuilt local data...");
}

pub(crate) fn print_diagnostic_guidance(startup_error: &LocalStateDbStartupError) {
    eprintln!("Codewith couldn't start because its local database appears to be damaged.");
    eprintln!("Run `codewith doctor` to check your setup and get next-step guidance.");
    eprintln!("If this keeps happening, share the technical details below when asking for help.");
    print_technical_details(startup_error);
}

pub(crate) fn print_locked_guidance(startup_error: &LocalStateDbStartupError) {
    eprintln!("Codewith couldn't start because another Codewith process is using its local data.");
    eprintln!("Quit any other copies of Codewith that may still be running, then try again.");
    let lock_holders = lock_holders_for_runtime_dbs(startup_error.state_db_path());
    if !lock_holders.is_empty() {
        eprintln!("Local processes currently using that local data:");
        for holder in lock_holders.iter().take(MAX_LOCK_HOLDERS_TO_PRINT) {
            eprintln!("  {}", holder.summary());
        }
        let hidden_count = lock_holders.len().saturating_sub(MAX_LOCK_HOLDERS_TO_PRINT);
        if hidden_count > 0 {
            eprintln!("  ... and {hidden_count} more");
        }
    }
    print_lock_scope_guidance(!lock_holders.is_empty());
    print_technical_details(startup_error);
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LockHolder {
    pid: u32,
    state: Option<String>,
    databases: BTreeSet<String>,
    command: String,
}

impl LockHolder {
    fn summary(&self) -> String {
        let databases = self
            .databases
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let databases = if databases.is_empty() {
            String::new()
        } else {
            format!(" ({databases})")
        };
        match self.state.as_deref() {
            Some(state) => format!(
                "pid {} [{}]{} {}",
                self.pid,
                truncate_for_display(state),
                databases,
                self.command
            ),
            None => format!("pid {}{} {}", self.pid, databases, self.command),
        }
    }
}

#[cfg(target_os = "linux")]
fn lock_holders_for_runtime_dbs(state_db_path: &Path) -> Vec<LockHolder> {
    use std::os::unix::fs::MetadataExt;

    let Some(sqlite_home) = state_db_path.parent() else {
        return Vec::new();
    };
    let mut targets = BTreeMap::new();
    let startup_lock_path = codex_state::state_runtime_startup_lock_path(sqlite_home);
    if let Ok(metadata) = fs::metadata(startup_lock_path.as_path()) {
        targets.insert(
            (metadata.dev(), metadata.ino()),
            "state startup lock".to_string(),
        );
    }
    for db in codex_state::runtime_db_paths(sqlite_home) {
        for path in sqlite_paths(db.path.as_path()) {
            if let Ok(metadata) = fs::metadata(path.as_path()) {
                targets.insert((metadata.dev(), metadata.ino()), db.label.to_string());
            }
        }
    }
    if targets.is_empty() {
        return Vec::new();
    }

    let Ok(proc_entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut holders = BTreeMap::new();
    for proc_entry in proc_entries.flatten() {
        let Some(pid) = proc_entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let fd_dir = proc_entry.path().join("fd");
        let Ok(fd_entries) = fs::read_dir(fd_dir) else {
            continue;
        };
        let mut databases = BTreeSet::new();
        for fd_entry in fd_entries.flatten() {
            if let Ok(metadata) = fs::metadata(fd_entry.path())
                && let Some(label) = targets.get(&(metadata.dev(), metadata.ino()))
            {
                databases.insert(label.clone());
            }
        }
        if !databases.is_empty() {
            holders.insert(pid, lock_holder_for_pid(pid, databases));
        }
    }
    holders.into_values().collect()
}

#[cfg(not(target_os = "linux"))]
fn lock_holders_for_runtime_dbs(_state_db_path: &Path) -> Vec<LockHolder> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn print_lock_scope_guidance(found_local_holders: bool) {
    if found_local_holders {
        eprintln!(
            "This list only includes processes on this Linux host. If CODEWITH_HOME or CODEX_SQLITE_HOME is shared across hosts, check the other hosts too."
        );
    } else {
        eprintln!(
            "No local Linux process was found holding the runtime DB files. If CODEWITH_HOME or CODEX_SQLITE_HOME is shared across hosts, the lock may be held on another host."
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn print_lock_scope_guidance(_found_local_holders: bool) {
    eprintln!(
        "Codewith cannot list lock holders automatically on this platform. If CODEWITH_HOME or CODEX_SQLITE_HOME is shared across hosts, check those hosts too."
    );
}

#[cfg(target_os = "linux")]
fn lock_holder_for_pid(pid: u32, databases: BTreeSet<String>) -> LockHolder {
    LockHolder {
        pid,
        state: linux_process_state(pid),
        databases,
        command: linux_process_command(pid),
    }
}

#[cfg(target_os = "linux")]
fn linux_process_state(pid: u32) -> Option<String> {
    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(target_os = "linux")]
fn linux_process_command(pid: u32) -> String {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok();
    if let Some(cmdline) = cmdline
        && let Some(command) = summarize_cmdline(&cmdline)
    {
        return command;
    }
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|command| truncate_for_display(command.trim()))
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| "<unknown command>".to_string())
}

#[cfg(any(target_os = "linux", test))]
fn summarize_cmdline(cmdline: &[u8]) -> Option<String> {
    let args: Vec<String> = cmdline
        .split(|byte| *byte == b'\0')
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    if args.is_empty() {
        return None;
    }

    let mut summary = Vec::new();
    summary.push(command_name(args[0].as_str()));
    let mut index = 1;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "exec" || arg == "resume" || arg == "app-server" {
            summary.push(arg.to_string());
            index += 1;
            continue;
        }
        if matches!(
            arg,
            "--no-alt-screen"
                | "--dangerously-bypass-approvals-and-sandbox"
                | "--last"
                | "--all"
                | "--include-non-interactive"
        ) {
            summary.push(arg.to_string());
            index += 1;
            continue;
        }
        if matches!(
            arg,
            "--auth-profile"
                | "--cd"
                | "-C"
                | "--sandbox"
                | "--ask-for-approval"
                | "--listen"
                | "--output-last-message"
                | "--model"
                | "-m"
        ) {
            summary.push(arg.to_string());
            if let Some(value) = args.get(index + 1) {
                summary.push(truncate_for_display(value));
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if arg == "-c" || arg == "--config" {
            summary.push(arg.to_string());
            if args.get(index + 1).is_some() {
                summary.push("<redacted>".to_string());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if let Some((flag, value)) = arg.split_once('=') {
            if flag_contains_sensitive_name(flag) {
                summary.push(format!("{flag}=<redacted>"));
            } else if is_safe_inline_flag(flag) {
                summary.push(format!("{flag}={}", truncate_for_display(value)));
            } else {
                summary.push(flag.to_string());
            }
            index += 1;
            continue;
        }
        if arg.starts_with('-') {
            summary.push(arg.to_string());
            index += 1;
            continue;
        }
        summary.push("...".to_string());
        break;
    }
    Some(truncate_for_display(summary.join(" ").as_str()))
}

#[cfg(any(target_os = "linux", test))]
fn command_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(command)
        .to_string()
}

#[cfg(any(target_os = "linux", test))]
fn flag_contains_sensitive_name(flag: &str) -> bool {
    let flag = flag.to_ascii_lowercase();
    flag.contains("api-key")
        || flag.contains("apikey")
        || flag.contains("token")
        || flag.contains("secret")
        || flag.contains("password")
        || flag.contains("credential")
}

#[cfg(any(target_os = "linux", test))]
fn is_safe_inline_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--auth-profile"
            | "--cd"
            | "--sandbox"
            | "--ask-for-approval"
            | "--listen"
            | "--output-last-message"
            | "--model"
    )
}

fn truncate_for_display(value: &str) -> String {
    const MAX_DISPLAY_CHARS: usize = 220;
    let sanitized = sanitize_for_display(value);
    let mut iter = sanitized.chars();
    let truncated: String = iter.by_ref().take(MAX_DISPLAY_CHARS).collect();
    if iter.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn sanitize_for_display(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if matches!(ch, '\n' | '\r' | '\t') {
                ' '
            } else if ch.is_control() {
                '?'
            } else {
                ch
            }
        })
        .collect()
}

fn sqlite_paths(db_path: &Path) -> Vec<PathBuf> {
    let mut wal_path = db_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let mut shm_path = db_path.as_os_str().to_os_string();
    shm_path.push("-shm");
    vec![
        db_path.to_path_buf(),
        PathBuf::from(wal_path),
        PathBuf::from(shm_path),
    ]
}

async fn backup_path(path: &std::path::Path, repair_suffix: &str) -> std::io::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::other(format!(
            "cannot create a repair backup name for {}",
            path.display()
        ))
    })?;
    let mut sequence = 0;
    loop {
        let mut backup_name = file_name.to_os_string();
        backup_name.push(format!(".{repair_suffix}.{sequence}.bak"));
        let backup_path = path.with_file_name(backup_name);
        if !tokio::fs::try_exists(backup_path.as_path()).await? {
            tokio::fs::rename(path, backup_path.as_path()).await?;
            return Ok(backup_path);
        }
        sequence += 1;
    }
}

fn print_technical_details(startup_error: &LocalStateDbStartupError) {
    eprintln!("Technical details:");
    eprintln!("  Location: {}", startup_error.state_db_path().display());
    eprintln!("  Cause: {}", startup_error.detail());
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    #[tokio::test]
    async fn repair_backs_up_owned_database_files() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let state_path = codex_state::state_db_path(temp_dir.path());
        let logs_path = codex_state::logs_db_path(temp_dir.path());
        let goals_path = codex_state::goals_db_path(temp_dir.path());
        let state_sidecars = sqlite_paths(state_path.as_path());
        tokio::fs::write(state_path.as_path(), b"state").await?;
        tokio::fs::write(state_sidecars[1].as_path(), b"state-wal").await?;
        tokio::fs::write(logs_path.as_path(), b"logs").await?;
        tokio::fs::write(goals_path.as_path(), b"goals").await?;

        let startup_error =
            LocalStateDbStartupError::new(state_path.clone(), "corrupt".to_string());
        let backups = repair_files(&startup_error).await?;

        assert_eq!(backups.len(), 4);
        assert!(!tokio::fs::try_exists(state_path.as_path()).await?);
        assert!(!tokio::fs::try_exists(state_sidecars[1].as_path()).await?);
        assert!(!tokio::fs::try_exists(logs_path.as_path()).await?);
        assert!(!tokio::fs::try_exists(goals_path.as_path()).await?);
        for backup in backups {
            assert!(tokio::fs::try_exists(backup.as_path()).await?);
        }
        Ok(())
    }

    #[tokio::test]
    async fn repair_replaces_blocking_sqlite_home_file() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let sqlite_home = temp_dir.path().join("sqlite-home");
        tokio::fs::write(sqlite_home.as_path(), b"not-a-directory").await?;
        let startup_error = LocalStateDbStartupError::new(
            codex_state::state_db_path(sqlite_home.as_path()),
            "File exists".to_string(),
        );

        let backups = repair_files(&startup_error).await?;

        assert_eq!(backups.len(), 1);
        assert!(tokio::fs::metadata(sqlite_home.as_path()).await?.is_dir());
        assert!(tokio::fs::try_exists(backups[0].as_path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn repair_replaces_blocking_startup_lock_directory() -> std::io::Result<()> {
        let temp_dir = TempDir::new()?;
        let state_path = codex_state::state_db_path(temp_dir.path());
        let startup_lock_path = codex_state::state_runtime_startup_lock_path(temp_dir.path());
        tokio::fs::create_dir(startup_lock_path.as_path()).await?;
        let startup_error = LocalStateDbStartupError::new(
            state_path,
            format!(
                "failed to acquire state runtime startup lock at {}: Is a directory",
                temp_dir.path().display()
            ),
        );

        let backups = repair_files(&startup_error).await?;

        assert_eq!(backups.len(), 1);
        assert!(!tokio::fs::try_exists(startup_lock_path.as_path()).await?);
        assert!(tokio::fs::metadata(backups[0].as_path()).await?.is_dir());
        Ok(())
    }

    #[test]
    fn lock_failures_skip_repair() {
        assert!(is_locked("database is locked"));
        assert!(is_locked("database is busy"));
        assert!(is_locked(
            "failed to acquire state runtime startup lock at /tmp/home: timed out waiting for Codewith state startup lock at /tmp/home/.state-runtime-startup.lock after 60s"
        ));
        assert!(!is_locked("database disk image is malformed"));
        assert!(!is_locked(
            "failed to acquire state runtime startup lock at /tmp/home: File exists"
        ));
    }

    #[test]
    fn command_summary_omits_freeform_prompt_args() {
        let summary = summarize_cmdline(
            b"/home/hasna/.bun/bin/codewith\0exec\0--auth-profile\0account001\0-C\0/repo\0please do sensitive work\0",
        )
        .expect("summary");

        assert_eq!(
            summary,
            "codewith exec --auth-profile account001 -C /repo ..."
        );
    }

    #[test]
    fn command_summary_redacts_config_and_sensitive_inline_values() {
        let summary = summarize_cmdline(
            b"/bin/codewith\0-c\0api_key=secret\0--api-key=secret\0--auth-profile=account002\0",
        )
        .expect("summary");

        assert_eq!(
            summary,
            "codewith -c <redacted> --api-key=<redacted> --auth-profile=account002"
        );
    }

    #[test]
    fn command_summary_sanitizes_control_characters() {
        let summary = summarize_cmdline(b"/bin/codewith\0--auth-profile\0account001\n\x1b[31m\0")
            .expect("summary");

        assert_eq!(summary, "codewith --auth-profile account001 ?[31m");
    }

    #[test]
    fn lock_holder_summary_includes_state_and_database_labels() {
        let holder = LockHolder {
            pid: 123,
            state: Some("T (stopped)".to_string()),
            databases: BTreeSet::from(["state DB".to_string(), "log DB".to_string()]),
            command: "codewith --auth-profile account001".to_string(),
        };

        assert_eq!(
            holder.summary(),
            "pid 123 [T (stopped)] (log DB, state DB) codewith --auth-profile account001"
        );
    }
}
