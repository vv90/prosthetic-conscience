# Session Behavior

Snapshot date: 2026-03-21

Status: **partial** — session kernel reduce logic implemented, session creation redesigned (`SessionRequested`). HTTP/WS adapters and runtime commands not yet wired.

Load into session context when working on: session kernel logic, session HTTP/WS adapters, session-related tests, session lifecycle.

## Relevant files

- `src/gateway/session/kernel.rs` — `State<SubId>`, `Event<SubId>`, `Effect<SubId>`, `reduce()`
- `src/gateway/session/mod.rs` — module declaration
- `src/gateway/kernel.rs` — `SessionId`, `sessions` field on `GatewayState`, `Event::SessionRequested`, `Effect::SessionCreated`, `Event::SessionEvent`, `generate_session_id()`
- `docs/codebase-state/todo-near-term.md` — session feature intent and implementation process

## Architecture

The session kernel is a fully independent submodule (`gateway::session::kernel`) with its own `Event`, `Effect`, `State`, and `reduce()`. It has no imports from or awareness of the parent kernel.

The parent kernel delegates via `Event::SessionEvent { session_id, event }` — it extracts the session from its `HashMap<SessionId, session::State<SId>>` using `extract` (which gives owned state without cloning) and passes it to `session::kernel::reduce()`. The updated session state is re-inserted via `update`. This enforces domain isolation structurally: session logic cannot access worker/stream state because it never receives it.

A `SessionEvent` for a non-existent session produces a `ProtocolViolation`. Sessions must be explicitly created before they can receive events.

### Mutation model

All session mutations (create, append, subscribe, unsubscribe) go through WS connections, not HTTP. HTTP is read-only (`GET /v1/sessions/:id/entries` for cursor-based reads). This avoids the kernel reply problem — the kernel returns `Transition { state, effects }` which works naturally with push-based WS but not with request-response HTTP.

## Types

- `SessionId(String)` — concrete newtype in parent `kernel.rs`. The parent's map key.
- `State<SubId: Clone + Eq + Hash>` — `{ entries: AppendLog, subscribers: HashMap<SubId, u64>, subscriber_ttl: u64 }`. Defined in `session::kernel`. Generic over subscriber identity type. Uses `im::HashMap` for immutable subscriber-to-deadline mapping.
- Entry index in the vec is the sequence number. No explicit `seq` field.

## Session kernel events

- `EntryAppended { payload }` — an entry was appended.
- `Subscribed { subscriber_id, tick }` — a client subscribed for push notifications. Sets deadline to `tick + subscriber_ttl`.
- `Unsubscribed { subscriber_id }` — a client unsubscribed. Emits `SubscriberRemoved` if subscriber existed.
- `Tick { tick }` — expires subscribers with `deadline <= tick`. Emits `SubscriberRemoved` for each expired subscriber.
- `SubscriberHeartbeat { subscriber_id, tick }` — resets subscriber deadline to `tick + subscriber_ttl`. No-op if subscriber unknown.

Session creation is the parent kernel's responsibility. No `session_id` on session events — the parent selects which session to delegate to. The session kernel has no access to the parent's tick — it must be passed in explicitly via event payloads.

## Session kernel effects

- `NotifySubscribers { entry_index, payload, subscribers }` — fan out a new entry to all current subscribers. Best-effort push; the kernel does not retry or track delivery.
- `SubscriberRemoved { subscriber_id }` — a subscriber was removed (by tick expiry or explicit unsubscribe). Emitted so the runtime can notify the subscriber's channel.

Errors (e.g. subscriber not in set on unsubscribe) are handled structurally by the session kernel. The parent kernel handles missing sessions before delegation, using its own `ProtocolViolation`.

## Invariants

These are properties that hold for all reachable states and all valid event sequences.

### Entry invariants

- **Entry permanence**: if `session.entries[i] == v` before a transition, then `session.entries[i] == v` after. Entries are never removed, reordered, or mutated.
- **Append-only growth**: `session.entries.len()` never decreases across a transition, and increases by at most 1.

### Isolation invariants

- **Cross-session isolation**: a transition affecting session `A` never mutates any other session's entries or subscribers. Enforced structurally — the session kernel receives only one session's state.
- **Domain isolation**: session events never mutate `available`, `active_streams`, or `tick`. Enforced structurally — the session kernel has no access to parent state.

### Notification invariants

- **Notification if and only if subscribers**: `NotifySubscribers` is emitted if and only if an entry was appended to a session with a non-empty subscriber set at the time of append.
- **Notification payload correctness**: when emitted, `entry_index` equals the index of the newly appended entry (`entries.len() - 1` after append), and `payload` equals the appended value.
- **Notification subscriber correctness**: the effect's `subscribers` list contains exactly the session's subscriber set at the time of append.

### Subscriber deadline invariants

- **Tick never adds subscribers**: subscriber count can only decrease or stay the same after `Tick`.
- **Tick preserves entries and TTL**: `Tick` never modifies `entries` or `subscriber_ttl`.
- **Heartbeat produces no effects**: `SubscriberHeartbeat` never emits any effects.
- **Heartbeat for unknown subscriber is a no-op**: state unchanged if subscriber not in set.
- **Surviving subscribers have valid deadlines**: all subscribers remaining after `Tick { tick }` have `deadline > tick`.
- **Append preserves subscribers**: `EntryAppended` does not modify the subscriber map.
- **Subscriber termination liveness**: every subscriber that enters a session eventually receives a `SubscriberRemoved` effect (given enough ticks without heartbeats).
- **Subscriber drain liveness**: all subscribers are eventually removed from the session (given enough ticks without heartbeats).

## Implemented

- Session kernel `reduce()` with five arms: `EntryAppended`, `Subscribed`, `Unsubscribed`, `Tick`, `SubscriberHeartbeat`
- `subscribers: HashMap<SubId, u64>` mapping subscriber to deadline tick
- `subscriber_ttl: u64` on session state, set from parent `GatewayState::subscriber_ttl`
- `Tick` expires stale subscribers (`deadline <= tick`), emits `SubscriberRemoved` for each
- `SubscriberHeartbeat` resets deadline to `tick + subscriber_ttl` for known subscribers, no-op for unknown
- `Subscribed` sets initial deadline to `tick + subscriber_ttl`
- `Unsubscribed` emits `SubscriberRemoved` if subscriber existed, no-op if not
- `SubscriberRemoved` effect for subscriber removal (tick expiry or explicit unsubscribe)
- Parent kernel `Tick` propagates to all sessions
- Parent kernel `SessionRequested` sets `subscriber_ttl` on initial session and computes initial deadline via session reducer
- Parent kernel `SessionRequested` event with deterministic ID generation via `hash(runtime_id, session_counter)`
- `runtime_id: u128` stored in `GatewayState`, set from `uuid::Uuid::new_v4().as_u128()` at runtime startup
- `session_counter: u64` monotonically incrementing in state
- `SessionRequested` creates session, subscribes the creator, emits `Effect::SessionCreated { session_id, subscriber_id }`
- No error path — unique IDs guaranteed
- Parent kernel `SessionEvent` delegation to child reducer
- `GET /v1/sessions/:id/entries` — read-only HTTP endpoint with cursor-based pagination
- Runtime handles `SubscriberRemoved` in `resolve_effects` (resolves subscriber_id to stream handle) and `spawn_effects` (no-op stub — WS adapters not yet wired)
- `subscriber_ttl: u64` on `GatewayConfig`, passed to `GatewayState::new()`
- 17 session kernel property tests (S6-S11 + 3 foundational + P1-P8) using `arb_state()` generator
- 12 session kernel unit tests (T1-T12) covering tick expiry, heartbeat, subscribe/unsubscribe
- 5 SessionRequested unit tests + 3 Chunk B unit tests (T13-T15) in parent kernel
- 6 SessionRequested/session property tests (I1-I5 + P9) in parent kernel

**Chunk C: Session expiry in parent kernel** (done)

- After tick propagation, parent kernel checks each session for empty subscriber sets.
- Sessions with no subscribers are removed from state.
- Emits `Effect::SessionExpired { session_id, entries: Vec<Value> }` carrying the full entry log so it can be persisted/forwarded before being lost from memory.
- `AppendLog::into_entries(self) -> Vec<Value>` consumes the log to extract entries for the effect.
- `SessionExpired` is a parent-level effect (like `SessionCreated`), not wrapped in `SessionEffect`.
- Runtime handles `SessionExpired` in `resolve_effects` (pass through) and `spawn_effects` (no-op stub — persistence adapters not yet wired).
- I8 (`sessions_only_grow`) replaced by P13 (sessions only removed when subscribers empty).
- 6 parent kernel unit tests (T16-T21) + 3 parent property tests (P10-P12).

## Not yet implemented

- WS adapters for session mutations (create, append, subscribe, unsubscribe)
- Runtime commands for session events (except `QuerySessionEntries` which is done)
- `SessionCreated` effect executor (resolve and spawn are stubbed)
- `SubscriberRemoved` effect executor (resolve done, spawn is no-op stub)
- `SessionExpired` effect executor (resolve done as pass-through, spawn is no-op stub — persistence adapters not yet wired)
- Cursor-based read improvements (adapter concern)
