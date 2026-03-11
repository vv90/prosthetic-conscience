# Worker Lifecycle

Snapshot date: 2026-03-07

## Overview

This document traces a worker's full lifecycle from websocket connection to disconnection. The lifecycle spans three layers: the adapter (websocket handler), the core (runtime + kernel + registry), and the transport (websocket).

## Current Architecture

Workers have persistent IDs that survive across jobs. The kernel tracks Idle/Busy transitions. After job completion, the `WorkerJobCompleted` runtime command reinstalls a fresh oneshot and transitions the worker back to Idle.

## Target Architecture

Workers have **one-use IDs**. A worker registers, gets dispatched, is consumed. After job completion, the worker handler re-registers with a fresh ID and fresh oneshot. From the kernel's perspective, it's a new worker every time. No Idle/Busy lifecycle -- the kernel just tracks available workers and active assignments.

## Phases

### Phase 1: Connection and Registration

```
Worker HTTP request -> WebSocket upgrade -> worker_ws_connection starts
  -> Creates oneshot::channel() -> (job_tx, job_rx)
  -> Calls runtime.register_worker(job_tx)
    -> Registry: inserts job_tx, generates WorkerId
    -> Kernel: WorkerRegistered -> adds to available pool
    -> Reply: WorkerId sent back via oneshot
  -> Handler stores worker_id, enters idle loop
```

**State after:**

- Kernel: worker_id in available pool
- Registry: `workers[worker_id] = oneshot::Sender<WorkerJob>`
- Handler: holds `worker_id`, `job_rx`, `socket`

### Phase 2: Idle (waiting for job)

```
tokio::select! {
    job_rx -> job arrives -> Phase 3
    socket.recv() ->
        Close/Error -> Phase 5
        Ping/Pong/other -> continue
}
```

### Phase 3: Job Dispatch

```
Job received via oneshot
  -> Serialize job frame as JSON
  -> socket.send(job_frame)
    -> Ok -> Phase 4
    -> Err -> Phase 5
```

**State during:**

- Kernel: worker_id in assignments (mapped to client_stream_id)
- Registry: handle consumed by `take_worker` -- no entry for this worker
- Handler: holds the `WorkerJob` with `client_tx`

### Phase 4: Job Processing (relay)

**Current:**

```
consume_until_terminal(socket) -> RelayOutcome
  -> Creates fresh oneshot::channel() -> (next_tx, next_rx)
  -> Calls runtime.worker_job_completed(worker_id, outcome, next_tx)
  -> Handler sets pending_job = next_rx, loops back to Phase 2
```

**Target:**

```
relay_job(socket, client_tx) -> RelayOutcome
  -> Relay forwards chunks directly to client via client_tx
  -> On terminal (end/error/disconnect): relay returns
  -> Sends AssignmentCleared to kernel (best-effort)
  -> Re-registers with fresh oneshot -> new WorkerId
  -> Loops back to Phase 2 with new identity
```

Key difference: in the target architecture, the relay handles data delivery directly via `client_tx`. The kernel only needs to know the assignment is done (for bookkeeping), but stream termination is guaranteed by timeout even if this signal never arrives.

### Phase 5: Disconnection

```
Handler breaks out of loop (any reason)
  -> Handler returns, websocket closed
  -> No explicit unregister command sent
```

**State after:**

- Kernel: worker may remain as stale entry (available or assigned)
- If available: stale entry is harmless -- dispatch fails at oneshot send, kernel handles via timeout
- If assigned: timeout fires, kernel emits terminal effects for the client, channel dies, relay stops

## Invariants

| #   | Invariant                                                                  | Status                                                                                                                                                                                                             |
| --- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| L1  | Every stream that enters the kernel gets terminal effects.                 | **Target.** Guaranteed by kernel timeout. Not yet implemented -- currently relies on adapter completion signals.                                                                                                   |
| L2  | A oneshot receiver may be dropped with an unconsumed job (`select!` race). | **Accepted.** The kernel treats the worker as assigned. Timeout handles client notification.                                                                                                                       |
| L3  | Stale entries are harmless.                                                | **Yes.** Disconnected workers leave stale entries. Dispatch to stale entry fails, timeout handles assigned entries. No resource leak beyond map entries. Future heartbeat can proactively clean available entries. |

## Transition Rules

### Registration

| #   | Scenario                                  | Expected behavior                                                                                    | Tested? |
| --- | ----------------------------------------- | ---------------------------------------------------------------------------------------------------- | ------- |
| T1  | Registration succeeds                     | Worker in available pool, handle in registry, handler has ID                                         | No      |
| T2  | Registration fails (runtime gone)         | Handler exits cleanly, nothing registered                                                            | No      |
| T3  | Reply channel dropped during registration | Orphan entry. Dispatch fails at oneshot send. Timeout handles if assigned. Heartbeat cleans if idle. | No      |

### Idle Phase

| #   | Scenario                                                      | Expected behavior                                                              | Tested? |
| --- | ------------------------------------------------------------- | ------------------------------------------------------------------------------ | ------- |
| T4  | Job dispatched while idle                                     | Handler receives job, sends frame to worker, enters Phase 4                    | No      |
| T5  | Websocket close while idle                                    | Handler exits. Stale available entry. Dispatch fails, cleaned by heartbeat.    | No      |
| T6  | Websocket close concurrent with job dispatch (`select!` race) | Job may be dropped. Stale assigned entry. Timeout handles client notification. | No      |

### Job Dispatch

| #   | Scenario                                | Expected behavior                                                         | Tested? |
| --- | --------------------------------------- | ------------------------------------------------------------------------- | ------- |
| T7  | `socket.send` fails during job dispatch | Handler exits. Stale assigned entry. Timeout handles client notification. | No      |

### Job Processing

| #   | Scenario                                 | Expected behavior                                                                                                  | Tested? |
| --- | ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------- |
| T8  | Worker sends `end`                       | Relay delivers done to client via `client_tx`. `AssignmentCleared` sent to kernel. Worker re-registers.            | No      |
| T9  | Worker sends `error`                     | Relay delivers error to client via `client_tx`. `AssignmentCleared` sent to kernel. Worker re-registers.           | No      |
| T10 | Worker disconnects during relay          | Relay detects disconnect, sends error to client via `client_tx`. `AssignmentCleared` sent to kernel (best-effort). | No      |
| T11 | Client disconnects during relay          | `client_tx.send()` fails. Relay stops. `AssignmentCleared` sent to kernel. Worker re-registers.                    | No      |
| T12 | `AssignmentCleared` fails (runtime gone) | Stale assigned entry. Timeout handles. Only during shutdown.                                                       | No      |
| T13 | Multi-job sequence (3 jobs)              | Each cycle: register -> dispatch -> relay -> clear -> re-register -> next dispatch                                 | No      |

### Disconnection

| #   | Scenario                    | Expected behavior                                                           | Tested? |
| --- | --------------------------- | --------------------------------------------------------------------------- | ------- |
| T14 | Clean disconnect while idle | Handler returns. Stale available entry. Cleaned by heartbeat.               | No      |
| T15 | Disconnect while assigned   | Handler returns. Stale assigned entry. Timeout handles client notification. | No      |

## Known Bugs

None.

## Status

- Worker websocket handler: implemented with current architecture (persistent IDs, `WorkerJobCompleted`). Relay is stubbed (`consume_until_terminal`).
- No integration tests for any lifecycle phase.
- Cleanup is internally-driven -- no external unregister commands.
- Target architecture (one-use IDs, re-registration, timeout) not yet implemented.

## Load into context when

- Modifying worker connection/disconnection logic.
- Implementing the relay bridge.
- Writing integration tests for worker lifecycle.
- Planning the refactor to one-use worker IDs.

## Relevant files

- `src/router/worker_ws_upgrade.rs` (handler)
- `src/gateway/runtime.rs` (registration, job completion)
- `src/gateway/channel_registry.rs` (handle storage)
- `src/gateway/kernel.rs` (state transitions)
- `src/gateway/relay.rs` (relay contract)
