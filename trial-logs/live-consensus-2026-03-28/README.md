# Live Consensus Trial

- Date: 2026-03-28
- Gateway: http://127.0.0.1:3000
- Session ID: `12b2e92d12c94f51`
- Topic assumption: "How should a small remote engineering team handle after-hours incident response?"

## Participants

- `ava`: Reliability engineer focused on incident response quality, severity clarity, and making sure serious issues are handled fast enough.
- `ben`: Product engineer focused on sustainability, low process overhead, and avoiding burnout from excessive paging.
- `casey`: Engineering manager focused on fairness, simple policy, and a plan the team can actually adopt next month.

## Change Under Test

- `pc-consensus` now sends `tool_choice: "required"` on every turn.
- The consensus tool surface now includes `no_structured_action` as the required non-drafting escape hatch.
- The LLM system prompt now explicitly requires tool use on every turn.

## Logging

Each participant owns one `pc-consensus` REPL and one log file:

- `ava` -> `ava.md`
- `ben` -> `ben.md`
- `casey` -> `casey.md`

Each participant should record:

- what they observed about the deliberation
- any drafting or sync issues
- whether required tool use improved behavior
- whether the system helped or hindered convergence
