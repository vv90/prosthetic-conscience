# Streaming Behavior

Snapshot date: 2026-02-27

## Behavior
- Stream partial responses in order with clear start/chunk/end/error semantics.

## Status
- Not implemented.

## Load into context when
- Implementing token/chunk streaming.
- Debugging stream ordering/termination/cancellation.
- Analyzing transport-level streaming behavior.

## Relevant files
- No implementation files yet.
- Specification source: `docs/gateway-specification.md`

## TODO (near-term)
- Add first failing test for ordered chunk streaming behavior.
