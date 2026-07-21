use crate::GOALS_DB_FILENAME;
use crate::LOGS_DB_FILENAME;
use crate::STATE_DB_FILENAME;
use serde::Serialize;
use serde_json::Value;
use sqlx::ConnectOptions;
use sqlx::Row;
use sqlx::SqliteConnection;
use sqlx::sqlite::SqliteConnectOptions;
use std::fs;
use std::fs::OpenOptions;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const SESSION_INDEX_FILE: &str = "session_index.jsonl";
const SESSIONS_DIR: &str = "sessions";
const SHELL_SNAPSHOTS_DIR: &str = "shell_snapshots";
const REDACTED_SECRET: &str = "[REDACTED_SECRET]";

#[derive(Debug, Clone)]
pub struct LocalStateSecretsDoctorOptions {
    pub codex_home: PathBuf,
    pub sqlite_home: PathBuf,
    pub repair: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStateSecretsDoctorReport {
    pub repair: bool,
    pub scanned_files: usize,
    pub scanned_sqlite_rows: usize,
    pub redacted_files: usize,
    pub redacted_sqlite_cells: usize,
    pub permission_fixes: usize,
    pub findings: Vec<LocalStateSecretFinding>,
    pub warnings: Vec<String>,
}

impl LocalStateSecretsDoctorReport {
    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStateSecretFinding {
    pub location: LocalStateSecretLocation,
    pub action: LocalStateSecretAction,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LocalStateSecretLocation {
    File {
        path: String,
    },
    SqliteCell {
        db: String,
        table: String,
        rowid: i64,
        column: String,
    },
    Permission {
        path: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStateSecretAction {
    WouldRedact,
    Redacted,
    WouldRestrictMode,
    RestrictedMode,
}

pub fn redact_local_state_string(input: impl AsRef<str>) -> String {
    codex_secrets::redact_secrets(input.as_ref().to_string())
}

pub fn local_state_string_contains_secret(input: impl AsRef<str>) -> bool {
    redact_local_state_string(input.as_ref()) != input.as_ref()
}

pub fn redact_local_state_json_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = redact_local_state_string(text.as_str());
        }
        Value::Array(items) => {
            for item in items {
                redact_local_state_json_value(item);
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                redact_local_state_json_value(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

pub fn redacted_local_state_json(value: &Value) -> Value {
    let mut redacted = value.clone();
    redact_local_state_json_value(&mut redacted);
    redacted
}

pub fn redacted_local_state_json_string(value: &Value) -> anyhow::Result<String> {
    Ok(serde_json::to_string(&redacted_local_state_json(value))?)
}

pub fn redacted_local_state_serialized_json_string<T: Serialize + ?Sized>(
    value: &T,
) -> anyhow::Result<String> {
    let mut json = serde_json::to_value(value)?;
    redact_local_state_json_value(&mut json);
    Ok(serde_json::to_string(&json)?)
}

pub async fn run_local_state_secrets_doctor(
    options: LocalStateSecretsDoctorOptions,
) -> anyhow::Result<LocalStateSecretsDoctorReport> {
    let mut report = LocalStateSecretsDoctorReport {
        repair: options.repair,
        ..Default::default()
    };

    scan_owner_only_modes(&options, &mut report)?;
    scan_local_state_files(&options, &mut report)?;
    scan_sqlite_local_state(&options, &mut report).await?;

    Ok(report)
}

fn scan_owner_only_modes(
    options: &LocalStateSecretsDoctorOptions,
    report: &mut LocalStateSecretsDoctorReport,
) -> anyhow::Result<()> {
    let mut paths = Vec::new();
    paths.push(options.codex_home.clone());
    if options.sqlite_home != options.codex_home {
        paths.push(options.sqlite_home.clone());
    }
    paths.push(options.codex_home.join(SESSION_INDEX_FILE));
    paths.push(options.codex_home.join(SESSIONS_DIR));
    paths.push(options.codex_home.join(SHELL_SNAPSHOTS_DIR));
    for filename in [STATE_DB_FILENAME, LOGS_DB_FILENAME, GOALS_DB_FILENAME] {
        let db_path = options.sqlite_home.join(filename);
        paths.push(db_path.clone());
        paths.push(path_with_suffix(&db_path, "-wal"));
        paths.push(path_with_suffix(&db_path, "-shm"));
    }
    paths.sort();
    paths.dedup();

    for path in paths {
        if !path.exists() {
            continue;
        }
        restrict_owner_only_if_needed(path.as_path(), options, report)?;
    }
    Ok(())
}

fn restrict_owner_only_if_needed(
    path: &Path,
    options: &LocalStateSecretsDoctorOptions,
    report: &mut LocalStateSecretsDoctorReport,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Ok(());
        }
        let mode = metadata.permissions().mode();
        if mode & 0o077 == 0 {
            return Ok(());
        }
        let target_mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        if options.repair {
            fs::set_permissions(path, fs::Permissions::from_mode(target_mode))?;
            report.permission_fixes += 1;
        }
        report.findings.push(LocalStateSecretFinding {
            location: LocalStateSecretLocation::Permission {
                path: display_local_path(path, &options.codex_home, &options.sqlite_home),
            },
            action: if options.repair {
                LocalStateSecretAction::RestrictedMode
            } else {
                LocalStateSecretAction::WouldRestrictMode
            },
        });
    }
    #[cfg(not(unix))]
    {
        let _ = (path, options, report);
    }
    Ok(())
}

fn scan_local_state_files(
    options: &LocalStateSecretsDoctorOptions,
    report: &mut LocalStateSecretsDoctorReport,
) -> anyhow::Result<()> {
    let mut files = Vec::new();
    collect_if_file(options.codex_home.join(SESSION_INDEX_FILE), &mut files);
    collect_files_recursive(options.codex_home.join(SESSIONS_DIR).as_path(), &mut files)?;
    collect_files_recursive(
        options.codex_home.join(SHELL_SNAPSHOTS_DIR).as_path(),
        &mut files,
    )?;
    files.sort();
    files.dedup();

    for path in files {
        match read_local_state_file(path.as_path()) {
            Ok(Some((contents, encoding))) => {
                report.scanned_files += 1;
                let redacted = redact_local_state_string(contents.as_str());
                if redacted == contents {
                    continue;
                }
                if options.repair {
                    write_local_state_file(path.as_path(), redacted.as_str(), encoding)?;
                    report.redacted_files += 1;
                }
                report.findings.push(LocalStateSecretFinding {
                    location: LocalStateSecretLocation::File {
                        path: display_local_path(
                            path.as_path(),
                            &options.codex_home,
                            &options.sqlite_home,
                        ),
                    },
                    action: if options.repair {
                        LocalStateSecretAction::Redacted
                    } else {
                        LocalStateSecretAction::WouldRedact
                    },
                });
            }
            Ok(None) => {}
            Err(err) => report.warnings.push(format!(
                "skipped file {}: {}",
                display_local_path(path.as_path(), &options.codex_home, &options.sqlite_home),
                err
            )),
        }
    }
    Ok(())
}

async fn scan_sqlite_local_state(
    options: &LocalStateSecretsDoctorOptions,
    report: &mut LocalStateSecretsDoctorReport,
) -> anyhow::Result<()> {
    for filename in [STATE_DB_FILENAME, LOGS_DB_FILENAME, GOALS_DB_FILENAME] {
        let path = options.sqlite_home.join(filename);
        if !path.exists() {
            continue;
        }
        if let Err(err) = scan_sqlite_database(filename, path.as_path(), options, report).await {
            report
                .warnings
                .push(format!("skipped sqlite db {filename}: {err}"));
        }
    }
    Ok(())
}

async fn scan_sqlite_database(
    db_name: &str,
    path: &Path,
    options: &LocalStateSecretsDoctorOptions,
    report: &mut LocalStateSecretsDoctorReport,
) -> anyhow::Result<()> {
    let mut connection = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(!options.repair)
        .busy_timeout(Duration::from_secs(30))
        .connect()
        .await?;
    let table_rows = sqlx::query(
        r#"
SELECT name
FROM sqlite_master
WHERE type = 'table'
  AND name NOT LIKE 'sqlite_%'
  AND name != '_sqlx_migrations'
ORDER BY name
        "#,
    )
    .fetch_all(&mut connection)
    .await?;

    for row in table_rows {
        let table: String = row.try_get("name")?;
        scan_sqlite_table(db_name, table.as_str(), options, report, &mut connection).await?;
    }
    Ok(())
}

async fn scan_sqlite_table(
    db_name: &str,
    table: &str,
    options: &LocalStateSecretsDoctorOptions,
    report: &mut LocalStateSecretsDoctorReport,
    connection: &mut SqliteConnection,
) -> anyhow::Result<()> {
    let pragma = format!("PRAGMA table_info({})", quote_identifier(table));
    let column_rows = sqlx::query(sqlx::AssertSqlSafe(pragma))
        .fetch_all(&mut *connection)
        .await?;
    let columns = column_rows
        .into_iter()
        .filter_map(|row| {
            let name: String = row.try_get("name").ok()?;
            let ty: String = row.try_get("type").unwrap_or_default();
            is_text_column_type(ty.as_str()).then_some(name)
        })
        .collect::<Vec<_>>();
    if columns.is_empty() {
        return Ok(());
    }

    let select = format!(
        "SELECT rowid AS __codewith_rowid, {} FROM {}",
        columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", "),
        quote_identifier(table)
    );
    let rows = match sqlx::query(sqlx::AssertSqlSafe(select))
        .fetch_all(&mut *connection)
        .await
    {
        Ok(rows) => rows,
        Err(err) => {
            report
                .warnings
                .push(format!("skipped sqlite table {db_name}.{table}: {err}"));
            return Ok(());
        }
    };

    for row in rows {
        report.scanned_sqlite_rows += 1;
        let rowid: i64 = row.try_get("__codewith_rowid")?;
        for column in &columns {
            let value: Option<String> = row.try_get(column.as_str())?;
            let Some(value) = value else {
                continue;
            };
            let redacted = redact_local_state_string(value.as_str());
            if redacted == value {
                continue;
            }
            if options.repair {
                let update = format!(
                    "UPDATE {} SET {} = ? WHERE rowid = ?",
                    quote_identifier(table),
                    quote_identifier(column)
                );
                sqlx::query(sqlx::AssertSqlSafe(update))
                    .bind(redacted.as_str())
                    .bind(rowid)
                    .execute(&mut *connection)
                    .await?;
                report.redacted_sqlite_cells += 1;
            }
            report.findings.push(LocalStateSecretFinding {
                location: LocalStateSecretLocation::SqliteCell {
                    db: db_name.to_string(),
                    table: table.to_string(),
                    rowid,
                    column: column.to_string(),
                },
                action: if options.repair {
                    LocalStateSecretAction::Redacted
                } else {
                    LocalStateSecretAction::WouldRedact
                },
            });
        }
    }
    Ok(())
}

fn is_text_column_type(ty: &str) -> bool {
    let ty = ty.trim().to_ascii_uppercase();
    ty.is_empty()
        || ty.contains("TEXT")
        || ty.contains("CHAR")
        || ty.contains("CLOB")
        || ty.contains("JSON")
        || ty.contains("VARCHAR")
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

#[derive(Clone, Copy)]
enum LocalStateFileEncoding {
    Plain,
    Zstd,
}

fn read_local_state_file(path: &Path) -> anyhow::Result<Option<(String, LocalStateFileEncoding)>> {
    if !path.is_file() {
        return Ok(None);
    }
    if path.extension().is_some_and(|extension| extension == "zst") {
        let bytes = fs::read(path)?;
        let decoded = zstd::stream::decode_all(Cursor::new(bytes))?;
        let contents = String::from_utf8(decoded)?;
        return Ok(Some((contents, LocalStateFileEncoding::Zstd)));
    }
    let contents = fs::read_to_string(path)?;
    Ok(Some((contents, LocalStateFileEncoding::Plain)))
}

fn write_local_state_file(
    path: &Path,
    contents: &str,
    encoding: LocalStateFileEncoding,
) -> anyhow::Result<()> {
    let bytes = match encoding {
        LocalStateFileEncoding::Plain => contents.as_bytes().to_vec(),
        LocalStateFileEncoding::Zstd => zstd::stream::encode_all(Cursor::new(contents), 0)?,
    };
    write_owner_only_file_atomically(path, bytes.as_slice())
}

fn write_owner_only_file_atomically(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("local state file has no parent: {}", path.display()))?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("local-state");
    let temp_path = parent.join(format!(".{filename}.redacted-{}", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(temp_path.as_path())?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp_path.as_path(), path)?;
    set_owner_only_file(path)?;
    Ok(())
}

pub fn set_owner_only_file(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn set_owner_only_dir(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn collect_if_file(path: PathBuf, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        files.push(path);
    }
}

fn collect_files_recursive(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        collect_files_recursive(entry.path().as_path(), files)?;
    }
    Ok(())
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn display_local_path(path: &Path, codex_home: &Path, sqlite_home: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(codex_home) {
        return format!("$CODEWITH_HOME/{}", relative.display());
    }
    if let Ok(relative) = path.strip_prefix(sqlite_home) {
        return format!("$CODEWITH_SQLITE_HOME/{}", relative.display());
    }
    path.display().to_string()
}

pub fn local_state_redaction_marker() -> &'static str {
    REDACTED_SECRET
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("codewith-local-state-{label}-{}", Uuid::new_v4()))
    }

    fn synthetic_openai_key() -> String {
        format!("{}{}", "sk-proj-", "a".repeat(32))
    }

    fn synthetic_github_key() -> String {
        format!("{}{}", "ghp_", "b".repeat(36))
    }

    #[test]
    fn redacts_strings_and_nested_json_values() {
        let sample_value = synthetic_openai_key();
        let mut value = serde_json::json!({
            "outer": {
                "token": sample_value,
                "safe": "thread-00000000-0000-0000-0000-000000000001"
            }
        });

        redact_local_state_json_value(&mut value);

        let rendered = value.to_string();
        assert!(rendered.contains(local_state_redaction_marker()));
        assert!(!rendered.contains("sk-proj-"));
        assert!(rendered.contains("thread-00000000-0000-0000-0000-000000000001"));
    }

    #[tokio::test]
    async fn doctor_reports_and_repairs_files_and_sqlite_cells_without_values() {
        let codex_home = temp_path("doctor");
        let sqlite_home = codex_home.clone();
        fs::create_dir_all(codex_home.join(SESSIONS_DIR)).expect("create sessions dir");
        fs::create_dir_all(codex_home.join(SHELL_SNAPSHOTS_DIR)).expect("create snapshots dir");
        let session_path = codex_home.join(SESSIONS_DIR).join("session.jsonl");
        let file_value = synthetic_openai_key();
        fs::write(
            session_path.as_path(),
            format!(r#"{{"message":"{file_value}"}}"#),
        )
        .expect("write session");

        let db_path = sqlite_home.join(STATE_DB_FILENAME);
        let mut connection = SqliteConnectOptions::new()
            .filename(db_path.as_path())
            .create_if_missing(true)
            .connect()
            .await
            .expect("open sqlite");
        sqlx::query("CREATE TABLE sample (body TEXT)")
            .execute(&mut connection)
            .await
            .expect("create table");
        let db_value = synthetic_github_key();
        sqlx::query("INSERT INTO sample (body) VALUES (?)")
            .bind(db_value.as_str())
            .execute(&mut connection)
            .await
            .expect("insert sample");
        connection.close().await.expect("close sqlite");

        let scan = run_local_state_secrets_doctor(LocalStateSecretsDoctorOptions {
            codex_home: codex_home.clone(),
            sqlite_home: sqlite_home.clone(),
            repair: false,
        })
        .await
        .expect("scan local state");
        assert!(scan.has_findings());
        assert_eq!(scan.redacted_files, 0);
        assert_eq!(scan.redacted_sqlite_cells, 0);
        let serialized = serde_json::to_string(&scan).expect("serialize scan");
        assert!(!serialized.contains(file_value.as_str()));
        assert!(!serialized.contains(db_value.as_str()));

        let repaired = run_local_state_secrets_doctor(LocalStateSecretsDoctorOptions {
            codex_home: codex_home.clone(),
            sqlite_home: sqlite_home.clone(),
            repair: true,
        })
        .await
        .expect("repair local state");
        assert_eq!(repaired.redacted_files, 1);
        assert_eq!(repaired.redacted_sqlite_cells, 1);

        let repaired_file = fs::read_to_string(session_path).expect("read repaired session");
        assert!(repaired_file.contains(local_state_redaction_marker()));
        assert!(!repaired_file.contains(file_value.as_str()));

        let mut connection = SqliteConnectOptions::new()
            .filename(db_path.as_path())
            .create_if_missing(false)
            .connect()
            .await
            .expect("reopen sqlite");
        let repaired_body: String = sqlx::query_scalar("SELECT body FROM sample")
            .fetch_one(&mut connection)
            .await
            .expect("select repaired body");
        assert_eq!(repaired_body, local_state_redaction_marker());
        connection.close().await.expect("close sqlite");
    }
}
