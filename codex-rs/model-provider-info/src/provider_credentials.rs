use std::collections::HashMap;
use std::ffi::OsStr;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use std::time::Instant;

const SECRETS_CLI: &str = "secrets";
const SECRETS_CLI_TIMEOUT: Duration = Duration::from_secs(2);
const SECRETS_CLI_MISSING_CACHE_TTL: Duration = Duration::from_secs(30);
const SECRETS_CLI_POLL_INTERVAL: Duration = Duration::from_millis(20);

static SECRETS_CLI_CACHE: OnceLock<Mutex<HashMap<String, CachedSecret>>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
enum CachedSecret {
    Found(String),
    Missing { cached_at: Instant },
}

pub(crate) fn provider_key_from_env_or_secret(env_key: &str) -> Option<String> {
    provider_key_from_env_or_secret_with(
        env_key,
        |env_key| std::env::var(env_key).ok(),
        cached_secret_from_secrets_cli,
    )
}

fn provider_key_from_env_or_secret_with(
    env_key: &str,
    env_var: impl FnOnce(&str) -> Option<String>,
    get_secret: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    non_empty_value(env_var(env_key))
        .or_else(|| provider_key_from_optional_secret_backend_with(env_key, get_secret))
}

fn provider_key_from_optional_secret_backend_with(
    env_key: &str,
    get_secret: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    let secret_name = default_secret_name_for_provider_env_key(env_key)?;
    non_empty_value(get_secret(&secret_name))
}

fn default_secret_name_for_provider_env_key(env_key: &str) -> Option<String> {
    const PROVIDER_SECRET_SUFFIXES: &[(&str, &str)] = &[
        ("_API_KEY", "api_key"),
        ("_ACCESS_TOKEN", "access_token"),
        ("_AUTH_TOKEN", "auth_token"),
        ("_BEARER_TOKEN", "bearer_token"),
        ("_TOKEN", "token"),
    ];

    PROVIDER_SECRET_SUFFIXES
        .iter()
        .find_map(|(env_suffix, secret_leaf)| {
            let provider = env_key.strip_suffix(env_suffix)?;
            let provider = provider.trim_matches('_').to_ascii_lowercase();
            if provider.is_empty() {
                return None;
            }
            Some(format!("{}/{secret_leaf}", provider.replace('_', "-")))
        })
}

fn cached_secret_from_secrets_cli(secret_name: &str) -> Option<String> {
    cached_secret_from_backend(secret_name, read_secret_from_secrets_cli)
}

fn cached_secret_from_backend(
    secret_name: &str,
    read_secret: impl FnOnce(&str) -> Option<String>,
) -> Option<String> {
    cached_secret_from_backend_with_clock(
        secret_name,
        read_secret,
        Instant::now,
        SECRETS_CLI_MISSING_CACHE_TTL,
    )
}

fn cached_secret_from_backend_with_clock(
    secret_name: &str,
    read_secret: impl FnOnce(&str) -> Option<String>,
    now: impl FnOnce() -> Instant,
    missing_ttl: Duration,
) -> Option<String> {
    let now = now();

    if let Some(cached) = SECRETS_CLI_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(secret_name).cloned())
    {
        match cached {
            CachedSecret::Found(secret) => return Some(secret),
            CachedSecret::Missing { cached_at }
                if now.saturating_duration_since(cached_at) < missing_ttl =>
            {
                return None;
            }
            CachedSecret::Missing { .. } => {}
        }
    }

    let secret = read_secret(secret_name).and_then(|secret| non_empty_value(Some(secret)));

    if let Ok(mut cache) = SECRETS_CLI_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
    {
        cache.insert(
            secret_name.to_string(),
            match secret.clone() {
                Some(secret) => CachedSecret::Found(secret),
                None => CachedSecret::Missing { cached_at: now },
            },
        );
    }

    secret
}

fn read_secret_from_secrets_cli(secret_name: &str) -> Option<String> {
    read_secret_from_command_with_timeout(SECRETS_CLI, secret_name, SECRETS_CLI_TIMEOUT)
}

fn read_secret_from_command_with_timeout(
    command: impl AsRef<OsStr>,
    secret_name: &str,
    timeout: Duration,
) -> Option<String> {
    let output = secret_command_output(command, secret_name, timeout)?;
    if !output.status.success() {
        return None;
    }

    secret_from_stdout(output.stdout)
}

fn secret_from_stdout(stdout: Vec<u8>) -> Option<String> {
    String::from_utf8(stdout)
        .ok()
        .and_then(|value| non_empty_value(Some(value)))
}

fn secret_command_output(
    command: impl AsRef<OsStr>,
    secret_name: &str,
    timeout: Duration,
) -> Option<Output> {
    let mut child = Command::new(command)
        .arg("get")
        .arg(secret_name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                thread::sleep(remaining.min(SECRETS_CLI_POLL_INTERVAL));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn non_empty_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::path::Path;
    #[cfg(unix)]
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use pretty_assertions::assert_eq;

    use super::*;

    static NEXT_SECRET_ID: AtomicUsize = AtomicUsize::new(0);

    fn unique_secret_name(prefix: &str) -> String {
        let id = NEXT_SECRET_ID.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}/{id}")
    }

    #[cfg(unix)]
    fn temp_test_dir(prefix: &str) -> PathBuf {
        let id = NEXT_SECRET_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codewith-provider-credentials-{prefix}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test temp dir should be created");
        path
    }

    #[test]
    fn provider_key_from_env_or_secret_prefers_env_value() {
        let env_var_called = Cell::new(false);

        let provider_key = provider_key_from_env_or_secret_with(
            "CEREBRAS_API_KEY",
            |_| {
                env_var_called.set(true);
                Some(" env-token ".to_string())
            },
            |_| Some("secret-token".to_string()),
        );

        assert_eq!(provider_key, Some("env-token".to_string()));
        assert!(env_var_called.get());
    }

    #[test]
    fn provider_key_from_env_or_secret_falls_back_to_secret_backend_value() {
        let provider_key = provider_key_from_env_or_secret_with(
            "CEREBRAS_API_KEY",
            |_| None,
            |_| Some("secret-token".to_string()),
        );

        assert_eq!(provider_key, Some("secret-token".to_string()));
    }

    #[test]
    fn provider_key_from_env_or_secret_falls_back_to_env_value() {
        let provider_key = provider_key_from_env_or_secret_with(
            "CEREBRAS_API_KEY",
            |_| Some(" env-token ".to_string()),
            |_| None,
        );

        assert_eq!(provider_key, Some("env-token".to_string()));
    }

    #[test]
    fn provider_key_from_env_or_secret_falls_back_to_env_when_secret_is_empty() {
        let provider_key = provider_key_from_env_or_secret_with(
            "CEREBRAS_API_KEY",
            |_| Some(" env-token ".to_string()),
            |_| Some(" \n".to_string()),
        );

        assert_eq!(provider_key, Some("env-token".to_string()));
    }

    #[test]
    fn provider_key_from_env_or_secret_uses_default_secret_backend() {
        let provider_key = provider_key_from_env_or_secret_with(
            "CEREBRAS_API_KEY",
            |_| None,
            |secret_name| {
                assert_eq!(secret_name, "cerebras/api_key");
                Some(" secret-token\n".to_string())
            },
        );

        assert_eq!(provider_key, Some("secret-token".to_string()));
    }

    #[test]
    fn provider_key_from_env_or_secret_allows_missing_secret_backend_value() {
        let provider_key =
            provider_key_from_env_or_secret_with("CEREBRAS_API_KEY", |_| None, |_| None);

        assert_eq!(provider_key, None);
    }

    #[test]
    fn provider_key_from_env_or_secret_derives_secret_backend_mapping() {
        let provider_key = provider_key_from_env_or_secret_with(
            "CUSTOM_PROVIDER_API_KEY",
            |_| None,
            |secret_name| {
                assert_eq!(secret_name, "custom-provider/api_key");
                Some("custom-provider-token".to_string())
            },
        );

        assert_eq!(provider_key, Some("custom-provider-token".to_string()));
    }

    #[test]
    fn provider_key_from_env_or_secret_skips_unknown_credential_suffix() {
        let provider_key = provider_key_from_env_or_secret_with(
            "CUSTOM_PROVIDER_SECRET",
            |_| None,
            |_| panic!("unknown credential suffixes should not query default secret backends"),
        );

        assert_eq!(provider_key, None);
    }

    #[test]
    fn provider_key_from_env_or_secret_ignores_empty_secret_backend_value() {
        let provider_key = provider_key_from_env_or_secret_with(
            "OPENROUTER_API_KEY",
            |_| None,
            |_| Some(" \n".to_string()),
        );

        assert_eq!(provider_key, None);
    }

    #[test]
    fn cached_secret_from_backend_caches_found_values() {
        let secret_name = unique_secret_name("found");
        let calls = Cell::new(0);

        let first = cached_secret_from_backend(&secret_name, |_| {
            calls.set(calls.get() + 1);
            Some(" token\n".to_string())
        });
        let second = cached_secret_from_backend(&secret_name, |_| {
            calls.set(calls.get() + 1);
            Some("other-token".to_string())
        });

        assert_eq!(first, Some("token".to_string()));
        assert_eq!(second, Some("token".to_string()));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn cached_secret_from_backend_caches_missing_values_until_ttl_expires() {
        let secret_name = unique_secret_name("missing");
        let calls = Cell::new(0);
        let start = Instant::now();
        let ttl = Duration::from_secs(30);

        let first = cached_secret_from_backend_with_clock(
            &secret_name,
            |_| {
                calls.set(calls.get() + 1);
                None
            },
            || start,
            ttl,
        );
        let second = cached_secret_from_backend_with_clock(
            &secret_name,
            |_| {
                calls.set(calls.get() + 1);
                Some("token".to_string())
            },
            || start,
            ttl,
        );
        let third = cached_secret_from_backend_with_clock(
            &secret_name,
            |_| {
                calls.set(calls.get() + 1);
                Some("token".to_string())
            },
            || start + ttl + Duration::from_millis(1),
            ttl,
        );

        assert_eq!(first, None);
        assert_eq!(second, None);
        assert_eq!(third, Some("token".to_string()));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn secret_from_stdout_trims_utf8_output() {
        assert_eq!(
            secret_from_stdout(b" secret-token\n".to_vec()),
            Some("secret-token".to_string())
        );
    }

    #[test]
    fn secret_from_stdout_ignores_non_utf8_output() {
        assert_eq!(secret_from_stdout(vec![0xff]), None);
    }

    #[cfg(unix)]
    fn write_fake_secret_command(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        fs::write(&path, body).expect("fake secret command should be written");
        let mut permissions = fs::metadata(&path)
            .expect("fake secret command metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("fake secret command should be executable");
        path
    }

    #[cfg(unix)]
    #[test]
    fn read_secret_from_command_with_timeout_reads_successful_stdout() {
        let temp_dir = temp_test_dir("success");
        let command = write_fake_secret_command(
            &temp_dir,
            "fake-secrets",
            r#"#!/bin/sh
if [ "$1" = "get" ] && [ "$2" = "cerebras/api_key" ]; then
  printf ' secret-token\n'
fi
"#,
        );

        assert_eq!(
            read_secret_from_command_with_timeout(
                command,
                "cerebras/api_key",
                Duration::from_secs(1),
            ),
            Some("secret-token".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_secret_from_command_with_timeout_bounds_slow_commands() {
        let temp_dir = temp_test_dir("slow");
        let command = write_fake_secret_command(
            &temp_dir,
            "slow-secrets",
            r#"#!/bin/sh
sleep 2
printf 'slow-token\n'
"#,
        );
        let started_at = Instant::now();

        let secret = read_secret_from_command_with_timeout(
            command,
            "cerebras/api_key",
            Duration::from_millis(50),
        );

        assert_eq!(secret, None);
        assert!(started_at.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn default_secret_names_cover_built_in_provider_env_keys() {
        assert_eq!(
            default_secret_name_for_provider_env_key("CEREBRAS_API_KEY"),
            Some("cerebras/api_key".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("NVIDIA_API_KEY"),
            Some("nvidia/api_key".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("OPENROUTER_API_KEY"),
            Some("openrouter/api_key".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("MIMO_API_KEY"),
            Some("mimo/api_key".to_string())
        );
    }

    #[test]
    fn default_secret_names_scale_to_new_provider_env_keys() {
        assert_eq!(
            default_secret_name_for_provider_env_key("ANTHROPIC_API_KEY"),
            Some("anthropic/api_key".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("XAI_API_KEY"),
            Some("xai/api_key".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("GOOGLE_VERTEX_ACCESS_TOKEN"),
            Some("google-vertex/access_token".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("CUSTOM_PROVIDER_BEARER_TOKEN"),
            Some("custom-provider/bearer_token".to_string())
        );
        assert_eq!(
            default_secret_name_for_provider_env_key("ANTHROPIC_TOKEN"),
            Some("anthropic/token".to_string())
        );
    }
}
