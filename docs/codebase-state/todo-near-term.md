# Near-Term TODO

Snapshot date: 2026-04-03

## Consensus protocol implementation

Goal: implement the client-side consensus protocol described in `docs/consensus-protocol-design.md`. The protocol runs entirely client-side — the gateway remains content-opaque. Pure consensus logic now lives in `crates/consensus/`, while the terminal app, gateway integration, eval harness, and seeding helpers live in `crates/prosthetic-conscience/`.

### ~~Phase 1: Solver~~ (done)

Grounded semantics fixpoint in `crates/consensus/src/solver.rs`. 14 unit tests + 7 property tests = 21 tests.

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
5. Attention rendering: prioritized list of contested, unexamined, and unresolved items. The current core rendering is global; participant-specific presentation is still a UI-layer concern.
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

9. Binary skeleton: `crates/prosthetic-conscience/src/bin/pc-consensus.rs` with clap arg parsing (`--gateway-url`, `--auth-token`, `--model`, `--participant`, `create`/`join` subcommands).
10. Session sync: `crates/prosthetic-conscience/src/consensus_cli/session.rs` — WS client with create/join handshake, `SessionEvent` stream, reconnect with exponential backoff, paginated `fetch_entries` for catch-up via `GET /v1/sessions/:id/entries`.
11. LLM system prompt: rebuilt on every turn with current overview, pending drafts, impact analysis, and filtered tool list. LLM explicitly told only the human can commit drafts.
12. Tool dispatch: LLM calls engine methods via `consensus::tools::dispatch()`. Tool errors returned as tool result messages (not fatal). Invalid JSON arguments reported back to LLM for self-correction.
13. Draft review cycle: LLM drafts entries, participant reviews via `/drafts` and `/submit` commands. Submission requires explicit `y` confirmation. Submissions tracked through reconnects via `PendingSubmission` state machine.

Shared infrastructure extracted during Phase 5:

- `crates/prosthetic-conscience/src/chat_gateway/` module: `GatewayClient` (HTTP/SSE) and `response_assembler` (delta chunk assembly) extracted from the app-side client code. Shared by both `pc-client` and `pc-consensus`. `assistant_message_value` and `tool_result_message` helpers live here.
- `crates/prosthetic-conscience/src/client/gateway_client.rs` and `crates/prosthetic-conscience/src/client/response_assembler.rs` are re-export shims (`pub use crate::chat_gateway::*`) for backward compatibility with `pc-client` and existing tests.

Additional features added during Phase 5 (continued):

- `tool_choice: "auto"` on every LLM turn, so the model can either call tools or answer directly in plain text.
- Plain conversational replies now use the normal assistant-message path; there is no `no_structured_action` tool in the LLM-visible schema.
- LLM-safe tool definitions still exclude `submit_drafts` and `clear_drafts`, keeping submission explicitly human-controlled.
- Hardened system prompt: hides protocol jargon from the participant-facing conversation, prefers clarification before ambiguous recording, instructs relation-vs-stance disambiguation, and keeps submission explicitly human-controlled.
- The current implementation exposes the same LLM-safe tool set on every request; clarify-first behavior is prompt-driven rather than enforced by turn-specific tool gating.
- Deterministic post-mutation confirmation: after a successful draft mutation, the harness renders a short local-draft confirmation from the actual tool result instead of asking the model to narrate what happened.
- `truncate_history()` preserves tool-call/result pairs and only cuts at safe conversational boundaries.
- Consolidated claim ref parsing: `ClaimRef::from_json_value` in `crates/consensus/src/engine.rs` is the single canonical parser for JSON → `ClaimRef`. Both `crates/consensus/src/tools.rs` (tool dispatch) and `crates/consensus/src/llm_turn.rs` (mutation confirmation rendering) delegate to it. Accepts `draft:<n>`, `#<n>`, `claim:<id>`, bare strings, and `{"claim_id":…}` / `{"draft_id":…}` objects.
- `LlmTurnTrace` round-by-round tracing: each LLM round captures request sizes, response chunk counts, assistant message, and tool execution traces (arguments, parse results, dispatch results). Used by both `--debug-tool-trace` in `pc-consensus` and the eval harness.
- `--debug-tool-trace` CLI flag on `pc-consensus`: prints compact per-round tool traces for each LLM turn.
- `MAX_COMPLETION_TOKENS` constant (512) is passed to the backend as `max_tokens`.
- `CompletedMessage` and `CompletedToolCall` derive `Serialize` for trace serialization.

Eval harness (new):

- `crates/prosthetic-conscience/src/consensus_support/eval.rs`: suite loader, synthetic context seeding, checkpoint-based scoring, aggregation. Runs the real `ConsensusLlm` wrapper over the pure `consensus::llm_turn` loop against fixture checkpoints with varying history lengths and truncation budgets.
- `crates/prosthetic-conscience/src/bin/pc-consensus-eval.rs`: CLI entrypoint for tool-calling reliability evaluation. Produces JSON reports and markdown summary tables.
- `fixtures/tool-call-eval/authentication-tool-reliability.json`: checked-in benchmark suite with deterministic rubrics.
- `docs/tool-calling-eval-methodology.md`: scoring methodology, metrics definitions, and recommended judge-model follow-up for ambiguous cases.
- Metrics per run: `tool_call_made`, `structured_tool_call_made`, `expected_tool_match`, `expected_argument_match`, `expected_outcome_match`, `turn_success`.
- Eval matching now scores draft-local `DraftContent` and semantic claim references rather than assuming committed-entry-shaped drafts.

Outstanding issues:

- **WS heartbeat**: `session.rs` `connected_loop` has no ping/pong or application-level heartbeat tick. Dead TCP connections (NAT timeout, network partition) won't be detected until the next send fails. Server-side auto-heartbeat keeps the kernel subscriber alive while the TCP connection is healthy, but silent failures can leave the client thinking it's connected for an extended period.
- ~~**Conversation history truncation**: LLM `history` grows without bound across turns.~~ Resolved: `truncate_history()` with configurable `max_history` preserves tool-call/result pairs.
- **Duplicate draft creation**: model sometimes creates identical drafts across consecutive tool rounds without deduplication.
- **Clarification quality is still model-limited**: the harness now reliably separates clarification from mutation, but the model's actual follow-up questions can still be generic, verbose, or semantically weaker than desired.

### Phase 6: WASM boundary and browser integration

Core extraction is done: the consensus runtime now lives in `crates/consensus/`, `prosthetic-conscience` depends on it as a workspace crate, and the crate already builds for `wasm32-unknown-unknown`.

Implementation process for the browser/WASM layer now lives in [`docs/codebase-state/consensus-browser-ui-implementation.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/consensus-browser-ui-implementation.md). Invariant and testing terminology now lives in [`docs/codebase-state/testing-methodology-and-invariants.md`](/Users/vladimir/devshells/prosthetic-conscience/docs/codebase-state/testing-methodology-and-invariants.md). Together they are the source of truth for the incremental interface -> constraints/correctness-properties -> logic loop described below.

**Coordinator reducer (implemented):** `crates/consensus/src/coordinator.rs` — pure reducer for bootstrap from an optional latest indexed entry, slot-based gap detection, page-bounded fetch planning (`FetchMissing { from, limit }`), and local `SubmitEntry` emission. 20 tests (13 targeted + 7 property-based). Does not yet own `EntryBuffer`, connection state, fetch lifecycle, or submission tracking.

**Phase 6 implementation order:**

14. Establish the new pure browser-facing app boundary in `crates/consensus/`:
    `AppState`, `AppInput`, `AppEffect`, `AppView`, `AppTransition`.
15. For that first app slice, write explicit app-layer constraints and correctness properties before expanding behavior.
16. Implement the smallest local-only interaction slice behind the app boundary:
    local drafts, overview, selected claim detail, impact analysis, explicit submit intent.
17. Expand the session coordinator into the higher-level pure source of truth for session sync policy:
    reconnect/catch-up, append gating, submission resume, queued-event draining.
18. Treat `EntryBuffer` as transitional. Fold its responsibilities into the higher-level coordinator rather than preserving both as long-term public boundaries.
19. Replace the imperative session/submission control flow in `consensus_cli/app.rs` with the higher-level pure app/coordinator loop.
20. Rename the session entry-fetch cursor from `after` to `from` across request types and related code paths.
21. Add bootstrap session metadata so connect handling can receive the latest entry index together with the full latest entry payload, and use that to simplify initial catch-up planning.
22. Only after the pure app boundary is stable, add a thin `wasm-bindgen` wrapper crate that exposes the app-level interface rather than engine/coordinator internals.
23. Build the first browser prototype against that wrapper with a minimal JS shell:
    DOM rendering, websocket/HTTP execution, timers, auth token handling.

**Remaining Phase 6 tasks:**

- Do not recreate the terminal REPL in the browser; build the browser interaction model from the app boundary outward.
- Keep JavaScript thin: JS executes effects and renders view data, while Rust owns decisions and state transitions.
- Add browser-facing functionality in small increments, and for each increment:
  1. establish interface
  2. establish constraints
  3. establish correctness properties
  4. implement logic
- Re-evaluate the JS↔WASM surface after each increment and avoid exposing temporary lower-level methods as permanent API.

### Protocol integrity concerns (identified, mitigations planned)

These concerns apply once consensus logic runs in distributed browser clients. See `docs/codebase-state/session-coordinator-behavior.md` for full details.

- **Version drift**: clients running different WASM versions may interpret entries differently. Mitigation: session-level protocol version advertisement, client-enforced compatibility checks, entry schema version tags.
- **Log pollution**: malicious or buggy clients could spam the session log. Mitigation: gateway-side per-session/per-client rate limiting (content-opaque), client-side activity signals.
- **Entry rejection**: kept unconditional to preserve gateway content-opacity. Schema version tags allow clients to skip unparseable entries.
- **Gateway graph management**: splitting entries into cleartext metadata + opaque content was rejected — graph topology IS the interesting part, exposing it changes the trust boundary.

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

### Panic-safety enforcement

46. Add a dedicated CI gate for panic safety on non-test targets:
    `cargo clippy --workspace --lib --bins -- -D warnings -W clippy::indexing_slicing -W clippy::panic -W clippy::unwrap_used -W clippy::expect_used -W clippy::unreachable -W clippy::todo -W clippy::unimplemented`
47. Add a second CI/scripted scan that rejects panic constructs in non-test Rust code before `#[cfg(test)]` blocks:
    `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `assert!`, `assert_eq!`, `assert_ne!`, `unwrap()`, `expect()`.
48. Add an explicit workspace lint/checking entrypoint so panic-safety verification is not just ad hoc terminal knowledge.
49. Audit production code for direct indexing/slicing and replace remaining cases with `.get()`, `.get_mut()`, `.first()`, `.get(..)` or explicit error paths.
50. Audit arithmetic and allocation hot paths for implicit panic/abort risks; prefer checked operations (`checked_*`, validation before subtraction/addition, defensive bounds handling).
51. Evaluate adding `#[no_panic]`-style checks to critical pure entrypoints only:
    reducers, coordinators, parsers, and other kernel-like functions where link-time no-panic assertions are tractable.
52. Document and enforce boundary rules for untrusted code:
    external tools run out-of-process; Rust panics must not cross FFI boundaries; use `catch_unwind` only as containment where unwind is enabled, not as a substitute for panic-free design.
53. Review in-process dependencies and libraries for trust boundaries; isolate crash-prone or low-trust behavior behind subprocess boundaries where practical.

43. Add worker handshake/version/capability validation.
44. Remove `GatewayAdapter` once runtime coverage is confirmed complete.
45. Keep behavior-state files synchronized as behavior transitions.
