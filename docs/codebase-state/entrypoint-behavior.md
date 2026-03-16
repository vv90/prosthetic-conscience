# Entrypoint Behavior

Snapshot date: 2026-03-16

## Binaries

### Gateway (`prosthetic-conscience`)

- Initializes tracing and starts an Axum server.
- CLI args: `--host` (default `127.0.0.1`), `--port` (default `3000`).
- Reads `PC_AUTH_TOKEN` env var for optional bearer auth.
- Exposes:
  - worker websocket endpoint: `/ws/worker`
  - chat completions endpoint: `POST /v1/chat/completions`

### Worker (`pc-worker`)

- Connects outbound to gateway via WebSocket, receives jobs, proxies to inference server.
- CLI args: `--gateway-url`, `--inference-url`, `--auth-token`.
- Reconnects with exponential backoff (1s to 30s cap).

### Client (`pc-client`)

- Interactive REPL that sends chat completion requests to the gateway.
- CLI args: `--gateway-url`, `--auth-token`, `--model`, `--system`.
- Reads user input from stdin, sends to gateway, assembles streamed SSE response, prints content.
- Maintains conversation history across turns.

## Status

- All three binaries implemented.

## Load into context when

- Changing startup/runtime server boot behavior.
- Changing bind address or tracing initialization behavior.
- Adding new CLI args to any binary.

## Relevant files

- `src/main.rs` (gateway)
- `src/worker_agent.rs` (worker)
- `src/client_agent.rs` (client)
- `src/router/mod.rs`
- `src/router/state.rs`
- `src/client/gateway_client.rs`
- `src/client/response_assembler.rs`
