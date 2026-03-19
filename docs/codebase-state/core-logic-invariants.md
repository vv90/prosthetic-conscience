# Core Logic Invariants

Snapshot date: 2026-03-09

## Design Philosophy

### Kernel as source of truth

The kernel is the single authority for system state. It assumes everything outside is unreliable and can act in unpredictable ways. Adapters (websocket handlers, HTTP handlers, relay, effect executors) may crash, hang, send duplicate messages, send messages out of order, or never send messages at all. The kernel must produce correct effects under all of these conditions.

The kernel's purpose: **match workers with jobs, ensure the correct worker gets the correct job data, ensure every client stream gets a terminal response, and classify all errors.**

### Minimum external requirements

The kernel can fulfill its purpose if and only if:

1. **`WorkerRegistered` events reach the kernel** -- worker supply. Something must register workers; the kernel doesn't care what or how.
2. **`HttpChatRequested` events reach the kernel** -- job demand / stream open. Something must submit client requests; the kernel doesn't care what or how.
3. **The runtime drives a timer** -- the kernel receives periodic `Tick` events. This is the mechanism that lets the kernel enforce stream termination without trusting adapters to deliver completion signals. If completion arrives before timeout, normal path. If not, the kernel times out the assignment and emits terminal effects.
4. **The runtime can execute effects** -- transport data between the worker_id and stream_id referenced in effects. Effect execution is best-effort; the kernel handles failures through the timeout mechanism.

That's it. Everything else (channel types, registry, handler structure, relay implementation) is implementation detail of how the runtime satisfies these four requirements.

### Where logic belongs

| Layer        | Responsibility                                                                                                    | Purity                            |
| ------------ | ----------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| **Kernel**   | All decisions: matching, error classification, stream termination guarantees, timeout policy, assignment tracking | Pure, deterministic, no I/O       |
| **Runtime**  | Dumb plumbing: translate IDs to handles, execute effects, drive timers, forward events. No decisions.             | Impure, but trivial logic         |
| **Adapters** | Transport concerns: websocket framing, SSE formatting, HTTP status codes, re-registration after job completion    | Impure, no system-state decisions |

The principle: **if it's a decision, it belongs in the kernel. If it's I/O, it belongs in the runtime/adapters. The runtime should be so simple that its correctness is obvious by inspection.**

## Scope

The core logic comprises three layers:

1. **Kernel** (`src/gateway/kernel.rs`) -- pure reducer: `reduce(state, event) -> {state, effects}`. All dispatch, lifecycle, and error-classification decisions live here. No I/O, no channels, no async. Generic over ID types.
2. **Runtime** (`src/gateway/runtime.rs`) -- single-threaded message loop that owns kernel state and channel registry. Translates between kernel abstractions (opaque IDs, abstract effects) and concrete handles (oneshot senders, mpsc senders). Serializes all state mutations through one `mpsc::Receiver`.
3. **Channel registry** (`src/gateway/channel_registry.rs`) -- mapping between opaque IDs and communication handles. Controls ID generation (UUIDs). ID constructors are module-private -- external code cannot forge IDs.

Everything outside these three modules is **adapter code**.

## Boundary and Interaction Surface

### Inbound (adapters -> core)

- **`RuntimeHandle`** (clone-friendly, async-safe): the only way adapters talk to the core.
  - `register_worker(handle) -> WorkerId` -- registers a oneshot sender, returns opaque ID.
  - `register_stream(handle) -> ClientStreamId` -- registers a stream sender, returns opaque ID.
  - `submit_command(RuntimeCommand)` -- for forwarding events to the kernel.

### Outbound (core -> adapters)

- **Effects**: the kernel emits `Effect<WId, SId>` values. The runtime resolves them to concrete handles and spawns their execution. Effect structs are the core's way of requesting I/O without performing it.
- **Channel delivery**: the runtime sends `WorkerJob` through oneshot channels and `StreamFrame` through mpsc channels. Adapters receive work through these channels.

### What the core does NOT do

- No network I/O (websocket, HTTP, TLS).
- No direct logging of content (privacy invariant).
- No persistence.
- No clock access (timer is externally driven via `Tick` events).

## Key Guarantee: Stream Termination

**Every `client_stream_id` that enters the kernel via `HttpChatRequested` gets terminal effects.** This is the kernel's central safety property. Every terminal path ends with `SendClientDone`, which sends a `[DONE]` frame and closes the stream. This matches the OpenAI SSE convention.

Terminal patterns:

- **Success** (`AssignmentCleared`): `SendClientDone`.
- **Failure** (`AssignmentFailed`, tick expiration, or pre-dispatch rejection): `SendClientError` + `SendClientDone`.

Pre-dispatch rejections (stream=false, duplicate stream ID, no capable worker) emit terminal effects immediately in the same transition. Post-dispatch terminations remove the stream from `active_streams` in the same transition.

Property test coverage: `invariant_i2_stream_removal_produces_done` checks post-dispatch termination. `every_http_chat_requested_produces_terminal_effects` checks that every request produces either `DispatchJob` (post-dispatch) or `SendClientError` + `SendClientDone` (pre-dispatch rejection).

The channel is the cancellation mechanism: when the kernel emits terminal effects for a stream, the effect executor delivers them and drops the client-side receiver. The relay's next `client_tx.send()` fails, relay stops, worker handler detects the dead channel and either re-registers or exits. The kernel doesn't kill the relay directly -- it kills the stream, and the relay dies as a consequence.

## Invariants

Universal properties that hold across all reachable states. Organized by layer.

### Structural (type-level)

Enforced at compile time. Violation is a compilation error.

| #   | Invariant                                    | Enforcement mechanism                                                                                                                 |
| --- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| S1  | A worker has at most one in-flight job       | Worker handle is `oneshot::Sender<WorkerJob>`. Consumed on use -- cannot send twice. Worker is removed from kernel state on dispatch. |
| S2  | One job delivered per dispatch cycle         | Worker handle is `oneshot::Sender<WorkerJob>`. Consumed on use -- cannot send twice.                                                  |
| S3  | External code cannot forge worker/stream IDs | `WorkerId::new` and `ClientStreamId::new` are module-private. Only `register_worker`/`register_stream` produce IDs.                   |
| S4  | All state mutations are serialized           | Runtime owns a single `mpsc::Receiver<RuntimeMessage>`. One consumer, one message at a time.                                          |

### Kernel

See `gateway-state-machine.md` for the full list. These are properties of the reducer's output -- checkable after any event applied to any reachable state.

**Current invariants (implemented):**

- **I1**: Workers only exist in `available`. Once dispatched, they leave kernel state entirely. No dual-state possible. Duplicate `WorkerRegistered` rejected.
- **I2**: Every stream removed from `active_streams` produces client-terminal effects (`SendClientDone`).
- **I3**: Every `DispatchJob` is emitted in the same transition that removes the worker from `available` and adds the stream to `active_streams`. Duplicate stream IDs are rejected before dispatch (no silent overwrite).
- **I4**: No silent state changes without corresponding effects.
- **I5**: Every `client_stream_id` that enters the kernel eventually gets terminal effects -- either immediately (pre-dispatch error), via `AssignmentCleared` (normal completion), or via tick expiration (timeout).

All five invariants are covered by property tests over arbitrary event sequences.

### Runtime

| #   | Invariant                                                          | Notes                                                                                                                                          |
| --- | ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| R1  | Kernel state and registry are eventually consistent                | Stale entries self-heal: dispatch fails at `take_worker` -> `AssignmentCleared` fallback -> kernel handles it. Worker entries expire via tick. |
| R2  | Every effect resolution failure produces a corrective kernel event | `take_worker` returns `None` -> `AssignmentCleared` fallback generated.                                                                        |

### Registry

| #   | Invariant                                                     | Notes                                                         |
| --- | ------------------------------------------------------------- | ------------------------------------------------------------- |
| G1  | Every registered ID is unique                                 | UUID generation.                                              |
| G2  | `take_worker` is the only way to obtain a handle for dispatch | Ensures handle is removed on use, preventing double-dispatch. |

## Guarantees for Adapter Code

Adapter code can rely on:

1. **IDs are opaque and unforgeable.** Adapters receive IDs from registration and pass them back. They cannot construct IDs.
2. **One job at a time per worker.** Oneshot channel enforces this structurally.
3. **The kernel handles all error classification.** Adapters report what happened; the kernel decides what to tell the client.
4. **Stream termination is kernel-guaranteed.** Adapters do not need to ensure clients get a response -- the kernel does, via completion signals or timeout.

Adapter code is responsible for:

1. **Forwarding events to the kernel** -- registration, completion signals. Best-effort; timeout covers failures.
2. **Executing effects** -- delivering data between the IDs specified in effects. Best-effort; kernel doesn't depend on success.
3. **Re-registering after job completion** -- worker handlers register a fresh oneshot for the next job.
4. **Transport-level concerns** -- websocket framing, SSE formatting, HTTP status codes.
5. **Privacy** -- never logging or persisting prompt/completion content.

Note: adapters do **not** need to send unregister commands, guarantee delivery of completion signals, or handle retry/ordering. The kernel tolerates all adapter failures through timeout.

## Current vs Target Architecture

### Current state

- **One-use worker IDs implemented.** Worker registers, gets dispatched, is consumed. No Idle/Busy lifecycle. After job completion, worker handler re-registers with a fresh ID and fresh oneshot. From the kernel's perspective, it's a new worker.
- **Kernel state**: `available: BTreeMap<WId, u64>` (workers waiting, deadline tick) + `active_streams: BTreeMap<SId, u64>` (streams with jobs in flight, deadline tick). Orthogonal collections (different ID types).
- `AssignmentCleared { client_stream_id }` event removes stream from `active_streams` and emits `SendClientDone` (success path).
- `AssignmentFailed { client_stream_id, message }` event removes stream from `active_streams` and emits `SendClientError` + `SendClientDone` (failure path).
- Runtime forwards `AssignmentCleared` commands to kernel and resolves effects. No `RelayOutcome` mapping, no `WorkerJobCompleted`, no `set_worker_handle`.
- **Tick-counted deadlines.** Deadlines are tick counts, not wall-clock time. Under congestion, ticks are skipped (via `try_send`), so deadlines stretch.
- **Timeout-driven stream termination.** `Tick` event expires stale workers (silently removed from `available`) and timed-out streams (emits `SendClientError` + `SendClientDone`).
- **Heartbeat events.** `WorkerHeartbeat` and `StreamHeartbeat` reset deadlines.
- **Worker heartbeat wired.** Worker ws handler sends `WorkerHeartbeat` every 15 seconds while idle via `tokio::time::interval_at`. Interval resets after re-registration.
- **Stream heartbeat wired.** `relay_job` sends `StreamHeartbeat` every 10 seconds while relaying chunks. Not yet active (relay not wired into worker handler — `consume_until_terminal` stub is used). Heartbeat failure is non-fatal (ignored with `let _ =`; timeout is the correct fallback).
- **Duplicate stream ID rejection.** `HttpChatRequested` with a `client_stream_id` already in `active_streams` is rejected with `SendClientError` + `SendClientDone`. No silent overwrite.
- **Completion signals are optional optimization.** If `AssignmentCleared` arrives before timeout, the stream is cleared early. If it never arrives, timeout handles it.

### Target state (remaining work)

- **Channel-based cancellation.** Implemented. Timeout terminal effects -> `take_stream` removes registry entry -> effect executor drops handle -> channel closes -> relay's `client_tx.send()` fails -> relay stops -> worker re-registers or exits. No explicit cancel command needed.
- **Adapter-side heartbeat signals.** Worker ws handler and stream relay need to periodically send heartbeat commands to the runtime.

## Known Bugs

None.

## Status

- Core kernel: implemented with one-use worker IDs, tick-counted deadlines, heartbeat events, timeout expiration, duplicate stream ID rejection. 44 unit tests + 6 property tests (I2-I5, tick monotonicity, stream timeout error+done pair).
- Runtime: implemented. `AssignmentCleared`, `AssignmentFailed`, `WorkerHeartbeat`, and `StreamHeartbeat` commands, tick task with `try_send`. No `WorkerJobCompleted` or `set_worker_handle`.
- Channel registry: implemented. `register_worker` + `take_worker` only.
- Effect execution: four effect executors implemented. `DispatchJob` constructs `WorkerJob` (passing `Capability` through) and sends via oneshot; signals `AssignmentFailed` on dispatch failure. `ProtocolViolation` logs via `tracing::warn!`. `SendClientError` sends `StreamFrame::Error` via cloned handle. `SendClientDone` sends `StreamFrame::Done` and drops the taken handle.
- Stream handle resolution: `resolve_effects` resolves both worker IDs and stream IDs. Resolved type is `Effect<WorkerHandle, (ClientStreamId, StreamHandle)>`. Non-terminal effects (`SendClientError`) use `clone_stream`; terminal effects (`SendClientDone`) use `take_stream` to remove the registry entry so the channel closes when the handle is dropped.
- Adapter-side heartbeat signals: worker heartbeat wired (15s while idle), stream heartbeat wired in `relay_job` (10s while relaying, now active).

## Load into context when

- Designing new features that interact with the core.
- Reviewing whether an invariant is enforced or only documented.
- Planning property tests.
- Assessing the impact of a change on system-wide guarantees.
- Planning the refactor to one-use worker IDs or timeout mechanism.

## Relevant files

- `src/gateway/kernel.rs`
- `src/gateway/runtime.rs`
- `src/gateway/channel_registry.rs`
- `src/gateway/effects/` (effect struct definitions)
- `src/gateway/relay.rs` (defines `RelayOutcome` and `StreamFrame`)
