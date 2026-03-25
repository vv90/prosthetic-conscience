# Near-Term TODO

Snapshot date: 2026-03-25

## Consensus protocol implementation

Goal: implement the client-side consensus protocol described in `docs/consensus-protocol-design.md`. The protocol runs entirely client-side — the gateway remains content-opaque. Implementation lives in `src/consensus/`.

### ~~Phase 1: Solver~~ (done)

Grounded semantics fixpoint in `src/consensus/solver.rs`. 14 unit tests + 7 property tests = 21 tests.

### ~~Phase 2: Reducer~~ (done)

Entry types (`types.rs`), log replay (`reducer.rs`), graph extraction (`to_graph()`). Serde-tagged `Entry` enum with `Claim`, `Relation`, `Stance`, `Resolve`, `Comment`. 17 serde + 20 unit + 5 property = 42 tests.

### ~~Phase 3: Epistemic status~~ (done)

`status.rs` combining solver labels + stances → 5-category status (Established, Unexamined, Contested, Defeated, Unresolved). 11 unit + 1 integration + 5 property = 17 tests.

### ~~Phase 4: Consensus engine and structured rendering~~ (done)

The engine is the stateful core that owns the log, materialized state, solver results, and draft buffer. Rendering returns structured types for both UI and LLM consumption.

1. Structured render types: `OverviewData`, `ClaimDetail`, `AttentionSignal` — serde-serializable for WASM↔JS boundary.
2. `ConsensusEngine` struct: owns log + state + drafts, provides query/draft/submit methods.
3. Overview rendering: claim counts, open items, proposals with statuses, attention signals.
4. Claim detail rendering: body, author, status, stances, relations in/out.
5. Attention rendering: participant-specific prioritized list (unexamined, unstanced, bottlenecks).
6. Text formatting layer: `format.rs` — renders structured types to text for terminal/LLM.
7. Draft buffer: `add_draft`, `edit_draft`, `remove_draft`, `show_drafts`, `submit_drafts`.
8. Impact analysis: run solver on current state + hypothetical draft entries, diff results.

Additional engine features added during Phase 5:

- `draft_comment`: draft freeform `Comment` entries (optionally attached to a claim).
- `impact_analysis()`: compare committed state with committed + drafts, report new claims and status changes.
- `submission_bundle()`: finalize drafts into entries with provisional→final claim ID rewriting.
- `Comment` entry type extended with optional `claim_id` field (backward-compatible, `skip_serializing_if`).
- `llm_tool_definitions()`: filtered tool list excluding `submit_drafts` and `clear_drafts` for LLM safety.
- `format_drafts()`, `format_impact_analysis()`: text rendering for draft buffer and impact diff.

### ~~Phase 5: Terminal prototype binary~~ (done)

Separate binary (`pc-consensus`), not an extension of `pc-client`. Uses the engine directly.

9. Binary skeleton: `src/bin/pc-consensus.rs` with clap arg parsing (`--gateway-url`, `--auth-token`, `--model`, `--participant`, `create`/`join` subcommands).
10. Session sync: `src/consensus_cli/session.rs` — WS client with create/join handshake, `SessionEvent` stream, reconnect with exponential backoff, paginated `fetch_entries` for catch-up via `GET /v1/sessions/:id/entries`.
11. LLM system prompt: rebuilt on every turn with current overview, pending drafts, impact analysis, and filtered tool list. LLM explicitly told only the human can commit drafts.
12. Tool dispatch: LLM calls engine methods via `consensus::tools::dispatch()`. Tool errors returned as tool result messages (not fatal). Invalid JSON arguments reported back to LLM for self-correction.
13. Draft review cycle: LLM drafts entries, participant reviews via `/drafts` and `/submit` commands. Submission requires explicit `y` confirmation. Submissions tracked through reconnects via `PendingSubmission` state machine.

Shared infrastructure extracted during Phase 5:

- `src/chat_gateway/` module: `GatewayClient` (HTTP/SSE) and `response_assembler` (delta chunk assembly) extracted from `src/client/`. Shared by both `pc-client` and `pc-consensus`. `assistant_message_value` and `tool_result_message` helpers live here.
- `src/client/gateway_client.rs` and `src/client/response_assembler.rs` are now re-export shims (`pub use crate::chat_gateway::*`) for backward compatibility with `pc-client` and existing tests.

Outstanding issues:

- **WS heartbeat**: `session.rs` `connected_loop` has no ping/pong or application-level heartbeat tick. Dead TCP connections (NAT timeout, network partition) won't be detected until the next send fails. Server-side auto-heartbeat keeps the kernel subscriber alive while the TCP connection is healthy, but silent failures can leave the client thinking it's connected for an extended period.
- **Conversation history truncation**: LLM `history` grows without bound across turns. Needs configurable max context size with older messages dropped (system prompt is rebuilt fresh each turn so it's always current).

### Phase 6: Crate extraction and WASM target

The consensus module has zero non-WASM dependencies (only `std::collections`, `serde`). Extract before UI work.

14. Extract `src/consensus/` → `crates/consensus/` as a standalone workspace crate.
15. Verify `cargo build --target wasm32-unknown-unknown` compiles clean.
16. Add `wasm-bindgen` exports for engine methods (for future JS/TS UI).

### Deferred

- `amend` entry type: add when needed (body-text update, stance invalidation semantics TBD).
- BAF support edge propagation: `supported_by` field exists in solver graph, semantics undecided.
- Incremental state update: full replay is sufficient at deliberation scale.
- Web UI: browser-side WASM module + JS for WS/LLM/DOM. Architecture decided, implementation deferred.

## Integration testing (remaining)

7. Pre-dispatch rejection: `stream=false` → 400.
8. Client disconnect mid-stream → relay detects `ClientGone`, worker re-registers.
9. Malformed worker message: garbage JSON skipped, valid chunks still arrive.
10. Dispatch failure: worker oneshot dropped before delivery → client gets error.
11. Rapid worker connect/disconnect churn: 100 cycles → no panics, no leaked state.
12. Channel close propagation: timeout fires → client mpsc receiver is actually closed.

## Performance baseline

19. Throughput benchmark: 1 worker, max-speed chunks → chunks/sec to client.
20. Concurrent streams benchmark: ramp 1–100 streams → p50/p99 first-chunk latency.
21. Backpressure test: slow client + fast worker → worker slows, no OOM.

## Real-world deployment validation (remaining)

51. Worker connects outbound to `wss://gateway-domain/ws/worker`. No inbound ports needed.
52. Client connects to `https://gateway-domain/v1/chat/completions` from anywhere.
53. Worker connects over WSS, shows "connected to gateway" in logs.
54. Client `curl -N https://gateway-domain/v1/chat/completions ...` streams tokens.
55. Kill worker mid-stream → client gets error + done.
56. Kill llama-server → worker sends error → client gets error.
57. Kill gateway → worker reconnects with backoff.
58. Unauthorized requests rejected.

## Structural changes (remaining)

### Cargo workspace conversion (do before worker agent work begins)

29. Restructure into workspace with `crates/protocol/`, `crates/gateway/`, `crates/worker-agent/`, `crates/client-sidecar/`.
    30–32. Move protocol, gateway, and integration tests into workspace structure.

### Payload encryption prep (do before encryption work)

33–36. Introduce `Payload` enum (`Plaintext(Value)` / `Encrypted(EncryptedBlob)`) in protocol crate.

## Other tasks

43. Add worker handshake/version/capability validation.
44. Remove `GatewayAdapter` once runtime coverage is confirmed complete.
45. Keep behavior-state files synchronized as behavior transitions.
