# Entrypoint Behavior

Snapshot date: 2026-03-01

## Behavior
- Program initializes tracing and starts an Axum server on `127.0.0.1:3000`.
- Exposes:
  - worker websocket endpoint: `/ws/worker`
  - chat completions endpoint: `POST /v1/chat/completions`

## Status
- Implemented.

## Load into context when
- Changing startup/runtime server boot behavior.
- Changing bind address or tracing initialization behavior.

## Relevant files
- `src/main.rs`
- `src/router/mod.rs`
- `src/router/state.rs`

## TODO (near-term)
- Make bind address configurable.
