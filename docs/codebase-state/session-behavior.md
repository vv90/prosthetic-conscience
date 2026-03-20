# Session Behavior

Snapshot date: 2026-03-20

Status: **partial** — session kernel reduce logic implemented, session creation implemented in parent kernel. HTTP/WS adapters and runtime commands not yet wired.

Load into session context when working on: session kernel logic, session HTTP/WS adapters, session-related tests.

## Relevant files

- `src/gateway/session/kernel.rs` — `State<SubId>`, `Event<SubId>`, `Effect<SubId>`, `reduce()`
- `src/gateway/session/mod.rs` — module declaration
- `src/gateway/kernel.rs` — `SessionId`, `sessions` field on `GatewayState`, `Event::CreateSession`, `Event::SessionEvent`
- `docs/codebase-state/todo-near-term.md` — session feature intent and implementation process

## Architecture

The session kernel is a fully independent submodule (`gateway::session::kernel`) with its own `Event`, `Effect`, `State`, and `reduce()`. It has no imports from or awareness of the parent kernel.

Sessions must be explicitly created via `Event::CreateSession { session_id }` before they can receive events. A `SessionEvent` for a non-existent session produces a `ProtocolViolation`.

The parent kernel delegates via `Event::SessionEvent { session_id, event }` — it extracts the session from its `HashMap<SessionId, session::State<SId>>` using `extract` (which gives owned state without cloning) and passes it to `session::kernel::reduce()`. The updated session state is re-inserted via `update`. This enforces domain isolation structurally: session logic cannot access worker/stream state because it never receives it.

## Types

- `SessionId(String)` — concrete newtype in parent `kernel.rs`. The parent's map key.
- `State<SubId: Clone + Eq + Hash>` — `{ entries: AppendLog, subscribers: HashSet<SubId> }`. Defined in `session::kernel`. Generic over subscriber identity type. Uses `im::HashSet` for immutable subscriber management.
- Entry index in the vec is the sequence number. No explicit `seq` field.

## Session kernel events

- `EntryAppended { payload }` — an entry was appended.
- `Subscribed { subscriber_id }` — a client subscribed for push notifications.
- `Unsubscribed { subscriber_id }` — a client unsubscribed.

Session creation is the parent kernel's responsibility via `Event::CreateSession` (inserting into its `HashMap`). No `session_id` on session events — the parent selects which session to delegate to.

## Session kernel effects

- `NotifySubscribers { entry_index, payload, subscribers }` — fan out a new entry to all current subscribers. Best-effort push; the kernel does not retry or track delivery.

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

## Implemented

- Session kernel `reduce()` with three arms: `EntryAppended`, `Subscribed`, `Unsubscribed`
- Parent kernel `CreateSession` event with duplicate detection
- Parent kernel `SessionEvent` delegation to child reducer
- 9 session property tests (S6-S11 + 3 foundational) using `arb_state()` generator
- 5 CreateSession unit tests (C1-C5) in parent kernel

## Not yet implemented

- HTTP/WS adapters for session operations
- Runtime commands for session events (CreateSession, SessionEvent)
- Cursor-based read (pure vec slice, adapter concern)
- Session completion/removal (future event, not yet defined)
