# Background Agent Architecture

This note defines the durable model for Codewith background agents. It is an
internal implementation contract for `codex-background-agent`, `codex-state`,
`codex-app-server`, `codex-cli`, and `codex-tui`.

## Durable Roster

The durable roster is `background_agent_runs` in the Codewith state database
under `CODEWITH_HOME`. A run row is the only source of truth for agent identity,
desired state, retention state, ownership generation, worker handle metadata,
heartbeat, and terminal result.

The following must not be used as the durable background-agent roster or
liveness source:

- `thread/loaded/list` or any loaded `ThreadManager` map.
- App-server connection or listener lifetime.
- Thread metadata rows, `thread_spawn_edges`, or rollout files alone.
- Legacy CSV-shaped `agent_jobs` and `agent_job_items` rows.

Those legacy records can be linked from a background-agent run for history and
compatibility, but they are read-only linkage inputs from the background-agent
system's perspective.

## Admission Contract

Admission is one `BEGIN IMMEDIATE` state transaction. The transaction first
looks up an idempotency key, validates that it is bound to the same request,
source, thread linkage, exact auth-profile alias, config fingerprint, and
admission schema, then either adopts that run or counts live/recoverable rows
and inserts exactly one new run. Stopped or terminal rows do not consume
capacity; queued, owned, waiting, stopping, and recoverable orphaned rows do.

The CLI, TUI, app server, persisted run, execution snapshot, and daemon pid
record use `codewith.background-agent.admission.v1` as the fail-closed schema
contract. A running daemon is reused only when package version, daemon protocol,
admission schema, and required capability set all match. An explicitly admitted
auth-profile alias must match the app-server profile and remains exact during
recovery; it is never silently replaced by profile auto-switching.

## Run And Thread Relationship

A background-agent run owns background execution. A thread owns transcript and
rollout/history persistence. A run may reference a thread through these fields:

- `thread_id`: the Codewith thread driven by the worker.
- `thread_store_kind`: the thread store implementation, for example `local` or
  `background-agent`.
- `thread_store_id`: the concrete store/database identifier when needed.
- `rollout_path`: a read-only pointer to the thread rollout file.
- `parent_thread_id`: foreground or parent thread that requested the run.
- `parent_agent_run_id`: parent background-agent run when a worker spawns a
  background child.
- `spawn_linkage_json`: compatibility details such as agent path, role, nickname,
  legacy `agent_jobs` ids, or thread-spawn-edge context.

Creating, loading, or listing thread metadata must not create background-agent
runs. Backfill may create a background-agent run only from an explicit migration
rule that writes a new `background_agent_runs` row and records the compatibility
source in `spawn_linkage_json`.

## State Machine

`status` is observed worker state:

- `queued`: accepted, not yet claimed by a supervisor.
- `starting`: claimed by a supervisor generation; worker startup is in progress.
- `running`: worker is driving a turn or is idle at a safe boundary.
- `waiting_on_approval`: worker is blocked on approval or permission grant.
- `waiting_on_user`: worker is blocked on user input or MCP elicitation.
- `stopping`: stop requested; worker should exit.
- `completed`: worker reached a successful terminal result.
- `failed`: worker reached an unrecoverable terminal error.
- `cancelled`: stop/cancel completed.
- `orphaned`: previous owner missed heartbeat and the run can be reconciled.

`desired_state` is caller intent:

- `running`: supervisor may claim or continue the run.
- `stopped`: supervisor should stop or avoid starting the run.
- `deleted`: supervisor should not start the run and cleanup may proceed.

`retention_state` tracks cleanup independently from execution: active, archived,
delete requested, or deleted.

## Ownership And Liveness

Supervisors claim runs by writing `supervisor_id`, incrementing `generation`, and
creating a process lease. Worker handles are recorded as `pid`, `pgid`, or
`job_id` when the execution backend owns an OS process. Heartbeats update the
claimed generation. Reconciliation may orphan stale non-terminal runs after the
configured heartbeat timeout, then re-claim only runs whose `desired_state` is
`running`.

The app-server may host a live worker bridge, but app-server client connection
lifetime is not liveness. Dropping a TUI or CLI connection detaches subscribers
only; it must not delete the run or imply worker death.

## Lifecycle Receipts And Fencing

Lifecycle receipts live in `background_agent_events`, alongside progress
events. Each receipt has a unique `(run_id, receipt_key)` identity and records
the run, generation, attempt, timestamp, and bounded redacted diagnostics.
Retries return the existing receipt instead of advancing the event cursor.
Admission, claim/recovery, first heartbeat for a generation, status transitions,
orphaning, stop, and cancellation all use deterministic receipt keys.

Supervisor-owned heartbeat, status, stop, and process-finalization mutations
compare both `supervisor_id` and `generation`. A stale owner cannot stop or
complete a reclaimed generation. User-requested stop reloads and retries the
current generation once if ownership changes while the request is in flight.

## Attach, Detach, Stop, Delete

Attach returns a durable snapshot: run row, status snapshot, latest execution
snapshot, replayable events after a cursor, and pending interactions. Pending
interactions are marked delivered after the attach snapshot is prepared.

Detach removes only the foreground subscriber. The run stays in its current
desired state.

Stop sets `desired_state = stopped`, records `agent.stopRequested`, and moves
non-terminal runs toward `stopping`. The execution backend is responsible for
graceful shutdown and hard kill when it owns an OS process.

Delete sets `desired_state = deleted`, marks retention as delete requested, and
records `agent.deleteRequested`. Workspace cleanup must refuse to delete dirty or
untracked work unless a force cleanup operation is explicitly recorded.

## Safety And Pending Interactions

Detached agents must never auto-approve unsafe work. Approvals, permission
grants, user input, and MCP elicitation are persisted in
`background_agent_pending_interactions` with request payload, response payload,
timeout, delivery, response status, and audit events.

The unattended policy is conservative:

- Approval and permission-grant requests deny by default without a client.
- User input and MCP elicitation wait for an attached client or timeout/cancel.
- Responses are idempotent by interaction id and worker request id.

The execution snapshot records the effective cwd, sandbox/approval profile,
auth profile, model/provider configuration, recovery policy, and config
fingerprint needed to resume only at safe boundaries.

## Compatibility And Backfill Rules

Compatibility is explicit and one-way:

- Existing threads remain normal threads until a background-agent run explicitly
  links them.
- `thread_spawn_edges` remain navigation/history edges and are never used to
  infer worker liveness.
- Legacy `agent_jobs` rows remain CSV job records and are never used as
  background-agent runs.
- Backfill must preserve legacy ids in `spawn_linkage_json`; it must not mutate
  legacy rows to represent new liveness.
- Historical runs with missing process handles are treated as queued or orphaned
  only when a background-agent run row says so.

Compatibility tests should prove that legacy thread, spawn-edge, and CSV job
rows do not populate the background-agent roster, and that explicit run linkage
preserves thread/store/job identifiers for replay.

## Status Rows

`background_agent_status_snapshots` are cheap roster rows for CLI/TUI list
surfaces. They should include the current status, desired state, concise summary,
pending interaction count, last event cursor, and payload metadata such as
current activity, waiting reason, tool counts, timestamps, final result, and
review/PR metadata when available.

The snapshot is a cache derived from durable events and run state. It improves
roster latency but does not replace the run row as the source of truth.
