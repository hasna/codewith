# Native Active-Session Messaging Plan

## Scope

Implement a native Codewith messaging path for currently active sessions only.
The first implementation does not persist messages for inactive sessions and
does not depend on Takumi, open-conversations, or any external conversation SDK.

## Existing Surfaces

- `ThreadManagerState` already owns the in-memory map of loaded non-internal
  threads. This is the authoritative active-session registry for app-server
  requests.
- `thread/loaded/list` exposes active thread IDs but not enough routing or
  display metadata for a messaging client.
- `Op::InterAgentCommunication` plus `InputQueue::enqueue_mailbox_communication`
  already provide live mailbox delivery without masquerading as raw user input.
- Multi-agent message tools already use `AgentControl::send_inter_agent_communication`
  for spawned-agent delivery.
- App-server v2 request dispatch is the right boundary for clients and future
  channel bridges because it keeps transport-specific behavior out of core.

## Implementation Slices

1. Add small active peer and envelope primitives near app-server/protocol, backed
   by the existing loaded-thread map. Track thread ID, cwd, parent/thread-source
   metadata, optional display labels, and capabilities.
2. Add v2 app-server methods for active peer listing and sending messages. These
   methods must return explicit not-found/inactive errors when the target is not
   currently loaded.
3. Deliver active messages through `Op::InterAgentCommunication` so idle targets
   queue mail and trigger behavior stays controlled by the existing mailbox mode.
4. Keep `/agent` lean by extending the existing slash surface only after the API
   works. Do not add a new inbox or large session UI.
5. Add a minimal adapter-facing envelope that can represent Claude channel-style
   metadata and later Telegram metadata without making those transports part of
   the core router.

## Deferred

Inactive/offline delivery is intentionally deferred. The first version should
not write a durable inbox, poll storage, or pretend an unloaded session received
a message. Cross-process or cross-product bridges may later use the same
adapter envelope, but they must preserve the active-only delivery contract until
durable delivery is designed separately.

## Commit Safety

All implementation work happens in `/tmp/codewith-active-messaging` on branch
`active-session-messaging`. The root checkout at
`/home/hasna/workspace/hasna/opensource/open-codewith` has unrelated TUI edits
and must not be rewritten or staged for this feature.
