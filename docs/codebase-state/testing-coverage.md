# Testing Coverage and Methodology

Snapshot date: 2026-04-03

## Overview

Testing is concentrated in the pure kernel layer, which has strong unit and property-based coverage. Integration coverage is now established across chat, transcription, sessions, and the consensus CLI flows. This document catalogs what is tested, what is not, and the planned approach for closing the remaining gaps.

## Current Coverage

### Kernel unit tests (44 tests)

Location: `crates/prosthetic-conscience/src/gateway/kernel.rs` (inline `#[cfg(test)]` module).

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

Location: `crates/prosthetic-conscience/src/gateway/kernel.rs` (inline, using `proptest`).

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

Location: `crates/prosthetic-conscience/src/gateway/channel_registry.rs` (inline `#[cfg(test)]` module).

| Test                       | What is tested                     |
| -------------------------- | ---------------------------------- |
| Worker/stream registration | Returns unique IDs                 |
| `take_worker`              | Removes handle (consumed on use)   |
| `clone_stream`             | Preserves entry                    |
| `take_stream`              | Removes entry                      |
| Property test              | All registered handles retrievable |

### Response assembler tests (11 unit + 4 property = 15 tests)

Location: `crates/prosthetic-conscience/src/client/response_assembler.rs` (inline `#[cfg(test)]` module).

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

Location: `crates/prosthetic-conscience/src/client/tools/mod.rs` (inline `#[cfg(test)]` module).

| Test                         | What is tested                                       |
| ---------------------------- | ---------------------------------------------------- |
| `register_and_get`           | Register tool, look up by name, unknown returns None |
| `execute_known_tool`         | Registry dispatches to correct tool                  |
| `execute_unknown_tool`       | Unknown tool name returns `ToolError::UnknownTool`   |
| `definitions_returns_openai` | OpenAI format: `{type: "function", function: {...}}` |
| `duplicate_overwrites`       | Re-registering same name replaces the previous tool  |

### GetCurrentTime tool tests (4 tests)

Location: `crates/prosthetic-conscience/src/client/tools/current_time.rs` (inline `#[cfg(test)]` module).

| Test                  | What is tested                               |
| --------------------- | -------------------------------------------- |
| `returns_iso8601_utc` | Output matches `YYYY-MM-DDTHH:MM:SSZ` format |
| `definition_name`     | Tool name is `get_current_time`              |
| `days_to_date_epoch`  | Day 0 → 1970-01-01                           |
| `days_to_date_known`  | Day 20528 → 2026-03-16                       |

### ShellTool tests (8 run + 4 ignored = 12 tests)

Location: `crates/prosthetic-conscience/src/client/tools/shell.rs` (inline `#[cfg(test)]` module).

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

Location: `crates/prosthetic-conscience/src/protocol.rs` (inline `#[cfg(test)]` module).

| Area              | Count | What is tested                                                                                                        |
| ----------------- | ----- | --------------------------------------------------------------------------------------------------------------------- |
| `WorkerMessage`   | 8     | Round-trip for Chunk/End/Error, from-JSON parsing, missing message field defaults, unknown/missing type, invalid JSON |
| `GatewayToWorker` | 2     | Round-trip for Job, wire format matches legacy `json!()` output                                                       |
| `ChatRequest`     | 4     | stream=true, stream=false, stream absent, all fields preserved in payload                                             |

### Integration tests (28 tests)

Location: `crates/prosthetic-conscience/tests/integration.rs` with helpers in `crates/prosthetic-conscience/tests/support/`.

| Test group                          | Count | What it proves                                                                                                                                               |
| ----------------------------------- | ----- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Session websocket flows             | 7     | Session create/append, subscribe notifications, nonexistent sessions, subscriber timeout, multi-subscriber fanout, disconnect cleanup, and handshake timeout |
| Consensus seed and bootstrap        | 3     | Seeding fixture logs into a live session, rejecting unknown sessions, and `ConsensusApp::join()` bootstrapping from committed entries                        |
| Transcription routing               | 5     | Transcription happy path, no-worker error, worker error propagation, capability isolation, and mixed chat/transcription worker pools                         |
| Client tool loop and gateway client | 4     | Generic tool loop re-requesting, consensus clarification-then-draft flow, SSE assembly, and graceful no-worker error handling                                |
| Chat streaming and lifecycle        | 9     | Happy path chat streaming, no-worker rejection, worker error relay, re-registration, concurrency isolation, state draining, timeout, disconnect, heartbeats  |

Test harness components:

- `TestGateway` (`crates/prosthetic-conscience/tests/support/gateway.rs`): starts real Axum server on random port with isolated state.
- `MockWorker` (`crates/prosthetic-conscience/tests/support/worker.rs`): tokio-tungstenite WS client with `recv_job()`, `send_chunk()`, `send_end()`, `send_error()`, `disconnect()`.
- `SseClient` (`crates/prosthetic-conscience/tests/support/client.rs`): reqwest HTTP client with manual SSE parsing, `next_event()`, `collect_all()`.

### Consensus tool dispatch tests (23 tests)

Location: `crates/consensus/src/tools.rs` (inline `#[cfg(test)]` module).

| Area             | Count | What is tested                                                                                                                                                                                                                               |
| ---------------- | ----- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Tool dispatch    | 14    | `overview`, `claim_detail`, `preview_overview`, `preview_claim_detail`, `draft_claim`, `draft_relation`, `draft_stance`, `draft_resolve`, `draft_comment`, `show_drafts`, `remove_draft`, `submit_drafts`, `clear_drafts`, `impact_analysis` |
| Error paths      | 4     | Unknown tool, missing required arguments, invalid claim-ref shapes, and engine errors such as removing a nonexistent draft                                                                                                                   |
| Tool definitions | 3     | Total count (14), LLM-filtered set excludes `submit_drafts`/`clear_drafts` and omits `no_structured_action`, draft tools do not expose `author`                                                                                              |
| Integration      | 2     | Round-trip draft→show→submit, preview includes uncommitted drafts                                                                                                                                                                            |

### Consensus LLM tests (7 tests)

Location: `crates/consensus/src/llm_turn.rs` (inline `#[cfg(test)]` module).

| Area               | Count | What is tested                                                                                                                                             |
| ------------------ | ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| System prompt      | 1     | Contains review boundary, clarify-first wording, claim-ref guidance, safe tool names, excludes `submit_drafts`, `clear_drafts`, and `no_structured_action` |
| Request payload    | 1     | Includes `tool_choice: "auto"` and `max_tokens`                                                                                                            |
| History truncation | 5     | Under-limit noop, drops oldest, preserves tool-call/result pairs, skips tool results at cut, noop when no safe cut                                         |

### Coordinator reducer tests (19 tests)

Location: `crates/consensus/src/coordinator.rs` (inline `#[cfg(test)]` module).

| Area                                       | Count | What is tested                                                                                                                                                  |
| ------------------------------------------ | ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Bootstrap and resync                       | 7     | `init` empty/latest/error cases, `sync_to_latest` noop, non-shrinking behavior, filling requested latest slot, preserving existing received value              |
| Entry reception, effects, and view access  | 6     | Duplicate receive noop, requested-slot fill, future receive emits eager `FetchMissing`, `next_expected()` semantics, public type shapes, committed-prefix omission of buffered future entries |
| Property: first writer wins | 1     | Duplicate indices never overwrite the first received value                                                                                                      |
| Property: hole coverage     | 1     | Future receives cover every newly requested slot below the current upper bound                                                                                  |
| Property: fetch bounds      | 1     | `FetchMissing` ranges are ascending, non-overlapping, bounded by `page_limit`, and never exceed `slots.len()`                                                 |
| Property: slot layout       | 1     | `next_expected()` always matches the first requested slot or `slots.len()`                                                                                      |
| Property: monotonic state   | 1     | `slots.len()` and `next_expected()` never decrease; previously received slots remain unchanged                                                                  |
| Property: fetch coverage    | 1     | Newly created requested slots are covered by fetch effects; non-extending receives emit no fetches                                                             |

### Consensus entry buffer tests (3 tests)

Location: `crates/consensus/src/entry_buffer.rs` (inline `#[cfg(test)]` module).

| Area                  | Count | What is tested                                            |
| --------------------- | ----- | --------------------------------------------------------- |
| Submission tracking   | 1     | `note_submission_payload` advances only on matching entry |
| Entry buffering       | 1     | Out-of-order entries buffered until gap is closed         |
| Tool trace formatting | 1     | `format_tool_trace` lists each tool round                 |

### Consensus drafts reducer tests (8 tests)

Location: `crates/consensus/src/drafts.rs` (inline `#[cfg(test)]` module).

| Area                        | Count | What is tested                                                                                       |
| --------------------------- | ----- | ---------------------------------------------------------------------------------------------------- |
| Draft transition rules      | 6     | Empty state, claim draft creation, middle removal, unknown removal, referenced-draft failure, invalid parent failure |
| Property: view fidelity     | 1     | `drafts::View.drafts` matches draft state exactly after arbitrary local traces                       |
| Property: removal ordering  | 1     | Removing an existing generated draft removes exactly one entry and preserves the relative order       |

### Consensus app reducer tests (12 tests)

Location: `crates/consensus/src/app.rs` (inline `#[cfg(test)]` module).

| Area                                 | Count | What is tested                                                                                                                        |
| ------------------------------------ | ----- | ------------------------------------------------------------------------------------------------------------------------------------- |
| App/coordinator composition          | 7     | Empty view, wrapped draft events emit no app effects, committed entry updates overview, future entries stay buffered, fetch requests surface as wrapped coordinator effects, coordinator events preserve existing draft state |
| Boundary serialization               | 3     | Wrapped drafts event shape, wrapped coordinator event shape, wrapped coordinator effect shape                                         |
| Property: draft isolation            | 1     | Draft-only traces preserve the coordinator-derived committed overview                                                                  |
| Property: entry isolation            | 1     | Entry-only traces do not change draft list or draft notice                                                                             |

### Consensus wasm wrapper tests (4 tests)

Location: `crates/consensus-wasm/src/lib.rs` (inline `#[cfg(test)]` module).

| Area                    | Count | What is tested                                                                                              |
| ----------------------- | ----- | ----------------------------------------------------------------------------------------------------------- |
| Wrapper view bootstrap  | 1     | Initial handle state produces the same empty `View` shape as the pure app                                   |
| Wrapper receive path    | 3     | Contiguous receive updates overview, out-of-order receive returns wrapped fetch effect, gap fill catches up |

Build verification:

- `cargo build -p consensus-wasm --target wasm32-unknown-unknown`

### Consensus CLI app tests (1 test)

Location: `crates/prosthetic-conscience/src/consensus_cli/app.rs` (inline `#[cfg(test)]` module).

| Area                  | Count | What is tested                            |
| --------------------- | ----- | ----------------------------------------- |
| Tool trace formatting | 1     | `format_tool_trace` lists each tool round |

### Consensus eval tests (3 tests)

Location: `crates/prosthetic-conscience/src/consensus_support/eval.rs` (inline `#[cfg(test)]` module).

| Area            | Count | What is tested                                                                                          |
| --------------- | ----- | ------------------------------------------------------------------------------------------------------- |
| Scoring         | 2     | Stance draft matches argument and draft buffer, plain-text response cases require an empty draft buffer |
| Context seeding | 1     | Synthetic history contains properly paired tool call/result messages                                    |

### What is NOT tested

| Area                     | Gap                                                                                                        | Risk                                                             |
| ------------------------ | ---------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| Runtime message loop     | No tests for command dispatch, effect resolution, tick driver                                              | Medium — logic is simple but wires everything together           |
| Effect executors         | No tests for `DispatchJob`, `SendClientError`, `SendClientDone`, `ProtocolViolation` execution             | Low — each is a few lines, but dispatch failure path matters     |
| Worker WS handler        | Only happy path tested via integration test                                                                | Medium — `select!` races, disconnect, re-registration need tests |
| Chat completions handler | Happy path + no-workers error tested; `stream=false` rejection not yet tested                              | Low — remaining gap is a simple HTTP 400 path                    |
| Relay (`relay_job`)      | Happy path, error relay, disconnect, timeout, heartbeat all tested                                         | Low — malformed messages not yet tested                          |
| Fault tolerance          | Worker disconnect, timeout, worker error all tested; client disconnect and dispatch failure not yet tested | Medium — client disconnect and dispatch failure gaps remain      |
| Leak detection           | `completed_streams_drain_from_state` covers success/error/timeout paths → all state drains to zero         | Low — channel close propagation not yet tested separately        |
| Performance              | No benchmarks for throughput, latency, backpressure, or concurrent stream scaling                          | Low for correctness, medium for production readiness             |

## Methodology

### Kernel: pure-function testing

The kernel is a pure reducer `reduce(state, event) -> (state, effects)` with no I/O. This enables:

- **Deterministic unit tests**: construct state, apply event, assert on output state + effects. No mocks, no async, no timing.
- **Property-based tests**: generate arbitrary event sequences via `proptest`, apply them to initial state, and sample-check semantic correctness properties across many traces. The small ID pool (3 workers, 3 streams) maximizes collision probability.

See [`testing-methodology-and-invariants.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/testing-methodology-and-invariants.md) for the canonical distinction between invariants, constraints, transition rules, and test evidence.

This methodology is mature and should continue to be the primary testing approach for kernel logic that cannot be enforced structurally or by the type system.

### Runtime and adapters: integration-tested

The runtime is intentionally simple ("so simple that its correctness is obvious by inspection"). The integration suite now exercises the full wiring between kernel, registry, effect executors, transport adapters, and the consensus/session flows. Fault tolerance and some HTTP/relay edge cases still have gaps, but the runtime is no longer covered only by the happy path.

### Integration testing: active

The test harness now covers session WS flows, transcription routing, chat streaming, generic tool calling, and consensus-specific session bootstrap flows. Remaining gaps are described below.

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

## Test-Suite Constraints

Test-suite discipline rules and coverage goals:

- **T1**: Every kernel transition rule in `gateway-state-machine.md` has a corresponding unit test.
- **T2**: Every kernel semantic correctness property that is not structurally or type-enforced should have a corresponding property test.
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
- Consensus tool dispatch: strong coverage (23 tests).
- Consensus LLM: adequate coverage (7 tests — prompt content, request payload, history truncation).
- Coordinator reducer: strong coverage (19 tests — 13 targeted + 6 property-based, covers bootstrap from latest entry, slot monotonicity, gap detection, page-bounded fetch planning, and committed-prefix access).
- Consensus entry buffer: adequate coverage (3 tests — submission tracking, entry buffering, trace formatting).
- Consensus drafts reducer: strong coverage (8 tests — draft transition rules plus view/removal properties).
- Consensus app reducer: strong coverage (12 tests — wrapped child-event delegation, wrapped coordinator effects, boundary serialization, and draft/entry isolation properties).
- Consensus wasm wrapper: adequate coverage (4 tests + wasm32 build verification — empty view bootstrap and receive/render wrapper flow).
- Consensus CLI app: minimal coverage (1 test — trace formatting).
- Consensus eval: adequate coverage (3 tests — scoring and context seeding).
- Integration: 28 tests passing. Includes consensus-specific tests: `consensus_llm_drafts_claim_after_clarification_turn`, `consensus_seed_*` (2), and `consensus_app_join_bootstraps_from_existing_session`. Remaining: `stream=false` rejection, client disconnect, malformed messages, dispatch failure, rapid churn, channel close propagation, performance tests.

## Load into context when

- Planning or writing integration tests.
- Assessing whether a code change has adequate test coverage.
- Designing the test harness or mock worker/client utilities.
- Reviewing test methodology decisions.

## Relevant files

- `crates/prosthetic-conscience/src/gateway/kernel.rs` (unit + property tests)
- `crates/prosthetic-conscience/src/gateway/channel_registry.rs` (registry tests)
- `crates/prosthetic-conscience/src/client/response_assembler.rs` (assembler unit + property tests)
- `crates/consensus/src/tools.rs` (tool dispatch tests)
- `crates/consensus/src/llm_turn.rs` (LLM turn loop tests)
- `crates/consensus/src/coordinator.rs` (coordinator reducer tests)
- `crates/consensus/src/entry_buffer.rs` (entry buffer tests)
- `crates/prosthetic-conscience/src/consensus_support/eval.rs` (eval scoring tests)
- `crates/prosthetic-conscience/src/consensus_cli/app.rs` (app tests)
- `crates/prosthetic-conscience/src/protocol.rs` (serde round-trip tests)
- `crates/prosthetic-conscience/tests/integration.rs` (integration tests)
- `crates/prosthetic-conscience/tests/support/` (test harness: `gateway.rs`, `worker.rs`, `client.rs`)
- `fixtures/tool-call-eval/` (eval benchmark suites)
- `proptest-regressions/gateway/kernel.txt` (regression cases)
- `Cargo.toml` (test dependencies: `proptest`, `reqwest`, `tokio-tungstenite`)
