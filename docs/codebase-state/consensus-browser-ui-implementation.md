# Consensus Browser UI Implementation

Snapshot date: 2026-04-05

Status: partial

This document describes the intended implementation process for the browser/WASM consensus UI.

It is deliberately not a plan to recreate the terminal REPL in the browser. The browser app should be treated as a fresh interaction layer with its own pure Rust state machine, its own invariants, and only a minimal browser shell in JavaScript.

Current status:

- the pure browser-facing app boundary is implemented in `crates/consensus/src/app.rs`
- draft-only behavior is separated into `crates/consensus/src/drafts.rs`
- a thin `wasm-bindgen` wrapper now exists in `crates/consensus-wasm`
- the browser shell, session bootstrap/join flow, and broader app surface are still planned

Load into session context when working on: browser/WASM architecture, consensus app boundary design, session coordinator integration, JS↔WASM interface design, UI implementation sequencing.

## Direction

### Product direction

- Do not mirror CLI commands or terminal interaction structure.
- Build the browser interaction layer from the ground up around a stateful app model.
- Keep JavaScript as thin as possible.
- Move as much decision-making and state evolution as possible into pure Rust.
- Treat the browser shell as an adapter for:
  - DOM rendering
  - websocket and HTTP I/O
  - timers
  - auth token storage/attachment
  - browser-only features such as microphone access

### Process direction

Work in small increments. For each increment:

1. Establish the app interface for that slice.
2. Establish the constraints for that slice.
3. Establish the correctness properties for that slice.
4. Implement the logic for that slice.

Then repeat.

This is the same discipline used elsewhere in the codebase: decisions belong in the pure core, not in adapters.

## Correctness Properties vs Constraints

Per [`testing-methodology-and-invariants.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/testing-methodology-and-invariants.md), invariants are universal correctness properties over reachable app states and valid app transitions. They are not the same thing as boundary rules, purity rules, or coding-style rules.

For this browser/WASM work, keep two separate lists in every increment:

- `Constraints`: rules about the Rust/JS boundary, adapter simplicity, and API discipline.
- `Correctness properties`: semantic properties that should hold for all reachable app states or valid app transition sequences.

## Target architecture

The long-term target is a pure app reducer in `crates/consensus/` that sits above the lower-level consensus engine and session coordination logic.

Conceptually:

- `ConsensusApp`
  - Owns the browser-facing app state.
  - Accepts user events and transport facts.
  - Produces:
    - next pure state
    - derived view model
    - requested browser-side effects
- `SessionCoordinator`
  - Owns session sync policy, catch-up, reconnect recovery, append gating, and submission progress.
  - Long-term target: subsume the responsibilities currently split between the coordinator reducer and `EntryBuffer`.
- `ConsensusEngine`
  - Owns deliberation semantics, draft semantics, preview, impact analysis, and rendering data.
- JS shell
  - Executes requested effects and feeds resulting facts back into the pure app.

The important boundary is:

- Rust decides.
- JS observes, renders, and performs I/O.

## First-class app boundary

The first major milestone is not "the whole UI". It is a stable pure app boundary.

That boundary should be app-shaped rather than CLI-shaped:

- user intent goes in as typed app inputs
- browser/network facts go in as typed app inputs
- requested I/O comes out as typed app effects
- rendered state comes out as a typed view model

This boundary should not expose internal sequencing details like:

- direct engine mutation methods
- manual draft submission bookkeeping
- imperative catch-up loops in JS
- JS-managed duplicate sources of truth for session state

## Minimal initial scope

The first browser prototype should be smaller than the eventual product UI from [`ui-visual-design-spec.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/ui-visual-design-spec.md).

Initial scope:

- session create/join controls
- connection state
- current overview
- current attention list
- current drafts
- explicit structured contribution authoring
- explicit submit / clear / remove draft actions

Out of scope for the first slice:

- voice interaction
- transcription
- LLM-mediated conversation
- attempt to preserve REPL commands or terminal affordances

## Incremental implementation loop

Every increment should explicitly document five things:

1. Interface added
2. Constraints added
3. Correctness properties added
4. Logic added
5. Compatibility/impact notes

The compatibility/impact note is required because this work sits on top of already-existing engine and coordinator logic.

Questions each increment should answer:

- Does this change the JS↔WASM boundary?
- Does this duplicate state already owned by engine or coordinator?
- Does this make future coordinator integration harder?
- Does this require reshaping existing pure types?
- Does this preserve content-opacity and adapter simplicity?

## Planned increments

### Increment 0: Establish the app skeleton

Goal: create the browser-facing pure app boundary without committing to full session logic yet.

#### Interface

Introduce the app-layer types in `crates/consensus/`:

- `app::{State, Event, Effect, View, Transition}`
- `drafts::{State, Event, Effect, View, Notice, Transition}`

The app `Event` surface should stay parent-shaped while delegating to child
reducers where that is already the real ownership boundary. For the current
slice, that means wrapper variants such as:

- `DraftsEvent { event: drafts::Event }`
- `CoordinatorEvent { event: coordinator::Event<Entry> }`

Future app-owned events such as create/join/connected/disconnected can still
exist directly on the parent reducer when no child reducer owns that domain yet.

The initial app `Effect` set should also stay small. When the parent is mainly
delegating, effects may be wrapped child effects rather than translated:

- `CoordinatorEffect { effect: coordinator::Effect<Entry> }`

The first version does not need to solve every transport edge case.

#### Constraints

- The app layer performs no I/O directly.
- JS does not call lower-level engine or coordinator methods directly.
- No browser shell contract is created below the app layer.

#### First correctness properties

- `APP1` — Local-only app inputs never mutate the committed session log.
- `APP2` — Removing one draft either leaves the draft set unchanged with an error, or removes exactly one draft while preserving the relative order of the remaining drafts.
- `APP3` — Every draft ID exposed in `View` corresponds to exactly one pending draft in app state; the view does not invent, omit, or duplicate drafts.

#### Logic

- Define the app state machine skeleton.
- Thread local draft creation and removal through the app layer.
- Return a derived `View` from Rust, even if initially small.

#### Concrete first implementation subset

The first shipped increment should be narrower than the full Increment 0 sketch above:

- Add `crates/consensus/src/app.rs` with:
  - `State { coordinator, drafts }`
  - `Event::{DraftsEvent { event: drafts::Event }, CoordinatorEvent { event: coordinator::Event<Entry> }}`
  - `Effect::CoordinatorEffect { effect: coordinator::Effect<Entry> }`
  - `View { overview, drafts, notice }`
  - `Transition`
- Add `crates/consensus/src/drafts.rs` with:
  - `State`
  - local `Event::{DraftClaimRequested, RemoveDraftRequested}`
  - empty local `Effect`
  - `Notice`
  - `View { drafts, notice }`
  - `Transition`
- Keep this slice narrow:
  - no session create/join behavior
  - no submission effects
  - no browser shell or transport adapter
- Derive `View` directly from Rust-owned state on every call:
  - `overview` from the coordinator's contiguous committed prefix
  - `drafts` from Rust-owned `drafts::State`
  - `notice` from Rust-owned `drafts::Notice`
- Delegate parent events structurally:
  - `DraftsEvent` passes through unchanged to `drafts::reduce()`
  - `CoordinatorEvent` passes through unchanged to `coordinator::reduce()`
  - coordinator effects are rewrapped rather than translated into app-shaped fetch variants

#### Enforcement / evidence notes for the first subset

- `APP1`
  - Structural/type design: local draft events are delegated entirely to `drafts`, while committed entries are delegated entirely to `coordinator`. The app does not expose append/submission APIs.
  - Test evidence: unit/property tests compare committed log and committed overview before/after local traces.
- `APP2`
  - Structural design: draft ordering remains `drafts::State`-owned; the app layer never rebuilds or reorders the draft sequence.
  - Test evidence: targeted unit tests plus successful-removal property tests.
- `APP3`
  - Structural design: `View.drafts` is derived directly from `drafts::view()` with no duplicate app-layer cache.
  - Test evidence: unit/property tests compare `View.drafts` to draft state after arbitrary local traces.

#### Compatibility / impact

- No browser shell contract should be created below the app layer.
- Existing terminal code can continue using current internals unchanged.
- This increment should not force immediate coordinator redesign.

### Increment 1: Own local deliberation interaction

Goal: move local draft and overview interaction fully behind the app boundary.

#### Interface

Expand app `Event` only for local interaction:

- `DraftRelationRequested`
- `DraftStanceRequested`
- `DraftResolveRequested`
- `DraftCommentRequested`
- `ClearDraftsRequested`
- `SelectClaimRequested`

Expand app `View` to include:

- overview
- selected claim detail
- drafts
- impact analysis
- local errors/warnings

#### Constraints

- Draft state stays Rust-owned; JS renders view data rather than reconstructing draft state.
- Rendering data crosses the boundary as view data, not as engine-internal mutation hooks.

#### Correctness properties

- `LD1` — Local interaction inputs never append to the committed session log.
- `LD2` — Every draft-local reference in reachable app state points either to a committed claim or to an existing draft claim.
- `LD3` — Clearing drafts removes all preview-only changes while leaving committed-log-derived state unchanged.

#### Logic

- Route local user intents through engine-backed app logic.
- Keep rendering data Rust-owned and serialize only view models.

#### Compatibility / impact

- This should reduce future JS surface area rather than expand it.
- This should not yet assume the final session coordinator contract.

### Increment 2: Integrate session synchronization

Goal: give the app a pure session-sync core without making JS responsible for coordination policy.

#### Interface

Add transport-fact inputs such as:

- `CoordinatorEvent { event: coordinator::Event::Received { index, entry } }`
- `SessionWarningObserved { message }`
- `CatchUpPageObserved { from, entries, total }`
- `CatchUpCompleteObserved`
- `TransportConnected`
- `TransportDisconnected { reason }`

Add transport effects such as:

- `CoordinatorEffect { effect: coordinator::Effect<Entry> }`
- `SendSessionMessage { payload }`
- `DrainQueuedTransportEvents`

#### Constraints

- Session-log sequencing stays behind the app/coordinator boundary.
- JS does not own reconnect, replay, or append sequencing policy.

#### Correctness properties

- `SS1` — Log replay semantics are identical regardless of whether entries arrive live or via catch-up.
- `SS2` — Connection loss never discards local drafts or decreases confirmed replay progress.
- `SS3` — While catch-up is active, the app emits no append/send effect that bypasses coordinator gating.

#### Logic

- Fold current session-sync decisions behind the pure app boundary.
- Use the session coordinator as the source of truth for replay and submission policy.
- Long-term target: remove `EntryBuffer` as a separate long-lived abstraction once coordinator logic fully subsumes its responsibilities.

#### Compatibility / impact

- This increment is the main bridge between current CLI logic and future browser logic.
- Changes here must be cross-checked against [`session-coordinator-behavior.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/session-coordinator-behavior.md).

### Increment 3: Submission and recovery properties

Goal: make submission progress, reconnect recovery, and echo-based confirmation fully pure.

#### Interface

Add only the minimum extra inputs/effects needed for append-result reporting and recovery:

- `AppendAccepted`
- `AppendRejected { reason }`
- `SubmissionEchoObserved { index, payload }`

#### Correctness properties

- `SB1` — Submission progress advances only from echoed session entries, not from send acceptance alone.
- `SB2` — Reconnect recovery never forgets already-confirmed submission progress.
- `SB3` — The app never requests append `N + 1` before append `N` is confirmed according to coordinator policy.
- `SB4` — Submission completion is emitted at most once per submission attempt.

#### Logic

- Move pending-submission recovery entirely behind the pure app boundary.
- Keep JS limited to reporting transport outcomes.

#### Compatibility / impact

- This increment should eliminate the need for JS-side submission bookkeeping.

### Increment 4: WASM wrapper and browser shell

Goal: expose the already-pure app to the browser with the thinnest possible wrapper.

#### Interface

The JS-facing wasm surface should be opaque and app-level:

- construct app state
- read the current view
- feed one committed entry
- receive next view and requested effects

The wrapper should not expose engine internals, coordinator internals, or ad hoc helper methods unless proven necessary.

#### Constraints

- The JS-facing WASM surface remains app-level and opaque.
- JS executes effects but does not reinterpret consensus semantics.

#### Correctness properties

- `WB1` — Feeding the same input sequence through the app directly or through the WASM wrapper yields observably equivalent view/effect sequences.

#### Logic

- Current implementation: `crates/consensus-wasm` exposes `ConsensusAppHandle::{new, bootstrap, view, receiveEntry}` over the pure app boundary.
- The wrapper intentionally does not expose generic event dispatch, draft actions, or engine/coordinator internals yet.
- Keep serialization formats explicit and narrow.

#### Compatibility / impact

- A stable app boundary should let the wasm wrapper remain thin even as internals evolve.

## Immediate coding order

The next implementation work should proceed in this order:

1. Expand the app boundary beyond the current receive/render slice to cover more local interaction.
2. Move session bootstrap, catch-up, and reconnect policy behind the pure app/coordinator boundary.
3. Build the first minimal browser shell against `crates/consensus-wasm`.

## Explicit non-goals

- Do not start from terminal commands and port them one-for-one into buttons.
- Do not let JS accumulate coordination logic "temporarily" without documenting which constraint or correctness property would be violated.
- Do not make the initial browser prototype depend on LLM conversation or voice.
- Do not expose a large wasm API just because lower-level Rust methods already exist.

## Related documents

- [`core-logic-invariants.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/core-logic-invariants.md)
- [`testing-methodology-and-invariants.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/testing-methodology-and-invariants.md)
- [`session-coordinator-behavior.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/session-coordinator-behavior.md)
- [`todo-near-term.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/todo-near-term.md)
- [`ui-visual-design-spec.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/ui-visual-design-spec.md)
