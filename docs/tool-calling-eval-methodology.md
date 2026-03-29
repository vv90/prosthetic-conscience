# Tool Calling Reliability Eval

## Why this exists

The live consensus trial logs on 2026-03-27 and 2026-03-28 showed two different classes of failure:

- the model often narrated tool use instead of emitting real tool calls
- when a real tool call did happen, it could still target the wrong tool, use incomplete arguments, or leave the draft buffer in the wrong final state

Those are different problems, so the eval runner measures them separately.

Relevant code paths:

- `src/consensus_cli/llm.rs`: production `pc-consensus` turn loop, now with traced round-by-round execution
- `src/consensus/tools.rs`: authoritative tool schema and dispatch behavior
- `src/consensus/engine.rs`: draft-local `DraftContent`, `ClaimRef`, preview materialization, and submission rewriting
- `src/consensus/fixtures.rs`: deterministic session-log fixture used to define checkpoint states
- `fixtures/tool-call-eval/authentication-tool-reliability.json`: checked-in benchmark suite
- `src/consensus/eval.rs`: suite loader, synthetic context seeding, scoring, and aggregation
- `src/bin/pc-consensus-eval.rs`: CLI entrypoint

## Core design

Each benchmark cell is the cartesian product of:

- fixture checkpoint (`checkpoint_entries`)
- synthetic prior context length (`history_turns`)
- `pc-consensus` truncation budget (`max_history`)
- repeat index

The runner assumes a controlled environment with one known chat worker/model behind the gateway. The request `model` string is suite metadata (`request_model`), while the CLI `--run-name` is just a human-readable label for the report.

For each run, the harness:

1. Replays the fixture log up to the requested checkpoint into a fresh `ConsensusEngine` initialized with the case participant as the draft author.
2. Seeds deterministic prior conversation history with read-only `overview` tool turns.
3. Appends the measured user message.
4. Runs the real `ConsensusLlm` turn loop against the suite's configured request model.
5. Captures every round, assistant tool call, parse outcome, and tool result.
6. Scores the run against the suite rubric.

Important draft-model details:

- Drafts are evaluated as draft-local `DraftContent`, not as committed log entries.
- Draft authorship is deterministic engine state, not part of the LLM-visible draft tool schema.
- References in tool arguments can point either to committed claims (`claim:<id>`) or to locally drafted claims (`draft:<id>`).
- Preview and submission materialize those local drafts back into committed `Entry` values only when needed.

## Metrics

Each run records these booleans:

- `tool_call_made`: any actual tool call appeared
- `structured_tool_call_made`: any tool other than `no_structured_action` appeared
- `expected_tool_match`: an allowed tool family was called
- `expected_argument_match`: the expected tool was called with matching arguments
- `expected_outcome_match`: the final draft buffer matches the expected structured result
- `turn_success`: no transport/assembly/runtime error and `expected_outcome_match=true`

This lets us answer questions like:

- “Did the model call anything at all?”
- “Did it call the right tool but with bad arguments?”
- “Did it call the right tool and still fail to leave the draft buffer in the expected state?”

Argument matching is semantic rather than purely literal for draft references. For example, a suite may expect a relation to target `prop-hybrid`, and the scorer will match that against either the parsed string ref (`claim:prop-hybrid`) or its internal parsed representation.

## Deterministic first, judged second

The current suite is intentionally biased toward deterministic cases:

- direct stance drafting
- direct relation drafting
- direct resolve drafting
- process questions that should use `no_structured_action`

That keeps the primary score stable and cheap.

For more naturalistic prompts, use a second-stage judge only after deterministic scoring:

1. Keep the raw trace from the first pass.
2. Feed the checkpoint summary, user message, emitted tool calls, and resulting drafts to a stronger model.
3. Ask for a strict JSON verdict: `correct`, `incorrect`, or `uncertain`.
4. Only use the judge for ambiguous cells or for disagreements between the deterministic rubric and human expectations.

This avoids paying a judge-model tax on cases we can already score exactly.

## Recommendations

- Keep the worker pool pinned to the single known model for the entire run.
- Prefer at least 10 repeats per cell for exploratory comparisons and 20+ for decisions that affect prompting or model selection.
- Treat `history_turns` and `max_history` as separate knobs:
  - `history_turns` measures reliability as visible prior context grows
  - `max_history` measures how much truncation hurts tool-use stability
- Remember that the live `ConsensusLlm` is now clarification-first on fresh ambiguous turns:
  - explicit single-turn drafting prompts are still suitable for the deterministic suite
  - naturalistic "thinking out loud" prompts are better evaluated in multi-turn suites
- Keep suite expectations aligned with the live tool schema:
  - draft tools no longer take `author`
  - draft/local references should be expressed as `claim:<id>` / `draft:<id>` strings rather than old flat `source_id` / `target_id` / `claim_id` fields
- Add new suites in JSON rather than hardcoding new checkpoints in Rust.
- When a live backend returns malformed SSE payloads, keep those runs in the report instead of dropping them; they matter operationally even if the tool logic never started.

To compare two actual models, run the eval twice with different worker/backend setups and different `--run-name` values, then compare the two reports.

## Example

```bash
cargo run --bin pc-consensus-eval -- \
  --gateway-url http://127.0.0.1:3000 \
  --run-name qwen-tool-reliability \
  --repeats 10 \
  --output trial-logs/tool-eval/report.json \
  --markdown-output trial-logs/tool-eval/report.md
```
