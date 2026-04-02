# Consensus App — UI Functional Spec

## What This App Is

A group decision-making tool. Multiple participants join a shared session to discuss a topic, propose solutions, raise concerns, and reach agreement. Each participant has a personal AI assistant that helps them follow the discussion and contribute to it.

The assistant understands the discussion state, helps the participant form their contributions, and prepares drafts on their behalf. The participant reviews what the assistant prepares and decides when to share it with the group. Nothing goes to the group without the participant's explicit approval.

The goal is consent-based decision-making: a proposal is accepted when no one has remaining objections, not when everyone enthusiastically agrees.

---

## Primary Interaction: Voice

The main way participants interact with their assistant is **voice** (push-to-talk). The assistant's responses are **displayed as text**, not narrated back. Text input is available as a fallback.

---

## Functional Areas

### 1. Conversation

The central surface. The participant talks (or types) to their assistant. The assistant responds in writing.

**What appears here:**

- **Participant input**: transcribed voice or typed text.
- **Assistant responses**: natural-language replies. When the assistant prepares a draft, the response includes a brief confirmation of what was prepared (e.g. "I've noted that you support the hybrid approach").
- **Group activity notices**: brief notifications when other participants contribute something new (e.g. "Carol just proposed an alternative approach"). Visually distinct from the conversation itself.

### 2. Drafts (always visible)

A persistent view of everything the assistant has prepared on the participant's behalf but that **has not yet been shared with the group**. This is the participant's staging area — a chance to review before committing.

**Each draft shows:**

- What kind of contribution it is (a proposal, an opinion, a supporting or opposing point, a comment, etc.)
- A short readable summary of what it says or what it refers to
- A way to remove it

**Actions on the draft area:**

- **Share with group**: sends all drafts to the shared session. Before sending, the app shows a preview of **what will change** — which proposals would be affected and how. The participant must confirm.
- **Discard all**: clears all drafts. Requires confirmation.

When there are no drafts, this is clearly indicated.

### 3. What Needs Your Attention (always visible)

A focused, computed view of **what is currently relevant to this participant**. Not a history or a full overview — just the items that warrant action or awareness right now.

**Types of attention items:**

- **Waiting for your input**: proposals or statements from others that this participant hasn't responded to yet.
- **Blocked or disputed**: proposals where someone has raised an objection, showing who objected and a summary of why.
- **Needs resolution**: points where conflicting arguments need a human judgment call.
- **Recent activity**: notable new contributions from other participants.

When nothing needs attention, this is clearly indicated — the discussion is in a settled state for this participant.

**Expanding an item** shows more detail:

- Who said it and what kind of contribution it is
- Current status (see Status below)
- What other participants think (their positions, grouped)
- What points support or oppose it
- Outcome, if it has been resolved

### 4. Session Info

Minimal, persistent indicators:

- Session identifier
- Participant's own name
- Number of participants
- Connection state (connected / reconnecting / disconnected)

---

## Status

Every proposal and statement in the discussion has a computed status. Five possible values:

| Status       | Meaning                                    |
| ------------ | ------------------------------------------ |
| Accepted     | The group agrees — no objections remain    |
| Needs review | No one has weighed in yet                  |
| Disputed     | Someone has raised an objection or concern |
| Overruled    | A stronger counterpoint has been accepted  |
| Conflicted   | Opposing arguments are deadlocked          |

"Overruled" items should be visually de-emphasized. "Disputed" and "Needs review" items should draw attention.

---

## Positions

When participants express their view on a proposal, the assistant maps it to one of these positions. The participant never selects from a list — positions are always expressed through conversation and captured by the assistant.

**On proposals:**

| Position   | Meaning                                    |
| ---------- | ------------------------------------------ |
| Block      | "I can't accept this — here's why"         |
| Object     | "I have serious concerns"                  |
| Step aside | "I disagree, but I won't stand in the way" |
| Abstain    | "No opinion"                               |
| Accept     | "Good enough to move forward"              |
| Support    | "I'm in favor"                             |
| Champion   | "I'll drive this forward"                  |

**On factual statements:**

| Position        | Meaning                         |
| --------------- | ------------------------------- |
| Reject          | "This is wrong"                 |
| Doubt           | "I'm not sure this is right"    |
| Unsure          | "I don't know"                  |
| Accept          | "This seems right"              |
| Strongly accept | "I'm confident this is correct" |

---

## Real-Time Behavior

- The attention view and draft area update live as the discussion progresses. No manual refresh.
- When connection is lost, sharing drafts is disabled. The app indicates reconnection state. On reconnect, state catches up automatically.
- Contributions from other participants appear as group activity notices in the conversation and may update the attention view.

---

## Interaction Boundaries

- All contributions to the group discussion are created through conversation with the assistant, staged as drafts, and shared explicitly. There are no forms or structured input controls for authoring contributions.
- Removing a draft is the one direct action on structured content.
- Share and discard are explicit participant actions, never initiated by the assistant.
- The assistant never shares anything with the group without the participant's confirmation.

---

## What Is Excluded

- No diagrams, flowcharts, or graph visualizations
- No full discussion history browser
- No direct authoring via forms
- No session switching or session list
- No admin, moderation, or settings
- No file upload or media
- No audio playback of assistant responses
