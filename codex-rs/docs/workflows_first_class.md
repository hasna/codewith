# First-class Codewith workflows

Status: in progress. This document describes the target design for promoting
workflows from the experimental `Workflows` feature flag to a first-class
capability, records the slice delivered so far, and enumerates the remaining
scope so the work can be picked up incrementally without re-deriving the plan.

## Goal

Workflows are saved, inspectable, runnable multi-step orchestrations that
coordinate subagents, background agent threads, monitors, schedules, hooks,
skills, permissions, progress, and verification. The single active `/goal`
behaviour is intentionally left untouched: `/goal` stays one live objective and
is never expanded into a queued sequence. Product work is concentrated on
`/workflow`, workflow run state, resumability, and explicit user approval for
workflow-generated actions.

## Existing surface (already shipped, experimental)

- `codex-workflows` crate: declarative YAML spec parse + validation boundary
  (no execution, no permissions).
- `codex-state`: durable workflow specs, runs, steps, verifiers, events, the
  goal-plan projection, the run orchestrator (claim / advance / branch admission
  / reconcile), verifiers, and automation timers.
- `codex-workflows-extension`: the model-facing `manage_workflow` tool and the
  `validate_workflow_yaml` tool, gated behind the `Workflows` feature and a
  saved non-review thread.
- `app-server`: `thread/workflow/*` and `thread/workflow/run/*` RPCs.
- TUI: the `/workflow` slash command family and workflow/run display cells.

## Delivered slice: explicit user approval for gated steps

Before this slice, a workflow step that declared an `approval_gate` became
`ready` but was permanently excluded from branch admission
(`ready_branch_candidates_in_tx` filtered on `approval_gate IS NULL`). There was
no way to record an approval, so gated steps were a dead end. This directly
contradicted the product requirement for "explicit user approval for
workflow-generated actions".

This slice makes the approval gate a real, resolvable control:

- Migration `0060_workflow_run_step_approvals.sql` adds an `approval_state`
  column (`NULL` for ungated steps, otherwise `pending` / `approved` /
  `rejected`) and backfills existing gated steps to `pending`.
- Run creation records `approval_state = 'pending'` for every gated step.
- `WorkflowStore::set_workflow_run_step_approval` records an explicit
  `approve` / `reject` decision inside a single transaction:
  - approve flips the gate to `approved` so the orchestrator admits the step on
    the next tick;
  - reject flips it to `rejected` and marks the step `skipped` so downstream
    dependents stall instead of running without consent;
  - both append an auditable `step_approval_granted` / `step_approval_rejected`
    run event with `actor_kind = "user"`. Decisions are idempotent, ungated and
    already-executed steps are no-ops, and the raw approval reason is never
    persisted (only its presence is recorded) to keep provenance free of
    injected content.
- The orchestrator admission query admits a step when it has no gate **or** its
  gate is `approved`; nothing else in the claim/advance/lease path changes, so
  approving a step does not fence out the active orchestrator owner.
- The `manage_workflow` tool exposes `approve_step` / `reject_step` actions
  (thread-bound, run-ownership checked) and surfaces `approvalGate` /
  `approvalState` in sanitized step output.

The run status enum, pause/resume/cancel resumability, and `/goal` are
unchanged.

## Remaining scope (not yet done)

1. **App-server RPCs.** Add `thread/workflow/run/step/approve` and
   `.../reject` to `app-server-protocol` (params + response reusing
   `ThreadWorkflowRunSnapshot`) and the `thread_workflow_processor`, plus the
   generated TS bindings, mirroring the existing pause/resume/cancel plumbing.
2. **TUI approval UX.** Extend the `/workflow run ...` slash family with
   `approve <run> <step>` / `reject <run> <step>`, add a
   `ThreadWorkflowAction::RunStepApprove/Reject`, surface pending approvals in
   the run detail cell, and render the approval decision inline. A pending-gate
   badge in the run summary makes stalled runs obvious.
3. **Protocol field exposure.** Add `approvalGate` / `approvalState` to
   `ThreadWorkflowRunStep` so GUIs and the app-server can render the gate state
   (the state model already carries it).
4. **Run-start approval gate.** Optionally require an explicit user confirmation
   before a *model*-initiated `manage_workflow start` projects a run into a
   durable goal plan, so autonomous multi-step execution always crosses a human
   checkpoint. User-initiated starts from the TUI remain explicit by
   construction.
5. **Run-level `approvals.required_before` enforcement.** Today the spec field
   is validated but not enforced at runtime; wire it to auto-gate the named
   steps at run creation instead of relying solely on per-step `approval_gate`.
6. **Feature graduation.** Move `Feature::Workflows` from
   `Stage::Experimental` toward `Stage::Stable`. This is deferred deliberately:
   default-enabling changes the model tool surface and many TUI/app-server
   snapshots, and should land as its own reviewed change once the approval and
   RPC surfaces above are complete.
7. **Approval expiry / batching.** Optional: expire stale pending approvals and
   allow approving a whole parallel group at once.

## Verification

- `codex-state` unit tests cover the end-to-end gate: a gated step is excluded
  from admission until approved, approval is idempotent and audited, rejection
  skips the step, ungated/unknown targets are no-ops, and the raw reason never
  leaks into events.
- `codex-workflows-extension` tests cover the `manage_workflow` tool surface.
