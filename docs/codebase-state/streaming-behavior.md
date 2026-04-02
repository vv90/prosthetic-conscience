# Streaming Behavior

Snapshot date: 2026-03-09

## Behavior

- Client streaming is implemented. `POST /v1/chat/completions` with `stream=true` registers a stream, submits `HttpChatRequested` to the kernel, and returns an SSE response reading from `mpsc::Receiver<StreamFrame>`.
- Worker websocket route has oneshot-based job dispatch with `select!` idle-phase monitoring.
- `relay_job` in `crates/prosthetic-conscience/src/gateway/relay.rs` reads worker websocket messages, forwards `StreamFrame` variants (`Chunk`, `Done`, `Error`) to a client stream channel (`mpsc::Sender<StreamFrame>`), and returns a `RelayOutcome` (`WorkerEnd`, `WorkerError`, `WorkerDisconnected`, `ClientGone`).
- `relay_job` is wired into the worker ws handler. The handler maps `RelayOutcome` to kernel events: `WorkerEnd` → `assignment_cleared`, `WorkerError`/`WorkerDisconnected` → `assignment_failed`, `ClientGone` → no event (timeout handles cleanup). `relay_job` sends `StreamHeartbeat` every 10 seconds while relaying (via `tokio::select!` between socket recv and heartbeat timer). Heartbeat failure is non-fatal.
- `WorkerJob` carries a `client_tx: StreamHandle` (`mpsc::Sender<StreamFrame>`) for the eventual relay wiring.
- SSE bridge implemented in `crates/prosthetic-conscience/src/router/chat_completions.rs`. Maps `StreamFrame::Chunk` → `data: {json}`, `StreamFrame::Done` → `data: [DONE]`, `StreamFrame::Error` → `data: {"error": {...}}`. SSE stream ends when channel closes.

## Stream Termination Guarantee

The central safety property of the system: **every client stream that enters the kernel eventually gets a terminal response.**

### How it works

1. **Pre-dispatch**: if no worker is available or `stream=false`, the kernel emits terminal effects (`SendClientError` + `SendClientDone`) immediately. Client gets an error response followed by `[DONE]`.

2. **Post-dispatch**: the kernel tracks the assignment (`worker_id -> stream_id`) and sets a deadline. Three paths to termination:
   - **Normal path**: relay delivers data via `client_tx`, sends `AssignmentCleared` to kernel, kernel emits `SendClientDone`.
   - **Failure path**: dispatch fails (worker channel closed) or relay reports error/disconnect, sends `AssignmentFailed` to kernel, kernel emits `SendClientError` + `SendClientDone`.
   - **Timeout path**: deadline expires, kernel emits terminal effects (`SendClientError` + `SendClientDone`), effect executor delivers error and drops client receiver. Relay's next `client_tx.send()` fails, relay stops.

3. **Channel-based cancellation**: the kernel doesn't directly kill relays. It kills the _stream_ (via terminal effects that close the client channel). The relay dies as a consequence because its output channel is dead. Late events for already-cleared assignments hit `ProtocolViolation`, which is harmless.

### Current status

- Pre-dispatch error handling: **implemented** in kernel.
- Post-dispatch assignment tracking: **implemented** (`active_streams` in kernel).
- Timeout mechanism: **implemented**. Tick-counted deadlines, stream heartbeats reset deadline. Timeout emits `SendClientError` + `SendClientDone`.
- Channel-based cancellation: **implemented**. The terminal effect (`SendClientDone`) uses `take_stream` to remove the registry entry, then drops the handle — closing the channel if no other senders remain. `SendClientError` uses `clone_stream` (non-terminal; `SendClientDone` follows and takes the handle).
- Relay wiring: **implemented**. `relay_job` called from worker ws handler, outcome mapped to kernel events.

## Invariants

- `StreamFrame` is the only type sent on the client stream channel -- all worker output is normalized through the relay.
- `relay_job` returns exactly one `RelayOutcome` per invocation (one terminal event per job).
- Chunk relay respects backpressure: if the client channel is full, the relay awaits (bounded by mpsc capacity). If the client channel is closed, relay returns `ClientGone` immediately.

## Status

- Implemented (relay wired into worker ws handler, SSE bridge in chat completions handler, end-to-end streaming path complete).

## Load into context when

- Modifying SSE framing/output behavior.
- Changing chunk ordering/terminal semantics.
- Debugging stream interruptions and disconnect handling.
- Wiring `relay_job` into the worker websocket handler.

## Relevant files

- `crates/prosthetic-conscience/src/gateway/relay.rs`
- `crates/prosthetic-conscience/src/router/worker_ws_upgrade.rs`
- `crates/prosthetic-conscience/src/router/chat_completions.rs`
- `crates/prosthetic-conscience/src/gateway/channel_registry.rs` (`WorkerJob`, `StreamHandle`)
- `crates/prosthetic-conscience/src/gateway/kernel.rs`

## TODO (near-term)

- ~~Wire `relay_job` into worker ws handler~~ (done — `consume_until_terminal` replaced, stream heartbeats and chunk forwarding active).
- ~~Implement chat completions handler~~ (done — SSE output from `mpsc::Receiver<StreamFrame>`, stream registration + `HttpChatRequested` submission).
- ~~Implement channel-based cancellation in effect executors~~ (done — terminal effects use `take_stream` + drop).
- Add explicit chunk/event protocol parsing and ordering guarantees for worker outputs.
