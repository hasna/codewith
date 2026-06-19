CREATE TABLE thread_pending_interactions (
    interaction_id TEXT PRIMARY KEY NOT NULL,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL CHECK(source_kind IN ('thread', 'background_agent', 'goal', 'usage_profile')),
    source_id TEXT,
    turn_id TEXT,
    worker_request_id TEXT,
    server_request_id_json TEXT CHECK(server_request_id_json IS NULL OR json_valid(server_request_id_json)),
    kind TEXT NOT NULL CHECK(kind IN (
        'command_approval',
        'file_change_approval',
        'user_input',
        'mcp_elicitation',
        'permission_grant',
        'dynamic_tool',
        'usage_limit',
        'profile_switch',
        'blocked'
    )),
    status TEXT NOT NULL CHECK(status IN (
        'pending',
        'delivered',
        'responded',
        'expired',
        'cancelled',
        'denied',
        'no_longer_waiting'
    )),
    request_payload_json TEXT NOT NULL CHECK(json_valid(request_payload_json)),
    request_payload_sha256 TEXT NOT NULL CHECK(
        LENGTH(request_payload_sha256) = 64
        AND request_payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    request_payload_preview TEXT NOT NULL,
    request_redactions_json TEXT NOT NULL CHECK(json_valid(request_redactions_json)),
    response_payload_json TEXT CHECK(response_payload_json IS NULL OR json_valid(response_payload_json)),
    response_payload_sha256 TEXT CHECK(
        response_payload_sha256 IS NULL
        OR (
            LENGTH(response_payload_sha256) = 64
            AND response_payload_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    response_payload_preview TEXT,
    response_redactions_json TEXT CHECK(response_redactions_json IS NULL OR json_valid(response_redactions_json)),
    no_client_policy TEXT NOT NULL CHECK(LENGTH(TRIM(no_client_policy)) > 0),
    timeout_at_ms INTEGER,
    created_at_ms INTEGER NOT NULL,
    delivered_at_ms INTEGER,
    responded_at_ms INTEGER,
    terminal_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    CHECK(delivered_at_ms IS NULL OR delivered_at_ms >= created_at_ms),
    CHECK(responded_at_ms IS NULL OR responded_at_ms >= created_at_ms),
    CHECK(terminal_at_ms IS NULL OR terminal_at_ms >= created_at_ms)
);

CREATE INDEX idx_thread_pending_interactions_thread_status_created
    ON thread_pending_interactions(thread_id, status, created_at_ms DESC, interaction_id);

CREATE INDEX idx_thread_pending_interactions_status_created
    ON thread_pending_interactions(status, created_at_ms DESC, interaction_id)
    WHERE status IN ('pending', 'delivered');

CREATE INDEX idx_thread_pending_interactions_source
    ON thread_pending_interactions(source_kind, source_id, created_at_ms DESC)
    WHERE source_id IS NOT NULL;

CREATE UNIQUE INDEX idx_thread_pending_interactions_worker_request
    ON thread_pending_interactions(thread_id, kind, worker_request_id)
    WHERE worker_request_id IS NOT NULL;

CREATE TABLE thread_pending_interaction_events (
    event_id TEXT PRIMARY KEY NOT NULL,
    interaction_id TEXT NOT NULL REFERENCES thread_pending_interactions(interaction_id) ON DELETE CASCADE,
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    event_kind TEXT NOT NULL CHECK(event_kind IN (
        'created',
        'delivered',
        'responded',
        'expired',
        'cancelled',
        'denied',
        'no_longer_waiting'
    )),
    status TEXT NOT NULL CHECK(status IN (
        'pending',
        'delivered',
        'responded',
        'expired',
        'cancelled',
        'denied',
        'no_longer_waiting'
    )),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    payload_sha256 TEXT NOT NULL CHECK(
        LENGTH(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    payload_preview TEXT NOT NULL,
    redactions_json TEXT NOT NULL CHECK(json_valid(redactions_json)),
    created_at_ms INTEGER NOT NULL
);

CREATE INDEX idx_thread_pending_interaction_events_interaction_created
    ON thread_pending_interaction_events(interaction_id, created_at_ms, event_id);

CREATE INDEX idx_thread_pending_interaction_events_thread_created
    ON thread_pending_interaction_events(thread_id, created_at_ms DESC, event_id);
