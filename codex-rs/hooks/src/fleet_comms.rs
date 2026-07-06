//! Canonical Hasna fleet-comms `SessionStart` hook definition.
//!
//! Codewith sessions participate in the Hasna fleet communication protocol
//! (HACP) by reading unread conversation blockers and the bounded
//! announcements digest at every session boundary (startup, resume, clear,
//! compact) and surfacing the results as model context. This module pins the
//! one canonical hook definition fleet tooling deploys into the user config
//! layer, together with its trust identity hash, so that:
//!
//! 1. Deployment is headless and deterministic: config management writes both
//!    the `[[hooks.SessionStart]]` entry and the matching
//!    `[hooks.state."<key>"] trusted_hash` in one pass, without requiring the
//!    interactive startup hooks-review flow on each machine.
//! 2. Engine drift is loud, not silent: user-layer hooks whose trust hash no
//!    longer matches are silently skipped by discovery, so any change to the
//!    normalized hook identity computation would otherwise disable the fleet
//!    hook fleet-wide without a signal. The pinned-hash test in this module
//!    turns that failure mode into a compile-time-adjacent test failure that
//!    forces a coordinated hash bump.
//!
//! The hook command itself is intentionally:
//! - **Read-only**: it never marks messages read; read/ack duty stays with the
//!   agent per HACP rule 2.
//! - **Fail-open**: a machine without the `conversations` CLI, or a downed
//!   conversations backend, produces empty output and exit code 0 instead of
//!   breaking session startup.
//! - **Bounded**: `--limit`/`--max-bytes`/`head -c` caps keep a large backlog
//!   from flooding the model context.
//! - **Machine-independent**: the command text contains no absolute paths or
//!   machine-specific values, so one trust hash is valid for the entire fleet.

use std::path::Path;

use codex_config::HookHandlerConfig;
use codex_config::MatcherGroup;
use codex_protocol::protocol::HookEventName;

/// Canonical POSIX-sh command for the fleet-comms `SessionStart` hook.
///
/// Uses only single quotes so the command embeds into a TOML basic string
/// (and JSON, if ever needed) without any escaping, keeping the on-disk bytes
/// identical everywhere the hook is deployed.
///
/// The first emitted line must never start with `{` or `[`: session-start
/// hook stdout that looks like JSON but does not parse as hook-output JSON is
/// treated as a failed hook (`output_parser::looks_like_json`), so the header
/// leads with plain text before the embedded blockers JSON.
pub const FLEET_COMMS_SESSION_START_COMMAND: &str = "command -v conversations >/dev/null 2>&1 || exit 0; { echo 'fleet-comms session-start check (read-only) - unread blockers JSON:'; conversations blockers -j --limit 20 2>/dev/null || true; echo 'fleet-comms announcements digest (unread, 7d):'; conversations digest announcements --unread --since 7d --limit 20 --max-bytes 6000 2>/dev/null || true; echo 'fleet-comms note: act per HACP - an unread blocker or [FREEZE] above means stop and resolve before risky work.'; } | head -c 8000; exit 0";

/// Timeout for the fleet-comms hook. Session start should stay snappy; a
/// wedged conversations backend gets cut off well before the 600s default.
pub const FLEET_COMMS_SESSION_START_TIMEOUT_SEC: u64 = 30;

/// Status message surfaced in the UI while the hook runs.
pub const FLEET_COMMS_SESSION_START_STATUS_MESSAGE: &str = "Checking fleet comms";

/// Pinned trust hash of the canonical fleet-comms hook identity.
///
/// This is the exact value fleet config management writes into
/// `[hooks.state."<config.toml path>:session_start:<group>:0"] trusted_hash`.
/// If the test guarding this constant fails, the normalized hook identity
/// computation changed: bump this constant **and** roll new state entries to
/// every machine in the same change, or deployed fleet hooks silently stop
/// running.
pub const FLEET_COMMS_SESSION_START_TRUSTED_HASH: &str =
    "sha256:92211d25ce588a90835f79ba7605f0884a3e7948e858f51d55893f5801b2fab7";

/// The canonical fleet-comms `SessionStart` matcher group exactly as config
/// parsing produces it from the deployed snippet.
///
/// `matcher: None` so the hook fires for every session-start source
/// (`startup`, `resume`, `clear`, `compact`) — resume/clear/compact re-inject
/// because earlier injected context may have been lost.
pub fn fleet_comms_session_start_matcher_group() -> MatcherGroup {
    MatcherGroup {
        matcher: None,
        hooks: vec![HookHandlerConfig::Command {
            command: FLEET_COMMS_SESSION_START_COMMAND.to_string(),
            command_windows: None,
            timeout_sec: Some(FLEET_COMMS_SESSION_START_TIMEOUT_SEC),
            r#async: false,
            status_message: Some(FLEET_COMMS_SESSION_START_STATUS_MESSAGE.to_string()),
        }],
    }
}

/// Computes the trust hash of the canonical hook through the same
/// normalization path hook discovery uses.
pub fn fleet_comms_session_start_trusted_hash() -> String {
    let group = fleet_comms_session_start_matcher_group();
    let normalized_handler = HookHandlerConfig::Command {
        command: FLEET_COMMS_SESSION_START_COMMAND.to_string(),
        command_windows: None,
        timeout_sec: Some(FLEET_COMMS_SESSION_START_TIMEOUT_SEC.max(1)),
        r#async: false,
        status_message: Some(FLEET_COMMS_SESSION_START_STATUS_MESSAGE.to_string()),
    };
    crate::engine::discovery::command_hook_hash(
        HookEventName::SessionStart,
        /*matcher*/ None,
        &group,
        normalized_handler,
    )
}

/// Persisted hook-state key for the canonical hook when it is the
/// `group_index`-th `[[hooks.SessionStart]]` entry of `config_toml_path`.
///
/// Hook-state keys are positional today (`<source>:<event>:<group>:<handler>`),
/// so deployment tooling that appends to a config which already declares
/// `SessionStart` hooks must pass the real group index.
pub fn fleet_comms_session_start_state_key(config_toml_path: &Path, group_index: usize) -> String {
    crate::hook_key(
        &config_toml_path.display().to_string(),
        HookEventName::SessionStart,
        group_index,
        /*handler_index*/ 0,
    )
}

/// Renders the deployable `config.toml` fragment: the canonical hook plus the
/// pre-seeded trust state entry for `config_toml_path`.
///
/// The fragment assumes the hook becomes the **first** `[[hooks.SessionStart]]`
/// entry in that file (state keys are positional; see
/// [`fleet_comms_session_start_state_key`]).
pub fn fleet_comms_config_toml_snippet(config_toml_path: &Path) -> String {
    let state_key = fleet_comms_session_start_state_key(config_toml_path, /*group_index*/ 0);
    format!(
        r#"[[hooks.SessionStart]]

[[hooks.SessionStart.hooks]]
type = "command"
command = "{FLEET_COMMS_SESSION_START_COMMAND}"
timeout = {FLEET_COMMS_SESSION_START_TIMEOUT_SEC}
statusMessage = "{FLEET_COMMS_SESSION_START_STATUS_MESSAGE}"

[hooks.state."{state_key}"]
trusted_hash = "{FLEET_COMMS_SESSION_START_TRUSTED_HASH}"
"#
    )
}

#[cfg(test)]
mod tests {
    use codex_config::ConfigLayerEntry;
    use codex_config::ConfigLayerSource;
    use codex_config::ConfigLayerStack;
    use codex_config::ConfigRequirements;
    use codex_config::ConfigRequirementsToml;
    use codex_config::TomlValue;
    use codex_protocol::protocol::HookEventName;
    use codex_protocol::protocol::HookTrustStatus;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;

    use super::FLEET_COMMS_SESSION_START_COMMAND;
    use super::FLEET_COMMS_SESSION_START_TRUSTED_HASH;
    use super::fleet_comms_config_toml_snippet;
    use super::fleet_comms_session_start_state_key;
    use super::fleet_comms_session_start_trusted_hash;
    use crate::engine::discovery::discover_handlers;

    fn user_config_path() -> AbsolutePathBuf {
        AbsolutePathBuf::try_from(if cfg!(windows) {
            std::path::PathBuf::from("C:\\Users\\hasna\\.codewith\\config.toml")
        } else {
            std::path::PathBuf::from("/home/hasna/.codewith/config.toml")
        })
        .expect("absolute config path")
    }

    fn user_layer_stack_from_snippet(config_path: &AbsolutePathBuf) -> ConfigLayerStack {
        let snippet = fleet_comms_config_toml_snippet(config_path.as_path());
        let config: TomlValue = toml::from_str(&snippet).expect("snippet parses as TOML");
        ConfigLayerStack::new(
            vec![ConfigLayerEntry::new(
                ConfigLayerSource::User {
                    file: config_path.clone(),
                    profile: None,
                },
                config,
            )],
            ConfigRequirements::default(),
            ConfigRequirementsToml::default(),
        )
        .expect("config layer stack")
    }

    /// Guards the fleet deployment contract: the pinned hash must equal the
    /// hash discovery computes for the canonical hook. If this fails, the
    /// normalized hook identity changed; bump the pinned constant and re-roll
    /// fleet trust state in the same change.
    #[test]
    fn pinned_trusted_hash_matches_discovery_identity() {
        assert_eq!(
            fleet_comms_session_start_trusted_hash(),
            FLEET_COMMS_SESSION_START_TRUSTED_HASH,
            "normalized hook identity computation changed; update \
             FLEET_COMMS_SESSION_START_TRUSTED_HASH and re-roll fleet hook state"
        );
    }

    /// The command must embed into a TOML basic string byte-for-byte: only
    /// single quotes, no backslashes, no control characters, single line.
    #[test]
    fn command_needs_no_toml_or_json_escaping() {
        assert!(!FLEET_COMMS_SESSION_START_COMMAND.contains('"'));
        assert!(!FLEET_COMMS_SESSION_START_COMMAND.contains('\\'));
        assert!(!FLEET_COMMS_SESSION_START_COMMAND.contains('\n'));
        assert!(
            FLEET_COMMS_SESSION_START_COMMAND
                .chars()
                .all(|ch| !ch.is_control())
        );
    }

    #[test]
    fn state_key_is_config_path_scoped_and_positional() {
        let key = fleet_comms_session_start_state_key(
            std::path::Path::new("/home/hasna/.codewith/config.toml"),
            /*group_index*/ 0,
        );
        assert_eq!(key, "/home/hasna/.codewith/config.toml:session_start:0:0");
    }

    /// End-to-end over the real discovery path: parsing the emitted snippet as
    /// the user config layer must yield exactly one enabled, trusted, runnable
    /// `SessionStart` handler without any interactive trust step.
    #[test]
    fn snippet_discovers_as_trusted_enabled_session_start_handler() {
        let config_path = user_config_path();
        let stack = user_layer_stack_from_snippet(&config_path);

        let discovered = discover_handlers(
            Some(&stack),
            /*plugin_hook_sources*/ Vec::new(),
            /*plugin_hook_load_warnings*/ Vec::new(),
            /*bypass_hook_trust*/ false,
        );

        assert_eq!(discovered.warnings, Vec::<String>::new());
        assert_eq!(
            discovered.hook_entries,
            vec![crate::HookListEntry {
                key: fleet_comms_session_start_state_key(
                    config_path.as_path(),
                    /*group_index*/ 0
                ),
                event_name: HookEventName::SessionStart,
                handler_type: codex_protocol::protocol::HookHandlerType::Command,
                matcher: None,
                command: Some(FLEET_COMMS_SESSION_START_COMMAND.to_string()),
                timeout_sec: super::FLEET_COMMS_SESSION_START_TIMEOUT_SEC,
                status_message: Some(super::FLEET_COMMS_SESSION_START_STATUS_MESSAGE.to_string()),
                source_path: config_path.clone(),
                source: codex_protocol::protocol::HookSource::User,
                plugin_id: None,
                display_order: 0,
                enabled: true,
                is_managed: false,
                current_hash: FLEET_COMMS_SESSION_START_TRUSTED_HASH.to_string(),
                trust_status: HookTrustStatus::Trusted,
            }]
        );

        // The handler must actually be runnable (not just listed).
        assert_eq!(discovered.handlers.len(), 1);
        assert_eq!(
            discovered.handlers[0].command,
            FLEET_COMMS_SESSION_START_COMMAND
        );
        assert_eq!(discovered.handlers[0].matcher, None);
        assert_eq!(
            discovered.handlers[0].timeout_sec,
            super::FLEET_COMMS_SESSION_START_TIMEOUT_SEC
        );
    }

    /// A stale trust hash must leave the hook listed but not runnable — this
    /// is the silent-skip failure mode the pinned hash test protects against,
    /// asserted here so the protection itself is tested.
    #[test]
    fn snippet_with_wrong_trusted_hash_is_listed_but_not_runnable() {
        let config_path = user_config_path();
        let snippet = fleet_comms_config_toml_snippet(config_path.as_path()).replace(
            FLEET_COMMS_SESSION_START_TRUSTED_HASH,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        );
        let config: TomlValue = toml::from_str(&snippet).expect("snippet parses as TOML");
        let stack = ConfigLayerStack::new(
            vec![ConfigLayerEntry::new(
                ConfigLayerSource::User {
                    file: config_path,
                    profile: None,
                },
                config,
            )],
            ConfigRequirements::default(),
            ConfigRequirementsToml::default(),
        )
        .expect("config layer stack");

        let discovered = discover_handlers(
            Some(&stack),
            Vec::new(),
            Vec::new(),
            /*bypass_hook_trust*/ false,
        );

        assert_eq!(discovered.hook_entries.len(), 1);
        assert_eq!(
            discovered.hook_entries[0].trust_status,
            HookTrustStatus::Modified
        );
        assert_eq!(discovered.handlers.len(), 0);
    }

    /// Runs the canonical command through the real engine with a controlled
    /// `PATH` (`env PATH=<dirs> /bin/sh -c <command>`).
    #[cfg(unix)]
    async fn run_canonical_hook_with_path(
        path_value: &str,
    ) -> crate::events::session_start::SessionStartOutcome {
        use codex_protocol::ThreadId;

        use crate::engine::ClaudeHooksEngine;
        use crate::engine::CommandShell;
        use crate::events::session_start::SessionStartRequest;
        use crate::events::session_start::SessionStartSource;
        use crate::events::session_start::StartHookTarget;

        let config_path = user_config_path();
        let stack = user_layer_stack_from_snippet(&config_path);
        let engine = ClaudeHooksEngine::new(
            /*enabled*/ true,
            /*bypass_hook_trust*/ false,
            Some(&stack),
            Vec::new(),
            Vec::new(),
            CommandShell {
                program: "/usr/bin/env".to_string(),
                args: vec![
                    format!("PATH={path_value}"),
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                ],
            },
        );
        assert_eq!(engine.warnings(), Vec::<String>::new());

        let cwd = AbsolutePathBuf::try_from(std::env::temp_dir()).expect("absolute temp dir");
        engine
            .run_session_start(
                SessionStartRequest {
                    session_id: ThreadId::new(),
                    cwd,
                    transcript_path: None,
                    model: "test-model".to_string(),
                    permission_mode: "default".to_string(),
                    target: StartHookTarget::SessionStart {
                        source: SessionStartSource::Startup,
                    },
                },
                /*turn_id*/ None,
            )
            .await
    }

    /// On a machine without the `conversations` CLI the hook must be a clean
    /// no-op: exit 0, empty stdout, no injected context. This is the fail-open
    /// property that makes fleet-wide deployment safe ahead of the
    /// conversations rollout.
    #[cfg(unix)]
    #[tokio::test]
    async fn command_fails_open_without_conversations_cli() {
        use codex_protocol::protocol::HookRunStatus;

        let empty_dir = tempfile::tempdir().expect("create temp dir");
        let outcome =
            run_canonical_hook_with_path(&format!("{}", empty_dir.path().display())).await;

        assert!(!outcome.should_stop);
        assert_eq!(outcome.hook_events.len(), 1);
        assert_eq!(
            outcome.hook_events[0].run.status,
            HookRunStatus::Completed,
            "fleet-comms hook must never fail session start: {:?}",
            outcome.hook_events[0].run.entries
        );
        assert_eq!(outcome.additional_contexts, Vec::<String>::new());
    }

    /// With a `conversations` CLI present, the emitted digest must inject as
    /// model context and stay bounded. The blockers JSON is embedded after a
    /// plain-text header line: stdout beginning with `{`/`[` would be parsed
    /// as (invalid) hook-output JSON and fail the hook, so this test is the
    /// regression guard for the header-before-JSON layout.
    #[cfg(unix)]
    #[tokio::test]
    async fn command_surfaces_bounded_context_with_conversations_cli() {
        use std::os::unix::fs::PermissionsExt;

        use codex_protocol::protocol::HookRunStatus;

        let shim_dir = tempfile::tempdir().expect("create temp dir");
        let shim_path = shim_dir.path().join("conversations");
        std::fs::write(
            &shim_path,
            "#!/bin/sh\ncase \"$1\" in\nblockers) echo '[{\"id\":\"m1\",\"priority\":\"blocking\"}]' ;;\ndigest) echo 'Digest #announcements'; echo '  [FREEZE] configs rollout paused' ;;\nesac\n",
        )
        .expect("write conversations shim");
        std::fs::set_permissions(&shim_path, std::fs::Permissions::from_mode(0o755))
            .expect("mark shim executable");

        let outcome =
            run_canonical_hook_with_path(&format!("{}:/usr/bin:/bin", shim_dir.path().display()))
                .await;

        assert!(!outcome.should_stop);
        assert_eq!(outcome.hook_events.len(), 1);
        assert_eq!(
            outcome.hook_events[0].run.status,
            HookRunStatus::Completed,
            "fleet-comms hook must never fail session start: {:?}",
            outcome.hook_events[0].run.entries
        );
        assert_eq!(outcome.additional_contexts.len(), 1);
        let context = &outcome.additional_contexts[0];
        assert!(context.starts_with("fleet-comms session-start check"));
        assert!(context.contains("\"priority\":\"blocking\""));
        assert!(context.contains("[FREEZE] configs rollout paused"));
        assert!(context.len() <= 8000);
    }
}
