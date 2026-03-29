# Prosthetic Conscience

An LLM gateway that routes client requests to GPU workers over reverse-initiated WebSocket connections. Workers dial out to the gateway — no VPN or inbound ports needed on the inference side.

## Architecture

```
Client (HTTP)  -->  Gateway (public)  <--  Worker (WebSocket)  -->  llama-server (local)
```

- **Gateway** accepts OpenAI-compatible HTTP requests and streams SSE responses back to clients. Dispatches jobs to workers over persistent WebSocket connections. Pure kernel architecture with tick-based timeouts and heartbeats.
- **Worker** (`pc-worker`) connects outbound to the gateway, receives jobs, proxies them to a local llama-server (chat) or whisper.cpp server (transcription), and streams chunks back. Declares capabilities based on configured backend URLs.
- **Client** (`pc-client`) interactive REPL that sends chat completion requests, assembles streamed responses, maintains conversation history, and executes tool calls locally. Supports a tool use loop with configurable tools including shell execution in Docker containers. Also usable via any OpenAI-compatible client (curl, Open WebUI, etc.).

The gateway never persists prompt or completion content.

## Building

Requires Rust 2024 edition (1.85+).

```
cargo build --release
```

This produces four primary binaries: `prosthetic-conscience` (gateway), `pc-worker` (worker agent), `pc-client` (interactive client), and `pc-consensus` (consensus terminal client). It also includes `pc-consensus-sim`, a deterministic log generator for offline consensus experiments, `pc-consensus-seed`, a session seeder for importing fixture logs into the gateway, and `pc-consensus-eval`, a checkpoint-based tool-calling reliability runner for a known consensus worker/model.

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

### 1b. (Optional) Start a whisper server for transcription

Any OpenAI Whisper-compatible endpoint works. With whisper.cpp:

```bash
# Download a model
mkdir -p models
curl -L -o models/ggml-tiny.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin

# Start the server (--convert requires ffmpeg, handles any audio format)
whisper-server --model ./models/ggml-tiny.en.bin --host 127.0.0.1 --port 8090 \
  --inference-path /v1/audio/transcriptions --convert
```

The `--inference-path` flag sets the endpoint to match the OpenAI API path the worker expects. The `--convert` flag enables ffmpeg-based transcoding so the server accepts any audio format (webm, mp4, ogg, wav, etc.) — required when audio comes from a browser's MediaRecorder.

Available models (from [ggerganov/whisper.cpp on HuggingFace](https://huggingface.co/ggerganov/whisper.cpp)):

| Model    | File                | Size    |
| -------- | ------------------- | ------- |
| tiny.en  | `ggml-tiny.en.bin`  | ~75 MB  |
| base.en  | `ggml-base.en.bin`  | ~142 MB |
| small.en | `ggml-small.en.bin` | ~466 MB |

If using the nix devshell, `whisper-server` is already on your path.

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
--inference-url <URL>    Inference server URL (enables chat capability)
--whisper-url <URL>      Whisper server URL (enables transcription capability)
--auth-token <TOKEN>     Bearer token for gateway auth (must match PC_AUTH_TOKEN)
```

At least one of `--inference-url` or `--whisper-url` must be provided. The worker declares capabilities based on which URLs are configured.

Examples:

```bash
# Chat only
cargo run --bin pc-worker -- --inference-url http://127.0.0.1:8080

# Transcription only
cargo run --bin pc-worker -- --whisper-url http://127.0.0.1:8090

# Both
cargo run --bin pc-worker -- \
  --inference-url http://127.0.0.1:8080 \
  --whisper-url http://127.0.0.1:8090
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

### 5. Start the consensus deliberation REPL

Use `pc-consensus` to join a shared consensus session with the LLM-backed deliberation assistant.

Create a new session:

```bash
cargo run --bin pc-consensus -- \
  --participant alice \
  --model default \
  create
```

Join an existing session, such as one created by `pc-consensus-seed`:

```bash
cargo run --bin pc-consensus -- \
  --participant evaluator \
  --model default \
  join <session-id>
```

Once inside the REPL, you can type natural-language deliberation messages or use commands like:

```text
/overview
/claim prop-hybrid
/drafts
/submit
/clear
/help
```

Notes:

- Draft authorship is derived from `--participant`; the LLM does not supply draft `author` fields directly.
- Pending draft claims are identified locally by `#<draft_id>` in `/drafts`.
- When a tool or trace refers to a committed claim it uses `claim:<id>`; when it refers to a local draft claim it uses `draft:<id>`.
- The assistant is configured to clarify or inspect first on ambiguous fresh turns, then prepare a local draft after confirmation when needed.
- Successful draft mutations are followed by a deterministic local-draft confirmation rather than another free-form model-generated narration.
- Design notes and behavior observations live in [docs/consensus-llm-behavior.md](/Users/vladimir/devshells/prosthetic-conscience/docs/consensus-llm-behavior.md).

Example prompts to try:

```text
Summarize the current deliberation and tell me what needs attention.
Draft the minimum entries needed to address the strongest remaining objection.
If I were Carol, what should I propose next to move this toward convergence?
```

### 6. Or send a raw request

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

### 7. Or use Open WebUI

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

### 8. Built-in transcription UI

The gateway serves a minimal web UI for testing audio transcription at the root URL. Open `http://127.0.0.1:3000/` in a browser.

- Hold the push-to-talk button (or hold spacebar) to record audio
- Release to send to the transcription endpoint
- Requires a worker with transcription capability connected to the gateway

If `PC_AUTH_TOKEN` is set, enter the token in the auth field before recording.

## Testing

```
cargo test --all-targets --all-features
```

The suite covers unit and integration tests across the gateway, protocol, worker, client, and consensus layers. Docker-dependent shell tool tests are `#[ignore]` by default.

### Consensus Log Fixture Generator

Generate a realistic consensus session log without needing a running gateway or worker:

```bash
cargo run --bin pc-consensus-sim -- --output fixtures/auth-deliberation.session.json
```

Useful formats:

- `--format session-response` matches the `GET /v1/sessions/<id>/entries` response shape.
- `--format entries` emits just the raw entry array.
- `--format bundle` adds scenario metadata plus a computed final overview for LLM experiments.
- `--format jsonl` writes one entry per line.

### Consensus Session Seeder

Seed an existing session in the gateway with either the checked-in fixture file or a built-in scenario.

Create the session beforehand, for example with:

```bash
cargo run --bin pc-consensus -- \
  --participant alice \
  --model default \
  create
```

Keep at least one subscriber connected to that session while and after seeding, otherwise the gateway may remove the session once it has no subscribers left.

Then seed that existing session id:

```bash
# Seed from the checked-in fixture file
cargo run --bin pc-consensus-seed -- \
  <session-id> \
  --input fixtures/auth-deliberation.session.json

# Seed from the built-in scenario directly
cargo run --bin pc-consensus-seed -- \
  <session-id> \
  --scenario authentication-deliberation
```

The seeder prints the session id plus a fetch URL for the seeded entries. One simple workflow is:

```text
terminal 1: cargo run --bin pc-consensus -- --participant alice --model default create
terminal 2: cargo run --bin pc-consensus-seed -- <session-id> --input fixtures/auth-deliberation.session.json
terminal 3: cargo run --bin pc-consensus -- --participant evaluator --model default join <session-id>
```

You can then join the same session with:

```bash
cargo run --bin pc-consensus -- \
  --participant evaluator \
  --model default \
  join <session-id>
```

### Consensus Tool-Calling Eval

Run the checked-in benchmark suite for a named test run against the currently pinned worker/model, across fixture checkpoints, prior-context lengths, and `max_history` budgets:

```bash
cargo run --bin pc-consensus-eval -- \
  --gateway-url http://127.0.0.1:3000 \
  --run-name qwen-tool-reliability \
  --repeats 10 \
  --output trial-logs/tool-eval/report.json \
  --markdown-output trial-logs/tool-eval/report.md
```

The default suite lives at `fixtures/tool-call-eval/authentication-tool-reliability.json`, includes the request `model` string for the known worker/backend, and reuses the real `pc-consensus` turn loop. See `docs/tool-calling-eval-methodology.md` for scoring details and suggested judge-model follow-up for ambiguous cases.

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
    audio_transcriptions.rs  POST /v1/audio/transcriptions handler
    worker_ws_upgrade.rs  WS /ws/worker upgrade handler
    ui.rs              Serves embedded transcription test UI
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
static/
  transcribe.html      Self-contained transcription test UI (embedded at compile time)
tests/
  integration.rs       End-to-end tests
  support/             Test harness (TestGateway, MockWorker, SseClient)
docs/
  gateway-specification.md
  implementation-plan.md
  codebase-state/      Living documentation of current behavior
```
