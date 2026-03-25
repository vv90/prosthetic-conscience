//! Text formatting of structured deliberation data.
//!
//! Renders `OverviewData` and `ClaimDetail` to human-readable text for
//! LLM system prompts and terminal display.

use std::fmt::Write;

use super::engine::{DraftEntry, ImpactAnalysis};
use super::render::{AttentionSignal, ClaimDetail, ClaimSummary, OverviewData};
use super::status::EpistemicStatus;
use super::types::{ClaimKind, Entry, Outcome, Position, RelationKind};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Format a full overview for an LLM system prompt or terminal display.
pub fn format_overview(data: &OverviewData) -> String {
    let mut out = String::new();

    // Header line
    let _ = writeln!(
        out,
        "Deliberation: {} claims, {} relations, {} stances from {} participants",
        data.total_claims,
        data.total_relations,
        data.total_stances,
        data.participants.len(),
    );

    // Summary counts
    let accepted = data
        .resolved
        .iter()
        .filter(|c| {
            matches!(
                c.resolution.as_ref().map(|r| r.outcome),
                Some(Outcome::Accepted)
            )
        })
        .count();
    let other_resolved = data.resolved.len() - accepted;

    let _ = write!(
        out,
        "Open items: {} | Active proposals: {} | Resolved: {}",
        data.items.len(),
        data.proposals.len(),
        data.resolved.len(),
    );
    if accepted > 0 || other_resolved > 0 {
        let mut parts = Vec::new();
        if accepted > 0 {
            parts.push(format!("{accepted} accepted"));
        }
        if other_resolved > 0 {
            parts.push(format!("{other_resolved} other"));
        }
        let _ = write!(out, " ({})", parts.join(", "));
    }
    out.push('\n');

    // Proposals section
    if !data.proposals.is_empty() {
        out.push('\n');
        let _ = writeln!(out, "Proposals:");
        for p in &data.proposals {
            let _ = writeln!(out, "  {}", format_claim_oneliner(p));
        }
    }

    // Items section
    if !data.items.is_empty() {
        out.push('\n');
        let _ = writeln!(out, "Open items:");
        for item in &data.items {
            let _ = writeln!(out, "  {}", format_claim_oneliner(item));
        }
    }

    // Resolved section
    if !data.resolved.is_empty() {
        out.push('\n');
        let _ = writeln!(out, "Resolved:");
        for r in &data.resolved {
            let outcome_str = r
                .resolution
                .as_ref()
                .map(|res| format_outcome(res.outcome))
                .unwrap_or("resolved");
            let _ = writeln!(
                out,
                "  [{}] \"{}\" by {} — {}",
                r.id.0, r.body, r.author, outcome_str
            );
        }
    }

    // Attention section
    if !data.attention.is_empty() {
        out.push('\n');
        let _ = writeln!(out, "Needs attention:");
        for signal in &data.attention {
            let _ = writeln!(out, "  {}", format_attention_signal(signal));
        }
    }

    out
}

/// Format a detailed view of a single claim.
pub fn format_claim_detail(detail: &ClaimDetail) -> String {
    let mut out = String::new();
    let c = &detail.claim;

    // Header
    let _ = writeln!(out, "[{}] \"{}\"", c.id.0, c.body);
    let _ = writeln!(
        out,
        "  Author: {} | Kind: {} | Status: {}",
        c.author,
        format_kind(c.kind),
        c.status.map(format_status).unwrap_or("unknown"),
    );

    // Resolution
    if let Some(ref res) = c.resolution {
        let _ = writeln!(
            out,
            "  Resolution: {} by {}",
            format_outcome(res.outcome),
            res.author
        );
    }

    // Stances
    if !c.stances.is_empty() {
        let _ = writeln!(out, "  Stances:");
        for s in &c.stances {
            let _ = writeln!(
                out,
                "    {}: {}",
                format_position(s.position),
                s.authors.join(", "),
            );
        }
    }

    // Incoming attacks
    if !detail.attacked_by.is_empty() {
        let _ = writeln!(out, "  Attacked by:");
        for a in &detail.attacked_by {
            let _ = writeln!(out, "    {}", format_claim_oneliner(a));
        }
    }

    // Incoming supports
    if !detail.supported_by.is_empty() {
        let _ = writeln!(out, "  Supported by:");
        for s in &detail.supported_by {
            let _ = writeln!(out, "    {}", format_claim_oneliner(s));
        }
    }

    // Outgoing attacks
    if !detail.attacks.is_empty() {
        let _ = writeln!(out, "  Attacks:");
        for a in &detail.attacks {
            let _ = writeln!(out, "    {}", format_claim_oneliner(a));
        }
    }

    // Outgoing supports
    if !detail.supports.is_empty() {
        let _ = writeln!(out, "  Supports:");
        for s in &detail.supports {
            let _ = writeln!(out, "    {}", format_claim_oneliner(s));
        }
    }

    out
}

/// Format the current draft buffer for terminal display.
pub fn format_drafts(drafts: &[DraftEntry]) -> String {
    if drafts.is_empty() {
        return String::from("Pending drafts: none");
    }

    let mut out = String::new();
    let _ = writeln!(out, "Pending drafts: {}", drafts.len());
    for draft in drafts {
        let _ = writeln!(out, "  {}", format_draft_entry(draft));
    }
    out
}

/// Format the current impact analysis for terminal display.
pub fn format_impact_analysis(impact: &ImpactAnalysis) -> String {
    let mut out = String::new();

    if impact.new_claims.is_empty() && impact.status_changes.is_empty() {
        return String::from("Impact: no structural changes");
    }

    let _ = writeln!(
        out,
        "Impact: {} new claims, {} status changes",
        impact.new_claims.len(),
        impact.status_changes.len(),
    );

    if !impact.new_claims.is_empty() {
        let _ = writeln!(out, "New claims:");
        for claim in &impact.new_claims {
            let status = claim.status.map(format_status).unwrap_or("unknown");
            let _ = writeln!(
                out,
                "  [{}] \"{}\" by {} — {} ({})",
                claim.claim_id.0,
                claim.body,
                claim.author,
                format_kind(claim.kind),
                status
            );
        }
    }

    if !impact.status_changes.is_empty() {
        let _ = writeln!(out, "Status changes:");
        for change in &impact.status_changes {
            let before = change.before.map(format_status).unwrap_or("none");
            let after = change.after.map(format_status).unwrap_or("resolved");
            let _ = writeln!(
                out,
                "  [{}] \"{}\" — {} -> {}",
                change.claim_id.0, change.body, before, after
            );
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn format_claim_oneliner(c: &ClaimSummary) -> String {
    let status_str = c.status.map(format_status).unwrap_or("unknown");
    let stance_str = format_stance_summary(&c.stances);

    if stance_str.is_empty() {
        format!(
            "[{}] \"{}\" by {} — {}",
            c.id.0, c.body, c.author, status_str
        )
    } else {
        format!(
            "[{}] \"{}\" by {} — {} ({})",
            c.id.0, c.body, c.author, status_str, stance_str
        )
    }
}

fn format_draft_entry(draft: &DraftEntry) -> String {
    match &draft.entry {
        Entry::Claim {
            claim_id,
            body,
            author,
            claim_kind,
            parent_id,
        } => {
            let parent = parent_id
                .as_ref()
                .map(|id| format!(" parent={}", id.0))
                .unwrap_or_default();
            format!(
                "#{} claim [{}] \"{}\" by {} ({}){}",
                draft.id.0,
                claim_id.0,
                body,
                author,
                format_kind(*claim_kind),
                parent
            )
        }
        Entry::Relation {
            source_id,
            target_id,
            kind,
            author,
        } => format!(
            "#{} relation {} {} {} by {}",
            draft.id.0,
            source_id.0,
            format_relation_kind(*kind),
            target_id.0,
            author
        ),
        Entry::Stance {
            target_id,
            author,
            position,
        } => format!(
            "#{} stance {} on {} by {}",
            draft.id.0,
            format_position(*position),
            target_id.0,
            author
        ),
        Entry::Resolve {
            claim_id,
            author,
            outcome,
        } => format!(
            "#{} resolve {} as {} by {}",
            draft.id.0,
            claim_id.0,
            format_outcome(*outcome),
            author
        ),
        Entry::Comment {
            claim_id,
            author,
            body,
        } => {
            let target = claim_id
                .as_ref()
                .map(|id| format!(" on {}", id.0))
                .unwrap_or_default();
            format!(
                "#{} comment{} by {} — \"{}\"",
                draft.id.0, target, author, body
            )
        }
    }
}

fn format_stance_summary(stances: &[super::render::StanceSummary]) -> String {
    let parts: Vec<String> = stances
        .iter()
        .map(|s| {
            let pos = format_position(s.position);
            format!("{}: {}", pos, s.authors.join(", "))
        })
        .collect();
    parts.join(" | ")
}

fn format_status(status: EpistemicStatus) -> &'static str {
    match status {
        EpistemicStatus::Established => "Established",
        EpistemicStatus::Unexamined => "Unexamined",
        EpistemicStatus::Contested => "Contested",
        EpistemicStatus::Defeated => "Defeated",
        EpistemicStatus::Unresolved => "Unresolved",
    }
}

fn format_position(pos: Position) -> &'static str {
    match pos {
        Position::Block => "block",
        Position::Object => "object",
        Position::StandAside => "stand-aside",
        Position::Abstain => "abstain",
        Position::Consent => "consent",
        Position::Support => "support",
        Position::Champion => "champion",
    }
}

fn format_kind(kind: super::types::ClaimKind) -> &'static str {
    match kind {
        ClaimKind::Item => "item",
        ClaimKind::Proposal => "proposal",
        ClaimKind::Fact => "fact",
        ClaimKind::Conditional => "conditional",
        ClaimKind::Value => "value",
        ClaimKind::Reference => "reference",
    }
}

fn format_relation_kind(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Attacks => "attacks",
        RelationKind::Supports => "supports",
    }
}

fn format_outcome(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Accepted => "Accepted",
        Outcome::Rejected => "Rejected",
        Outcome::Tabled => "Tabled",
        Outcome::Withdrawn => "Withdrawn",
    }
}

fn format_attention_signal(signal: &AttentionSignal) -> String {
    match signal {
        AttentionSignal::Contested {
            claim_id,
            body,
            blockers,
        } => {
            format!(
                "[{}] \"{}\" — Contested (blockers: {})",
                claim_id.0,
                body,
                blockers.join(", "),
            )
        }
        AttentionSignal::Unexamined {
            claim_id,
            body,
            author,
        } => {
            format!("[{}] \"{}\" by {} — Unexamined", claim_id.0, body, author)
        }
        AttentionSignal::UnresolvedCycle { claim_ids } => {
            let ids: Vec<&str> = claim_ids.iter().map(|id| id.0.as_str()).collect();
            format!("{} — Unresolved cycle", ids.join(" <-> "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::reducer::{replay, to_graph};
    use crate::consensus::render;
    use crate::consensus::solver::grounded_labelling;
    use crate::consensus::status::compute_all;
    use crate::consensus::types::*;

    fn build(
        entries: &[Entry],
    ) -> (
        crate::consensus::types::MaterializedState,
        std::collections::HashMap<ClaimId, EpistemicStatus>,
    ) {
        let state = replay(entries);
        let (graph, index) = to_graph(&state);
        let labels = grounded_labelling(&graph);
        let statuses = compute_all(&state, &labels, &index);
        (state, statuses)
    }

    #[test]
    fn format_overview_empty() {
        let state = MaterializedState::new();
        let data = render::overview(&state, &std::collections::HashMap::new());
        let text = format_overview(&data);
        assert!(text.contains("0 claims"));
        assert!(text.contains("0 participants"));
        assert!(!text.is_empty());
    }

    #[test]
    fn format_overview_includes_proposals() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("p1".into()),
                author: "alice".into(),
                body: "Use JWT for auth".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: None,
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "bob".into(),
                position: Position::Consent,
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "carol".into(),
                position: Position::Block,
            },
        ]);

        let data = render::overview(&state, &statuses);
        let text = format_overview(&data);
        assert!(text.contains("Proposals:"));
        assert!(text.contains("[p1]"));
        assert!(text.contains("Use JWT for auth"));
        assert!(text.contains("alice"));
        assert!(text.contains("Contested"));
    }

    #[test]
    fn format_overview_includes_attention() {
        let (state, statuses) = build(&[Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Unexamined claim".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        }]);

        let data = render::overview(&state, &statuses);
        let text = format_overview(&data);
        assert!(text.contains("Needs attention:"));
        assert!(text.contains("Unexamined"));
    }

    #[test]
    fn format_claim_detail_complete() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "Main claim".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("c2".into()),
                author: "bob".into(),
                body: "Counter-argument".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Relation {
                source_id: ClaimId("c2".into()),
                target_id: ClaimId("c1".into()),
                kind: RelationKind::Attacks,
                author: "bob".into(),
            },
            Entry::Stance {
                target_id: ClaimId("c1".into()),
                author: "carol".into(),
                position: Position::Support,
            },
        ]);

        let detail = render::claim_detail(&state, &statuses, &ClaimId("c1".into())).unwrap();
        let text = format_claim_detail(&detail);
        assert!(text.contains("[c1]"));
        assert!(text.contains("Main claim"));
        assert!(text.contains("alice"));
        assert!(text.contains("proposal"));
        assert!(text.contains("Stances:"));
        assert!(text.contains("support: carol"));
        assert!(text.contains("Attacked by:"));
        assert!(text.contains("[c2]"));
    }

    #[test]
    fn format_overview_snapshot() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("item1".into()),
                author: "alice".into(),
                body: "Auth approach?".into(),
                claim_kind: ClaimKind::Item,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("p1".into()),
                author: "alice".into(),
                body: "Use JWT for auth".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: Some(ClaimId("item1".into())),
            },
            Entry::Claim {
                claim_id: ClaimId("p2".into()),
                author: "bob".into(),
                body: "Use session cookies".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: Some(ClaimId("item1".into())),
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "bob".into(),
                position: Position::Consent,
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "carol".into(),
                position: Position::Consent,
            },
            Entry::Stance {
                target_id: ClaimId("p2".into()),
                author: "bob".into(),
                position: Position::Support,
            },
            Entry::Stance {
                target_id: ClaimId("p2".into()),
                author: "carol".into(),
                position: Position::Block,
            },
        ]);

        let data = render::overview(&state, &statuses);
        let text = format_overview(&data);

        // Verify structural elements
        assert!(text.contains("3 participants"));
        assert!(text.contains("Open items: 1"));
        assert!(text.contains("Active proposals: 2"));
        assert!(text.contains("Resolved: 0"));
        assert!(text.contains("Proposals:"));
        assert!(text.contains("[p1]"));
        assert!(text.contains("[p2]"));
        assert!(text.contains("Contested"));
        assert!(text.contains("Established"));
    }
}
