# Session Coordinator Behavior

Snapshot date: 2026-04-03

Status: **proposed** — target pure state machine for client-side session synchronization, catch-up orchestration, and submission resume. `EntryBuffer` and `BackoffPolicy` already live in `consensus`; the coordinator itself is the planned extraction from `consensus_cli/app.rs`.

Load into session context when working on: session coordinator extraction, browser/WASM session wrappers, catch-up and reconnect behavior, pending submission recovery, coordinator property tests.

## Behavior

- `SessionCoordinator` is the proposed pure source of truth for client-side session sync policy.
- It consumes `CoordinatorEvent` values from an impure shell and returns `CoordinatorAction` values for the shell to execute.
- It performs no I/O, no async waiting, no sleeping, no printing, and no direct access to websocket, HTTP, or channels.
- It owns only pure decision state:
  - `EntryBuffer` — applied session history, out-of-order buffering, pending submission tracking.
  - `connected: bool` — whether the shell currently has a working live session transport.
  - `catching_up: bool` — whether the shell is currently replaying missing entries via HTTP.
  - `append_in_flight: bool` — whether the shell has already been asked to send one append and the send result has not yet been reported back.
  - `catch_up_page_limit: usize` — page size requested for catch-up fetches.
- The shell remains responsible for:
  - websocket connection management and reconnect backoff
  - HTTP `GET /v1/sessions/:id/entries`
  - websocket append sends
  - draining queued transport events
  - rendering/logging/UI output
  - converting transport results into coordinator events

The design goal is to replace the current imperative control flow spread across `handle_session_event`, `catch_up`, `drain_queued_session_events`, and `resume_pending_submission` with a pure event/action loop.

## Event / Action Contract

### Events

The shell reports only facts that already happened:

- `Entry { index, payload }`
  - A live session entry arrived from websocket or any other transport source.
- `Disconnected { reason }`
  - The live session transport was lost.
- `Reconnected`
  - The live session transport was restored and is ready for catch-up.
- `Warning(String)`
  - A non-fatal session warning arrived from the transport.
- `CatchUpPage { entries }`
  - One HTTP page of missing session entries was fetched.
- `CatchUpComplete`
  - The shell determined there are no more catch-up pages to fetch.
- `AppendAcknowledged`
  - The shell successfully handed one append message to the transport.
- `AppendFailed { reason }`
  - The shell failed to hand one append message to the transport.

### Actions

The coordinator requests work but does not perform it:

- `FetchEntries { from, limit }`
  - Fetch a catch-up page from the session HTTP endpoint.
- `SendAppend(Value)`
  - Send one append message over the live session transport.
- `DrainQueuedEvents`
  - Immediately drain any already-buffered transport events and feed each one back through `handle_event`.
- `EntryApplied { index }`
  - A session entry became newly applied to the consensus engine.
- `EntrySkipped { index, error }`
  - A session payload consumed a log index but was not a valid consensus entry.
- `SubmissionComplete`
  - The current pending submission was fully echoed and finalized.
- `SubmissionPaused`
  - Submission progress is blocked on reconnect or a transport retry boundary.
- `ConnectionChanged { connected }`
  - Observational connection-state update for shell/UI.
- `WarningReceived(String)`
  - Observational warning for shell/UI.

## Coordinator State

### Core state

- `buffer: EntryBuffer`
  - Tracks applied entries, out-of-order buffering, and pending submission echo progress.
- `connected: bool`
  - Starts `true` in the current plan because startup occurs from a connected session handshake.
- `catching_up: bool`
  - Starts `false`. Set `true` only after reconnect or any other transition that explicitly enters catch-up mode.
- `append_in_flight: bool`
  - Starts `false`. Set `true` only when the coordinator emits `SendAppend`.
- `catch_up_page_limit: usize`
  - Fixed constructor parameter used by `FetchEntries`.

### Relationship to `EntryBuffer`

`EntryBuffer` remains responsible for the local log/application semantics:

- `next_index()` is the next expected session log index.
- `apply_or_buffer_entry()` handles duplicate, future, and contiguous entries.
- `begin_submission()` snapshots local drafts into concrete payloads.
- `note_submission_payload()` advances submission progress only when the expected echoed payload is observed.
- `finish_submission()` removes the submitted drafts after the entire pending submission is confirmed.

The coordinator is responsible for when those pure operations are invoked and what actions follow from them.

## Invariants

These are universal properties that must hold for every reachable coordinator state after any valid event sequence.

### Structural invariants

- `SC1` — The coordinator performs no I/O.
  - All network, timer, channel, and UI work is expressed only through returned actions.
- `SC2` — Session-management decisions are centralized in the coordinator.
  - Connection state, catch-up state, append send gating, and submission resume policy must not be duplicated as independent sources of truth in the shell.
- `SC3` — State transitions are deterministic.
  - For a fixed starting state and event sequence, the coordinator must always produce the same next state and same ordered action sequence.
- `SC4` — Event and action classification helpers must use explicit match arms with no wildcard.
  - Adding a new event or action variant must force the author to decide how it affects invariants and tests.

### Log synchronization invariants

- `LG1` — `buffer.next_index()` is monotonic non-decreasing.
- `LG2` — Every session index `< buffer.next_index()` has already been finalized exactly once as either applied or skipped.
- `LG3` — An entry is only applied when its index equals the next missing session log index.
- `LG4` — If an entry arrives with `index > buffer.next_index()`, it is buffered and not applied early.
- `LG5` — Once a gap is filled, any now-contiguous buffered entries are drained immediately in ascending index order.
- `LG6` — Duplicate delivery of any already-finalized index is a no-op.
- `LG7` — Invalid or non-consensus payloads still consume their log index and therefore advance the replay frontier.
- `LG8` — The applied engine state depends only on the indexed session log prefix, not on whether entries arrived via live websocket events or HTTP catch-up pages.
- `LG9` — `EntryApplied` and `EntrySkipped` actions correspond exactly to entries that became newly contiguous in that transition.

### Connection invariants

- `CN1` — `connected` changes only in response to `Disconnected` and `Reconnected` events.
- `CN2` — Duplicate disconnects and duplicate reconnects are idempotent.
  - They must not emit duplicate `ConnectionChanged` actions when the boolean state did not actually change.
- `CN3` — Loss of connection never discards buffered future entries or pending submission progress.
- `CN4` — `Warning` events are observational only.
  - They emit `WarningReceived` but do not change connection, catch-up, or submission state.

### Catch-up invariants

- `CU1` — Entering catch-up always starts from `buffer.next_index()`.
- `CU2` — While `catching_up == true`, the coordinator must not emit `SendAppend`.
- `CU3` — `FetchEntries` always uses the current replay frontier as its `from` cursor.
- `CU4` — Catch-up mode ends only on `CatchUpComplete`.
- `CU5` — `CatchUpComplete` emits `DrainQueuedEvents` before any submission-resume send is emitted.
  - This is the race boundary between HTTP replay and already-queued live websocket events.
- `CU6` — Catch-up page processing uses the same entry application path and invariants as live entry processing.
- `CU7` — A full catch-up page may request another `FetchEntries`; a non-full final page does not itself finalize catch-up unless the shell reports `CatchUpComplete`.

### Submission invariants

- `SB1` — `pending_submission.next_entry` is monotonic non-decreasing.
- `SB2` — Submission progress advances only when the echoed session payload matches the expected pending payload.
- `SB3` — `AppendAcknowledged` does not by itself advance submission progress.
  - It only clears the send-in-flight barrier.
- `SB4` — `AppendFailed` does not advance submission progress.
  - It only clears the send-in-flight barrier and may trigger `SubmissionPaused`.
- `SB5` — The coordinator never emits `SendAppend` for payload `N + 1` before payload `N` has been echoed.
- `SB6` — The coordinator never emits `SendAppend` unless all of these are true:
  - a submission is pending
  - `connected == true`
  - `catching_up == false`
  - `append_in_flight == false`
- `SB7` — `begin_submission()` snapshots a fixed payload sequence for one submission attempt.
- `SB8` — `begin_submission()` with no drafts is a pure no-op.
- `SB9` — Disconnecting during a pending submission pauses progress but does not lose already confirmed entries.
- `SB10` — `SubmissionComplete` may only be emitted after every payload in the pending submission has been echoed and `finish_submission()` has succeeded.
- `SB11` — `SubmissionComplete` is emitted at most once per pending submission.

### Action-contract invariants

- `AC1` — `ConnectionChanged { connected }` is emitted if and only if the connection-state boolean actually changed.
- `AC2` — `SubmissionPaused` is emitted only when progress is blocked by transport state rather than by local draft state.
- `AC3` — `DrainQueuedEvents` is a coordinator request to the shell, not a state mutation by itself.
- `AC4` — `AppendAcknowledged` means only that the transport accepted the send request, not that the entry exists in the session log.
- `AC5` — `CatchUpComplete` means only that the shell has exhausted catch-up fetches, not that queued live events have already been drained.

### Conditional liveness invariants

- `LV1` — If missing indices eventually arrive, every buffered out-of-order entry is eventually finalized as applied or skipped.
- `LV2` — If the transport eventually reconnects and the shell keeps executing requested fetches, catch-up eventually reaches the current session log frontier.
- `LV3` — If a pending submission’s payloads are eventually sent successfully and echoed back in order, the coordinator eventually emits `SubmissionComplete`.

## Lifecycle Rules

### Startup

- The current plan constructs the coordinator in connected, non-catch-up state.
- The shell may still choose to perform an initial catch-up fetch at startup, but the target pure design treats this as the same `FetchEntries` / `CatchUpPage` / `CatchUpComplete` contract used after reconnect.

### Live entry flow

1. The shell receives a live session entry.
2. It emits `CoordinatorEvent::Entry { index, payload }`.
3. The coordinator delegates to `EntryBuffer::apply_or_buffer_entry()`.
4. For each newly contiguous result:
  - `ApplyResult::Applied` becomes `EntryApplied { index }`
  - `ApplyResult::Skipped` becomes `EntrySkipped { index, error }`
5. If the echoed payload advances a pending submission and there is more work to send, the coordinator may emit the next `SendAppend`, subject to `SB6`.

### Disconnect and reconnect flow

1. The shell detects transport loss and emits `Disconnected { reason }`.
2. The coordinator sets `connected = false`, emits `ConnectionChanged { connected: false }`, and may emit `SubmissionPaused`.
3. The shell eventually restores the transport and emits `Reconnected`.
4. The coordinator sets `connected = true`, `catching_up = true`, emits `ConnectionChanged { connected: true }`, then emits `FetchEntries { from: buffer.next_index(), limit }`.

### Catch-up flow

1. The shell executes `FetchEntries`.
2. Each HTTP page becomes `CatchUpPage { entries }`.
3. The coordinator applies each entry under the same log-ordering rules as live traffic.
4. When the shell determines there are no more pages, it emits `CatchUpComplete`.
5. The coordinator exits catch-up, emits `DrainQueuedEvents`, and only after that may emit the next `SendAppend` for a paused pending submission.

### Submission flow

1. The shell calls `begin_submission(next_claim_id)`.
2. If no drafts exist, the result is `None` and no actions are emitted.
3. Otherwise the coordinator snapshots the submission payloads inside `EntryBuffer`.
4. If `SB6` is satisfied, the coordinator emits `SendAppend(first_payload)` and sets `append_in_flight = true`.
5. The shell reports `AppendAcknowledged` or `AppendFailed`.
6. Real progress occurs only when the matching echoed `Entry` arrives and advances `pending_submission.next_entry`.
7. Once all entries are echoed, the coordinator finalizes drafts via `finish_submission()` and emits `SubmissionComplete`.

## Current vs Target Architecture

### Current implementation

The current CLI behavior is correct but encoded imperatively across:

- `crates/prosthetic-conscience/src/consensus_cli/app.rs`
  - `handle_session_event`
  - `catch_up`
  - `drain_queued_session_events`
  - `resume_pending_submission`
- `crates/prosthetic-conscience/src/consensus_cli/session.rs`
  - reconnect loop and transport event stream
- `crates/consensus/src/entry_buffer.rs`
  - pure entry buffering and submission echo tracking

### Target implementation

- `crates/consensus/src/coordinator.rs`
  - new pure coordinator state machine
- CLI shell
  - converts `SessionEvent` and HTTP/send outcomes into coordinator events
  - executes returned coordinator actions
- Browser/WASM shell
  - reuses the same pure coordinator with a different transport adapter

The core design principle is the same one used in the gateway kernel: if it is a decision, it belongs in the pure state machine. If it is I/O, it belongs in the shell.

## Status

- `EntryBuffer` is implemented and already satisfies the pure log-application and submission-echo portion of the design.
- `BackoffPolicy` is implemented as a pure helper in `consensus`.
- `SessionCoordinator` is not yet implemented.
- Current CLI behavior is the reference behavior that the coordinator extraction must preserve.

## Load into Context When

- Extracting session coordination logic from `consensus_cli/app.rs`.
- Designing property tests for reconnect, catch-up, and submission resume.
- Reviewing whether a new event or action variant preserves the state machine invariants.
- Building a browser/WASM session wrapper around the consensus core.

## Relevant Files

- `crates/prosthetic-conscience/src/consensus_cli/app.rs`
- `crates/prosthetic-conscience/src/consensus_cli/session.rs`
- `crates/consensus/src/entry_buffer.rs`
- `crates/consensus/src/backoff.rs`
- `crates/consensus/src/protocol.rs`
- `crates/consensus/src/coordinator.rs` (planned)
- `docs/codebase-state/session-behavior.md`
- `docs/codebase-state/core-logic-invariants.md`
