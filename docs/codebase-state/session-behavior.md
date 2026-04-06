# Session Behavior

Snapshot date: 2026-03-22

Status: **partial** — session kernel, transport layer (WS handler, runtime commands, effect executors), and integration tests implemented. Persistence (`SessionExpired` executor) not yet wired.

Load into session context when working on: session kernel logic, session HTTP/WS adapters, session-related tests, session lifecycle.

## Relevant files

- `crates/prosthetic-conscience/src/gateway/session/kernel.rs` — `State<SubId>`, `Event<SubId>`, `Effect<SubId>`, `reduce()`
- `crates/prosthetic-conscience/src/gateway/session/mod.rs` — module declaration
- `crates/prosthetic-conscience/src/gateway/kernel.rs` — `SessionId`, `sessions: HashMap<SessionId, session::State<SubId>>` on `GatewayState`, `Event::SessionRequested { subscriber_id: SubId }`, `Effect::SessionCreated`, `Event::SessionEvent`, `generate_session_id()`
- `crates/prosthetic-conscience/src/router/session_ws.rs` — WS upgrade handler and connection handler for `/v1/sessions`
- `crates/prosthetic-conscience/src/protocol.rs` — `SessionClientMessage`, `SessionGatewayMessage` wire types
- `crates/prosthetic-conscience/src/gateway/channel_registry.rs` — `SubscriberHandle` type alias
- `docs/codebase-state/todo-near-term.md` — session feature intent and implementation process

## Architecture

The session kernel is a fully independent submodule (`gateway::session::kernel`) with its own `Event`, `Effect`, `State`, and `reduce()`. It has no imports from or awareness of the parent kernel.

The parent kernel's `SubId` generic flows directly into the session kernel's `SubId` generic — the same type parameter, enforcing that session subscriber identities are distinct from chat stream identities (`SId`) at compile time.

The parent kernel delegates via `Event::SessionEvent { session_id, event }` — it extracts the session from its `HashMap<SessionId, session::State<SubId>>` using `extract` (which gives owned state without cloning) and passes it to `session::kernel::reduce()`. The updated session state is re-inserted via `update`. This enforces domain isolation structurally: session logic cannot access worker/stream state because it never receives it.

A `SessionEvent` for a non-existent session produces a `ProtocolViolation`. If the event carries a SubId (`Subscribed`, `Unsubscribed`, `SubscriberHeartbeat`), the kernel also emits `SubscriberRemoved` for defensive cleanup — the kernel cannot trust that the impure runtime already cleaned up the subscriber handle. Sessions must be explicitly created before they can receive events.

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

### Parent kernel invariants (session-related)

- **P14 — Universal subscriber cleanup**: every SubId that enters the parent kernel via any event (`SessionRequested`, `SessionEvent::Subscribed`, `SessionEvent::Unsubscribed`, `SessionEvent::SubscriberHeartbeat`) eventually receives a `SubscriberRemoved` effect (given enough ticks). This is enforced by two mechanisms: (1) session-level expiry for subscribers inside sessions, and (2) defensive cleanup in the parent kernel's unknown-session arm, which emits `SubscriberRemoved` for any SubId in a `SessionEvent` targeting a non-existent session.

### Exhaustiveness enforcement

Both the parent kernel's unknown-session cleanup and the P14 test use explicit match arms with no wildcard (`_ =>`) over `session::Event` variants. Adding a new variant that carries a `SubId` without handling it in either location is a compile error. The test uses `#[cfg(test)] Event::sub_ids()` methods (defined adjacent to each enum) for the same purpose.

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

**Registry and effect resolution separation** (done)

- `SubscriberId` newtype in `channel_registry.rs` — distinct from `ClientStreamId`. UUID-based generation.
- `ChannelRegistry` has three maps: `workers` (WorkerId → WorkerHandle), `streams` (ClientStreamId → StreamHandle), `subscribers` (SubscriberId → SubscriberHandle).
- Session effects resolve through the subscriber registry: `SessionCreated` and `NotifySubscribers` use `clone_subscriber` (non-terminal), `SubscriberRemoved` uses `take_subscriber` (terminal — removes handle from registry).
- Chat stream effects resolve through the stream registry as before.
- `StateSnapshot` includes `subscriber_registry_count`.
- 5 subscriber registry unit tests (register unique IDs, clone without removing, clone unknown, take removes, take unknown).

**Transport layer** (done)

- Wire protocol types: `SessionClientMessage` (Create, Subscribe, Append, Heartbeat) and `SessionGatewayMessage` (Subscribed, Entry, SubscriberRemoved, Error) in `protocol.rs`. `Subscribed` now carries `{ session_id, latest_entry_index }`, where `latest_entry_index` is `None` for empty/new sessions and `Some(n)` for the current committed upper bound. 20 serde round-trip tests.
- `SubscriberHandle = mpsc::Sender<SessionGatewayMessage>` type alias in `channel_registry.rs`.
- 5 `RuntimeCommand` variants: `SessionCreate`, `SessionSubscribe`, `SessionAppendEntry`, `SessionSubscriberHeartbeat`, `SessionUnsubscribe`. Each has a handler method on `GatewayRuntime` and a convenience method on `RuntimeHandle`.
- Effect executors in `spawn_effects`: `SessionCreated` sends `Subscribed { latest_entry_index: None }` via handle, `NotifySubscribers` fans out `Entry` to all handles, `SubscriberRemoved` sends removal notice and drops handle.
- WS handler at `/v1/sessions` (`session_ws.rs`): single endpoint with message-based handshake (first message is `Create` or `Subscribe`), connection loop with `tokio::select!` (gateway→client forwarding, client→gateway dispatch, automatic heartbeat tick at 10s), cleanup sends `Unsubscribe` on disconnect. Handshake timeout: 5s.
- Create flow: WS handler registers subscriber, sends `SessionCreate` command, waits for `Subscribed` from mpsc (sent by `SessionCreated` effect executor), then forwards that handshake payload to the client unchanged.
- Subscribe flow: WS handler registers subscriber, sends `SessionSubscribe` command, receives `{ subscriber_id, latest_entry_index }` back from the runtime, and immediately sends `Subscribed` to the client. If the session doesn't exist, the handshake still carries `latest_entry_index: None`, and the kernel emits `SubscriberRemoved` via P14 defensive cleanup, which arrives through mpsc and closes the connection.
- 9 integration tests: create+append, create handshake metadata, subscribe notifications, subscribe handshake metadata, nonexistent session (P14), subscriber timeout, multiple subscribers, disconnect cleanup, handshake timeout.

## Known Issues

### Stream registry orphan on runtime shutdown

A similar (less severe) issue exists for chat streams: if `register_stream` succeeds but the subsequent `http_chat_requested` command fails (runtime channel closed), the stream handle is orphaned in the registry. The kernel never heard about it, so no `SendClientDone` / `take_stream` will clean it up. In practice this only occurs during runtime shutdown (channel closed), so the registry is about to be dropped anyway. Noted for completeness.

## Client-side session consumer

`crates/prosthetic-conscience/src/consensus_cli/session.rs` implements a WS session client (`SessionClient`) used by the `pc-consensus` binary. It connects to `/v1/sessions`, performs the Create/Subscribe handshake, and exposes an async `SessionEvent` stream (Entry, Disconnected, Reconnected, Warning). Uses `tokio_tungstenite` for WS transport.

Key behaviors:

- **Reconnect with backoff**: on WS disconnect, the background task reconnects with exponential backoff (1s → 30s cap). Commands received during disconnect get `SessionError::Disconnected`.
- **Catch-up via HTTP**: after connect/reconnect, the app fetches missing entries via `GET /v1/sessions/:id/entries?after={next_index}&limit=1000` and replays them through the consensus engine.
- **Out-of-order buffering**: entries arriving out of order (index > next_index) are buffered in a `BTreeMap` and drained when the gap is filled.
- **No heartbeat yet**: the client does not send WS ping/pong or application-level heartbeat messages. Dead connections are only detected on the next send attempt.

## Not yet implemented

- `SessionExpired` effect executor (resolve done as pass-through, spawn is no-op stub — persistence adapters not yet wired)
- Session entry replay on subscribe (new subscriber gets no history)
- Cursor-based read improvements (adapter concern)
- Client-side WS heartbeat (ping/pong + application-level heartbeat tick)
