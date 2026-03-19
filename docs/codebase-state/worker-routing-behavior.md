# Worker Routing Behavior

Snapshot date: 2026-03-18

## Behavior

- `GET /ws/worker?capabilities=chat,transcription` upgrades to websocket. The `capabilities` query parameter is required and declares which job types the worker can handle. Unknown capability values or missing parameter returns HTTP 400.
- Workers declare capabilities via the URL query string. The gateway parses these using `parse_capabilities()` and passes the resulting `BTreeSet<Capability>` through registration.
- On connect, worker route allocates a per-job `oneshot::Sender<WorkerJob>` and registers it via runtime (`RuntimeHandle::register_worker`), receiving an opaque `WorkerId`.
- The oneshot channel enforces one-job-at-a-time structurally — the runtime cannot queue multiple jobs to a single worker.
- Idle phase uses `tokio::select!` to race job arrival against websocket activity and a heartbeat timer. While idle, the handler sends `WorkerHeartbeat` to the runtime every 15 seconds (via `tokio::time::interval_at` starting one period after registration). The interval resets after each re-registration to avoid stale heartbeats with the old worker ID.
- On receiving a job, the handler sends the job frame to the worker websocket and calls `relay_job` to relay chunks to the client stream channel (`job.client_tx`). `relay_job` sends `StreamHeartbeat` every 10 seconds while relaying.
- After relay completes, the handler maps `RelayOutcome` to kernel events:
  - `WorkerEnd` → `assignment_cleared` (success).
  - `WorkerError { message }` → `assignment_failed` with message. Worker stays connected, re-registers.
  - `WorkerDisconnected` → `assignment_failed` + exit connection loop. Worker is gone.
  - `ClientGone` → no kernel event. Timeout handles cleanup. Worker stays connected, re-registers.
- Worker IDs are one-use: consumed on dispatch, never reused. After each successful job or recoverable error, the handler re-registers with a fresh oneshot and receives a new `WorkerId`.
- On socket close/read failure, the handler exits. Stale kernel/registry entries self-heal when dispatch fails.

## Invariants

- A worker can receive at most one job at a time (enforced by oneshot channel type, not just kernel logic).
- Worker IDs are one-use: each registration produces a fresh ID, dispatch consumes it, and the worker re-registers after job completion.
- Worker disconnect during idle phase is detected promptly via `select!` on websocket, not deferred until next dispatch attempt.
- A job is only dispatched to a worker whose declared capabilities include the required capability for that job type (e.g., chat requests require `Chat` capability).
- Workers without the required capability are invisible to dispatch — they remain idle and are not considered.

## Status

- Implemented (worker websocket registration + capability declaration via query param + capability-based dispatch + oneshot job dispatch + one-use ID flow + relay-based job handling with outcome mapping). Full worker protocol/event integration is complete including capability routing end-to-end.

## Load into context when

- Modifying worker lifecycle/registration logic.
- Changing worker selection or in-flight policies.
- Investigating worker disconnect and dispatch failures.

## Relevant files

- `src/router/mod.rs`
- `src/router/state.rs`
- `src/router/worker_ws_upgrade.rs`
- `src/gateway/runtime.rs`
- `src/gateway/channel_registry.rs`
- `src/gateway/kernel.rs`
- `src/gateway/relay.rs`
- `src/gateway/effects/dispatch_job.rs`

## TODO (near-term)

- Add worker handshake/version validation and explicit registration acknowledgement message format.
