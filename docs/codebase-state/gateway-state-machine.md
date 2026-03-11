# Gateway State Machine (Kernel)

Snapshot date: 2026-03-09

## Behavior

- A pure reducer `reduce(state, event) -> {state, effects}` is implemented under `src/gateway/kernel.rs`.
- Generic over worker/stream ID types (`GatewayState<WId, SId>`, `Event<WId, SId>`, `Effect<WId, SId>`).
- The `reduce` function requires `WId: Clone + Ord + Display` and `SId: Clone + Ord + Display`.

### State

- `tick: u64` -- monotonic tick counter, incremented on each `Tick` event.
- `worker_ttl: u64` -- ticks until an idle worker expires.
- `stream_ttl: u64` -- ticks until an active stream expires.
- `available: BTreeMap<WId, u64>` -- workers waiting for jobs, mapped to their deadline tick.
- `active_streams: BTreeMap<SId, u64>` -- client streams with jobs in flight, mapped to their deadline tick.

Workers are one-use: consumed on dispatch, gone from kernel state. After job completion, the worker handler re-registers with a fresh ID. From the kernel's perspective, it's a new worker.

Deadlines are expressed as tick counts, not wall-clock time. Under channel congestion, ticks are skipped (via `try_send`), so deadlines stretch — the right behavior under load.

### Events (inputs)

| Event                                                     | Trigger                                     | State change                                                                   |
| --------------------------------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------ |
| `WorkerRegistered { worker_id }`                          | Worker connects or re-registers after job   | Adds to `available` with deadline `tick + worker_ttl` (rejects duplicates)     |
| `HttpChatRequested { client_stream_id, payload, stream }` | Client submits a job                        | Removes worker from `available`, adds stream to `active_streams` with deadline |
| `AssignmentCleared { client_stream_id }`                  | Relay reports normal completion             | Removes from `active_streams`, emits `SendClientDone`                          |
| `AssignmentFailed { client_stream_id, message }`          | Dispatch failure, relay error, worker crash | Removes from `active_streams`, emits `SendClientError` + `SendClientDone`      |
| `WorkerHeartbeat { worker_id }`                           | Worker signals liveness                     | Resets worker deadline to `tick + worker_ttl`                                  |
| `StreamHeartbeat { client_stream_id }`                    | Stream signals activity                     | Resets stream deadline to `tick + stream_ttl`                                  |
| `Tick`                                                    | Timer driven by runtime                     | Increments tick, expires stale workers and timed-out streams                   |

### Effects (outputs)

| Effect                                                 | Purpose                            |
| ------------------------------------------------------ | ---------------------------------- |
| `DispatchJob { worker_id, client_stream_id, payload }` | Dispatch job to a worker           |
| `SendClientError { client_stream_id, message }`        | Send error to client stream        |
| `SendClientDone { client_stream_id }`                  | Signal stream completion to client |
| `CloseStream { client_stream_id }`                     | Close client connection            |
| `ProtocolViolation { worker_description, message }`    | Log invalid behavior               |

### Worker assignment policy

- `first_available_worker_id` returns the first key from `BTreeMap<WId, u64>`. Selection order is deterministic (BTreeMap key order) but not a documented fairness policy.

## Invariants

Universal properties that hold on every reachable state after any event sequence. Each is a property-test target.

| #   | Invariant                                                                                                                      | Status      | Notes                                                                                                   |
| --- | ------------------------------------------------------------------------------------------------------------------------------ | ----------- | ------------------------------------------------------------------------------------------------------- |
| I1  | Workers only exist in `available`. Once dispatched, they leave kernel state entirely.                                          | Implemented | One-use IDs. No dual-state possible.                                                                    |
| I2  | Every stream removed from `active_streams` produces client-terminal effects (`SendClientDone`)                                 | Implemented | Via `AssignmentCleared` or `Tick` expiration.                                                           |
| I3  | Every `DispatchJob` is emitted in the same transition that removes worker from `available` and adds stream to `active_streams` | Implemented | Dispatch and state update are atomic within the reducer. Duplicate stream IDs rejected before dispatch. |
| I4  | No event silently changes state without corresponding effects                                                                  | Implemented | Exceptions: `WorkerRegistered` (safe), `Tick` expiring workers (no client).                             |
| I5  | Every `client_stream_id` that enters the kernel eventually gets terminal effects                                               | Implemented | Via pre-dispatch error, `AssignmentCleared`, or tick expiration.                                        |

## Transition Rules

Specific input-output contracts for each event. Each is a unit-test target.

### `HttpChatRequested`

| Precondition                                                  | Effects                                                                     | State change                                                                    |
| ------------------------------------------------------------- | --------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `stream=false`                                                | `[SendClientError("stream=true is required"), CloseStream]`                 | None                                                                            |
| `stream=true`, `client_stream_id` already in `active_streams` | `[SendClientError("stream already has an active assignment"), CloseStream]` | None                                                                            |
| `stream=true`, at least one available worker                  | `[DispatchJob { first_available, stream_id, payload }]`                     | Worker removed from `available`, stream added to `active_streams` with deadline |
| `stream=true`, no available worker                            | `[SendClientError("no idle worker available"), CloseStream]`                | None                                                                            |

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

| Precondition          | Effects                                                          | State change                                  |
| --------------------- | ---------------------------------------------------------------- | --------------------------------------------- |
| No expired entries    | None                                                             | `tick` incremented                            |
| Workers past deadline | None (stale worker, no client affected)                          | Expired workers removed from `available`      |
| Streams past deadline | `[SendClientError("stream timed out"), SendClientDone]` for each | Expired streams removed from `active_streams` |

## Test Coverage

### Unit tests (44 tests)

| Test                                                              | Covers                                                            |
| ----------------------------------------------------------------- | ----------------------------------------------------------------- |
| `stream_false_emits_error_and_close`                              | `HttpChatRequested` stream=false                                  |
| `stream_false_does_not_mutate_state`                              | stream=false leaves state unchanged                               |
| `stream_true_assigns_first_available_worker`                      | `HttpChatRequested` dispatch happy path                           |
| `stream_true_without_available_worker_emits_error_and_close`      | `HttpChatRequested` no available worker                           |
| `stream_true_no_workers_does_not_mutate_state`                    | No-worker rejection leaves state unchanged                        |
| `dispatched_worker_is_consumed_from_available`                    | Worker removed from `available`, stream added to `active_streams` |
| `dispatch_sets_stream_deadline`                                   | Stream deadline = tick + stream_ttl                               |
| `registration_sets_worker_deadline`                               | Worker deadline = tick + worker_ttl                               |
| `second_request_rejected_when_no_workers_available`               | `HttpChatRequested` rejection when sole worker consumed           |
| `duplicate_stream_id_rejected_with_error_and_close`               | `HttpChatRequested` with already-active stream ID                 |
| `fresh_registration_after_assignment_cleared_allows_new_dispatch` | Full cycle: dispatch -> clear -> re-register -> dispatch          |
| `assignment_cleared_emits_done`                                   | `AssignmentCleared` on active stream                              |
| `assignment_cleared_for_unknown_stream_emits_protocol_violation`  | `AssignmentCleared` unknown stream                                |
| `double_assignment_cleared_second_emits_protocol_violation`       | Second `AssignmentCleared` for same stream = protocol violation   |
| `duplicate_worker_registered_emits_protocol_violation`            | `WorkerRegistered` duplicate                                      |
| `worker_registration_adds_to_available`                           | `WorkerRegistered` happy path                                     |
| `multiple_distinct_registrations_all_succeed`                     | Multiple unique registrations all added                           |
| `worker_heartbeat_resets_deadline`                                | `WorkerHeartbeat` resets deadline                                 |
| `worker_heartbeat_unknown_emits_protocol_violation`               | `WorkerHeartbeat` unknown worker                                  |
| `heartbeat_for_dispatched_worker_emits_protocol_violation`        | Heartbeat for consumed (dispatched) worker                        |
| `stream_heartbeat_resets_deadline`                                | `StreamHeartbeat` resets deadline                                 |
| `stream_heartbeat_unknown_emits_protocol_violation`               | `StreamHeartbeat` unknown stream                                  |
| `heartbeat_for_cleared_stream_emits_protocol_violation`           | Heartbeat for already-cleared stream                              |
| `tick_increments_counter`                                         | `Tick` counter monotonicity                                       |
| `tick_with_no_entries_emits_no_effects`                           | `Tick` no-op when empty                                           |
| `tick_does_not_expire_entries_before_deadline`                    | `Tick` respects deadlines                                         |
| `worker_expires_after_ttl_ticks`                                  | Worker expiration after TTL                                       |
| `stream_expires_after_ttl_ticks`                                  | Stream expiration with terminal effects                           |
| `heartbeat_prevents_expiration`                                   | Heartbeat resets deadline, prevents expiry                        |
| `multiple_expirations_in_single_tick`                             | Multiple workers/streams expire in one tick                       |
| `assignment_cleared_before_timeout_no_timeout_effects`            | Normal completion prevents timeout                                |
| `assignment_cleared_after_timeout_emits_protocol_violation`       | Late clear after timeout = protocol violation                     |
| `assignment_failed_emits_error_and_done`                          | `AssignmentFailed` on active stream emits error + done            |
| `assignment_failed_unknown_stream_emits_protocol_violation`       | `AssignmentFailed` unknown stream = protocol violation            |
| `double_assignment_failed_second_emits_protocol_violation`        | Second `AssignmentFailed` for same stream = protocol violation    |
| `assignment_failed_before_timeout_no_timeout_effects`             | Early failure prevents later timeout effects                      |
| `mixed_deadlines_only_expired_entries_removed`                    | Only entries past deadline expire; fresh entries survive          |
| `worker_expiration_does_not_affect_active_streams`                | Worker/stream expiration independence                             |
| `stream_expiration_does_not_affect_available_workers`             | Stream/worker expiration independence                             |
| `zero_ttl_expires_on_next_tick`                                   | TTL=0 entries expire on first tick                                |
| `tick_preserves_ttl_config`                                       | Tick doesn't corrupt TTL configuration                            |

### Property tests (6 tests)

All use `proptest` over arbitrary sequences of up to 100 events from a small ID pool (3 worker IDs, 3 stream IDs) to encourage collisions.

| Test                                            | Invariant verified                                                                              |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `invariant_i2_stream_removal_produces_done`     | I2: every stream removed from `active_streams` has `SendClientDone` in effects                  |
| `invariant_i3_dispatch_atomicity`               | I3: every `DispatchJob` removes worker from `available` and adds new stream to `active_streams` |
| `invariant_i4_no_silent_state_changes`          | I4: stream additions/removals always have corresponding effects                                 |
| `invariant_i5_all_streams_eventually_terminate` | I5: after draining with ticks, all streams that ever entered kernel got `SendClientDone`        |
| `tick_counter_is_monotonic`                     | Tick counter never decreases                                                                    |
| `stream_timeout_always_emits_error_and_done`    | Every tick-expired stream gets both `SendClientError` and `SendClientDone`                      |

A proptest regression file at `proptest-regressions/gateway/kernel.txt` captures the minimal case that caught the duplicate stream ID bug (now fixed, replayed on each run).

## Known Issues

None.

## Status

- Implemented with one-use worker IDs, tick-counted deadlines, heartbeat events, timeout-driven expiration, and duplicate stream ID rejection.
- 44 kernel unit tests covering all transition rules including expiration scenarios.
- 6 property tests covering invariants I2-I5, tick monotonicity, and stream timeout effect pairs.
- Runtime spawns a tick task using `try_send` (skips ticks under congestion).

## Load into context when

- Modifying the reducer or adding new events/effects.
- Writing property-based tests for kernel invariants.
- Reviewing kernel-level state transition correctness.
- Adding adapter-side heartbeat signals.

## Relevant files

- `src/gateway/kernel.rs`
- `src/gateway/runtime.rs`
- `src/gateway/effects/` (`dispatch_job.rs`, `send_client_error.rs`, `send_client_done.rs`, `close_stream.rs`, `protocol_violation.rs`)

## TODO (near-term)

- Wire `relay_job` into worker handler (replacing `consume_until_terminal` stub) to activate stream heartbeats and chunk forwarding.
