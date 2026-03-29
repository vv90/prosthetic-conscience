# Near-Term TODO

Snapshot date: 2026-03-29

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

- Draft buffer refactor: local drafts are now stored as draft-local `DraftContent` rather than committed-style `Entry` values.
- Draft-local claim references use `DraftId` via `ClaimRef::{Committed, Draft}` and are materialized into committed `Entry` values only for preview/submission.
- Draft authorship is now engine-scoped from the active participant rather than LLM-supplied tool arguments.
- `draft_comment`: draft freeform `Comment` entries (optionally attached to a claim).
- `impact_analysis()`: compare committed state with committed + drafts, report new claims and status changes.
- `submission_bundle()`: finalize drafts into entries with `DraftId -> final ClaimId` rewriting.
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

Additional features added during Phase 5 (continued):

- `tool_choice: "required"` on every LLM turn — model must always select a tool.
- `no_structured_action` is now the natural-language conversation path for summaries, explanations, strategy, and clarification turns. Handled as a short-circuit in the tool loop — extracts text as conversational content, ends the turn, and writes the plain assistant reply back into history.
- Hardened system prompt: hides protocol jargon from the participant-facing conversation, requires clarification before ambiguous recording, instructs relation-vs-stance disambiguation, and keeps submission explicitly human-controlled.
- State-based LLM turn policy: fresh ambiguous turns start in clarify/inspect mode with read-only tools plus `no_structured_action`; mutation tools open only after a clarification handoff or when a local draft buffer already exists.
- Internal clarification marker: `no_structured_action` turns with `reason=need_clarification` leave a hidden history marker so the next user reply can unlock mutation tools without brittle word matching.
- Deterministic post-mutation confirmation: after a successful draft mutation, the harness now renders a short local-draft confirmation from the actual tool result instead of asking the model to narrate what happened.
- Clarification marker protected from history truncation: `truncate_history` treats a user message immediately following a clarification marker as an unsafe cut point, preventing the marker from being silently dropped under aggressive truncation budgets.
- Consolidated claim ref parsing: `ClaimRef::from_json_value` in `engine.rs` is the single canonical parser for JSON → `ClaimRef`. Both `tools.rs` (tool dispatch) and `llm.rs` (mutation confirmation rendering) delegate to it. Accepts `draft:<n>`, `#<n>`, `claim:<id>`, bare strings, and `{"claim_id":…}` / `{"draft_id":…}` objects.
- `LlmTurnTrace` round-by-round tracing: each LLM round captures request sizes, response chunk counts, assistant message, tool execution traces (arguments, parse results, dispatch results). Used by both `--debug-tool-trace` in `pc-consensus` and the eval harness.
- `--debug-tool-trace` CLI flag on `pc-consensus`: prints compact per-round tool traces for each LLM turn.
- `MAX_COMPLETION_TOKENS` constant (512) added to LLM requests.
- `CompletedMessage` and `CompletedToolCall` now derive `Serialize` (needed for trace serialization).

Eval harness (new):

- `src/consensus/eval.rs`: suite loader, synthetic context seeding, checkpoint-based scoring, aggregation. Runs the real `ConsensusLlm` turn loop against fixture checkpoints with varying history lengths and truncation budgets.
- `src/bin/pc-consensus-eval.rs`: CLI entrypoint for tool-calling reliability evaluation. Produces JSON reports and markdown summary tables.
- `fixtures/tool-call-eval/authentication-tool-reliability.json`: checked-in benchmark suite with deterministic rubrics.
- `docs/tool-calling-eval-methodology.md`: scoring methodology, metrics definitions, and recommended judge-model follow-up for ambiguous cases.
- Metrics per run: `tool_call_made`, `structured_tool_call_made`, `expected_tool_match`, `expected_argument_match`, `expected_outcome_match`, `turn_success`.
- Eval matching now scores draft-local `DraftContent` and semantic claim references rather than assuming committed-entry-shaped drafts.

Outstanding issues:

- **WS heartbeat**: `session.rs` `connected_loop` has no ping/pong or application-level heartbeat tick. Dead TCP connections (NAT timeout, network partition) won't be detected until the next send fails. Server-side auto-heartbeat keeps the kernel subscriber alive while the TCP connection is healthy, but silent failures can leave the client thinking it's connected for an extended period.
- ~~**Conversation history truncation**: LLM `history` grows without bound across turns.~~ Resolved: `truncate_history()` with configurable `max_history` preserves tool-call/result pairs.
- ~~**History contamination from prose-only turns**~~ Resolved: `no_structured_action` replies now replace the assistant tool-call stub in history with the actual plain assistant reply the participant saw.
- **Duplicate draft creation**: model sometimes creates identical drafts across consecutive tool rounds without deduplication.
- **Clarification quality is still model-limited**: the harness now reliably separates clarification from mutation, but the model's actual follow-up questions can still be generic, verbose, or semantically weaker than desired.

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
