# Bug Report: `pc-consensus` Live Trial Fails to Reliably Produce Structured Drafts

## Summary

A live multi-participant trial against session `d74ac74c248eeae3` showed that the `pc-consensus` REPL can connect to the gateway and remain interactive, but the inner drafting assistant does not reliably transition from natural-language discussion into structured draft creation.

This blocks the core protocol workflow. In most turns, the assistant discussed what it would draft but did not emit tool calls, so the draft buffer remained empty and participants had nothing to review or submit.

One participant (`ava`) eventually succeeded in submitting a single structured proposal after repeated explicit steering. The other two participants were unable to produce valid structured drafts. One of those runs also hit a transport/model-path failure: `response assembly failed: chunk 0 missing 'choices' array`.

## Severity

High

This is a core usability failure for consensus trials because the system can converse but often cannot create the shared structured entries that the protocol depends on.

## Trial Environment

- Date: 2026-03-27
- Gateway: `http://127.0.0.1:3000`
- Session ID: `d74ac74c248eeae3`
- Participants: `ava`, `ben`, `casey`
- Worker pool: 3 connected workers, some CPU-backed and slow
- Trial logs:
  - `ava.md`
  - `ben.md`
  - `casey.md`

## Expected Behavior

When a participant asks the assistant to introduce a topic, create a proposal, add a stance, or otherwise structure deliberation, the assistant should use the consensus tools to create local drafts. The REPL should then show those drafts in `Pending drafts`, allow the participant to review them, and allow `/submit` to append them to the shared session log.

If the backend returns malformed or non-chat data, the app should surface enough diagnostic context to understand what happened.

## Actual Behavior

- All three participants successfully launched `pc-consensus` and joined the live session.
- The session appeared initially empty.
- The assistant usually responded in prose instead of invoking tools like `draft_claim` or `draft_stance`.
- The REPL often ended turns with `Pending drafts: none`, even after explicit requests to create structured entries.
- `ava` eventually managed to submit one structured proposal after repeated explicit steering.
- `ben` repeatedly failed to get any structured draft, including stance creation against Ava's committed proposal.
- `casey` hit repeated response assembly failures: `chunk 0 missing 'choices' array`.

## Reproduction

### Preconditions

1. Start the local gateway on port `3000`.
2. Ensure chat-capable workers are connected.
3. Join existing session `d74ac74c248eeae3` with `pc-consensus`.

### Participant Command

```bash
nix develop -c cargo run --bin pc-consensus -- \
  --gateway-url http://127.0.0.1:3000 \
  --participant <name> \
  join d74ac74c248eeae3
```

### Repro Flow

1. Run `/overview`.
2. Ask the assistant to introduce or structure the topic:
   - `How should a small remote engineering team handle after-hours incident response?`
   - or explicitly ask for a draft proposal / stance.
3. Run `/drafts`.
4. Repeat with stronger language asking the assistant to create an actual structured draft.

### Result

In most runs, the assistant discussed the policy but did not create draft entries. `Pending drafts` remained empty. In one run, response assembly failed before a stable drafting loop could proceed.

## Evidence

### Participant Observations

- `ava`
  - The assistant initially narrated draft creation instead of using tools.
  - After repeated explicit steering, it eventually created and submitted one real claim.

- `ben`
  - The assistant repeatedly stayed in prose mode.
  - Even after Ava's proposal existed, Ben still could not get a structured stance drafted.
  - `/drafts` continued to show `Pending drafts: none`.

- `casey`
  - First-turn latency was slow but the REPL stayed responsive.
  - The assistant produced prose and pseudo-draft language, but no actual draft.
  - The run also hit `response assembly failed: chunk 0 missing 'choices' array`.

### Final Shared Log State

At the end of the trial, the gateway reported exactly one committed entry:

```json
{
  "author": "ava",
  "body": "Proposal: After-hours incident response policy. 1. Define Sev1 as critical production outage. 2. Require named Incident Commander for Sev1. 3. Route non-Sev1 to business hours.",
  "claim_id": "58b4c050-73d4-4a27-a209-870c6474fc1b",
  "claim_kind": "proposal",
  "type": "claim"
}
```

This confirms that the session path and submission path worked at least once, but draft creation was not reliable enough to support multi-participant consensus flow.

## Impact

- Prevents realistic consensus trials from progressing past discussion.
- Makes the human reviewer do structure recovery manually.
- Masks whether the consensus protocol itself is working, because the bottleneck is the assistant's failure to materialize entries.
- Reduces confidence in the REPL as a practical human-facing consensus tool.

## Likely Relevant Code Areas

### 1. Prompting may be too permissive about prose-only behavior

In [src/consensus_cli/llm.rs](/Users/vladimir/devshells/prosthetic-conscience/src/consensus_cli/llm.rs#L117), the system prompt tells the model to prefer structured entries, but it also says:

- "Explain what you are doing in natural language"
- "use tools whenever you need exact state"

That wording appears soft enough that the model can satisfy itself with explanation rather than tool use. The trial behavior is consistent with the model narrating intended structure without actually drafting it.

### 2. The app has no fallback when user intent is clearly "draft this" but no tool calls are emitted

In [src/consensus_cli/llm.rs](/Users/vladimir/devshells/prosthetic-conscience/src/consensus_cli/llm.rs#L74), the loop returns immediately whenever `msg.tool_calls.is_empty()`. If the model answers in prose, the turn ends successfully even if the participant explicitly asked for a structured draft.

### 3. Assembler failures are fatal and not very diagnosable

In [src/chat_gateway/response_assembler.rs](/Users/vladimir/devshells/prosthetic-conscience/src/chat_gateway/response_assembler.rs#L42), `assemble()` errors if a chunk has no `choices` array. That is reasonable for strict OpenAI-format chunks, but the live trial suggests some backends may occasionally return a different JSON shape or an error payload over SSE.

When that happens, [src/consensus_cli/app.rs](/Users/vladimir/devshells/prosthetic-conscience/src/consensus_cli/app.rs#L191) only surfaces a generic `LLM turn failed: ...` message and drops the user message from history. It does not preserve the raw chunk content needed for debugging.

### 4. Tool surface exists, but the model is not consistently using it

The drafting tools are available and clearly defined in [src/consensus/tools.rs](/Users/vladimir/devshells/prosthetic-conscience/src/consensus/tools.rs#L251), including `draft_claim`, `draft_relation`, `draft_stance`, and `impact_analysis`. The issue does not appear to be missing tools; it appears to be unreliable invocation.

## Suggested Next Steps

1. Tighten the consensus system prompt so explicit user requests for structured contributions require tool use rather than optional tool use.
2. Add a guardrail in `ConsensusLlm::run_turn()` for "draft-seeking" turns:
   - if the user asked to create a claim/stance/relation and the model returns prose without tool calls, retry with a stronger corrective system message.
3. Improve diagnostics for assembler failures:
   - log or surface the first offending raw chunk when `choices` is missing.
4. Add an integration test that simulates a live turn where the model responds with prose-only text to an explicit draft request, and verify the client retries or flags the turn as structurally incomplete.
5. Add an integration test for malformed SSE payloads reaching `assemble()` so the failure mode is easier to debug.

## Suggested Owner Areas

- Prompting / tool-use loop:
  - `src/consensus_cli/llm.rs`
- REPL error handling:
  - `src/consensus_cli/app.rs`
- SSE/message assembly robustness:
  - `src/chat_gateway/response_assembler.rs`
- Consensus tool affordances:
  - `src/consensus/tools.rs`

## Related Logs

- [ava.md](/Users/vladimir/devshells/prosthetic-conscience/trial-logs/live-consensus-2026-03-27/ava.md)
- [ben.md](/Users/vladimir/devshells/prosthetic-conscience/trial-logs/live-consensus-2026-03-27/ben.md)
- [casey.md](/Users/vladimir/devshells/prosthetic-conscience/trial-logs/live-consensus-2026-03-27/casey.md)
