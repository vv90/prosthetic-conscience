# Gateway State Machine (Kernel)

Snapshot date: 2026-03-21

## Behavior

- A pure reducer `reduce(state, event) -> {state, effects}` is implemented under `src/gateway/kernel.rs`.
- Generic over worker/stream ID types (`GatewayState<WId, SId>`, `Event<WId, SId>`, `Effect<WId, SId>`).
- The `reduce` function requires `WId: Clone + Ord + Display` and `SId: Clone + Eq + Hash + Display`.

### Pure immutable design

The kernel is strictly pure and immutable. `reduce` takes owned `state` (not `mut state`) and returns a new `GatewayState` constructed via struct literals. No in-place mutation is used anywhere in the reducer. Every match arm builds a new state value with `..state` for unchanged fields.

This is a non-negotiable design constraint. All future kernel logic must follow the same pattern:

- No `&mut` on state or its fields.
- No `.insert()`, `.remove()`, `.retain()`, or field assignment on state.
- State transitions are expressed as pure constructions: `GatewayState { changed_field: new_value, ..state }`.
- Use `.update()`, `.without()`, `.extract()` and functional pipelines (`into_iter().filter().collect()`) for collection updates.

### Persistent collections

State collections use persistent immutable data structures from the `im` crate with O(1) clone via structural sharing. This enables the pure functional style without performance penalty.

- `im::OrdMap` — used only for `available` (worker pool) where deterministic iteration order is needed for capability-based selection.
- `im::HashMap` / `im::HashSet` — used for `active_streams`, `sessions`, `capabilities`, and session `subscribers`. These do not require ordering and use hash-based collections for O(1) amortized operations.

`BTreeSet<Capability>` is kept at the protocol/API boundary (`protocol.rs`, `RuntimeCommand`). Conversion to `im::HashSet` happens at exactly one point: `handle_register_worker` in `runtime.rs`.

### State

- `tick: u64` -- monotonic tick counter, incremented on each `Tick` event.
- `worker_ttl: u64` -- ticks until an idle worker expires.
- `stream_ttl: u64` -- ticks until an active stream expires.
- `subscriber_ttl: u64` -- ticks until an idle session subscriber expires. Propagated to session state on creation.
- `runtime_id: u128` -- UUID bits, set at runtime startup via `uuid::Uuid::new_v4().as_u128()`. Used for deterministic session ID generation.
- `session_counter: u64` -- monotonic counter for session ID generation. Incremented on each `SessionRequested`.
- `available: OrdMap<WId, WorkerEntry>` -- workers waiting for jobs. `WorkerEntry` contains `deadline: u64` and `capabilities: HashSet<Capability>`.
- `active_streams: HashMap<SId, u64>` -- client streams with jobs in flight, mapped to their deadline tick.
- `sessions: HashMap<SessionId, session::State<SId>>` -- active sessions (see `session-behavior.md`).

Workers are one-use: consumed on dispatch, gone from kernel state. After job completion, the worker handler re-registers with a fresh ID. From the kernel's perspective, it's a new worker.

Deadlines are expressed as tick counts, not wall-clock time. Under channel congestion, ticks are skipped (via `try_send`), so deadlines stretch — the right behavior under load.

### Events (inputs)

| Event                                                                          | Trigger                                     | State change                                                                                                 |
| ------------------------------------------------------------------------------ | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `WorkerRegistered { worker_id, capabilities }`                                 | Worker connects or re-registers after job   | Adds to `available` with deadline `tick + worker_ttl` (rejects duplicates)                                   |
| `HttpChatRequested { client_stream_id, payload, stream, required_capability }` | Client submits a job                        | Removes worker from `available`, adds stream to `active_streams` with deadline                               |
| `AssignmentCleared { client_stream_id }`                                       | Relay reports normal completion             | Removes from `active_streams`, emits `SendClientDone`                                                        |
| `AssignmentFailed { client_stream_id, message }`                               | Dispatch failure, relay error, worker crash | Removes from `active_streams`, emits `SendClientError` + `SendClientDone`                                    |
| `WorkerHeartbeat { worker_id }`                                                | Worker signals liveness                     | Resets worker deadline to `tick + worker_ttl`                                                                |
| `StreamHeartbeat { client_stream_id }`                                         | Stream signals activity                     | Resets stream deadline to `tick + stream_ttl`                                                                |
| `Tick`                                                                         | Timer driven by runtime                     | Increments tick, expires stale workers and timed-out streams                                                 |
| `SessionRequested { subscriber_id }`                                           | WS client requests new session              | Generates deterministic session ID, adds to `sessions` with creator subscribed, increments `session_counter` |
| `SessionEvent { session_id, event }`                                           | Session operation delegated to child kernel | Delegates to `session::kernel::reduce` (see `session-behavior.md`)                                           |

### Effects (outputs)

| Effect                                                             | Purpose                            |
| ------------------------------------------------------------------ | ---------------------------------- |
| `DispatchJob { worker_id, client_stream_id, capability, payload }` | Dispatch job to a worker           |
| `SendClientError { client_stream_id, message }`                    | Send error to client stream        |
| `SendClientDone { client_stream_id }`                              | Signal stream completion to client |
| `SessionCreated { session_id, subscriber_id }`                     | Notify creator of new session ID   |
| `SessionEffect(session::Effect<SId>)`                              | Wrapped session effect             |
| `ProtocolViolation { source: ViolationSource, message }`           | Log invalid behavior               |

### Worker assignment policy

- `first_capable_worker_id` returns the first key from `OrdMap<WId, WorkerEntry>` whose capabilities include the required capability. Selection order is deterministic (OrdMap key order) but not a documented fairness policy.

## Invariants

Universal properties that hold on every reachable state after any event sequence. Each is a property-test target.

| #   | Invariant                                                                                                                       | Status      | Notes                                                                                                   |
| --- | ------------------------------------------------------------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------- |
| I1  | Workers only exist in `available`. Once dispatched, they leave kernel state entirely.                                           | Implemented | One-use IDs. No dual-state possible.                                                                    |
| I2  | Every stream removed from `active_streams` produces client-terminal effects (`SendClientDone`)                                  | Implemented | Via `AssignmentCleared` or `Tick` expiration.                                                           |
| I3  | Every `DispatchJob` is emitted in the same transition that removes worker from `available` and adds stream to `active_streams`  | Implemented | Dispatch and state update are atomic within the reducer. Duplicate stream IDs rejected before dispatch. |
| I4  | No event silently changes state without corresponding effects                                                                   | Implemented | Exceptions: `WorkerRegistered` (safe), `Tick` expiring workers (no client).                             |
| I5  | Every `client_stream_id` that enters the kernel eventually gets terminal effects                                                | Implemented | Via pre-dispatch error, `AssignmentCleared`, or tick expiration.                                        |
| I6  | Dispatch only targets workers whose capabilities include the required capability                                                | Implemented | `first_capable_worker_id` filters by capability.                                                        |
| I7  | `session_counter` never decreases across any transition                                                                         | Implemented | Only `SessionRequested` increments it; all other events leave it unchanged.                             |
| I8  | `sessions.keys()` only grows across transitions (no session is ever removed)                                                    | Implemented | Until Chunk C adds session expiry.                                                                      |
| I9  | `SessionRequested` only modifies `sessions` and `session_counter` — all other state fields unchanged                            | Implemented | Structural isolation.                                                                                   |
| I10 | All session IDs produced across any event sequence are unique (no duplicates in `sessions.keys()`)                              | Implemented | Deterministic `hash(runtime_id, session_counter)` with monotonic counter.                               |
| I11 | Two gateway states with different `runtime_id` values produce strictly disjoint sets of session IDs for the same event sequence | Implemented | Ensures uniqueness across server restarts.                                                              |

## Transition Rules

Specific input-output contracts for each event. Each is a unit-test target.

### `HttpChatRequested`

| Precondition                                                  | Effects                                                                        | State change                                                                    |
| ------------------------------------------------------------- | ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------- |
| `stream=false`                                                | `[SendClientError("stream=true is required"), SendClientDone]`                 | None                                                                            |
| `stream=true`, `client_stream_id` already in `active_streams` | `[SendClientError("stream already has an active assignment"), SendClientDone]` | None                                                                            |
| `stream=true`, at least one capable available worker          | `[DispatchJob { first_capable, stream_id, payload }]`                          | Worker removed from `available`, stream added to `active_streams` with deadline |
| `stream=true`, no capable available worker                    | `[SendClientError("no idle worker available"), SendClientDone]`                | None                                                                            |

### `WorkerRegistered`

| Precondition                       | Effects                                                | State change                              |
| ---------------------------------- | ------------------------------------------------------ | ----------------------------------------- |
| `worker_id` not in `available`     | None                                                   | Worker added to `available` with deadline |
| `worker_id` already in `available` | `[ProtocolViolation("duplicate worker registration")]` | No change                                 |

### `AssignmentCleared`

| Precondition                               | Effects                                                            | State change                         |
| ------------------------------------------ | ------------------------------------------------------------------ | ------------------------------------ |
| `client_stream_id` in `active_streams`     | `[SendClientDone]`                                                 | Stream removed from `active_streams` |
| `client_stream_id` not in `active_streams` | `[ProtocolViolation("assignment cleared for unknown stream ...")]` | No change                            |

### `AssignmentFailed`

| Precondition                               | Effects                                                           | State change                         |
| ------------------------------------------ | ----------------------------------------------------------------- | ------------------------------------ |
| `client_stream_id` in `active_streams`     | `[SendClientError(message), SendClientDone]`                      | Stream removed from `active_streams` |
| `client_stream_id` not in `active_streams` | `[ProtocolViolation("assignment failed for unknown stream ...")]` | No change                            |

### `WorkerHeartbeat`

| Precondition                   | Effects                                                | State change                          |
| ------------------------------ | ------------------------------------------------------ | ------------------------------------- |
| `worker_id` in `available`     | None                                                   | Deadline reset to `tick + worker_ttl` |
| `worker_id` not in `available` | `[ProtocolViolation("heartbeat from unknown worker")]` | No change                             |

### `StreamHeartbeat`

| Precondition                               | Effects                                                   | State change                          |
| ------------------------------------------ | --------------------------------------------------------- | ------------------------------------- |
| `client_stream_id` in `active_streams`     | None                                                      | Deadline reset to `tick + stream_ttl` |
| `client_stream_id` not in `active_streams` | `[ProtocolViolation("heartbeat for unknown stream ...")]` | No change                             |

### `Tick`

| Precondition              | Effects                                                          | State change                                               |
| ------------------------- | ---------------------------------------------------------------- | ---------------------------------------------------------- |
| No expired entries        | None                                                             | `tick` incremented                                         |
| Workers past deadline     | None (stale worker, no client affected)                          | Expired workers removed from `available`                   |
| Streams past deadline     | `[SendClientError("stream timed out"), SendClientDone]` for each | Expired streams removed from `active_streams`              |
| Sessions with subscribers | `SessionEffect(SubscriberRemoved)` for each expired subscriber   | Tick propagated to all sessions, stale subscribers removed |

### `SessionRequested`

| Precondition | Effects                                          | State change                                                                                   |
| ------------ | ------------------------------------------------ | ---------------------------------------------------------------------------------------------- |
| Always       | `[SessionCreated { session_id, subscriber_id }]` | New session added with creator in `subscribers`, `session_counter` incremented. No error path. |

### `SessionEvent`

| Precondition                   | Effects                                                | State change                            |
| ------------------------------ | ------------------------------------------------------ | --------------------------------------- |
| `session_id` in `sessions`     | Wrapped session effects from child reducer             | Session state updated via child reducer |
| `session_id` not in `sessions` | `[ProtocolViolation("event for unknown session ...")]` | No change                               |

## Test Coverage

### ProtocolViolation

`ProtocolViolation` uses a `ViolationSource` enum to identify the origin of the violation:

- `ViolationSource::Worker(String)` — violation from a worker
- `ViolationSource::Stream(String)` — violation from a client stream
- `ViolationSource::Session(String)` — violation from a session

The source uses `String` (not generic ID types) to avoid type complexity at the runtime boundary where effects are resolved from kernel types to channel-handle types.

### Unit tests (57 tests)

| Test                                                              | Covers                                                                        |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| `stream_false_emits_error_and_done`                               | `HttpChatRequested` stream=false                                              |
| `stream_false_does_not_mutate_state`                              | stream=false leaves state unchanged                                           |
| `stream_true_assigns_first_available_worker`                      | `HttpChatRequested` dispatch happy path                                       |
| `stream_true_without_available_worker_emits_error_and_done`       | `HttpChatRequested` no available worker                                       |
| `stream_true_no_workers_does_not_mutate_state`                    | No-worker rejection leaves state unchanged                                    |
| `dispatched_worker_is_consumed_from_available`                    | Worker removed from `available`, stream added to `active_streams`             |
| `dispatch_sets_stream_deadline`                                   | Stream deadline = tick + stream_ttl                                           |
| `registration_sets_worker_deadline`                               | Worker deadline = tick + worker_ttl                                           |
| `registration_stores_capabilities`                                | Capabilities stored on worker entry                                           |
| `second_request_rejected_when_no_workers_available`               | `HttpChatRequested` rejection when sole worker consumed                       |
| `duplicate_stream_id_rejected_with_error_and_done`                | `HttpChatRequested` with already-active stream ID                             |
| `fresh_registration_after_assignment_cleared_allows_new_dispatch` | Full cycle: dispatch -> clear -> re-register -> dispatch                      |
| `assignment_cleared_emits_done`                                   | `AssignmentCleared` on active stream                                          |
| `assignment_cleared_for_unknown_stream_emits_protocol_violation`  | `AssignmentCleared` unknown stream                                            |
| `assignment_cleared_before_timeout_no_timeout_effects`            | Normal completion prevents timeout                                            |
| `assignment_cleared_after_timeout_emits_protocol_violation`       | Late clear after timeout = protocol violation                                 |
| `double_assignment_cleared_second_emits_protocol_violation`       | Second `AssignmentCleared` for same stream = protocol violation               |
| `duplicate_worker_registered_emits_protocol_violation`            | `WorkerRegistered` duplicate                                                  |
| `worker_registration_adds_to_available`                           | `WorkerRegistered` happy path                                                 |
| `multiple_distinct_registrations_all_succeed`                     | Multiple unique registrations all added                                       |
| `worker_heartbeat_resets_deadline`                                | `WorkerHeartbeat` resets deadline                                             |
| `worker_heartbeat_unknown_emits_protocol_violation`               | `WorkerHeartbeat` unknown worker                                              |
| `heartbeat_for_dispatched_worker_emits_protocol_violation`        | Heartbeat for consumed (dispatched) worker                                    |
| `stream_heartbeat_resets_deadline`                                | `StreamHeartbeat` resets deadline                                             |
| `stream_heartbeat_unknown_emits_protocol_violation`               | `StreamHeartbeat` unknown stream                                              |
| `heartbeat_for_cleared_stream_emits_protocol_violation`           | Heartbeat for already-cleared stream                                          |
| `tick_increments_counter`                                         | `Tick` counter monotonicity                                                   |
| `tick_with_no_entries_emits_no_effects`                           | `Tick` no-op when empty                                                       |
| `tick_does_not_expire_entries_before_deadline`                    | `Tick` respects deadlines                                                     |
| `worker_expires_after_ttl_ticks`                                  | Worker expiration after TTL                                                   |
| `stream_expires_after_ttl_ticks`                                  | Stream expiration with terminal effects                                       |
| `heartbeat_prevents_expiration`                                   | Heartbeat resets deadline, prevents expiry                                    |
| `multiple_expirations_in_single_tick`                             | Multiple workers/streams expire in one tick                                   |
| `assignment_failed_emits_error_and_done`                          | `AssignmentFailed` on active stream emits error + done                        |
| `assignment_failed_unknown_stream_emits_protocol_violation`       | `AssignmentFailed` unknown stream = protocol violation                        |
| `double_assignment_failed_second_emits_protocol_violation`        | Second `AssignmentFailed` for same stream = protocol violation                |
| `assignment_failed_before_timeout_no_timeout_effects`             | Early failure prevents later timeout effects                                  |
| `mixed_deadlines_only_expired_entries_removed`                    | Only entries past deadline expire; fresh entries survive                      |
| `worker_expiration_does_not_affect_active_streams`                | Worker/stream expiration independence                                         |
| `stream_expiration_does_not_affect_available_workers`             | Stream/worker expiration independence                                         |
| `zero_ttl_expires_on_next_tick`                                   | TTL=0 entries expire on first tick                                            |
| `tick_preserves_ttl_config`                                       | Tick doesn't corrupt TTL configuration                                        |
| `chat_job_dispatches_to_chat_capable_worker`                      | Chat request dispatches to Chat-capable worker                                |
| `transcription_job_dispatches_to_transcription_capable_worker`    | Transcription request dispatches to Transcription-capable worker              |
| `chat_job_skips_transcription_only_worker`                        | Chat request skips worker without Chat capability                             |
| `no_capable_worker_available_returns_error`                       | No capable worker returns error                                               |
| `selects_capable_worker_when_mixed_pool`                          | Selects correct worker from mixed-capability pool                             |
| `multi_capable_worker_serves_either_job_type`                     | Worker with both capabilities serves either job type                          |
| `session_requested_adds_session`                                  | T1: SessionRequested adds a new session to sessions                           |
| `session_requested_starts_with_creator_subscribed`                | T2: New session starts with creator in subscribers, empty entries             |
| `session_requested_deterministic_id`                              | T3: Generated session ID is deterministic given runtime_id + counter          |
| `session_requested_increments_counter`                            | T4: session_counter increments by 1                                           |
| `session_requested_emits_session_created`                         | T5: Emits exactly one SessionCreated effect with session_id and subscriber_id |
| `t13_tick_propagates_removes_stale_subscriber`                    | T13: Tick propagates to sessions — stale subscriber removed after deadline    |
| `t14_tick_propagates_keeps_fresh_subscriber`                      | T14: Tick propagates to sessions — fresh subscriber kept before deadline      |
| `t15_session_requested_sets_subscriber_ttl_and_deadline`          | T15: SessionRequested sets subscriber_ttl and initial deadline on creator     |

### Property tests (14 tests)

All use `proptest` over arbitrary sequences of up to 100 events from a small ID pool (3 worker IDs, 3 stream IDs, 2 session IDs) to encourage collisions.

| Test                                                   | Invariant verified                                                                              |
| ------------------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| `invariant_i2_stream_removal_produces_done`            | I2: every stream removed from `active_streams` has `SendClientDone` in effects                  |
| `invariant_i3_dispatch_atomicity`                      | I3: every `DispatchJob` removes worker from `available` and adds new stream in `active_streams` |
| `invariant_i4_no_silent_state_changes`                 | I4: stream additions/removals always have corresponding effects                                 |
| `invariant_i5_all_streams_eventually_terminate`        | I5: after draining with ticks, all streams that ever entered kernel got `SendClientDone`        |
| `invariant_i6_dispatch_respects_capability`            | I6: dispatched worker always has the required capability                                        |
| `tick_counter_is_monotonic`                            | Tick counter never decreases                                                                    |
| `stream_timeout_always_emits_error_and_done`           | Every tick-expired stream gets both `SendClientError` and `SendClientDone`                      |
| `every_http_chat_requested_produces_terminal_effects`  | Every request produces either `DispatchJob` or `SendClientError` + `SendClientDone`             |
| `session_counter_never_decreases`                      | I7: `session_counter` never decreases across any transition                                     |
| `sessions_only_grow`                                   | I8: `sessions.keys()` only grows (no session removed)                                           |
| `session_requested_only_modifies_sessions_and_counter` | I9: `SessionRequested` only modifies `sessions` and `session_counter`                           |
| `all_session_ids_are_unique`                           | I10: all session IDs across any event sequence are unique                                       |
| `different_runtime_ids_produce_disjoint_session_ids`   | I11: different `runtime_id` values produce disjoint session ID sets                             |
| `p9_tick_never_increases_total_subscribers`            | P9: subscriber count across all sessions never increases from `Tick`                            |

A proptest regression file at `proptest-regressions/gateway/kernel.txt` captures the minimal case that caught the duplicate stream ID bug (now fixed, replayed on each run).

## Known Issues

None.

## Status

- Implemented as pure immutable reducer with persistent collections (`im::OrdMap`, `im::HashMap`, `im::HashSet`).
- Capability-based worker dispatch.
- Session creation via `SessionRequested` with deterministic ID generation (`hash(runtime_id, session_counter)`).
- `runtime_id` set from UUID at runtime startup, ensuring unique IDs across restarts.
- Session delegation to independent child kernel.
- One-use worker IDs, tick-counted deadlines, heartbeat events, timeout-driven expiration, duplicate stream ID rejection.
- `ProtocolViolation` uses `ViolationSource` enum (Worker/Stream/Session) with string IDs.
- `subscriber_ttl` on state, propagated to sessions on creation.
- `Tick` propagates to all sessions, expiring stale subscribers.
- 57 kernel unit tests covering all transition rules including capability routing, expiration, session creation, and tick propagation to sessions.
- 14 property tests covering invariants I2-I11, P9, tick monotonicity, stream timeout effect pairs, request terminal effects, and session ID uniqueness/isolation.
- Runtime spawns a tick task using `try_send` (skips ticks under congestion).

## Load into context when

- Modifying the reducer or adding new events/effects.
- Writing property-based tests for kernel invariants.
- Reviewing kernel-level state transition correctness.
- Adding adapter-side heartbeat signals.

## Relevant files

- `src/gateway/kernel.rs`
- `src/gateway/runtime.rs`
- `src/gateway/session/kernel.rs`
- `src/gateway/effects/` (`dispatch_job.rs`, `send_client_error.rs`, `send_client_done.rs`, `protocol_violation.rs`)

## TODO (near-term)

- Wire `relay_job` into worker handler (replacing `consume_until_terminal` stub) to activate stream heartbeats and chunk forwarding.
