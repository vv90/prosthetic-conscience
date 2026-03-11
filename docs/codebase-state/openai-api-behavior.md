# OpenAI-Compatible API Behavior

Snapshot date: 2026-03-11

## Behavior

- `POST /v1/chat/completions` accepts streaming chat completion requests.
- Request body is parsed as JSON. The `stream` field is extracted; all other fields are passed through as an opaque `Value` payload to the kernel and ultimately to the worker.
- `stream=true` is required. Requests with `stream=false` or omitted `stream` receive a `400 Bad Request` with `{"error": {"message": "stream=true is required"}}`.
- On acceptance, the handler:
  1. Creates an `mpsc::channel<StreamFrame>(32)` for the client stream.
  2. Registers the sender with the runtime via `register_stream`, receiving a `ClientStreamId`.
  3. Submits `HttpChatRequested { client_stream_id, payload, stream: true }` to the kernel.
  4. Returns an SSE response that reads from the `mpsc::Receiver<StreamFrame>`.
- SSE output mapping:
  - `StreamFrame::Chunk { data }` → `data: {json}\n\n` (worker's chunk data passed through as-is).
  - `StreamFrame::Done` → `data: [DONE]\n\n` (OpenAI-compatible stream termination signal).
  - `StreamFrame::Error { message }` → `data: {"error": {"message": "..."}}\n\n` (machine-readable error).
- The SSE stream ends naturally when the `mpsc::Receiver` is closed (after terminal effects drop the last sender via `take_stream`).
- `KeepAlive::default()` sends periodic SSE comments to prevent proxy timeouts.
- If the runtime channel is closed (gateway shutting down), the handler returns `503 Service Unavailable`.

## Invariants

- The handler never decides whether to dispatch — it always submits `HttpChatRequested` and lets the kernel decide. Pre-dispatch errors (no worker, duplicate stream) are delivered as `StreamFrame::Error` + channel close via kernel effects.
- The handler never logs or persists request/response content (privacy invariant).
- The payload is opaque — the gateway does not interpret model, messages, or other fields.

## Status

- Implemented (streaming only). Non-streaming mode (`stream=false`) is rejected at the handler level; the kernel also has a safety guard for `stream=false`.

## Load into context when

- Modifying request validation or response semantics for `/v1/chat/completions`.
- Extending toward broader OpenAI compatibility.
- Adding non-streaming support.

## Relevant files

- `src/router/chat_completions.rs`
- `src/router/mod.rs`
- `src/router/state.rs`
- `src/gateway/runtime.rs` (`HttpChatRequested` command, `register_stream`)
- `src/gateway/relay.rs` (`StreamFrame`)

## TODO (near-term)

- Add request validation (e.g., required `model` field, `messages` array).
- Support non-streaming mode (`stream=false`) — accumulate all chunks, return as single JSON response.
- Add OpenAI-compatible response structure for chunks (`chat.completion.chunk` object wrapping).
