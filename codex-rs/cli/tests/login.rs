use std::path::Path;

use anyhow::Result;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    cmd.env("CODEX_HOME", codex_home);
    Ok(cmd)
}

fn write_file_auth_config(codex_home: &Path) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )?;
    Ok(())
}

fn read_auth_json(codex_home: &Path) -> Result<Value> {
    let auth_json = std::fs::read_to_string(codex_home.join("auth.json"))?;
    Ok(serde_json::from_str(&auth_json)?)
}

#[test]
fn login_with_api_key_reads_stdin_and_writes_auth_json() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_file_auth_config(codex_home.path())?;

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args([
        "-c",
        "forced_login_method=\"api\"",
        "login",
        "--with-api-key",
    ])
    .write_stdin("sk-test\n")
    .assert()
    .success()
    .stderr(contains("Successfully logged in"));

    let auth = read_auth_json(codex_home.path())?;
    assert_eq!(auth["OPENAI_API_KEY"], "sk-test");
    assert!(auth.get("tokens").is_none());
    assert!(auth.get("agent_identity").is_none());

    Ok(())
}

#[test]
fn login_with_api_key_profile_saves_and_switches_auth_profiles() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_file_auth_config(codex_home.path())?;

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["login", "--with-api-key", "--profile", "work"])
        .write_stdin("sk-work\n")
        .assert()
        .success()
        .stderr(contains("Saved auth profile `work`"));

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["login", "--with-api-key", "--profile", "personal"])
        .write_stdin("sk-personal\n")
        .assert()
        .success()
        .stderr(contains("Saved auth profile `personal`"));

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["profile", "list"])
        .assert()
        .success()
        .stdout(contains("* personal"))
        .stdout(contains("  work"));

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["profile", "switch", "work"])
        .assert()
        .success()
        .stderr(contains("Switched to auth profile `work`"));

    let auth = read_auth_json(codex_home.path())?;
    assert_eq!(auth["OPENAI_API_KEY"], "sk-work");

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["profile", "remove", "personal"])
        .assert()
        .success()
        .stderr(contains("Removed auth profile `personal`"));

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["profile", "list"])
        .assert()
        .success()
        .stdout(contains("* work"));

    Ok(())
}

#[test]
fn login_with_access_token_rejects_invalid_jwt() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_file_auth_config(codex_home.path())?;

    let mut cmd = codex_command(codex_home.path())?;
    cmd.args(["login", "--with-access-token"])
        .write_stdin("not-a-jwt\n")
        .assert()
        .failure()
        .stderr(contains("Error logging in with access token"));

    Ok(())
}
