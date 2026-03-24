# Near-Term TODO

Snapshot date: 2026-03-24

## Consensus protocol implementation

Goal: implement the client-side consensus protocol described in `docs/consensus-protocol-design.md`. The protocol runs entirely client-side — the gateway remains content-opaque. Implementation lives in `src/consensus/`.

### Phase 1: Solver (pure graph computation)

No entry types needed. The solver works on an abstract graph: node IDs and directed edges with a kind (attack/support).

1. Grounded semantics fixpoint algorithm: iteratively label nodes as IN/OUT/UNDEC.
2. Property tests: idempotency, unattacked nodes are IN, attacked-by-IN is OUT, fixpoint stability, empty graph, single-node graph, mutual attack cycles.
3. Support edge propagation (BAF extension): decide and implement semantics for support edges alongside attacks.

### Phase 2: Reducer (log replay → graph)

Introduce entry types as the reducer needs them. Minimal set to produce a graph the solver can consume.

4. Entry types: `claim` (claim_id, author, body), `relation` (source_id, target_id, kind, author), `stance` (target_id, author, position). Minimal position vocabulary.
5. Pure reducer: fold over `Vec<Entry>` → `MaterializedState` containing the argumentation graph + stance maps.
6. Property tests: deterministic replay, monotonic claim growth, stance supersession, unknown-ID handling.

### Phase 3: Epistemic status

7. Combine solver output (IN/OUT/UNDEC) + stance coverage → epistemic status per claim (established, unexamined, contested, defeated, unresolved).
8. Tests for all five status categories and edge cases (e.g., IN with no stances vs IN with mixed stances).

### Phase 4: Resolve, amend, impact analysis

Add entry types incrementally:

9. `resolve` entry: close a proposal, reducer removes it from active graph. Tests for all outcome variants.
10. `amend` entry: update claim body, verify solver results recompute correctly. Tests for stance preservation.
11. Impact analysis: run solver on `current_graph + hypothetical_entries`, diff results. Tests for status change detection.

### Phase 5: LLM harness

12. Entry drafting: natural language → structured entry candidates.
13. Attention routing: surface unexamined claims, solver-detected bottlenecks, stance coverage gaps.
14. Conversational interface: manage draft review/edit/submit cycle with participant.

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
