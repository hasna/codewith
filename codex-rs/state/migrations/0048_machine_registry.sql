CREATE TABLE machine_registry_machines (
    machine_id TEXT PRIMARY KEY NOT NULL,
    installation_id TEXT UNIQUE,
    display_name TEXT,
    trust_state TEXT NOT NULL CHECK(trust_state IN ('local', 'trusted', 'untrusted', 'disabled', 'revoked')),
    enrollment_state TEXT NOT NULL CHECK(enrollment_state IN ('local', 'manual', 'discovered', 'enrolled')),
    health_state TEXT NOT NULL CHECK(health_state IN ('unknown', 'online', 'offline', 'degraded')),
    source_kind TEXT NOT NULL CHECK(source_kind IN ('local', 'manual', 'adapter')),
    adapter_name TEXT,
    capabilities_json TEXT NOT NULL CHECK(json_valid(capabilities_json)),
    last_seen_at_ms INTEGER,
    disabled_at_ms INTEGER,
    forgotten_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    CHECK(installation_id IS NULL OR LENGTH(TRIM(installation_id)) > 0),
    CHECK(display_name IS NULL OR LENGTH(TRIM(display_name)) > 0),
    CHECK(adapter_name IS NULL OR LENGTH(TRIM(adapter_name)) > 0),
    CHECK(disabled_at_ms IS NULL OR disabled_at_ms >= created_at_ms),
    CHECK(forgotten_at_ms IS NULL OR forgotten_at_ms >= created_at_ms)
);

CREATE INDEX idx_machine_registry_machines_trust_health
    ON machine_registry_machines(trust_state, health_state, updated_at_ms DESC, machine_id);

CREATE INDEX idx_machine_registry_machines_seen
    ON machine_registry_machines(last_seen_at_ms DESC, machine_id)
    WHERE last_seen_at_ms IS NOT NULL;

CREATE TABLE machine_registry_endpoints (
    endpoint_id TEXT PRIMARY KEY NOT NULL,
    machine_id TEXT NOT NULL REFERENCES machine_registry_machines(machine_id) ON DELETE CASCADE,
    transport TEXT NOT NULL CHECK(transport IN ('lan', 'tailscale', 'manual', 'remote_control', 'adapter')),
    normalized_address TEXT NOT NULL CHECK(LENGTH(TRIM(normalized_address)) > 0),
    display_address TEXT NOT NULL CHECK(LENGTH(TRIM(display_address)) > 0),
    priority INTEGER NOT NULL DEFAULT 0,
    capabilities_json TEXT NOT NULL CHECK(json_valid(capabilities_json)),
    last_success_at_ms INTEGER,
    last_error TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE(transport, normalized_address)
);

CREATE INDEX idx_machine_registry_endpoints_machine
    ON machine_registry_endpoints(machine_id, priority DESC, updated_at_ms DESC, endpoint_id);

CREATE TABLE machine_registry_workspace_roots (
    machine_id TEXT NOT NULL REFERENCES machine_registry_machines(machine_id) ON DELETE CASCADE,
    path TEXT NOT NULL CHECK(LENGTH(TRIM(path)) > 0),
    trust_level TEXT NOT NULL CHECK(trust_level IN ('unknown', 'read_only', 'trusted')),
    read_only INTEGER NOT NULL CHECK(read_only IN (0, 1)),
    last_seen_at_ms INTEGER NOT NULL,
    PRIMARY KEY(machine_id, path)
);

CREATE TABLE machine_registry_repo_summaries (
    machine_id TEXT NOT NULL REFERENCES machine_registry_machines(machine_id) ON DELETE CASCADE,
    repo_path TEXT NOT NULL CHECK(LENGTH(TRIM(repo_path)) > 0),
    git_origin_hash TEXT,
    git_origin_redacted TEXT,
    branch TEXT,
    dirty_state TEXT NOT NULL CHECK(dirty_state IN ('unknown', 'clean', 'dirty')),
    last_seen_at_ms INTEGER NOT NULL,
    PRIMARY KEY(machine_id, repo_path)
);

CREATE TABLE machine_registry_session_summaries (
    machine_id TEXT NOT NULL REFERENCES machine_registry_machines(machine_id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL,
    cwd TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('unknown', 'running', 'idle', 'stopped')),
    goal_status TEXT,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY(machine_id, thread_id)
);
