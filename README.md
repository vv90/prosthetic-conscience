# Prosthetic Conscience

An LLM gateway that routes client requests to GPU workers over reverse-initiated WebSocket connections. Workers dial out to the gateway — no VPN or inbound ports needed on the inference side.

## Architecture

```
Client (HTTP)  -->  Gateway (public)  <--  Worker (WebSocket)  -->  llama-server (local)
```

- **Gateway** accepts OpenAI-compatible HTTP requests and streams SSE responses back to clients. Dispatches jobs to workers over persistent WebSocket connections. Pure kernel architecture with tick-based timeouts and heartbeats.
- **Worker** (`pc-worker`) connects outbound to the gateway, receives jobs, proxies them to a local llama-server (or any OpenAI-compatible inference endpoint), and streams chunks back.
- **Client** (`pc-client`) interactive REPL that sends chat completion requests, assembles streamed responses, maintains conversation history, and executes tool calls locally. Supports a tool use loop with configurable tools including shell execution in Docker containers. Also usable via any OpenAI-compatible client (curl, Open WebUI, etc.).

The gateway never persists prompt or completion content.

## Building

Requires Rust 2024 edition (1.85+).

```
cargo build --release
```

This produces three binaries: `prosthetic-conscience` (gateway), `pc-worker` (worker agent), and `pc-client` (interactive client).

## Running

### 1. Start an inference server

Any OpenAI-compatible endpoint works. With llama.cpp:

```
llama-server -m model.gguf --port 8080
```

```
llama-server \
  -hf bartowski/Qwen2.5-1.5B-Instruct-GGUF:Q4_K_M \
  --ctx-size 4096 \
  --port 8080
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

### 4. Start the client

```
cargo run --bin pc-client
```

Options:

```
--gateway-url <URL>      Gateway base URL [default: http://127.0.0.1:3000]
--auth-token <TOKEN>     Bearer token for gateway auth (must match PC_AUTH_TOKEN)
--model <MODEL>          Model name to include in requests [default: default]
--system <PROMPT>        Optional system prompt
--max-rounds <N>         Maximum tool call rounds per user message [default: 10]
--container <NAME>       Docker container name for shell tool (enables execute_shell)
--shell-timeout <SECS>   Timeout in seconds for shell commands [default: 30]
--max-output <BYTES>     Maximum output bytes per shell command [default: 51200]
```

Type a message at the `> ` prompt and the model's response will be printed. Conversation history is maintained across turns. The `get_current_time` tool is always available. When `--container` is provided, the `execute_shell` tool lets the model run commands inside the specified Docker container.

#### Using the shell tool with Docker

```bash
# Start a sandbox container
docker run -d --name my-sandbox -v /path/to/project:/workspace ubuntu:24.04 sleep infinity

# Start the client with shell tool enabled
cargo run --bin pc-client -- --container my-sandbox

> list the files in /workspace
# Model calls execute_shell, tool runs "ls /workspace" in container, model reports results
```

### 5. Or send a raw request

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

### 6. Or use Open WebUI

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

106 tests (102 run + 4 ignored): 94 unit (kernel + protocol + registry + response assembler + tool trait + tool implementations), 12 integration (full pipeline through real HTTP/WebSocket including tool loop round-trip). 4 Docker-dependent shell tool tests are `#[ignore]` by default.

## Project structure

```
src/
  main.rs              Gateway binary
  worker_agent.rs      Worker binary
  client_agent.rs      Client binary (interactive REPL)
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
  client/
    gateway_client.rs  HTTP + SSE client for gateway communication
    response_assembler.rs  Assembles streamed delta chunks into complete messages
    tool_loop.rs       Request-execute-request cycle for tool calling
    tools/
      mod.rs           Tool trait, ToolRegistry, ToolError
      current_time.rs  Trivial tool (UTC timestamp) for testing the loop
      shell.rs         Docker container shell execution tool
tests/
  integration.rs       End-to-end tests
  support/             Test harness (TestGateway, MockWorker, SseClient)
docs/
  gateway-specification.md
  implementation-plan.md
  codebase-state/      Living documentation of current behavior
```
