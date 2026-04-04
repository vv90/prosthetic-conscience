# Testing Methodology and Invariants

Snapshot date: 2026-04-04

Status: canonical reference

This document defines how the repository uses terms such as "invariant", "constraint", "transition rule", and "property test".

Load into session context when working on: new core logic, reducer/coordinator behavior, correctness-property design, property tests, structural enforcement choices, and test-plan reviews.

## Core Definitions

### Invariant

An invariant is a correctness property that must hold for every reachable state of a component after any valid sequence of operations on that component.

- It is not limited to state machines or reducers.
- It may apply to any component with meaningful state transitions.
- The relevant observables may include component state and semantically relevant returned outputs or effects.
- Violating an invariant indicates a correctness bug, not merely a style or architecture concern.

Historically, some docs in this repo also use "invariant" as a loose umbrella term for temporal properties such as eventual termination. Those are not strict single-state invariants, but they are still correctness properties and should be labeled clearly when listed.

### Constraint

A constraint is a design or implementation rule, not a universal semantic property of reachable states.

Examples:

- "The reducer performs no I/O directly."
- "JS must not call lower-level engine methods directly."
- "Every enum variant must be matched explicitly."

Constraints are important, but they should not be listed as invariants unless they are restated as actual correctness properties over reachable states or transitions.

### Transition Rule

A transition rule describes what one operation does in one class of cases.

Examples:

- "Connected sets `connected = true` and emits `FetchMissing`."
- "Removing an unknown draft returns `DraftNotFound`."

Transition rules are usually covered by targeted unit tests.

### Scenario Test / Regression Test

A scenario test checks one concrete case. It can demonstrate that a specific case works or prevent a known bug from reappearing, but by itself it cannot establish a universal invariant.

## Enforcement vs Test Evidence

Only construction-level mechanisms enforce invariants.

### Preferred enforcement order

1. Structural enforcement
2. Type-system enforcement
3. Semantic properties backed by broad test evidence

### Structural enforcement

Choose data structures and APIs that make violation impossible by construction.

Examples:

- uniqueness enforced by `HashMap` / `BTreeMap` key spaces
- one-shot delivery enforced by `oneshot::Sender`
- monotonic ownership transfer enforced by consuming operations

### Type-system enforcement

Use opaque types, visibility, ownership, and distinct ID spaces so invalid states or illegal operations are unrepresentable.

Examples:

- opaque `SessionId` / `WorkerId` constructors
- distinct ID types for different identity spaces
- module-private constructors that prevent forgery

### What tests can and cannot do

- Unit tests do not enforce invariants and cannot establish universal properties by their nature. They check selected scenarios and transition rules.
- Property tests do not enforce invariants either. They provide broader sampled evidence by exploring many generated traces or state transitions.
- Integration tests provide evidence about cross-component contracts and end-to-end behavior, not universal proof.

Use property tests when a property matters semantically but cannot be made impossible by structure or types. Even then, record them as "property-tested", not "enforced".

## Writing a Good Invariant

A good invariant should:

- quantify over every reachable state or valid transition sequence in the component's scope
- be stated in terms of stable observables at the component boundary
- catch a meaningful correctness bug if violated
- avoid unnecessary coupling to temporary implementation details
- name any temporal assumptions explicitly when the property is liveness-like

### Screening questions

Before calling something an invariant, ask:

1. Would violating this mean the component is semantically wrong?
2. Is it universal rather than about one operation or one example?
3. Can it be stated in terms of reachable state, valid transitions, and boundary-observable outputs?
4. Can it be enforced structurally or by types instead of only being tested?
5. If only tested, is the test strategy broad enough to provide useful evidence?

If the statement is really about layering, purity, coding style, or API discipline, put it under `Constraints` instead.

## Recommended Doc Structure

When documenting a component, keep these sections separate:

1. Constraints
2. Correctness properties
3. Transition rules
4. Test coverage / evidence

For each correctness property, document:

- ID
- property statement
- scope
- observables
- enforcement or evidence status
- why it matters

## Examples From This Repo

Good correctness properties:

- `tick_counter_is_monotonic`
- `invariant_i5_all_streams_eventually_terminate`
- "received slots are never overwritten"
- "submission progress advances only from echoed payloads"

Good structurally/type-enforced properties:

- uniqueness of keyed entities in maps
- one in-flight worker job enforced by `oneshot::Sender`
- opaque ID construction preventing forged identities

Not invariants:

- "the reducer performs no I/O directly"
- "every match arm is explicit"
- "JS is not a second reducer"

Those belong under constraints.

## Testing Methodology Summary

- Use structural and type-system enforcement first.
- Use unit tests for transition rules, edge cases, and regressions.
- Use property tests for semantic properties that remain after structural/type design.
- Use integration tests for boundary contracts and end-to-end behavior.
- Do not describe a property as "enforced by tests". Tests provide evidence; only construction enforces.

## Related Documents

- [`core-logic-invariants.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/core-logic-invariants.md)
- [`session-coordinator-behavior.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/session-coordinator-behavior.md)
- [`consensus-browser-ui-implementation.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/consensus-browser-ui-implementation.md)
- [`testing-coverage.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/testing-coverage.md)
