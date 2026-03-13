# Prosthetic Conscience

An LLM gateway that routes client requests to GPU workers over reverse-initiated WebSocket connections. Workers dial out to the gateway — no VPN or inbound ports needed on the inference side.

## Architecture

```
Client (HTTP)  -->  Gateway (public)  <--  Worker (WebSocket)  -->  llama-server (local)
```

- **Gateway** accepts OpenAI-compatible HTTP requests and streams SSE responses back to clients. Dispatches jobs to workers over persistent WebSocket connections. Pure kernel architecture with tick-based timeouts and heartbeats.
- **Worker** (`pc-worker`) connects outbound to the gateway, receives jobs, proxies them to a local llama-server (or any OpenAI-compatible inference endpoint), and streams chunks back.
- **Client** sends standard `POST /v1/chat/completions` requests with `"stream": true`.

The gateway never persists prompt or completion content.

## Building

Requires Rust 2024 edition (1.85+).

```
cargo build --release
```

This produces two binaries: `prosthetic-conscience` (gateway) and `pc-worker` (worker agent).

## Running

### 1. Start an inference server

Any OpenAI-compatible endpoint works. With llama.cpp:

```
llama-server -m model.gguf --port 8080
```

### 2. Start the gateway

```
cargo run --bin prosthetic-conscience
```

Options:

```
--host <HOST>  Host address to bind to [default: 127.0.0.1]
--port <PORT>  Port to listen on [default: 3000]
```

### 3. Start a worker

```
cargo run --bin pc-worker
```

Options:

```
--gateway-url <URL>      Gateway WebSocket URL [default: ws://127.0.0.1:3000/ws/worker]
--inference-url <URL>    Inference server base URL [default: http://127.0.0.1:8080]
--auth-token <TOKEN>     Bearer token for gateway auth (must match PC_AUTH_TOKEN)
```

### 4. Send a request

```
curl -N http://127.0.0.1:3000/v1/chat/completions \
  -H "Authorization: Bearer <your-token>" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "test",
    "stream": true,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

(The `Authorization` header is only required when `PC_AUTH_TOKEN` is set on the gateway.)

You should see SSE chunks streaming back:

```
data: {"choices":[{"delta":{"content":"Hi"},...}],...}
data: {"choices":[{"delta":{"content":" there"},...}],...}
data: [DONE]
```

### 5. Or use Open WebUI

Any OpenAI-compatible client works. For a full chat interface, run [Open WebUI](https://github.com/open-webui/open-webui):

```
docker run -d \
  --name open-webui \
  -p 8080:8080 \
  -e OPENAI_API_BASE_URL=https://your-gateway-domain/v1 \
  -e OPENAI_API_KEY=<your-auth-token> \
  ghcr.io/open-webui/open-webui:main
```

Then open `http://localhost:8080`.

## Testing

```
cargo test --all-targets --all-features
```

79 tests: 70 unit (kernel + protocol + registry), 9 integration (full pipeline through real HTTP/WebSocket).

## Project structure

```
src/
  main.rs              Gateway binary
  worker_agent.rs      Worker binary
  lib.rs
  protocol.rs          Shared wire types (WorkerMessage, GatewayToWorker, ChatRequest)
  gateway/
    kernel.rs          Pure state machine: reduce(state, event) -> (state, effects)
    runtime.rs         Async message loop, effect execution, tick driver
    relay.rs           Job relay: worker WS <-> client SSE channel
    channel_registry.rs  Worker/stream handle storage
  router/
    mod.rs             Axum router setup
    chat_completions.rs  POST /v1/chat/completions handler
    worker_ws_upgrade.rs  WS /ws/worker upgrade handler
  worker/
    client.rs          Gateway WS client with reconnection
    inference.rs       HTTP SSE proxy to inference server
tests/
  integration.rs       End-to-end tests
  support/             Test harness (TestGateway, MockWorker, SseClient)
docs/
  gateway-specification.md
  implementation-plan.md
  codebase-state/      Living documentation of current behavior
```
