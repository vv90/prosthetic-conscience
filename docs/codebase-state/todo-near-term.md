# Near-Term TODO

Snapshot date: 2026-03-12

## Integration testing (next)

See `testing-coverage.md` for full methodology and current gaps.

### Phase A: prerequisites and first tests

1. Make `stream_ttl` and `worker_ttl` configurable via a `GatewayConfig` struct passed to the kernel. Currently hardcoded in `GatewayState::new`. Required for fast, deterministic timeout tests.
2. Add `#[cfg(test)]` state inspection on runtime (or kernel query command) to read active worker count, stream count, and registry entry counts. Required for leak detection assertions.
3. ~~Build integration test harness~~ (done). `TestGateway`, `MockWorker`, `SseClient` in `tests/support/`. Each test gets isolated state on random port.
4. ~~Happy path integration test~~ (done). `happy_path_streams_chunks_and_done` in `tests/integration.rs`. 2 chunks + end → 2 SSE data events + `[DONE]`.
5. Worker disconnect integration test: worker closes WS mid-stream → client gets error frame + done.
6. Timeout integration test: worker accepts job but never responds → client gets "stream timed out" + done.

### Phase B: core integration coverage

7. Pre-dispatch rejection tests: `stream=false` → 400; no workers → SSE error frame.
8. Worker error propagation test: worker sends `{"type":"error"}` → client sees error + done.
9. Multiple concurrent streams test: N workers, N clients, interleaved → no cross-contamination.
10. Worker re-registration test: worker completes job, receives second job, second stream works.
11. Client disconnect test: client drops connection mid-stream → relay detects `ClientGone`, worker re-registers.
12. Malformed worker message test: garbage JSON skipped, valid chunks still arrive.
13. Dispatch failure test: worker oneshot dropped before delivery → client gets error.
14. Rapid worker connect/disconnect churn test: 100 cycles → no panics, no leaked state.

### Phase B: leak detection

15. Stream registry drain: complete N streams (success/error/timeout mix) → registry empty.
16. Worker registry drain: N workers connect, do jobs, disconnect → registry empty.
17. Kernel state drain: process N jobs to completion → `available` and `active_streams` both empty.
18. Channel close propagation: timeout fires → client mpsc receiver is actually closed.

### Phase C: performance baseline

19. Throughput benchmark: 1 worker, max-speed chunks → chunks/sec to client.
20. Concurrent streams benchmark: ramp 1–100 streams → p50/p99 first-chunk latency.
21. Backpressure test: slow client + fast worker → worker slows, no OOM.

## Structural changes for multi-component architecture

The project is currently a single crate producing a single binary. The spec describes three separate components (gateway, worker agent, client sidecar) that share a wire protocol. The following changes prepare the codebase for that future without disrupting current work.

### ~~Step 1: extract protocol types~~ (done)

Completed. `src/protocol.rs` defines `WorkerMessage`, `GatewayToWorker`, and `ChatRequest` with serde derives. `relay.rs` deserializes `WorkerMessage` instead of ad-hoc `Value` field access. `worker_ws_upgrade.rs` serializes `GatewayToWorker::Job` instead of inline `json!()`. `chat_completions.rs` imports `ChatRequest` from the protocol module. 19 serde round-trip tests added. All 70 tests pass.

### Step 2: convert to Cargo workspace (do before worker agent work begins)

29. Restructure into workspace:
    ```
    prosthetic-conscience/
    ├── Cargo.toml                  [workspace]
    ├── crates/
    │   ├── protocol/               shared wire types (from step 1)
    │   ├── gateway/                kernel + runtime + effects + adapters
    │   ├── worker-agent/           connects to gateway, calls inference backend
    │   └── client-sidecar/         local proxy, future encryption
    └── tests/                      workspace-level integration tests
    ```
30. Move `src/protocol.rs` → `crates/protocol/src/lib.rs`.
31. Move remaining `src/` → `crates/gateway/src/`. Preserve `lib.rs`/`main.rs` split.
32. Move integration test harness and tests to workspace-level `tests/` (or `crates/integration-tests`). Test helpers (`MockWorker`, `SseClient`) will share code patterns with real worker-agent and sidecar.

### Step 3: introduce `Payload` enum in protocol crate (do before encryption work)

33. Replace `payload: Value` with `Payload` enum in the protocol crate:
    - `Payload::Plaintext(Value)` — Phase 1 (current behavior)
    - `Payload::Encrypted(EncryptedBlob)` — Phase 2 (ciphertext, nonce, auth_tag, encrypted_session_key)
34. Gateway passes `Payload` through without matching on variant — kernel and runtime remain opaque.
35. Worker agent decrypts `Payload::Encrypted` or reads `Payload::Plaintext`.
36. Client sidecar encrypts to `Payload::Encrypted` or passes `Payload::Plaintext`.

### Step 4: worker agent crate

37. Create `crates/worker-agent/` with `InferenceBackend` trait:
    - `async fn stream_completion(&self, payload) -> impl Stream<Item = Result<Chunk, Error>>`
    - Adapters: `LlamaCppBackend`, `VllmBackend`, `EchoBackend` (testing)
38. Implement gateway WS client: connect, reconnect, heartbeat, job receive, chunk streaming.
39. Wire backend output → gateway protocol messages.

### Step 5: client sidecar crate

40. Create `crates/client-sidecar/` with local OpenAI-compatible HTTP endpoint.
41. Implement gateway HTTP/SSE client: forward requests, stream responses.
42. Add `envelope` module for Phase 2 encryption (encrypt request payload, decrypt streaming chunks).

### What does NOT need to change

- **Kernel** — already generic, pure, payload-opaque. No structural changes needed.
- **Runtime message loop** — treats payload as pass-through. Fine as-is.
- **Channel registry** — gateway-internal, no external consumers.
- **Effect system** — gateway-internal.
- **Timeout/heartbeat mechanism** — gateway-internal.

## Other tasks

43. Add worker handshake/version/capability validation.
44. Remove `GatewayAdapter` once runtime coverage is confirmed complete.
45. Keep behavior-state files synchronized as behavior transitions from placeholder -> partial -> implemented.

## Recently completed

- **Chat completions handler implemented**: `POST /v1/chat/completions` with `stream=true` registers stream, submits `HttpChatRequested` to kernel, returns SSE response. `StreamFrame` mapped to SSE events: `Chunk` → `data: {json}`, `Done` → `data: [DONE]`, `Error` → `data: {"error": {...}}`. Added `RuntimeCommand::HttpChatRequested` + handler + convenience method. `stream=false` returns 400. Runtime unavailable returns 503.
- **Relay wired into worker ws handler**: Replaced `consume_until_terminal` stub with `relay_job` call. `RelayOutcome` mapped to kernel events: `WorkerEnd` → `assignment_cleared`, `WorkerError`/`WorkerDisconnected` → `assignment_failed`, `ClientGone` → no event (timeout handles cleanup). `WorkerDisconnected` exits the connection loop; other outcomes re-register. Stream heartbeats now active during job relay.
- **Client effect executors implemented**: `SendClientError` sends `StreamFrame::Error` via cloned handle. `SendClientDone` sends `StreamFrame::Done` via taken handle (removes registry entry). `CloseStream` drops the taken handle without sending a frame. Added `take_stream` to registry. Terminal effects use `take_stream` in `resolve_effects`; non-terminal `SendClientError` uses `clone_stream`. Closed-channel sends are benign (debug-logged). 2 new registry tests. All 5 effect executors now implemented.
- **`AssignmentFailed` event + dispatch failure handling**: Added `AssignmentFailed { client_stream_id, message }` kernel event — removes stream, emits `SendClientError` + `SendClientDone`. `DispatchJob::execute()` sends `AssignmentFailed` back to kernel when worker oneshot fails. `resolve_effects` fallbacks changed from `AssignmentCleared` to `AssignmentFailed`. Effect names renamed: `DispatchJob`, `SendClientError`, `SendClientDone`, `CloseStream`, `ProtocolViolation`.
- **Effect executors (partial)**: `DispatchJob::execute()` implemented — constructs `WorkerJob` and sends via oneshot; signals `AssignmentFailed` on dispatch failure. `ProtocolViolation::execute()` implemented — logs via `tracing::warn!`. `resolve_effects` now resolves both worker IDs and stream IDs; resolved type is `Effect<WorkerHandle, (ClientStreamId, StreamHandle)>`. Added `clone_stream` to registry. Client-side effects remain stubs on concrete resolved types.
- **Stream heartbeats**: added `RuntimeCommand::StreamHeartbeat` and `RuntimeHandle::stream_heartbeat()`. `relay_job` sends `StreamHeartbeat` every 10 seconds while relaying via `tokio::select!`. Heartbeat failure is non-fatal. Not yet active until `relay_job` is wired in.
- **Idle worker heartbeats**: added `RuntimeCommand::WorkerHeartbeat` and `RuntimeHandle::worker_heartbeat()`. Worker ws handler sends heartbeat every 15 seconds while idle via `tokio::time::interval_at`. Interval resets after re-registration.
- **Property tests and duplicate stream ID fix**: 7 property tests over arbitrary event sequences covering I2-I5, tick monotonicity, no duplicate workers, stream timeout effect pairs. Proptest I3 found a real bug: duplicate `client_stream_id` in `HttpChatRequested` silently overwrote `active_streams` entry. Fixed with pre-dispatch guard. 15 new unit tests added (40 total). Proptest regression file retained.
- **Timeout-driven stream termination**: added `Tick` event, tick-counted deadlines (`BTreeMap<WId, u64>` / `BTreeMap<SId, u64>`), `WorkerHeartbeat`/`StreamHeartbeat` events, tick expiration logic. Runtime spawns a tick task using `try_send` (skips ticks under congestion).
- **One-use worker IDs**: refactored kernel state from `BTreeMap<WId, WorkerStatus>` to `available: BTreeMap<WId, u64>` + `active_streams: BTreeMap<SId, u64>`. Workers are consumed on dispatch, gone from kernel state. Worker handler re-registers with fresh ID after each job.
- Replaced `WorkerEnd`/`WorkerError` kernel events with `AssignmentCleared { client_stream_id }`.
- Removed `WorkerJobCompleted` runtime command. Added `AssignmentCleared` runtime command.
- Removed `set_worker_handle` from registry.
- Updated worker ws handler to re-register with fresh oneshot after each job instead of sending `WorkerJobCompleted`.
- Removed external `UnregisterWorker` and `UnregisterStream` commands -- cleanup is now internally-driven.
- Removed `WorkerUnregistered` and `RegistryCleanedUp` kernel events and hint sets.
- Worker job channel changed from `mpsc` to `oneshot`.
- Added `relay_job` function in `src/gateway/relay.rs`.
- Added `StreamHandle` type alias and `client_tx` field to `WorkerJob`.
