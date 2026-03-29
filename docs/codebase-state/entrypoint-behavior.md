# Entrypoint Behavior

Snapshot date: 2026-03-29

## Binaries

### Gateway (`prosthetic-conscience`)

- Initializes tracing and starts an Axum server.
- CLI args: `--host` (default `127.0.0.1`), `--port` (default `3000`).
- Reads `PC_AUTH_TOKEN` env var for optional bearer auth.
- Exposes:
  - transcription test UI: `GET /` (self-contained HTML embedded via `include_str!`)
  - worker websocket endpoint: `/ws/worker`
  - chat completions endpoint: `POST /v1/chat/completions`
  - audio transcriptions endpoint: `POST /v1/audio/transcriptions`

### Worker (`pc-worker`)

- Connects outbound to gateway via WebSocket, receives jobs, routes to appropriate backend based on the `capability` field (`Chat` → inference server, `Transcription` → whisper server).
- CLI args: `--gateway-url`, `--inference-url` (optional), `--whisper-url` (optional), `--auth-token`. At least one of `--inference-url` or `--whisper-url` must be provided.
- Derives capabilities from configured URLs: `--inference-url` → `Chat`, `--whisper-url` → `Transcription`. Declares capabilities via `?capabilities=` query parameter on the WebSocket upgrade URL.
- Reconnects with exponential backoff (1s to 30s cap).

### Client (`pc-client`)

- Interactive REPL that sends chat completion requests to the gateway.
- CLI args: `--gateway-url`, `--auth-token`, `--model`, `--system`, `--max-rounds`, `--container`, `--shell-timeout`, `--max-output`.
- Reads user input from stdin, sends to gateway via tool use loop, assembles streamed SSE response, prints content.
- Maintains conversation history across turns.
- Tool use loop: detects `tool_calls` in model responses, executes tools locally, appends results, re-requests until final answer or max rounds exceeded.
- Built-in tools:
  - `get_current_time` — always registered, returns UTC timestamp.
  - `execute_shell` — registered when `--container` is provided, runs shell commands in a Docker container via `docker exec`. Supports configurable timeout and output truncation.

### Consensus Terminal Client (`pc-consensus`)

- Interactive REPL for multi-participant consensus deliberation over a shared session log.
- CLI args: `--gateway-url`, `--auth-token`, `--model`, `--participant`, `--max-history`, `--debug-tool-trace`.
- Subcommands: `create` (new session), `join <session-id>` (existing session).
- Connects to gateway session via WS, syncs shared log, runs LLM-assisted drafting loop.
- LLM turn loop sends `tool_choice: "required"` — model must always select a tool.
- `no_structured_action` fallback tool for conversational responses (deliberately unappealing: listed last, required reason enum).
- REPL commands: `/overview`, `/claim <id>`, `/drafts`, `/submit`, `/clear`, `/help`, `/quit`.
- `--debug-tool-trace` prints per-round tool execution traces for each LLM turn.

### Consensus Sim (`pc-consensus-sim`)

- Deterministic log generator for offline consensus experiments.
- Produces fixture logs without requiring a running gateway.

### Consensus Seed (`pc-consensus-seed`)

- Session seeder for importing fixture logs into a live gateway session.
- Used to pre-populate sessions for trials and evaluation runs.

### Consensus Eval (`pc-consensus-eval`)

- Checkpoint-based tool-calling reliability runner.
- CLI args: `--gateway-url`, `--auth-token`, `--suite`, `--run-name`, `--repeats`, `--history-turns`, `--max-history`, `--output`, `--markdown-output`.
- Runs the real `ConsensusLlm` turn loop against fixture checkpoints with varying history lengths and truncation budgets.
- Produces JSON reports and markdown summary tables with per-run metrics: `tool_call_made`, `structured_tool_call_made`, `expected_tool_match`, `expected_argument_match`, `expected_outcome_match`, `turn_success`.
- Default suite: `fixtures/tool-call-eval/authentication-tool-reliability.json`.

## Status

- All binaries implemented (gateway, worker, client, pc-consensus, pc-consensus-sim, pc-consensus-seed, pc-consensus-eval).

## Load into context when

- Changing startup/runtime server boot behavior.
- Changing bind address or tracing initialization behavior.
- Adding new CLI args to any binary.

## Relevant files

- `src/main.rs` (gateway)
- `src/worker_agent.rs` (worker)
- `src/client_agent.rs` (client)
- `src/bin/pc-consensus.rs` (consensus terminal client)
- `src/bin/pc-consensus-sim.rs` (consensus sim)
- `src/bin/pc-consensus-seed.rs` (consensus seed)
- `src/bin/pc-consensus-eval.rs` (consensus eval)
- `src/consensus_cli/app.rs` (consensus app logic)
- `src/consensus_cli/llm.rs` (consensus LLM turn loop)
- `src/consensus/tools.rs` (tool definitions and dispatch)
- `src/consensus/eval.rs` (eval suite runner and scoring)
- `src/router/mod.rs`
- `src/router/ui.rs`
- `src/router/state.rs`
- `static/transcribe.html`
- `src/client/gateway_client.rs`
- `src/client/response_assembler.rs`
- `src/client/tool_loop.rs`
- `src/client/tools/mod.rs`
- `src/client/tools/current_time.rs`
- `src/client/tools/shell.rs`
- `fixtures/tool-call-eval/` (eval benchmark suites)
- `docs/tool-calling-eval-methodology.md`
