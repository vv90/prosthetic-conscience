# Near-Term TODO

Snapshot date: 2026-03-13

## Real-world deployment test (next)

Goal: deploy the gateway on a public host and have a worker + client connect over the internet. This validates the reverse-connection architecture end-to-end.

### Prerequisites (code changes)

46. ~~**Configurable bind address**~~ (done): `--host` and `--port` CLI args on gateway binary via clap. Defaults to `127.0.0.1:3000`.
47. ~~**WSS support in worker**~~ (done): enabled `rustls-tls-webpki-roots` feature on `tokio-tungstenite`. Worker can now connect via `wss://` URLs. Consistent with `reqwest`'s `rustls-tls` — pure Rust, no system OpenSSL dependency.
48. ~~**Basic auth / shared secret**~~ (done): axum middleware (`src/router/auth.rs`) checks `Authorization: Bearer <token>` on all routes. Gateway reads `PC_AUTH_TOKEN` env var — if set, auth required; if unset, auth disabled (open access). Worker accepts `--auth-token` CLI arg, sends header on WS connect. Tests unaffected (auth disabled by default in `TestGateway`).

### Infrastructure

49. Deploy gateway binary to a VPS (any small instance — gateway is CPU-light).
50. TLS termination: put caddy or nginx in front of the gateway for automatic Let's Encrypt. Caddy is simplest — reverse proxy to `127.0.0.1:3000` with automatic HTTPS.
51. Worker runs wherever the GPU is (home machine, cloud GPU). Connects outbound to `wss://gateway-domain/ws/worker`. No inbound ports needed.
52. Client connects to `https://gateway-domain/v1/chat/completions` from anywhere.

### Validation checklist

53. Worker connects over WSS, shows "connected to gateway" in logs.
54. Client `curl -N https://gateway-domain/v1/chat/completions ...` streams tokens from llama.cpp through the gateway.
55. Kill the worker mid-stream → client gets error + done.
56. Kill llama-server → worker sends error to gateway → client gets error.
57. Kill the gateway → worker reconnects with backoff when gateway comes back.
58. Unauthorized requests (wrong/missing token) get rejected.

## Integration testing

See `testing-coverage.md` for full methodology and current gaps.

### Phase A: prerequisites and first tests

1. ~~Make TTLs and heartbeat intervals configurable via `GatewayConfig`~~ (done). `GatewayConfig` in `runtime.rs` bundles `tick_interval`, `worker_ttl`, `stream_ttl`, `stream_heartbeat_interval`, `worker_heartbeat_interval`. `RuntimeHandle` carries both heartbeat intervals. `relay_job` accepts `stream_heartbeat_interval` parameter (was hardcoded 10s). Worker WS handler uses `runtime.worker_heartbeat_interval` (was hardcoded 15s). All 74 tests pass.
2. ~~`#[cfg(test)]` state inspection~~ (done). `RuntimeHandle::query_state()` returns `StateSnapshot { tick, available_workers, active_streams, worker_registry_count, stream_registry_count }`. `QueryState` command + handler in runtime message loop. `ChannelRegistry::stream_count()` added. `TestGateway` now stores `RuntimeHandle`. Made unconditional (not `#[cfg(test)]`) because integration tests are an external crate and cannot see `#[cfg(test)]` items.
3. ~~Build integration test harness~~ (done). `TestGateway`, `MockWorker`, `SseClient` in `tests/support/`. Each test gets isolated state on random port.
4. ~~Happy path integration test~~ (done). `happy_path_streams_chunks_and_done` in `tests/integration.rs`. 2 chunks + end → 2 SSE data events + `[DONE]`.
5. ~~Worker disconnect integration test~~ (done). `worker_disconnect_mid_stream_sends_error_and_done` in `tests/integration.rs`. Worker sends 1 chunk, closes WS → client gets chunk, error, `[DONE]`.
6. ~~Timeout integration test~~ (done). `stream_timeout_sends_error_and_done` in `tests/integration.rs`. Gateway started with `stream_ttl: 3, tick_interval: 50ms`. Worker accepts job but never responds → client gets `{"error":{"message":"stream timed out"}}` + `[DONE]` after ~150ms.
   6b. ~~Long-running stream heartbeat test~~ (done). `long_running_stream_survives_with_heartbeats` in `tests/integration.rs`. Gateway with `stream_ttl: 3, tick_interval: 50ms, stream_heartbeat_interval: 100ms`. Worker sends chunk, sleeps 250ms (past original TTL), sends second chunk + end → client gets both chunks + `[DONE]`, no timeout error.

### Phase B: core integration coverage

7. Pre-dispatch rejection tests: `stream=false` → 400; ~~no workers → SSE error frame~~ (7b done: `no_workers_returns_sse_error`).
8. ~~Worker error propagation test~~ (done): `worker_error_sends_error_and_done`. Worker sends `{"type":"error"}` → client sees chunk + 2 errors (relay + kernel) + done.
9. ~~Multiple concurrent streams test~~ (done): `concurrent_streams_no_cross_contamination`. 3 workers, 3 clients, interleaved chunks → each client receives exactly its own worker's data, all 3 workers used.
10. ~~Worker re-registration test~~ (done): `worker_re_registration_handles_second_job`. Worker completes first job, re-registers, receives and completes second job.
11. Client disconnect test: client drops connection mid-stream → relay detects `ClientGone`, worker re-registers.
12. Malformed worker message test: garbage JSON skipped, valid chunks still arrive.
13. Dispatch failure test: worker oneshot dropped before delivery → client gets error.
14. Rapid worker connect/disconnect churn test: 100 cycles → no panics, no leaked state.

### Phase B: leak detection

15–17. ~~Stream/worker/kernel state drain~~ (done): `completed_streams_drain_from_state`. Runs 3 jobs (success, error, timeout) then asserts all state drains to zero (active_streams, stream_registry, available_workers, worker_registry). 18. Channel close propagation: timeout fires → client mpsc receiver is actually closed.

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

### Step 4: worker agent ~~crate~~ (initial implementation done as second binary)

37. ~~Create worker agent with inference backend~~: implemented as `src/worker/` module + `src/worker_agent.rs` binary (`pc-worker`). `InferenceClient` proxies to llama-server (or any OpenAI-compatible endpoint) via HTTP SSE streaming. No trait abstraction yet — single concrete backend.
38. ~~Implement gateway WS client~~: `WorkerClient` in `src/worker/client.rs` — connect, reconnect with exponential backoff (1s→30s cap), job receive, chunk streaming.
39. ~~Wire backend output → gateway protocol messages~~: `process_job` reads `InferenceClient::stream_completion()` stream, sends `WorkerMessage::Chunk`/`End`/`Error` over WebSocket.

    **Not yet implemented**: `InferenceBackend` trait, `VllmBackend`, `EchoBackend`, auth/handshake, token counting, cancellation, worker binary integration tests.

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

43. Add worker handshake/version/capability validation (supersedes stopgap auth in item 48).
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
