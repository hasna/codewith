//! Regression coverage for `codewith usage --auth-profile`.
//!
//! The defect these tests exist for: the command exited 0 when the target's
//! auth was dead, and returned a full, plausible body — a plan tier and a
//! `redactedAccountId` — because both are read from the LOCAL auth file before
//! any request is made. The only signal of failure was
//! `.targets[0].error.reason`, nested inside a body that otherwise looked
//! exactly like a healthy one. The exit code therefore discriminated NAME
//! RESOLUTION only: an unknown profile failed loudly (1) while a dead profile
//! succeeded quietly (0).
//!
//! All three states are asserted here, because fixing only the failing one
//! would leave the command free to fail on everything:
//!
//! | target                | exit |
//! |-----------------------|------|
//! | unknown profile name  | 1    |
//! | dead / unreachable    | 2    |
//! | provider answered     | 0    |
//!
//! Every credential in this file is synthetic. No real auth file is read,
//! copied, or mutated, and no request leaves the loopback interface: the
//! "dead" case points at a closed local port and the "healthy" case at a
//! canned local listener, so no live refresh token can ever be consumed.

use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc;

use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;

/// Synthetic ID token. Header/payload are unsigned base64url; the payload
/// carries `chatgpt_plan_type: "pro"` and a fake account id so the report body
/// is populated from the local file exactly as a real profile's would be.
const SYNTHETIC_ID_TOKEN: &str = concat!(
    "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.",
    "eyJlbWFpbCI6InN5bnRoZXRpY0BleGFtcGxlLmludmFsaWQiLCJleHAiOjQwNzA5MDg4MDAsImh0dHBzOi8vYXBpLm9",
    "wZW5haS5jb20vYXV0aCI6eyJjaGF0Z3B0X3BsYW5fdHlwZSI6InBybyIsImNoYXRncHRfdXNlcl9pZCI6InVzZXItU1",
    "lOVEhFVElDLTAwMDAwMDAwMDAwMCIsImNoYXRncHRfYWNjb3VudF9pZCI6ImFjY3QtU1lOVEhFVElDLTEyMzQ1Njc4O",
    "TAxMiIsImNoYXRncHRfYWNjb3VudF9pc19mZWRyYW1wIjpmYWxzZX19.sig-not-verified"
);

/// Synthetic access token with `exp` in 2099. The far-future expiry is
/// load-bearing: it makes the refresh check return early, so the fixture never
/// attempts a token refresh and stays deterministic regardless of when it runs.
const SYNTHETIC_ACCESS_TOKEN: &str =
    "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJleHAiOjQwNzA5MDg4MDB9.sig-not-verified";

const SYNTHETIC_ACCOUNT_ID: &str = "acct-SYNTHETIC-123456789012";

fn usage_command(codex_home: &Path, base_url: &str) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    cmd.env("CODEWITH_HOME", codex_home);
    cmd.env("CODEX_HOME", codex_home);
    // Keep the run hermetic: no inherited API key, no inherited profile.
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env_remove("CODEX_API_KEY");
    cmd.env_remove("CODEWITH_AUTH_PROFILE");
    cmd.env_remove("CODEX_AUTH_PROFILE");
    cmd.arg("-c")
        .arg(format!("chatgpt_base_url=\"{base_url}\""))
        .arg("usage");
    Ok(cmd)
}

/// Writes a ChatGPT auth profile whose stored credentials are entirely
/// synthetic. This is what "a profile that exists on disk" looks like; whether
/// its auth actually works is decided only by whether `base_url` answers.
fn write_synthetic_profile(codex_home: &Path, name: &str) -> Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )?;
    let profile_dir = codex_home.join("auth_profiles").join(name);
    std::fs::create_dir_all(&profile_dir)?;
    std::fs::write(
        profile_dir.join("profile.json"),
        serde_json::to_string(&serde_json::json!({ "subscriptionProvider": "chat-gpt" }))?,
    )?;
    std::fs::write(
        profile_dir.join("auth.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "id_token": SYNTHETIC_ID_TOKEN,
                "access_token": SYNTHETIC_ACCESS_TOKEN,
                "refresh_token": "synthetic-refresh-token-never-sent-off-loopback",
                "account_id": SYNTHETIC_ACCOUNT_ID,
            },
            // Any value satisfies the "token data is available" check; the
            // access token's far-future `exp` is what prevents a refresh.
            "last_refresh": "2020-01-01T00:00:00Z",
        }))?,
    )?;
    Ok(())
}

/// A base URL that is guaranteed to refuse connections: bind an ephemeral port,
/// learn its number, then release it. This models dead auth without needing the
/// network — the provider request cannot succeed.
fn closed_loopback_base_url() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(format!("http://127.0.0.1:{port}"))
}

/// Minimal canned backend: answers any request with a valid usage payload, so
/// the "provider answered" path can be exercised offline.
struct CannedBackend {
    base_url: String,
    shutdown: Option<mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl CannedBackend {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let base_url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let (shutdown, rx) = mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if rx.try_recv().is_ok() {
                    return;
                }
                let Ok(mut stream) = stream else { return };
                // Read just enough to let the client finish sending; the canned
                // reply is the same for every route this command touches.
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let body = serde_json::json!({ "plan_type": "pro" }).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Ok(Self {
            base_url,
            shutdown: Some(shutdown),
            handle: Some(handle),
        })
    }
}

impl Drop for CannedBackend {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // Unblock the accept loop so the thread can observe the shutdown.
        let _ = std::net::TcpStream::connect(self.base_url.trim_start_matches("http://"));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// THE REGRESSION. Before the fix this exited 0.
#[test]
fn dead_auth_profile_exits_non_zero_and_says_so_in_json() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_synthetic_profile(codex_home.path(), "deadprofile")?;
    let base_url = closed_loopback_base_url()?;

    let output = usage_command(codex_home.path(), &base_url)?
        .arg("--auth-profile")
        .arg("deadprofile")
        .arg("--json")
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(2),
        "a profile whose provider never answered must not exit 0; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["ok"], Value::Bool(false));
    assert_eq!(report["targets"][0]["ok"], Value::Bool(false));
    assert_eq!(report["targets"][0]["error"]["reason"], "fetch_failed");

    // The body is STILL fully populated, from the local auth file. This is not
    // incidental: it is the specific thing that made a dead profile convincing,
    // so it is pinned rather than left to be rediscovered.
    assert_eq!(report["targets"][0]["plan"], "Pro");
    assert!(
        report["targets"][0]["redactedAccountId"].is_string(),
        "a dead profile still reports an account id read from the local file"
    );

    // ...and the failure is announced on stderr even in --json mode, where
    // stdout must stay parseable.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("NOT VERIFIED"), "stderr was: {stderr}");
    assert!(stderr.contains("deadprofile"), "stderr was: {stderr}");
    assert!(stderr.contains("LOCAL auth file"), "stderr was: {stderr}");
    Ok(())
}

/// The other half of acceptance: the failure must also be unmissable to a
/// person reading the DEFAULT output, not only to a script reading `$?`.
#[test]
fn dead_auth_profile_is_loud_in_default_output() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_synthetic_profile(codex_home.path(), "deadprofile")?;
    let base_url = closed_loopback_base_url()?;

    let output = usage_command(codex_home.path(), &base_url)?
        .arg("--auth-profile")
        .arg("deadprofile")
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("STATUS: NOT VERIFIED"),
        "stdout was: {stdout}"
    );
    assert!(stdout.contains("LOCAL auth file"), "stdout was: {stdout}");
    // The plan line is the one a reader mistakes for proof of health, so it
    // must carry its provenance inline.
    assert!(
        stdout.contains("Plan: Pro  [local file]"),
        "stdout was: {stdout}"
    );
    assert!(!stdout.contains("STATUS: VERIFIED"), "stdout was: {stdout}");
    Ok(())
}

/// Do not break the working case: a profile the provider answers for still
/// exits 0 and still reports its usage.
#[test]
fn verified_auth_profile_still_exits_zero() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_synthetic_profile(codex_home.path(), "liveprofile")?;
    let backend = CannedBackend::start()?;

    let output = usage_command(codex_home.path(), &backend.base_url)?
        .arg("--auth-profile")
        .arg("liveprofile")
        .arg("--json")
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(0),
        "a target the provider answered for must still exit 0; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["ok"], Value::Bool(true));
    assert_eq!(report["targets"][0]["ok"], Value::Bool(true));
    assert!(report["targets"][0]["error"].is_null());
    Ok(())
}

/// Do not break the working case: an unknown profile name is still a command
/// failure, and must stay distinguishable from an unhealthy one.
#[test]
fn unknown_auth_profile_still_exits_one() -> Result<()> {
    let codex_home = TempDir::new()?;
    write_synthetic_profile(codex_home.path(), "deadprofile")?;
    let base_url = closed_loopback_base_url()?;

    let output = usage_command(codex_home.path(), &base_url)?
        .arg("--auth-profile")
        .arg("nosuchprofile")
        .arg("--json")
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(1),
        "an unknown profile name is a command failure, not an unhealthy target"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown auth profile"),
        "stderr was: {stderr}"
    );
    Ok(())
}
