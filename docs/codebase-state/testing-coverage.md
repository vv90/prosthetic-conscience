# Testing Coverage and Methodology

Snapshot date: 2026-03-16

## Overview

Testing is concentrated in the pure kernel layer, which has strong unit and property-based coverage. Integration testing has begun — the test harness is implemented and the happy path test passes. This document catalogs what is tested, what is not, and the planned approach for closing the remaining gaps.

## Current Coverage

### Kernel unit tests (44 tests)

Location: `src/gateway/kernel.rs` (inline `#[cfg(test)]` module).

All transition rules from `gateway-state-machine.md` are covered:

| Area                | Count | What is tested                                                                                                                                                                                        |
| ------------------- | ----- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `HttpChatRequested` | 9     | stream=false rejection, no-worker rejection, duplicate stream ID rejection, state immutability on rejection, worker selection and consumption, deadline assignment                                    |
| `WorkerRegistered`  | 3     | Successful registration, duplicate rejection, multiple distinct registrations                                                                                                                         |
| `AssignmentCleared` | 3     | Normal clear, unknown stream, double clear                                                                                                                                                            |
| `AssignmentFailed`  | 4     | Normal failure, unknown stream, double failure, early failure prevents timeout                                                                                                                        |
| Heartbeats          | 6     | Worker/stream heartbeat resets deadline, unknown ID, stale heartbeat after clear/dispatch                                                                                                             |
| Tick and timeout    | 12    | Counter increment, worker/stream expiration after TTL, heartbeat prevents expiration, multiple expirations in one tick, mixed deadlines, zero TTL edge case, independence of worker/stream expiration |
| Full lifecycle      | 7     | dispatch→clear→re-register→dispatch, dispatch→fail→next request, timeout after clear (no duplicate effects)                                                                                           |

### Kernel property tests (6 tests)

Location: `src/gateway/kernel.rs` (inline, using `proptest`).

All use arbitrary sequences of up to 100 events drawn from a small ID pool (3 worker IDs, 3 stream IDs) to encourage collisions.

| Test                                            | Invariant                                                                               |
| ----------------------------------------------- | --------------------------------------------------------------------------------------- |
| `invariant_i2_stream_removal_produces_done`     | Every stream removed from `active_streams` has `SendClientDone` in effects              |
| `invariant_i3_dispatch_atomicity`               | Every `DispatchJob` removes worker from `available` and adds stream to `active_streams` |
| `invariant_i4_no_silent_state_changes`          | Stream additions/removals always have corresponding effects                             |
| `invariant_i5_all_streams_eventually_terminate` | After draining with ticks, all streams that ever entered kernel got `SendClientDone`    |
| `tick_counter_is_monotonic`                     | Tick counter never decreases                                                            |
| `stream_timeout_always_emits_error_and_done`    | Every tick-expired stream gets both `SendClientError` and `SendClientDone`              |

Regression file: `proptest-regressions/gateway/kernel.txt` captures the minimal case that caught the duplicate stream ID bug (now fixed, replayed on each run).

### Channel registry tests (6 tests)

Location: `src/gateway/channel_registry.rs` (inline `#[cfg(test)]` module).

| Test                       | What is tested                     |
| -------------------------- | ---------------------------------- |
| Worker/stream registration | Returns unique IDs                 |
| `take_worker`              | Removes handle (consumed on use)   |
| `clone_stream`             | Preserves entry                    |
| `take_stream`              | Removes entry                      |
| Property test              | All registered handles retrievable |

### Response assembler tests (11 unit + 4 property = 15 tests)

Location: `src/client/response_assembler.rs` (inline `#[cfg(test)]` module).

| Area             | Count | What is tested                                                                                       |
| ---------------- | ----- | ---------------------------------------------------------------------------------------------------- |
| Content assembly | 2     | Single chunk, multiple chunks concatenation                                                          |
| Tool calls       | 3     | Single-chunk llama-server style, fragmented arguments OpenAI style, multiple concurrent tool calls   |
| Mixed            | 1     | Content + tool calls in same response                                                                |
| Edge cases       | 3     | Empty delta chunks, missing finish_reason, empty input                                               |
| Error paths      | 2     | Missing `choices` array, empty `choices` array                                                       |
| Property: P1     | 1     | Content concatenation is split-invariant (any string split into N fragments reassembles identically) |
| Property: P2     | 1     | Arguments concatenation is split-invariant                                                           |
| Property: P3     | 1     | Tool call count is preserved regardless of delta interleaving                                        |
| Property: P4     | 1     | Finish reason captured from whichever chunk has one                                                  |

### Tool trait and registry tests (5 tests)

Location: `src/client/tools/mod.rs` (inline `#[cfg(test)]` module).

| Test                         | What is tested                                       |
| ---------------------------- | ---------------------------------------------------- |
| `register_and_get`           | Register tool, look up by name, unknown returns None |
| `execute_known_tool`         | Registry dispatches to correct tool                  |
| `execute_unknown_tool`       | Unknown tool name returns `ToolError::UnknownTool`   |
| `definitions_returns_openai` | OpenAI format: `{type: "function", function: {...}}` |
| `duplicate_overwrites`       | Re-registering same name replaces the previous tool  |

### GetCurrentTime tool tests (4 tests)

Location: `src/client/tools/current_time.rs` (inline `#[cfg(test)]` module).

| Test                  | What is tested                               |
| --------------------- | -------------------------------------------- |
| `returns_iso8601_utc` | Output matches `YYYY-MM-DDTHH:MM:SSZ` format |
| `definition_name`     | Tool name is `get_current_time`              |
| `days_to_date_epoch`  | Day 0 → 1970-01-01                           |
| `days_to_date_known`  | Day 20528 → 2026-03-16                       |

### ShellTool tests (4 run + 4 ignored = 8 tests)

Location: `src/client/tools/shell.rs` (inline `#[cfg(test)]` module).

Non-Docker (always run):

| Test                                    | What is tested                                         |
| --------------------------------------- | ------------------------------------------------------ |
| `definition_has_correct_name`           | Tool name is `execute_shell`                           |
| `definition_parameters_require_command` | JSON schema has `command` in `required`                |
| `missing_command`                       | `execute({})` returns `InvalidArguments`               |
| `non_string_command`                    | `execute({"command": 123})` returns `InvalidArguments` |
| `truncate_output_short`                 | Output under limit passes through unchanged            |
| `truncate_output_over_limit`            | Output over limit is truncated with message            |
| `format_output_stdout_only`             | No stderr section when stderr is empty                 |
| `format_output_with_stderr`             | Both stdout and stderr sections present                |

Docker-dependent (`#[ignore]`, require `pc-test-sandbox` container):

| Test                    | What is tested                                      |
| ----------------------- | --------------------------------------------------- |
| `executes_echo`         | `echo hello` → exit code 0, stdout contains "hello" |
| `captures_stderr`       | `echo err >&2` → stderr section present             |
| `nonzero_exit_code`     | `false` → exit code 1                               |
| `timeout_returns_error` | `sleep 60` with 1s timeout → `ExecutionFailed`      |

### Protocol serde tests (14 tests)

Location: `src/protocol.rs` (inline `#[cfg(test)]` module).

| Area              | Count | What is tested                                                                                                        |
| ----------------- | ----- | --------------------------------------------------------------------------------------------------------------------- |
| `WorkerMessage`   | 8     | Round-trip for Chunk/End/Error, from-JSON parsing, missing message field defaults, unknown/missing type, invalid JSON |
| `GatewayToWorker` | 2     | Round-trip for Job, wire format matches legacy `json!()` output                                                       |
| `ChatRequest`     | 4     | stream=true, stream=false, stream absent, all fields preserved in payload                                             |

### Integration tests (12 tests)

Location: `tests/integration.rs` with helpers in `tests/support/`.

| Test                                                | What it proves                                                                                                                                                                                                                                        |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `happy_path_streams_chunks_and_done`                | Full pipeline: HTTP POST → kernel dispatch → WS job frame → 2 worker chunks → relay → SSE events → client receives 2 data + `[DONE]`                                                                                                                  |
| `no_workers_returns_sse_error`                      | No workers connected → kernel rejects with `SendClientError` + `CloseStream` → client sees error event, no `[DONE]`                                                                                                                                   |
| `worker_error_sends_error_and_done`                 | Worker sends chunk then `{"type":"error"}` → relay sends error directly + handler calls `assignment_failed` → client sees chunk + 2 errors + `[DONE]`                                                                                                 |
| `worker_re_registration_handles_second_job`         | Worker completes first job, re-registers with fresh ID, receives and completes second job on same WS connection                                                                                                                                       |
| `concurrent_streams_no_cross_contamination`         | 10 workers, 10 clients, 20 chunks each — all spawned concurrently. Each chunk tagged with `client_stream_id:index`. Asserts: no cross-contamination, chunk ordering preserved, all 10 stream IDs distinct                                             |
| `completed_streams_drain_from_state`                | 3 jobs (success, error, timeout) then assert all state drains: active_streams=0, stream_registry=0, available_workers=0, worker_registry=0                                                                                                            |
| `stream_timeout_sends_error_and_done`               | Gateway with short TTLs (`stream_ttl: 3, tick_interval: 50ms`), worker accepts job but never responds → client gets `"stream timed out"` error + `[DONE]`                                                                                             |
| `long_running_stream_survives_with_heartbeats`      | Short TTL + fast heartbeat (`stream_heartbeat_interval: 100ms`). Worker sends chunk, waits 250ms past original TTL, sends second chunk + end → both chunks arrive, no timeout. Proves heartbeats work                                                 |
| `worker_disconnect_mid_stream_sends_error_and_done` | Worker sends 1 chunk then closes WS → relay detects disconnect → `AssignmentFailed` → client receives chunk, error event, `[DONE]`                                                                                                                    |
| `gateway_client_collects_chunks_and_assembles`      | `GatewayClient` sends request, collects 4 SSE chunks, `response_assembler::assemble()` produces correct `CompletedMessage` with content "Hello there" and finish_reason "stop"                                                                        |
| `gateway_client_returns_error_on_no_workers`        | `GatewayClient.chat()` does not panic when gateway returns SSE error (no workers available). Returns Ok with error chunks.                                                                                                                            |
| `tool_loop_executes_tool_and_re_requests`           | Two-round tool loop: MockWorker returns `tool_calls` for `get_current_time` → tool executes → re-request includes tool result message → MockWorker returns final text. Verifies tools in payload, tool result format, conversation history structure. |

Test harness components:

- `TestGateway` (`tests/support/gateway.rs`): starts real Axum server on random port with isolated state.
- `MockWorker` (`tests/support/worker.rs`): tokio-tungstenite WS client with `recv_job()`, `send_chunk()`, `send_end()`, `send_error()`, `disconnect()`.
- `SseClient` (`tests/support/client.rs`): reqwest HTTP client with manual SSE parsing, `next_event()`, `collect_all()`.

### What is NOT tested

| Area                     | Gap                                                                                                           | Risk                                                             |
| ------------------------ | ------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| Runtime message loop     | No tests for command dispatch, effect resolution, tick driver                                                 | Medium — logic is simple but wires everything together           |
| Effect executors         | No tests for `DispatchJob`, `SendClientError`, `SendClientDone`, `CloseStream`, `ProtocolViolation` execution | Low — each is a few lines, but dispatch failure path matters     |
| Worker WS handler        | Only happy path tested via integration test                                                                   | Medium — `select!` races, disconnect, re-registration need tests |
| Chat completions handler | Happy path + no-workers error tested; `stream=false` rejection not yet tested                                 | Low — remaining gap is a simple HTTP 400 path                    |
| Relay (`relay_job`)      | Happy path, error relay, disconnect, timeout, heartbeat all tested                                            | Low — malformed messages not yet tested                          |
| Fault tolerance          | Worker disconnect, timeout, worker error all tested; client disconnect and dispatch failure not yet tested    | Medium — client disconnect and dispatch failure gaps remain      |
| Leak detection           | `completed_streams_drain_from_state` covers success/error/timeout paths → all state drains to zero            | Low — channel close propagation not yet tested separately        |
| Performance              | No benchmarks for throughput, latency, backpressure, or concurrent stream scaling                             | Low for correctness, medium for production readiness             |

## Methodology

### Kernel: pure-function testing

The kernel is a pure reducer `reduce(state, event) -> (state, effects)` with no I/O. This enables:

- **Deterministic unit tests**: construct state, apply event, assert on output state + effects. No mocks, no async, no timing.
- **Property-based tests**: generate arbitrary event sequences via `proptest`, apply them to initial state, assert invariants hold on every intermediate state. The small ID pool (3 workers, 3 streams) maximizes collision probability.

This methodology is mature and should continue to be the primary testing approach for all kernel logic.

### Runtime and adapters: integration-tested

The runtime is intentionally simple ("so simple that its correctness is obvious by inspection"). The happy path integration test exercises the full wiring between kernel, registry, effect executors, and adapters. Fault tolerance and error paths are not yet tested at the integration level.

### Integration testing: in progress

The test harness is implemented and the happy path test passes. Remaining tests are described below.

## Planned End-to-End Testing Approach

### Test harness design

Tests spin up a real Axum server on `127.0.0.1:0` (random port), connect mock workers via `tokio-tungstenite`, and send requests via `reqwest` with SSE streaming. All in-process, no containers.

```
TestHarness {
    server_addr: SocketAddr,
    runtime_handle: RuntimeHandle,  // for state inspection in leak tests
}

MockWorker {
    ws: WebSocketStream,
    // helpers: send_chunk(), send_end(), send_error(), disconnect()
}

SseClient {
    response: reqwest streaming response,
    // helpers: next_event() -> StreamFrame, collect_all()
}
```

Each test creates its own harness (isolated state). Tests are `#[tokio::test]` with `tokio::time::timeout` wrapping the body to catch hangs.

### Prerequisites

Before integration tests can be fast and deterministic:

- **Configurable TTLs**: `stream_ttl` and `worker_ttl` must be injectable (currently hardcoded in `GatewayState::new`). Short TTLs (2–3 ticks) keep timeout tests fast.
- **State inspection**: a `#[cfg(test)]` method on the runtime (or kernel query command) to read active worker/stream counts for leak assertions.

### Correctness tests

Prove the assembled system behaves correctly under normal conditions.

| Test                                                                                                            | What it proves                                         |
| --------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------ |
| Happy path: worker sends N chunks + end → client receives N SSE chunks + `[DONE]`                               | Full pipeline works, chunk content and order preserved |
| Pre-dispatch rejection: `stream=false` → HTTP 400                                                               | HTTP error path before kernel dispatch                 |
| No workers available: request before any worker connects → SSE error frame                                      | Rejection through SSE channel                          |
| Worker error: worker sends `{"type":"error"}` → client sees error frame + done                                  | Error propagation through relay                        |
| Multiple concurrent streams: N workers, N clients, interleaved chunks → each client gets its own correct stream | No cross-stream contamination                          |
| Worker re-registration: worker completes job, gets second job → second stream works                             | One-use ID + re-register cycle                         |
| Malformed worker messages: worker sends garbage JSON → skipped, valid chunks still arrive                       | Relay resilience to bad input                          |
| Large payload: many chunks → all arrive in order, no drops                                                      | Channel FIFO, no truncation                            |

### Fault tolerance tests

Prove the system degrades correctly under failure conditions.

| Test                                                                                          | What it proves                                 |
| --------------------------------------------------------------------------------------------- | ---------------------------------------------- |
| Worker disconnects mid-stream → client gets error + done                                      | `WorkerDisconnected` → `AssignmentFailed` path |
| Client disconnects mid-stream → relay detects `ClientGone`, worker re-registers for next job  | No leaked state, worker not permanently lost   |
| Stream timeout: worker accepts job but never responds → client gets "stream timed out" + done | Timeout guarantee (I5) works end-to-end        |
| Worker idle timeout: worker connects, sends no heartbeat → expires silently from kernel       | Idle worker TTL cleanup                        |
| Dispatch failure: worker oneshot dropped before job delivery → client gets error              | `AssignmentFailed` from effect executor        |
| Rapid connect/disconnect: workers churn 100 times → no panics, no leaked state                | Registry cleanup under churn                   |

### Leak detection tests

Prove the system does not accumulate unbounded state.

| Test                                                                                           | What it proves                              |
| ---------------------------------------------------------------------------------------------- | ------------------------------------------- |
| Stream registry drain: complete N streams (mix of success/error/timeout) → registry empty      | No orphaned stream handles                  |
| Worker registry drain: N workers connect, do jobs, disconnect → registry empty                 | No orphaned worker handles                  |
| Kernel state drain: process N jobs to completion → `available` and `active_streams` both empty | No phantom entries                          |
| Channel close propagation: timeout fires → client mpsc receiver is actually closed             | Channel drops, no zombie channels           |
| Repeat cycle: run many connect→dispatch→complete cycles → state stays flat                     | No slow leaks in handles, tasks, or buffers |

### Performance tests

Characterize throughput and latency, detect regressions. Run separately from CI (not on every commit).

| Test                                                                              | What it measures             |
| --------------------------------------------------------------------------------- | ---------------------------- |
| Throughput: 1 worker, max-speed chunks → chunks/sec delivered to client           | Baseline relay throughput    |
| Concurrent streams: ramp 1–100 simultaneous streams → p50/p99 first-chunk latency | Dispatch latency under load  |
| Backpressure: slow client + fast worker → worker slows, no OOM                    | Channel backpressure works   |
| Dispatch latency: time from HTTP request to first SSE chunk with idle worker      | Baseline scheduling overhead |

## Implementation Priority

**Phase A** (prerequisite + first tests):

1. Make TTLs configurable via a config struct.
2. Add `#[cfg(test)]` state inspection on runtime.
3. Build test harness (server + mock worker + SSE client helpers).
4. Happy path test + worker disconnect test + timeout test.

**Phase B** (core coverage): 5. Remaining correctness tests. 6. Remaining fault tolerance tests. 7. Leak detection tests.

**Phase C** (performance baseline): 8. Throughput and latency benchmarks. 9. Backpressure verification.

## Invariants

Testing-level invariants (properties the test suite itself must maintain):

- **T1**: Every kernel transition rule in `gateway-state-machine.md` has a corresponding unit test.
- **T2**: Every kernel invariant (I1–I5) has a corresponding property test.
- **T3**: Integration tests must not depend on wall-clock timing for correctness (use short TTLs, not sleeps).
- **T4**: Integration tests must be isolated — each test gets its own server and state.
- **T5**: No test should log or assert on prompt/completion content (privacy constraint applies to tests too).

## Known Issues

- ~~TTLs are not yet configurable~~ — resolved: `GatewayConfig` struct threads `tick_interval`, `worker_ttl`, `stream_ttl` from `TestGateway::start_with_config()` to kernel.
- ~~No state inspection API~~ — resolved: `RuntimeHandle::query_state()` returns `StateSnapshot` with tick, worker/stream counts from kernel and registry. `TestGateway` exposes `RuntimeHandle`. Made unconditional (not `#[cfg(test)]`) because integration tests are an external crate.
- `worker-lifecycle.md` lists 15 transition scenarios (T1–T15), all marked "Tested? No".

## Status

- Kernel: strong coverage (44 unit + 6 property tests).
- Registry: adequate coverage (6 tests).
- Response assembler: strong coverage (11 unit + 4 property tests).
- Tool trait and registry: adequate coverage (5 tests).
- GetCurrentTime tool: adequate coverage (4 tests).
- ShellTool: adequate coverage (8 tests, 4 non-Docker + 4 Docker-gated `#[ignore]`).
- Protocol: strong coverage (14 serde tests).
- Integration: 12 tests passing (happy path, no-workers error, worker error, re-registration, concurrent streams, leak detection, stream timeout, heartbeat survival, worker disconnect, gateway client chunk collection, gateway client error handling, tool loop round-trip). Remaining: `stream=false` rejection, client disconnect, malformed messages, dispatch failure, rapid churn, channel close propagation, performance tests.

## Load into context when

- Planning or writing integration tests.
- Assessing whether a code change has adequate test coverage.
- Designing the test harness or mock worker/client utilities.
- Reviewing test methodology decisions.

## Relevant files

- `src/gateway/kernel.rs` (unit + property tests)
- `src/gateway/channel_registry.rs` (registry tests)
- `src/client/response_assembler.rs` (assembler unit + property tests)
- `src/protocol.rs` (serde round-trip tests)
- `tests/integration.rs` (integration tests)
- `tests/support/` (test harness: `gateway.rs`, `worker.rs`, `client.rs`)
- `proptest-regressions/gateway/kernel.txt` (regression cases)
- `Cargo.toml` (test dependencies: `proptest`, `reqwest`, `tokio-tungstenite`)
