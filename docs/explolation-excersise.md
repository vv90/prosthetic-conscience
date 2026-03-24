Mapping

### Turn 1 (You): "Let's think through how the actual protocol is going to work"

This is an **item** (claim_kind: "item"): "How should the consensus protocol work?" It's the root topic for the entire conversation. No proposal yet, just a problem statement.

**Fits:** Yes. `claim` with `claim_kind: "item"`.

### Turn 2 (Me): Three architectural options (A, B, C)

Three **proposals** under that item:

- A: Structured entries, dumb gateway
- B: Validated entries, thin gateway layer
- C: Gateway-mediated consensus with dedicated events

Each with pro/con **arguments** (which in our model would be claims + relations).

**Fits:** Yes. Three `claim` entries with `claim_kind: "proposal"`, `parent_id` pointing to the root item. Arguments are `claim` + `relation` (supports/attacks each proposal).

### Turn 3 (You): "Option A sounds good, but let's explore existing systems"

A **stance** on proposal A: `consent` or `support`. Not a `champion` — you're endorsing the direction, not committing to drive implementation.

Also implicitly `stand_aside` on B and C — not blocking, just moving past them.

And a new **item**: "What can we learn from existing consensus systems?"

**Fits:** Yes. `stance` on proposal A + new `claim` with `claim_kind: "item"`.

### Turn 4 (Me): Research synthesis across five domains

This is a large volume of **claims** (facts about how various systems work) and **relations** (how those facts are relevant to our design).

For example: "Sociocracy uses consent (no objections) rather than consensus (everyone agrees)" is a `claim` with `claim_kind: "fact"`. Its relevance to our protocol is a `relation` (supports the design direction of consent-based resolution).

**Fits:** Yes, but this is where the volume gets interesting. This single turn would produce dozens of claims and relations. In our system, the LLM would be extracting these and presenting them as drafts. Would a participant actually review 40 draft entries? Probably not — they'd review a summary and trust the LLM on the details.

**Potential issue:** The framework assumes every entry is explicitly reviewed. But research/information-sharing turns produce a lot of factual claims that aren't really "submissions to the deliberation" — they're context. More on this below.

### Turn 5 (You): Three important corrections

1. "It's not necessarily just for small groups" — a `claim` (fact) that **attacks** my implicit assumption about group size. This changes which patterns from the research are relevant.

2. "It could have vote weight mechanisms in the future" — a `claim` (fact/value) about future requirements. Relates to several proposals about stance semantics.

3. "LLMs will mediate; participants won't interact with the log directly" — a `claim` (fact) that fundamentally reshapes the architecture. This one **attacks** several earlier proposals about entry schema (which assumed human-readable entries) and **supports** richer machine-oriented structure.

**Fits:** Yes. These are claims with relations to prior claims. The third one is particularly interesting — it's a claim that, once accepted, changes the defensibility of many other claims downstream. The solver would detect this: "accepting this claim about LLM mediation defeats 5 claims about human-readable schemas and supports 3 claims about machine-oriented structure."

### Turn 6 (Me): Revised schema with LLM mediation, synthesis entries, etc.

New **proposals** (revised entry types) plus **arguments** for each. Also several **claims** about how LLMs change the design space.

**Fits:** Yes. Proposals + supporting claims + relations.

### Turn 7 (You): Items vs. proposals, continuous scale, should synthesis/facilitation be in the log?

Three distinct **items** (questions), each with implicit **claims** that challenge my proposals:

1. "Should we separate items from proposals?" — a new item. Your framing implies a `claim` that the current conflation is a problem.

2. "What if we use a continuous scale?" — a `claim` that challenges (attacks) the discrete scale proposal. With an argument: LLMs produce continuous signal, not discrete categories.

3. "Should synthesis be in the log? Is facilitation just a tool for converging chaotic conversation?" — Two items. The second one is particularly interesting: it's a **conditional** claim: "If the protocol structure itself creates convergence, then facilitation entries are unnecessary."

**Fits:** Yes. Claims, relations, items. The conditional in point 3 maps to a `claim` with `claim_kind: "conditional"` — "if X then Y." The relation would be: this conditional `attacks` the proposal to include facilitation entries.

### Turn 8 (Me): Revised entry types, convergence discussion

Responses to each of your three items. New proposals, arguments. Notably: I **withdraw** several earlier proposals (synthesis entries, facilitate entries) based on your arguments.

**Fits:** Yes. `resolve` entries with `outcome: "withdrawn"` on the synthesis and facilitate proposals.

### Turn 9 (You): "What about facts/statements as independent entities?"

A new **item**: "How should facts fit into the model?" Plus a **claim** that facts are independent entities with relationships to multiple items. This claim **attacks** the current model where arguments are subordinate to specific items.

**Fits:** Yes. The claim challenges (attacks) a structural assumption in the existing proposal.

### Turn 10 (Me): Epistemic graph, statements as independent, three layers

Major revision. New **proposal**: "Everything is a claim; relations are edges in a graph." This **attacks** the earlier flat model and is **supported by** several claims about formal argumentation theory.

**Fits:** Yes. Though this is a case where a proposal fundamentally restructures multiple earlier proposals. The solver would show: "accepting this proposal defeats the earlier entry type list and requires revising the schema."

### Turn 11 (You): "Aren't statements, relevance, conflicts, conditionals all the same thing?"

A **claim** that simplifies the model: all of these are just claims about what is true. This **attacks** my proposal to have separate `statement` and `relevance` entry types.

Plus a second **claim**: "LLMs are not good at complex deterministic logical evaluation." This **supports** introducing a formal solver and **attacks** any design that relies on LLMs for logical reasoning.

**Fits:** Yes. Two claims, each with clear attack/support relations to existing proposals.

### Turn 12 (Me): Formal argumentation research, solver introduction, simplified types

Response incorporates both of your claims. The entry type list shrinks from 8+ types to 6. The solver is introduced.

**Fits:** Yes. New proposals (solver architecture, simplified types) supported by claims from argumentation theory research.

### Turn 13 (You): "What if Carol's claim isn't valid?"

A **claim** that identifies a flaw in my reasoning: the solver computes defensibility, not truth. Unattacked is not the same as verified. This **attacks** my example's implicit assumption.

**Fits:** Yes. A claim attacking a specific logical step. This is exactly the kind of thing the framework is designed for — pinpointing where a disagreement or error lives.

### Turn 14 (Me): Three-state epistemic status, Carneades proof standards

Revised proposal incorporating your correction. Introduces the established/unexamined/contested/defeated/unresolved status model.

**Fits:** Yes. New proposal supported by claims about Carneades and Sociocratic practice.

### Turn 15 (You): "Let's check the responsibility distribution"

A new **item**: "Are responsibilities assigned to the right components?" This is a meta-level question about the design itself.

**Fits:** Yes. Item with subsequent proposals and claims about what each component is good/bad at.

### Turn 16 (Me): Capability matrix, relation extraction as the weak point

Analysis identifying the concerning assignment (LLM as sole graph constructor). Multiple **claims** about failure modes. Revised proposal for responsibility assignment.

**Fits:** Yes. Claims about failure modes attack the current assignment. New proposal for conversational confirmation.

### Turn 17 (You): "What if the LLM is a drafting tool with explicit submit?"

A **proposal** that addresses the relation extraction concern. This **attacks** my conversational-confirmation proposal (which still has the LLM as probabilistic intermediary) and proposes a cleaner alternative where humans explicitly approve all submissions.

**Fits:** Yes. A proposal that supersedes mine.

### Turn 18 (Me): Analysis of the draft-and-submit model

Supporting **claims** and identified **risks** (rubber-stamping, cognitive load, anchoring). Each risk is a claim that partially attacks the proposal, with mitigations that defend against those attacks.

**Fits:** Yes. Claims and relations.

### Turn 19 (You): "Make sure we got responsibilities right"

Follow-up on turn 15. Seeking confirmation.

### Turn 20 (Me): Final responsibility matrix

**Fits:** Yes. Claims and a proposal (the responsibility table).

### Turn 21 (You): "Create a design document"

An action request, not a deliberation contribution. This doesn't map to any entry type.

**Potential issue:** Process/meta actions ("let's write this down," "let's move on to the next topic") aren't modeled. See below.

---

## What Doesn't Fit

### 1. Information sharing vs. deliberation contributions

Much of this conversation was one party presenting research or analysis for the other to consider. Turns 4, 12, and 16 each contain dozens of factual claims — but they're not really "submissions to be debated." They're context, background, shared understanding.

Our framework models every entry as a claim that can be attacked, supported, and stanced on. But when I present "Sociocracy uses consent rather than consensus," nobody needs to take a stance on that. It's just information.

**The gap:** We don't have a concept of "shared context" or "reference material" that's distinct from "claim under deliberation." Everything in our model is a claim, but not everything _should_ be debatable. Some things are just facts being introduced for context.

**Possible resolution:** This might not be a real problem. The claim exists in the log. If nobody takes a stance on it or references it, it's inert. The solver treats it as unattacked (IN) by default. The LLM doesn't surface it for deliberation because nobody's engaging with it. It's effectively background. The framework handles it by neglect, which might be fine.

Or: `claim_kind` could include `"reference"` — a claim that's introduced as context, not as an assertion to be evaluated. The solver could exclude references from extension computation.

### 2. Exploratory conversation and brainstorming

Turns 2, 6, 8, and 10 are me presenting multiple options and analyzing tradeoffs. This is generative exploration, not structured claims. "Here are three approaches, here are the pros and cons of each" is a brainstorming pattern that precedes structured deliberation.

**The gap:** Our framework models the _products_ of deliberation (claims, stances, relations) but not the _process_ of exploration that generates them. In practice, much of the value in this conversation came from the exploratory phase — considering and discarding options — before crystallizing into structured decisions.

**Possible resolution:** This is probably fine. The exploratory conversation happens between a participant and their LLM agent. It's local, not shared. Only the crystallized outputs (claims, proposals, stances) go to the log. Our design already accounts for this: the LLM drafts, the human submits only what they want.

### 3. Progressive refinement of proposals

The entry type schema was proposed in turn 6, revised in turn 8, fundamentally restructured in turn 10, simplified in turn 12, and finalized in turn 18. Each version superseded the previous one. Our `amend` entry handles small revisions, but this was repeated wholesale replacement.

**The gap:** `amend` changes the body of a claim. But "I'm withdrawing this entire proposal and replacing it with a fundamentally different one" is a different operation. It's withdraw + new proposal. The old proposal's relations and stances become irrelevant.

**Possible resolution:** Withdraw + new proposal is the correct mechanism. The old stances die with the old proposal. If someone supported the old version and the new version is different, they need to re-evaluate. This actually works — it just means the log accumulates withdrawn proposals. The reducer filters them out of the active view. No framework change needed.

### 4. Meta-conversation and process steering

"Let's explore existing systems," "let's check if we got responsibilities right," "create a document" — these are process directives that don't map to claims, relations, or stances. They're about _how_ to conduct the deliberation, not contributions _to_ the deliberation.

**The gap:** We explicitly decided that facilitation isn't in the log. But these steering moves are coming from a participant, not a facilitator. They're shaping the agenda in real time.

**Possible resolution:** "Let's explore existing systems" is actually a new **item** (claim_kind: "item"): "What can we learn from existing consensus systems?" The participant is proposing a topic for discussion. "Create a document" is an action outside the deliberation entirely. The gap is narrow — most process steering is just item creation. The truly meta stuff ("let's take a different approach to this discussion") is handled by the participant's local conversation with their LLM, not the shared log.

### 5. Two-party vs. multi-party dynamics

This entire conversation was between two people. The framework is designed for multi-party deliberation. In a two-person conversation, many features are irrelevant:

- No attention routing needed (both people see everything)
- No stance aggregation (only two opinions)
- No solver bottleneck detection (disputes are immediately visible)
- No unexamined claims (the other person responds to everything)

**Not a gap** — the framework is designed for the harder problem. It just means this conversation is an easy case that doesn't stress-test certain features.

### 6. Asymmetric roles

In this conversation, I was doing most of the research and proposal generation, while you were steering, critiquing, and making key design decisions. This is a natural pattern — not everyone contributes equally or in the same way. Our framework doesn't model roles.

**Not clearly a gap.** The framework tracks what each person contributed (claims, stances). The asymmetry is visible in the log. Whether it matters depends on the weighting model (future concern).

---

## Summary

**Fits well:**

- Claims (items, proposals, facts, conditionals, values) — covered everything substantive
- Relations (attacks, supports) — every disagreement and build-upon mapped cleanly
- Stances — every "I agree," "that's not right," "let's go with this" mapped to a position
- Resolve/withdraw — proposals being dropped and replaced worked
- The item/proposal distinction — problems vs. solutions was a real pattern throughout

**Marginal fits (work but feel awkward):**

- Large information-sharing turns (dozens of claims that aren't really under deliberation)
- Progressive wholesale replacement of proposals (withdraw + new proposal works but is noisy)

**Genuine gaps:**

- Reference material / shared context that isn't meant to be debated — might need a `claim_kind: "reference"` or might just be fine as inert claims
- Process steering from participants — mostly maps to new items, but "create a document" is an external action with no representation

The framework held up reasonably well. The biggest insight is that the **exploratory/generative phase** of deliberation (brainstorming, research, analysis) happens locally between a participant and their LLM, and only the crystallized outputs enter the shared log. That's already in our design — it just became very concrete seeing how much of this conversation was exploration rather than structured claims.
