# Consensus Protocol Trial Methodology

## Goal

Validate the consensus protocol's end-to-end behavior by running multiple automated agents through the full deliberation flow. Each agent drives a `pc-consensus` instance as a subprocess, playing the role of a human participant. The inner LLM (via the gateway worker) handles summarization, draft preparation, and tool use. The outer agent evaluates the inner LLM's output and makes participant-level decisions.

This tests:
- Quality of LLM summarization of deliberation state
- Accuracy of structured draft preparation (claims, relations, stances)
- Correctness of the consensus engine under concurrent multi-participant use
- Session synchronization across participants (WS entry delivery, catch-up)
- Convergence behavior of the consent-based protocol
- Impact analysis usefulness in guiding participant decisions

## Architecture

```
                           ┌─────────────────┐
                           │    Gateway       │
                           │  (prosthetic-    │
                           │   conscience)    │
                           └────┬────┬────┬───┘
                         WS/HTTP│    │    │
              ┌─────────────────┘    │    └─────────────────┐
              │                      │                      │
      ┌───────┴───────┐     ┌───────┴───────┐     ┌───────┴───────┐
      │   pc-worker   │     │   pc-worker   │     │   pc-worker   │
      │ (inference    │     │ (shared or    │     │ (shared or    │
      │  backend)     │     │  separate)    │     │  separate)    │
      └───────────────┘     └───────────────┘     └───────────────┘

      ┌───────────────┐     ┌───────────────┐     ┌───────────────┐
      │ pc-consensus  │     │ pc-consensus  │     │ pc-consensus  │
      │ --participant │     │ --participant │     │ --participant │
      │   alice       │     │   bob         │     │   carol       │
      └───────┬───────┘     └───────┬───────┘     └───────┬───────┘
          stdin/stdout          stdin/stdout          stdin/stdout
              │                      │                      │
      ┌───────┴───────┐     ┌───────┴───────┐     ┌───────┴───────┐
      │  Outer Agent  │     │  Outer Agent  │     │  Outer Agent  │
      │  (persona:    │     │  (persona:    │     │  (persona:    │
      │   Alice)      │     │   Bob)        │     │   Carol)      │
      └───────────────┘     └───────────────┘     └───────────────┘
```

Two LLM layers per participant:
- **Inner LLM** (via gateway/worker): the consensus drafting assistant. It reads the deliberation state, uses tools to draft entries, explains its reasoning, and presents drafts for review. This is what `pc-consensus` already does.
- **Outer agent**: plays the human role. It reads the inner LLM's output, decides what to say next, reviews drafts, issues `/submit` when satisfied. This is the layer being added for the trial.

The outer agent does not bypass or modify the inner LLM's behavior. It interacts through the same stdin/stdout interface a human would use.

## Outer Agent Responsibilities

The outer agent drives `pc-consensus` as a subprocess:

1. **Read stdout** — parse session state, LLM responses, draft summaries, impact analysis
2. **Decide action** — based on persona, goals, and current deliberation state:
   - Type natural language to discuss, propose, or object
   - `/overview` to inspect current state
   - `/claim <id>` to examine specific claims
   - `/drafts` to review pending drafts
   - `/submit` to commit drafts (followed by `y` to confirm)
   - `/clear` to discard drafts
3. **Write to stdin** — send the decided action
4. **Repeat** — until the deliberation reaches a conclusion or a turn limit is hit

## Persona Design

Each outer agent receives a persona that defines its perspective, priorities, and behavioral style. Personas should create productive tension to exercise the protocol's convergence mechanisms.

Example persona set for an "authentication approach" deliberation:

**Alice (Security Engineer)**
```
You are Alice, a senior security engineer. You prioritize security
guarantees above developer convenience. You favor well-established
standards. You are skeptical of novel approaches without proven track
records. You will block proposals that have unaddressed security
concerns. When reviewing drafts, verify that claims are precise and
that relations correctly capture logical dependencies.
```

**Bob (Product Engineer)**
```
You are Bob, a product engineer focused on developer experience and
iteration speed. You favor pragmatic solutions that unblock the team
quickly. You push back on security requirements that significantly
increase implementation complexity without clear threat models. You
are willing to consent to imperfect solutions with a plan to improve.
```

**Carol (Architect)**
```
You are Carol, a systems architect. You think about long-term
maintainability, migration paths, and system boundaries. You raise
concerns about coupling and reversibility. You often propose
alternatives that balance competing priorities. You use the "reference"
claim kind to introduce relevant background context.
```

## Running the Trial

### Prerequisites

- Gateway binary built
- At least one worker with access to an OpenAI-compatible inference server
- Outer agent framework capable of subprocess I/O (Claude Code Task tool, a Python script with pexpect, or similar)

### Setup

```bash
# 1. Start the gateway
cargo run --bin prosthetic-conscience

# 2. Start one or more workers
cargo run --bin pc-worker -- --inference-url http://localhost:11434/v1

# 3. Create the session (first participant)
cargo run --bin pc-consensus -- --participant alice create
# Note the session ID from output

# 4. Join with remaining participants
cargo run --bin pc-consensus -- --participant bob join <session-id>
cargo run --bin pc-consensus -- --participant carol join <session-id>
```

### Seeding the Deliberation

The first outer agent (or a human) provides the opening topic. Example:

```
We need to decide on an authentication approach for our API.
The options being discussed are JWT tokens, session cookies,
and OAuth2 with a third-party provider. Please start by
proposing the approach you think is best.
```

### Turn Management

Outer agents can operate concurrently (each in its own process), or be sequenced for more controlled observation. Options:

- **Concurrent**: all agents run simultaneously, reading and responding as entries arrive via `[sync]` notifications. Most realistic.
- **Round-robin**: agents take turns, with each agent given a fixed number of interactions before yielding. Easier to follow and debug.
- **Facilitator-guided**: a human (or a fourth agent) prompts each participant in turn, directing attention to unresolved disputes.

### Termination

The trial ends when:
- A proposal is resolved (accepted/rejected/tabled) via `/submit` of a resolve entry
- A turn limit is reached (e.g., 10 turns per agent)
- The outer agents converge (all express consent/support with no blocks)
- A human observer stops the trial

## What to Observe

### Inner LLM Quality

- **Summarization accuracy**: does the overview presented to each participant correctly reflect the deliberation state?
- **Draft correctness**: are claims categorized correctly (item/proposal/fact)? Are relations directionally correct (attack vs support, correct source/target)?
- **Tool use efficiency**: does the LLM use `overview` and `claim_detail` before drafting, or does it draft blind?
- **Stance extraction**: when a participant expresses a position in natural language, does the LLM correctly map it to the right discrete position (block/object/consent/support)?
- **Impact awareness**: does the LLM use `impact_analysis` or `preview_overview` before suggesting submission?

### Protocol Behavior

- **Convergence**: does the set of unresolved objections shrink over time?
- **Epistemic status accuracy**: do the solver's labels (Established, Contested, Defeated, Unexamined) match intuitive assessment of the deliberation?
- **Attention routing**: does the LLM surface unexamined claims and bottleneck disputes?
- **Consent mechanics**: are blocks accompanied by reasoning? Does the protocol surface what would need to change to resolve a block?

### System Behavior

- **Session sync**: do all participants see the same entries in the same order?
- **Entry integrity**: do entries survive the full round-trip (draft → submit → WS append → gateway → WS notify → engine append)?
- **Reconnect resilience**: if a participant disconnects and reconnects, does catch-up restore full state?
- **Concurrent submissions**: if two participants submit simultaneously, are both entries appended correctly?

## Recording Results

For each trial run, capture:

1. **Session log**: the raw append-only entry log (retrieve via `GET /v1/sessions/<id>/entries`)
2. **Per-participant transcript**: stdout from each `pc-consensus` instance (the full conversation between outer agent and inner LLM, including tool calls)
3. **Final state snapshot**: `/overview` output from any participant at trial end
4. **Observations**: notes on inner LLM quality, protocol behavior, and any issues encountered

## Iteration

After each trial, review:
- Were there misclassified relations? (most likely failure mode per the design doc)
- Did the LLM draft entries the outer agent had to reject and redo?
- Did the protocol converge, stall, or diverge?
- Were there synchronization issues or lost entries?

Use findings to refine:
- System prompt (in `consensus_cli/llm.rs`)
- Tool descriptions (in `consensus/tools.rs`)
- Persona prompts for outer agents
- Protocol mechanics if structural issues are found
