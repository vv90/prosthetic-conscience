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

### Phase 4: Consensus engine and structured rendering

The engine is the stateful core that owns the log, materialized state, solver results, and draft buffer. Rendering returns structured types for both UI and LLM consumption.

1. Structured render types: `OverviewData`, `ClaimDetail`, `AttentionSignal` — serde-serializable for WASM↔JS boundary.
2. `ConsensusEngine` struct: owns log + state + drafts, provides query/draft/submit methods.
3. Overview rendering: claim counts, open items, proposals with statuses, attention signals.
4. Claim detail rendering: body, author, status, stances, relations in/out.
5. Attention rendering: participant-specific prioritized list (unexamined, unstanced, bottlenecks).
6. Text formatting layer: `format.rs` — renders structured types to text for terminal/LLM.
7. Draft buffer: `add_draft`, `edit_draft`, `remove_draft`, `show_drafts`, `submit_drafts`.
8. Impact analysis: run solver on current state + hypothetical draft entries, diff results.

### Phase 5: Terminal prototype binary

Separate binary (`pc-consensus`), not an extension of `pc-client`. Uses the engine directly.

9. Binary skeleton: session WS subscription + LLM tool loop.
10. Session sync: subscribe to session log, feed entries through engine's `append_entry`.
11. LLM system prompt: inject overview rendering as context before each turn.
12. Tool dispatch: LLM calls engine methods via tool interface (query, draft, submit).
13. Draft review cycle: LLM drafts entries, participant reviews in conversation, explicit submit.

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
