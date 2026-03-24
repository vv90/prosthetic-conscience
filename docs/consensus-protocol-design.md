# Consensus Protocol Design

## Problem Statement

We need a protocol for collaborative deliberation that produces consensus among participants. Participants suggest topics, propose solutions, provide arguments, and express positions. The protocol sits on top of the gateway's existing append-only session log — a shared, ordered sequence of JSON entries with real-time pub-sub.

The system must work asynchronously, potentially at scale (dozens to hundreds of participants), and with LLM agents mediating between participants and the shared log. Participants interact in natural language with their personal LLM agent; they do not read or write log entries directly.

## Design Exploration

### Systems Studied

We surveyed consensus and deliberation mechanisms across five domains:

**Online deliberation platforms:**

- Loomio (consent-based proposals with agree/abstain/disagree/block stances, timeboxed decisions, position + reasoning pattern)
- Polis (statement voting, PCA-based opinion clustering, group-aware consensus detection)
- Kialo (structured pro/con argument trees, impact voting on argument quality)

**Blockchain governance:**

- Snapshot/Governor (time-boxed proposal lifecycle with quorum)
- Optimistic governance (approved-unless-objected within window)
- Conviction voting (time-weighted continuous voting)
- Quadratic voting (diminishing returns on concentrated influence)
- Holographic consensus (prediction-market-based attention filtering)
- Rage quit / exit rights (Moloch DAO)
- Liquid democracy / delegation

**Traditional deliberation protocols:**

- Robert's Rules of Order (motion lifecycle, precedence stack, seconding)
- Quaker consensus (sense of the meeting, clerk's minute, standing aside vs blocking)
- Sociocracy (consent-based, "no reasoned objections," structured rounds)
- Fist-to-Five / Gradients of Agreement (Sam Kaner) (multi-level position scales)
- Dot voting (budget-constrained prioritization)
- Deliberative polling (informed deliberation with balanced briefing materials)
- Open Space Technology (self-organizing agenda creation)

**Formal argumentation theory:**

- Dung's Abstract Argumentation Frameworks (arguments + attacks, extension semantics)
- Bipolar Argumentation Frameworks (adds support relations)
- ASPIC+ (structured arguments with defeasible reasoning)
- Carneades (per-claim proof standards)
- Assumption-Based Argumentation
- Answer Set Programming / SAT solvers for computing extensions

### Key Insights That Shaped the Design

**From Sociocracy/Quaker practice:** Consent (no objections) is more achievable and more useful than consensus (everyone agrees). The protocol should converge by narrowing objections, not broadening enthusiasm.

**From Loomio:** Stances should carry both a discrete position (with protocol-meaningful semantics like "block") and reasoning. Timeboxing creates urgency. Position mutability (changing your mind) is essential.

**From formal argumentation:** Claims, attacks, and supports form a graph. Deterministic solvers can compute which claims are defensible, find bottleneck disputes, and do hypothetical reasoning — things LLMs cannot do reliably.

**From Polis/deliberative polling:** At scale, participants cannot read everything. Summarization and attention routing are essential. LLMs fill this role naturally.

**From optimistic governance:** Default-approve (resolved unless objected to) reduces overhead for uncontested items and aligns with consent-based decision-making.

**From Kialo:** Separating argument quality from agreement prevents echo chambers. Structured pro/con trees make reasoning legible.

### Patterns We Deferred

| Pattern                                    | Reason for deferral                                                       |
| ------------------------------------------ | ------------------------------------------------------------------------- |
| Polis-style PCA clustering                 | Needs 50+ participants; can be added later without protocol changes       |
| Quadratic voting                           | Requires Sybil resistance; overkill until identity/weighting layer exists |
| Conviction voting (time-weighted)          | Interesting but complex; future extension                                 |
| Liquid democracy / delegation              | Reduces to a weighting concern; future extension                          |
| Holographic consensus / prediction markets | Needs scale and liquidity                                                 |
| Robert's Rules precedence stack            | Too procedurally heavy                                                    |

## Design Decisions

### 1. Items vs. Proposals

**Decision:** Separate agenda items (topics/problems) from proposals (suggested solutions).

An item like "how should we handle authentication?" can have multiple competing proposals ("use JWT," "use session cookies"). Stances attach to proposals, not items. This avoids forcing opposition to one proposal to imply opposition to discussing the topic. Items are resolved when one of their proposals is accepted, or when the item is tabled/withdrawn.

### 2. Everything Is a Claim

**Decision:** Facts, conditionals, relevance assertions, and value statements are all modeled as claims. The protocol does not distinguish them structurally.

A claim like "70% of our traffic is unauthenticated" and a claim like "that fact is relevant to the caching proposal" are both just claims. Their relationship is captured by explicit relation entries (attack/support edges), not by entry type.

This maps directly to formal argumentation frameworks (Dung, BAF) where arguments are opaque nodes and the graph structure carries all the reasoning.

A `claim_kind` field provides a classification hint ("item," "proposal," "fact," "conditional," "value," "reference") for rendering and attention routing, but the solver and protocol treat all claims identically.

The `"reference"` kind marks informational claims introduced as shared context rather than assertions to be evaluated — background research, definitions, external data. The solver treats them like any other claim (unattacked = IN), but the LLM knows not to solicit stances on them unless someone challenges one. This handles the common case where a participant shares a large body of context (e.g., research findings) that produces many claims, most of which are not meant to be debated.

### 3. Stances Use a Discrete Position Vocabulary with Continuous Intensity

**Decision:** Stances carry a discrete `position` from a fixed vocabulary (with protocol-meaningful semantics) plus an optional continuous `intensity` value (0.0-1.0) for aggregation.

The discrete position is what the protocol acts on. Blocks trigger specific behavior (require justification, must be addressed). The continuous intensity is metadata that the LLM extracts from natural conversation for richer aggregation and visualization.

Position vocabulary for proposals (derived from Sociocracy + Fist-to-Five + Kaner):

| Position      | Meaning                                                  |
| ------------- | -------------------------------------------------------- |
| `block`       | Cannot proceed; must explain why (Sociocratic objection) |
| `object`      | Serious concerns; need discussion                        |
| `stand_aside` | Disagree but won't prevent it                            |
| `abstain`     | No opinion / pass                                        |
| `consent`     | Acceptable; "good enough for now, safe enough to try"    |
| `support`     | Actively endorse                                         |
| `champion`    | Will drive implementation                                |

For fact-type claims: `reject` / `doubt` / `unsure` / `accept` / `strongly_accept`.

### 4. Three-Layer Architecture: People, LLM, Solver

**Decision:** Distribute responsibilities according to each component's strengths.

Each component does what it's best at. No component is trusted beyond its capability.

**People** are good at: judging truth from experience, applying values and priorities, creative problem-solving, detecting wrong framings, bringing domain knowledge, final authority on acceptance.

**LLM agents** (per-participant, client-side) are good at: natural language understanding and generation, summarizing large volumes of text, extracting structure from conversation, translating between human intent and structured entries, explaining complex state, routing attention, detecting sentiment and patterns.

**Argumentation solver** (deterministic, client-side) is good at: computing which claims are defensible (grounded/preferred extensions), consistency checking, hypothetical reasoning ("what changes if this claim is removed?"), bottleneck detection ("resolving this one dispute unblocks three proposals"), dependency tracing, exact aggregation.

### 5. People Own the Graph — LLM as Drafting Assistant

**Decision:** The LLM never writes to the shared log directly. It drafts structured entries that the participant reviews and explicitly submits.

This is the critical trust boundary. The most failure-prone task in the system is relation extraction — determining which claims attack or support which other claims. LLMs are probabilistic and can misclassify relations (phantom attacks, wrong direction, wrong target). Since the solver's output is only as good as its input graph, graph quality is paramount.

The interaction model:

1. Participant converses naturally with their LLM agent
2. LLM prepares a draft submission (claim + relations + stance) displayed in a persistent view
3. Participant reviews the draft in a summarized, human-readable format
4. Participant can request edits through natural language ("that's not an attack, it's an alternative")
5. LLM adjusts the draft
6. Participant explicitly submits when the draft matches their intent
7. Before submission, the solver can show impact analysis ("submitting this changes the status of 3 proposals")

This eliminates the class of errors where the LLM silently misinterprets intent. Every relation in the graph was approved by a human.

### 6. Solver Computes Defensibility, Not Truth

**Decision:** The solver computes logical defensibility within the current graph. It does not determine truth. The gap between "defensible" and "accepted" is tracked explicitly.

An unattacked claim is defensible (IN in the solver's grounded extension) but might be wrong — nobody has checked it yet. The system tracks a three-state epistemic status per claim:

| Solver says | Stances say              | Status          | Meaning                              |
| ----------- | ------------------------ | --------------- | ------------------------------------ |
| IN          | Accepted by participants | **Established** | Defensible and endorsed              |
| IN          | No stances yet           | **Unexamined**  | Defensible but nobody has checked    |
| IN          | Mixed/disputed stances   | **Contested**   | Logically sound but people disagree  |
| OUT         | --                       | **Defeated**    | Attacked by an established claim     |
| UNDEC       | --                       | **Unresolved**  | Involved in cycles or mutual attacks |

The LLM uses this to guide productive conversation:

- **Unexamined**: "Carol claimed X. Nobody has confirmed or challenged this. Does it match your understanding?"
- **Contested**: "The solver shows this is logically defensible, but two people doubt it. Can someone provide evidence?"
- **Defeated**: "This concern has been addressed by Y — if Y is accurate. The group hasn't confirmed Y yet."

### 7. Convergence Is Structural, Not Facilitated

**Decision:** The protocol structure plus LLM mediation create natural convergence pressure. No explicit facilitator role is needed.

Convergence mechanisms:

- **Consent-based resolution:** Proposals require "no objections" (blocks), not unanimous support. The bar is lower and naturally achievable.
- **Optimistic resolution:** Proposals can default to accepted if no blocks are raised within a deadline.
- **Blocks require justification:** A block must include reasoning, which creates engagement rather than veto.
- **LLM-mediated objection resolution:** Each participant's LLM asks blockers what amendment would address their concern, and suggests amendments to proposers.
- **Solver-detected bottlenecks:** "Resolving this one factual dispute would unblock three proposals" — the LLM surfaces this to focus attention.
- **Narrowing objections is monotonic:** Each addressed objection removes a blocker. The set of unresolved objections only shrinks.

Facilitation-like functions are distributed:

- Attention routing: each participant's LLM, based on solver output and stance coverage
- Turn management / prompting: LLM notices who hasn't weighed in
- Deadline enforcement: field on proposals, automatic
- Process enforcement: protocol logic in the client-side reducer
- Synthesis: LLM summarizes for each participant locally (not in the shared log)

### 8. Synthesis and Facilitation Are Not Log Entries

**Decision:** Summaries and facilitation actions are client-side concerns, not shared log entries.

Each participant's LLM maintains its own summary of the log state to guide its conversation with that participant. Different participants may get different summaries emphasizing different things. These are local computations, not shared artifacts.

If a participant wants to share a summary or synthesis with the group (like a Quaker clerk drafting a minute), they do so as a regular claim or comment — not as a special system entry type.

### 9. Vote Weighting Is a Future Concern

**Decision:** The schema carries author identity on every entry. Weighting (by stake, seniority, reputation, or any other mechanism) is a separate aggregation layer applied at query time.

The log and protocol are weight-agnostic. A future weighting oracle maps authors to weights without changing the entry format.

### 10. Exploratory Work Happens Locally; Only Crystallized Outputs Enter the Log

**Decision:** The shared log contains only structured deliberation artifacts (claims, relations, stances, resolutions). The exploratory process that produces them — brainstorming, research, analysis, considering and discarding options — happens locally between a participant and their LLM agent.

This distinction emerged from examining how deliberation actually works: most of the productive work is generative exploration (considering multiple options, analyzing tradeoffs, researching context). The shared log sees only the results — a specific proposal, a factual claim, a stance. The log is not a transcript of conversation; it's a record of deliberation outputs.

This also means participants can take as long as they need to formulate their contribution. The LLM helps them explore, refine, and structure their thinking before anything is submitted to the group.

### 11. Gateway Remains Content-Opaque

**Decision:** The gateway stores and relays log entries without interpreting their content.

The gateway's session kernel (append-only log with pub-sub) is unchanged. All protocol logic — entry parsing, graph construction, solver computation, stance aggregation, LLM mediation — runs client-side. This preserves the Phase 2 encryption upgrade path where the gateway cannot see entry contents.

The solver is also content-opaque: it sees only claim IDs and relation edges, never natural language.

## Entry Types

Six entry types, plus a freeform escape hatch.

### Common Fields

Every entry carries:

| Field          | Category     | Producer                                            |
| -------------- | ------------ | --------------------------------------------------- |
| `type`         | MVP          | system                                              |
| `author`       | MVP          | human/system                                        |
| `authored_via` | nice-to-have | system (`"llm_mediated"` / `"direct"` / `"system"`) |

The log provides `index` (position) and ordering. Timestamps are a potential addition if entries need to be self-contained.

### `claim`

Any assertion: agenda items, proposals, facts, conditionals, value statements.

| Field           | Category     | Notes                                                                                                  |
| --------------- | ------------ | ------------------------------------------------------------------------------------------------------ |
| `type: "claim"` | MVP          |                                                                                                        |
| `claim_id`      | MVP          | unique identifier                                                                                      |
| `author`        | MVP          |                                                                                                        |
| `body`          | MVP          | natural language text of the claim                                                                     |
| `claim_kind`    | MVP          | `"item"` / `"proposal"` / `"fact"` / `"conditional"` / `"value"` / `"reference"` — classification hint |
| `parent_id`     | MVP          | optional; links proposals to their parent item                                                         |
| `resolve_mode`  | feature      | `"explicit"` / `"optimistic"` — for proposals                                                          |
| `deadline`      | feature      | ISO 8601 — when stances close, for proposals                                                           |
| `tags`          | nice-to-have | topic tags for attention routing                                                                       |
| `sources`       | nice-to-have | references, links, evidence                                                                            |
| `authored_via`  | nice-to-have | provenance                                                                                             |

### `relation`

An edge in the argumentation graph. One claim attacks or supports another.

| Field              | Category     | Notes                           |
| ------------------ | ------------ | ------------------------------- |
| `type: "relation"` | MVP          |                                 |
| `source_id`        | MVP          | claim making the attack/support |
| `target_id`        | MVP          | claim being attacked/supported  |
| `kind`             | MVP          | `"attacks"` / `"supports"`      |
| `author`           | MVP          | who asserts this relation       |
| `body`             | nice-to-have | explanation of the relationship |
| `authored_via`     | nice-to-have | provenance                      |

### `stance`

A participant's position on a claim or relation.

| Field            | Category     | Notes                                     |
| ---------------- | ------------ | ----------------------------------------- |
| `type: "stance"` | MVP          |                                           |
| `target_id`      | MVP          | what this stance is about                 |
| `target_kind`    | MVP          | `"claim"` / `"relation"`                  |
| `author`         | MVP          |                                           |
| `position`       | MVP          | discrete label from fixed vocabulary      |
| `intensity`      | nice-to-have | continuous 0.0-1.0                        |
| `body`           | nice-to-have | reasoning (required for `block` position) |
| `supersedes`     | feature      | log index of prior stance being replaced  |
| `authored_via`   | nice-to-have | provenance                                |

### `amend`

Revise a claim.

| Field           | Category     | Notes                      |
| --------------- | ------------ | -------------------------- |
| `type: "amend"` | feature      |                            |
| `claim_id`      | feature      | which claim                |
| `author`        | feature      |                            |
| `new_body`      | feature      | revised text               |
| `reason`        | nice-to-have | why the amendment was made |
| `authored_via`  | nice-to-have | provenance                 |

**Proposal replacement pattern:** When a proposal needs fundamental restructuring (not a minor wording change), the correct pattern is `resolve` with `outcome: "withdrawn"` on the old proposal, followed by a new `claim` with `claim_kind: "proposal"`. This is distinct from `amend`, which is for revisions where the identity and intent of the claim are preserved. Withdrawal + new proposal explicitly signals that existing stances and relations on the old proposal may not apply to the new one, and participants should re-evaluate.

### `resolve`

Close a proposal-kind claim.

| Field             | Category     | Notes                                                          |
| ----------------- | ------------ | -------------------------------------------------------------- |
| `type: "resolve"` | MVP          |                                                                |
| `claim_id`        | MVP          | which proposal                                                 |
| `author`          | MVP          | who resolved it (or `"system"` for optimistic auto-resolution) |
| `outcome`         | MVP          | `"accepted"` / `"rejected"` / `"tabled"` / `"withdrawn"`       |
| `authored_via`    | nice-to-have | provenance                                                     |

### `comment`

Freeform discussion for contributions that don't fit the structured types.

| Field             | Category     | Notes                         |
| ----------------- | ------------ | ----------------------------- |
| `type: "comment"` | MVP          |                               |
| `claim_id`        | nice-to-have | optional — what this is about |
| `author`          | MVP          |                               |
| `body`            | MVP          |                               |
| `authored_via`    | nice-to-have | provenance                    |

## Client-Side Architecture

### Log Replay and Materialized State

The client replays the append-only log through a pure reducer to build the current state:

```
Log entries --> Reducer --> {claims, relations, stances} --> Solver --> extensions
```

The reducer handles: claim creation and amendment (latest amend wins), relation assertion, stance supersession (latest stance per author per target), withdrawal, resolution.

### Argumentation Solver

Rust-native implementation of grounded semantics (iterative fixpoint, O(n^2 \* m) worst case, microseconds for realistic sizes). The algorithm:

1. Start: IN = {}, OUT = {}, UNDEC = all claims
2. Label any claim with no attackers as IN
3. Label any claim attacked by an IN claim as OUT
4. Repeat until fixpoint
5. Everything still unlabeled is UNDEC

For Bipolar Argumentation (with support edges), the algorithm extends to propagate support influence alongside attacks.

The solver is content-opaque: it sees only claim IDs and relation edges. It runs client-side, producing identical results for all participants from the same log (deterministic).

### Epistemic Status Computation

Combines solver output with stance coverage:

- **Established**: IN + accepted stances
- **Unexamined**: IN + no stances
- **Contested**: IN + disputed stances
- **Defeated**: OUT
- **Unresolved**: UNDEC

### Anomaly Detection

The solver flags structural anomalies for the LLM to surface:

- Orphan claims (no relations to anything)
- Unexpected UNDEC clusters (possible misclassified relations)
- High-impact edges (one relation changes status of many claims)
- Stance/solver disagreement (many people accept what the solver says is OUT)

### LLM Agent Role

Per-participant, client-side. Responsibilities:

- Translate natural language to draft structured entries
- Present drafts for participant review and editing
- Summarize log state and route attention
- Explain solver results in natural language
- Surface anomalies and unexamined claims
- Guide conversation toward resolving bottleneck disputes

The LLM never writes to the shared log. All submissions go through explicit participant approval.

### Pre-Submission Impact Analysis

Before a participant submits, the client runs the solver on the hypothetical graph (current graph + draft entries) and shows what would change: "Submitting this will change proposal P1 from 'established' to 'contested'."

## Feature Priority

### MVP

- Claim creation (items, proposals, facts)
- Relations (attacks, supports)
- Stances with discrete position vocabulary
- Resolution (explicit)
- Client-side reducer (log replay to materialized state)
- Grounded semantics solver
- Epistemic status computation
- Comment (freeform escape hatch)

### Essential for Specific Features

- Amendments
- Optimistic resolution (auto-resolve after deadline with no blocks)
- Stance supersession (changing your mind)
- Deadline management on proposals
- Pre-submission impact analysis

### Nice to Have

- Continuous intensity on stances
- `authored_via` provenance tracking
- Tags for attention routing
- Sources/evidence on claims
- Solver anomaly detection
- Preferred/stable extension semantics (beyond grounded)

### Future Extensions

- Vote weighting (stake, seniority, reputation)
- Delegation
- Budget-constrained prioritization (dot voting)
- Opinion clustering (Polis-style, for large groups)
- Conviction voting (time-weighted stances)

## Open Questions

1. **`claim_id` generation**: UUID from the client? Deterministic hash (like session IDs)? Needs to be unique across the session.

2. **Position vocabulary for relations**: Should stances on relations use the same vocabulary as stances on fact-type claims (`reject`/`accept`), or a separate vocabulary?

3. **BAF variant**: Which Bipolar Argumentation Framework semantics for support propagation? Necessary support (supported claim only accepted if supporter is accepted) vs deductive support (supporter's acceptance implies supported's acceptance)?

4. **Optimistic resolution threshold**: "No blocks" alone (pure Sociocratic consent)? Or "no blocks and at least N stances" (quorum)?

5. **Amendment scope**: Can anyone amend, or only the original author? What happens to existing stances when a claim is amended?

6. **Withdrawn claim cleanup**: What happens to stances and relations that reference a withdrawn claim? Options: the reducer ignores them (they become inert), the reducer explicitly marks them as orphaned, or the LLM prompts authors of affected entries to re-evaluate.

7. **Reference claim semantics**: Should `"reference"` claims be excluded from solver computation entirely (never IN/OUT, just inert context), or should they participate in the graph like any other claim and only be treated differently by the LLM's attention routing?

8. **Amend vs. withdraw-and-replace threshold**: How does the LLM (or participant) decide whether a revision is an `amend` or a withdrawal + new proposal? Is this a judgment call, or can we define criteria (e.g., if existing stances would be invalidated, it's a replacement)?
