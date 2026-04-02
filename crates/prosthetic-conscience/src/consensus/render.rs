//! Structured rendering of deliberation state.
//!
//! Produces serde-serializable types from the materialized state and solver
//! results. These types serve as the data layer for both text formatting
//! (LLM system prompts, terminal) and UI rendering (JSON over WASM↔JS).

use std::collections::{BTreeSet, HashMap};

use serde::Serialize;

use super::status::EpistemicStatus;
use super::types::{
    ClaimId, ClaimKind, ClaimState, MaterializedState, Position, RelationKind, Resolution,
};

// ---------------------------------------------------------------------------
// Structured types
// ---------------------------------------------------------------------------

/// Stance counts grouped by position for a single claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StanceSummary {
    pub position: Position,
    pub authors: Vec<String>,
}

/// Summary of a single claim for overview and detail views.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimSummary {
    pub id: ClaimId,
    pub body: String,
    pub author: String,
    pub kind: ClaimKind,
    pub status: Option<EpistemicStatus>,
    pub stances: Vec<StanceSummary>,
    pub parent_id: Option<ClaimId>,
    pub resolution: Option<Resolution>,
}

/// A signal that something in the deliberation needs participant attention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum AttentionSignal {
    Unexamined {
        claim_id: ClaimId,
        body: String,
        author: String,
    },
    Contested {
        claim_id: ClaimId,
        body: String,
        blockers: Vec<String>,
    },
    UnresolvedCycle {
        claim_ids: Vec<ClaimId>,
    },
}

/// High-level overview of the deliberation state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverviewData {
    pub total_claims: usize,
    pub total_relations: usize,
    pub total_stances: usize,
    pub participants: Vec<String>,
    pub items: Vec<ClaimSummary>,
    pub proposals: Vec<ClaimSummary>,
    pub resolved: Vec<ClaimSummary>,
    pub attention: Vec<AttentionSignal>,
}

/// Detailed view of a single claim with its relations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimDetail {
    pub claim: ClaimSummary,
    pub attacked_by: Vec<ClaimSummary>,
    pub supported_by: Vec<ClaimSummary>,
    pub attacks: Vec<ClaimSummary>,
    pub supports: Vec<ClaimSummary>,
}

// ---------------------------------------------------------------------------
// Builder functions
// ---------------------------------------------------------------------------

fn build_claim_summary(
    claim: &ClaimState,
    statuses: &HashMap<ClaimId, EpistemicStatus>,
    state: &MaterializedState,
) -> ClaimSummary {
    let status = statuses.get(&claim.id).copied();

    // Group stances by position
    let mut position_authors: HashMap<Position, Vec<String>> = HashMap::new();
    for stance in state.stances.values() {
        if stance.target_id == claim.id {
            position_authors
                .entry(stance.position)
                .or_default()
                .push(stance.author.clone());
        }
    }

    let mut stances: Vec<StanceSummary> = position_authors
        .into_iter()
        .map(|(position, mut authors)| {
            authors.sort();
            StanceSummary { position, authors }
        })
        .collect();
    stances.sort_by_key(|s| s.position as u8);

    ClaimSummary {
        id: claim.id.clone(),
        body: claim.body.clone(),
        author: claim.author.clone(),
        kind: claim.kind,
        status,
        stances,
        parent_id: claim.parent_id.clone(),
        resolution: claim.resolution.clone(),
    }
}

fn build_attention_signals(
    state: &MaterializedState,
    statuses: &HashMap<ClaimId, EpistemicStatus>,
) -> Vec<AttentionSignal> {
    let mut signals = Vec::new();

    for (claim_id, status) in statuses {
        let claim = match state.claims.get(claim_id) {
            Some(c) => c,
            None => continue,
        };

        match status {
            EpistemicStatus::Unexamined => {
                signals.push(AttentionSignal::Unexamined {
                    claim_id: claim_id.clone(),
                    body: claim.body.clone(),
                    author: claim.author.clone(),
                });
            }
            EpistemicStatus::Contested => {
                let blockers: Vec<String> = state
                    .stances
                    .values()
                    .filter(|s| s.target_id == *claim_id && s.position.is_negative())
                    .map(|s| s.author.clone())
                    .collect();
                signals.push(AttentionSignal::Contested {
                    claim_id: claim_id.clone(),
                    body: claim.body.clone(),
                    blockers,
                });
            }
            EpistemicStatus::Unresolved => {
                // Collect as individual signals; cycle detection would need
                // solver graph analysis which we defer. Each UNDEC claim is
                // surfaced independently.
                signals.push(AttentionSignal::UnresolvedCycle {
                    claim_ids: vec![claim_id.clone()],
                });
            }
            _ => {}
        }
    }

    // Sort for deterministic output
    signals.sort_by(|a, b| {
        let key = |s: &AttentionSignal| -> (u8, String) {
            match s {
                AttentionSignal::Contested { claim_id, .. } => (0, claim_id.0.clone()),
                AttentionSignal::Unexamined { claim_id, .. } => (1, claim_id.0.clone()),
                AttentionSignal::UnresolvedCycle { claim_ids } => (
                    2,
                    claim_ids.first().map_or(String::new(), |id| id.0.clone()),
                ),
            }
        };
        key(a).cmp(&key(b))
    });

    signals
}

/// Build an overview of the current deliberation state.
pub fn overview(
    state: &MaterializedState,
    statuses: &HashMap<ClaimId, EpistemicStatus>,
) -> OverviewData {
    let mut participants = BTreeSet::new();
    let mut items = Vec::new();
    let mut proposals = Vec::new();
    let mut resolved = Vec::new();

    for claim in state.claims.values() {
        participants.insert(claim.author.clone());
        let summary = build_claim_summary(claim, statuses, state);

        if claim.resolution.is_some() {
            resolved.push(summary);
            continue;
        }

        match claim.kind {
            ClaimKind::Item => items.push(summary),
            ClaimKind::Proposal => proposals.push(summary),
            _ => {} // facts, conditionals, etc. are not listed in overview categories
        }
    }

    // Collect participants from stances too
    for stance in state.stances.values() {
        participants.insert(stance.author.clone());
    }

    let attention = build_attention_signals(state, statuses);

    // Sort lists for deterministic output
    items.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    proposals.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    resolved.sort_by(|a, b| a.id.0.cmp(&b.id.0));

    OverviewData {
        total_claims: state.claims.len(),
        total_relations: state.relations.len(),
        total_stances: state.stances.len(),
        participants: participants.into_iter().collect(),
        items,
        proposals,
        resolved,
        attention,
    }
}

/// Build a detailed view of a single claim with its relations.
pub fn claim_detail(
    state: &MaterializedState,
    statuses: &HashMap<ClaimId, EpistemicStatus>,
    claim_id: &ClaimId,
) -> Option<ClaimDetail> {
    let claim = state.claims.get(claim_id)?;
    let summary = build_claim_summary(claim, statuses, state);

    let mut attacked_by = Vec::new();
    let mut supported_by = Vec::new();
    let mut attacks = Vec::new();
    let mut supports = Vec::new();

    for rel in &state.relations {
        if rel.target_id == *claim_id
            && let Some(source_claim) = state.claims.get(&rel.source_id)
        {
            let source_summary = build_claim_summary(source_claim, statuses, state);
            match rel.kind {
                RelationKind::Attacks => attacked_by.push(source_summary),
                RelationKind::Supports => supported_by.push(source_summary),
            }
        }
        if rel.source_id == *claim_id
            && let Some(target_claim) = state.claims.get(&rel.target_id)
        {
            let target_summary = build_claim_summary(target_claim, statuses, state);
            match rel.kind {
                RelationKind::Attacks => attacks.push(target_summary),
                RelationKind::Supports => supports.push(target_summary),
            }
        }
    }

    Some(ClaimDetail {
        claim: summary,
        attacked_by,
        supported_by,
        attacks,
        supports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::reducer::{replay, to_graph};
    use crate::consensus::solver::grounded_labelling;
    use crate::consensus::status::compute_all;
    use crate::consensus::types::*;

    fn empty_statuses() -> HashMap<ClaimId, EpistemicStatus> {
        HashMap::new()
    }

    /// Helper: build state and statuses from entries.
    fn build(entries: &[Entry]) -> (MaterializedState, HashMap<ClaimId, EpistemicStatus>) {
        let state = replay(entries);
        let (graph, index) = to_graph(&state);
        let labels = grounded_labelling(&graph);
        let statuses = compute_all(&state, &labels, &index);
        (state, statuses)
    }

    // -- overview() tests --

    #[test]
    fn overview_empty_state() {
        let state = MaterializedState::new();
        let data = overview(&state, &empty_statuses());
        assert_eq!(data.total_claims, 0);
        assert_eq!(data.total_relations, 0);
        assert_eq!(data.total_stances, 0);
        assert!(data.participants.is_empty());
        assert!(data.items.is_empty());
        assert!(data.proposals.is_empty());
        assert!(data.resolved.is_empty());
        assert!(data.attention.is_empty());
    }

    #[test]
    fn overview_categorizes_by_kind() {
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
                author: "bob".into(),
                body: "Use JWT".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: Some(ClaimId("item1".into())),
            },
            Entry::Claim {
                claim_id: ClaimId("f1".into()),
                author: "carol".into(),
                body: "JWT is stateless".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
        ]);

        let data = overview(&state, &statuses);
        assert_eq!(data.items.len(), 1);
        assert_eq!(data.items[0].id, ClaimId("item1".into()));
        assert_eq!(data.proposals.len(), 1);
        assert_eq!(data.proposals[0].id, ClaimId("p1".into()));
        // Facts don't appear in items or proposals
        assert_eq!(data.total_claims, 3);
    }

    #[test]
    fn overview_separates_resolved() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("p1".into()),
                author: "alice".into(),
                body: "Use JWT".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("p2".into()),
                author: "bob".into(),
                body: "Use cookies".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: None,
            },
            Entry::Resolve {
                claim_id: ClaimId("p1".into()),
                author: "alice".into(),
                outcome: Outcome::Accepted,
            },
        ]);

        let data = overview(&state, &statuses);
        assert_eq!(data.proposals.len(), 1);
        assert_eq!(data.proposals[0].id, ClaimId("p2".into()));
        assert_eq!(data.resolved.len(), 1);
        assert_eq!(data.resolved[0].id, ClaimId("p1".into()));
    }

    #[test]
    fn overview_groups_stances_by_position() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("p1".into()),
                author: "alice".into(),
                body: "Use JWT".into(),
                claim_kind: ClaimKind::Proposal,
                parent_id: None,
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "bob".into(),
                position: Position::Support,
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "carol".into(),
                position: Position::Support,
            },
            Entry::Stance {
                target_id: ClaimId("p1".into()),
                author: "dave".into(),
                position: Position::Block,
            },
        ]);

        let data = overview(&state, &statuses);
        let p1 = &data.proposals[0];
        // Should have two stance groups: Block and Support
        assert_eq!(p1.stances.len(), 2);
        let block_group = p1
            .stances
            .iter()
            .find(|s| s.position == Position::Block)
            .unwrap();
        assert_eq!(block_group.authors, vec!["dave"]);
        let support_group = p1
            .stances
            .iter()
            .find(|s| s.position == Position::Support)
            .unwrap();
        assert_eq!(support_group.authors, vec!["bob", "carol"]);
    }

    #[test]
    fn overview_attention_unexamined() {
        let (state, statuses) = build(&[Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Unexamined fact".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        }]);

        let data = overview(&state, &statuses);
        assert_eq!(data.attention.len(), 1);
        assert!(matches!(
            &data.attention[0],
            AttentionSignal::Unexamined { claim_id, .. } if *claim_id == ClaimId("c1".into())
        ));
    }

    #[test]
    fn overview_attention_contested() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "Contested claim".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Stance {
                target_id: ClaimId("c1".into()),
                author: "bob".into(),
                position: Position::Support,
            },
            Entry::Stance {
                target_id: ClaimId("c1".into()),
                author: "carol".into(),
                position: Position::Block,
            },
        ]);

        let data = overview(&state, &statuses);
        let contested = data
            .attention
            .iter()
            .find(|s| matches!(s, AttentionSignal::Contested { .. }));
        assert!(contested.is_some());
        if let Some(AttentionSignal::Contested { blockers, .. }) = contested {
            assert_eq!(blockers, &["carol"]);
        }
    }

    #[test]
    fn overview_collects_participants_from_stances() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "A claim".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Stance {
                target_id: ClaimId("c1".into()),
                author: "bob".into(),
                position: Position::Support,
            },
        ]);

        let data = overview(&state, &statuses);
        assert!(data.participants.contains(&"alice".to_string()));
        assert!(data.participants.contains(&"bob".to_string()));
    }

    // -- claim_detail() tests --

    #[test]
    fn claim_detail_unknown_returns_none() {
        let state = MaterializedState::new();
        assert!(claim_detail(&state, &empty_statuses(), &ClaimId("nope".into())).is_none());
    }

    #[test]
    fn claim_detail_includes_relations() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "Target claim".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("c2".into()),
                author: "bob".into(),
                body: "Attacks c1".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("c3".into()),
                author: "carol".into(),
                body: "Supports c1".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Relation {
                source_id: ClaimId("c2".into()),
                target_id: ClaimId("c1".into()),
                kind: RelationKind::Attacks,
                author: "bob".into(),
            },
            Entry::Relation {
                source_id: ClaimId("c3".into()),
                target_id: ClaimId("c1".into()),
                kind: RelationKind::Supports,
                author: "carol".into(),
            },
        ]);

        let detail = claim_detail(&state, &statuses, &ClaimId("c1".into())).unwrap();
        assert_eq!(detail.attacked_by.len(), 1);
        assert_eq!(detail.attacked_by[0].id, ClaimId("c2".into()));
        assert_eq!(detail.supported_by.len(), 1);
        assert_eq!(detail.supported_by[0].id, ClaimId("c3".into()));
    }

    #[test]
    fn claim_detail_includes_outgoing_relations() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "Attacker".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("c2".into()),
                author: "bob".into(),
                body: "Target".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Relation {
                source_id: ClaimId("c1".into()),
                target_id: ClaimId("c2".into()),
                kind: RelationKind::Attacks,
                author: "alice".into(),
            },
        ]);

        let detail = claim_detail(&state, &statuses, &ClaimId("c1".into())).unwrap();
        assert_eq!(detail.attacks.len(), 1);
        assert_eq!(detail.attacks[0].id, ClaimId("c2".into()));
    }

    #[test]
    fn claim_detail_includes_stances() {
        let (state, statuses) = build(&[
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "A claim".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Stance {
                target_id: ClaimId("c1".into()),
                author: "bob".into(),
                position: Position::Consent,
            },
            Entry::Stance {
                target_id: ClaimId("c1".into()),
                author: "carol".into(),
                position: Position::Object,
            },
        ]);

        let detail = claim_detail(&state, &statuses, &ClaimId("c1".into())).unwrap();
        assert_eq!(detail.claim.stances.len(), 2);
    }
}
