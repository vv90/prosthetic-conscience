//! Epistemic status computation: combines solver labels with stance data.
//!
//! The solver tells us what's logically defensible; stances tell us what
//! participants actually think. The gap between the two is the epistemic
//! status — it guides the LLM's attention routing and conversation strategy.

use std::collections::HashMap;

use super::solver::Label;
use super::types::{ClaimId, MaterializedState, StanceState};

/// The epistemic status of a claim, combining solver output with stance coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EpistemicStatus {
    /// IN + accepted by participants (no negative stances).
    Established,
    /// IN + no stances yet — nobody has checked.
    Unexamined,
    /// IN + disputed stances (at least one negative).
    Contested,
    /// OUT — attacked by a defensible claim.
    Defeated,
    /// UNDEC — involved in cycles or mutual attacks.
    Unresolved,
}

/// Compute the epistemic status of a single claim from its solver label
/// and the stances on it.
pub fn epistemic_status(label: Label, stances: &[&StanceState]) -> EpistemicStatus {
    match label {
        Label::Out => EpistemicStatus::Defeated,
        Label::Undec => EpistemicStatus::Unresolved,
        Label::In => {
            if stances.is_empty() {
                EpistemicStatus::Unexamined
            } else if stances.iter().any(|s| s.position.is_negative()) {
                EpistemicStatus::Contested
            } else {
                EpistemicStatus::Established
            }
        }
    }
}

/// Compute epistemic status for all active (non-resolved) claims.
///
/// Takes the materialized state from the reducer, solver labels, and the
/// index-to-claim mapping from `to_graph`. Returns a status per claim.
pub fn compute_all(
    state: &MaterializedState,
    labels: &[Label],
    index_to_claim: &[ClaimId],
) -> HashMap<ClaimId, EpistemicStatus> {
    let mut result = HashMap::new();

    for (i, claim_id) in index_to_claim.iter().enumerate() {
        let stances: Vec<&StanceState> = state
            .stances
            .values()
            .filter(|s| s.target_id == *claim_id)
            .collect();

        let status = epistemic_status(labels[i], &stances);
        result.insert(claim_id.clone(), status);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::reducer::{replay, to_graph};
    use crate::consensus::solver::grounded_labelling;
    use crate::consensus::types::*;

    fn make_stance(target: &str, author: &str, position: Position) -> StanceState {
        StanceState {
            target_id: ClaimId(target.into()),
            author: author.into(),
            position,
        }
    }

    // -- Unit tests: epistemic_status() --

    #[test]
    fn in_no_stances_is_unexamined() {
        assert_eq!(
            epistemic_status(Label::In, &[]),
            EpistemicStatus::Unexamined
        );
    }

    #[test]
    fn in_all_positive_is_established() {
        let s1 = make_stance("c1", "alice", Position::Support);
        let s2 = make_stance("c1", "bob", Position::Champion);
        assert_eq!(
            epistemic_status(Label::In, &[&s1, &s2]),
            EpistemicStatus::Established
        );
    }

    #[test]
    fn in_all_neutral_is_established() {
        let s1 = make_stance("c1", "alice", Position::Abstain);
        let s2 = make_stance("c1", "bob", Position::StandAside);
        assert_eq!(
            epistemic_status(Label::In, &[&s1, &s2]),
            EpistemicStatus::Established
        );
    }

    #[test]
    fn in_mixed_positive_and_neutral_is_established() {
        let s1 = make_stance("c1", "alice", Position::Support);
        let s2 = make_stance("c1", "bob", Position::Abstain);
        let s3 = make_stance("c1", "carol", Position::Consent);
        assert_eq!(
            epistemic_status(Label::In, &[&s1, &s2, &s3]),
            EpistemicStatus::Established
        );
    }

    #[test]
    fn in_with_block_is_contested() {
        let s1 = make_stance("c1", "alice", Position::Support);
        let s2 = make_stance("c1", "bob", Position::Block);
        assert_eq!(
            epistemic_status(Label::In, &[&s1, &s2]),
            EpistemicStatus::Contested
        );
    }

    #[test]
    fn in_with_object_is_contested() {
        let s1 = make_stance("c1", "alice", Position::Object);
        assert_eq!(
            epistemic_status(Label::In, &[&s1]),
            EpistemicStatus::Contested
        );
    }

    #[test]
    fn in_positive_and_negative_is_contested() {
        let s1 = make_stance("c1", "alice", Position::Champion);
        let s2 = make_stance("c1", "bob", Position::Consent);
        let s3 = make_stance("c1", "carol", Position::Object);
        assert_eq!(
            epistemic_status(Label::In, &[&s1, &s2, &s3]),
            EpistemicStatus::Contested
        );
    }

    #[test]
    fn out_with_stances_is_defeated() {
        let s1 = make_stance("c1", "alice", Position::Support);
        assert_eq!(
            epistemic_status(Label::Out, &[&s1]),
            EpistemicStatus::Defeated
        );
    }

    #[test]
    fn out_no_stances_is_defeated() {
        assert_eq!(epistemic_status(Label::Out, &[]), EpistemicStatus::Defeated);
    }

    #[test]
    fn undec_with_stances_is_unresolved() {
        let s1 = make_stance("c1", "alice", Position::Support);
        assert_eq!(
            epistemic_status(Label::Undec, &[&s1]),
            EpistemicStatus::Unresolved
        );
    }

    #[test]
    fn undec_no_stances_is_unresolved() {
        assert_eq!(
            epistemic_status(Label::Undec, &[]),
            EpistemicStatus::Unresolved
        );
    }

    // -- Full pipeline integration test --

    #[test]
    fn full_pipeline_all_five_statuses() {
        // Set up a scenario with all five epistemic statuses:
        //
        // "established_fact" — IN, has positive stances → Established
        // "unexamined_fact"  — IN, no stances → Unexamined
        // "contested_fact"   — IN, has a Block → Contested
        // "defeated_fact"    — OUT (attacked by established_fact) → Defeated
        // "cycle_a" / "cycle_b" — mutual attack, both UNDEC → Unresolved

        let entries = vec![
            Entry::Claim {
                claim_id: ClaimId("established".into()),
                author: "alice".into(),
                body: "Well-accepted fact".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("unexamined".into()),
                author: "bob".into(),
                body: "Nobody checked this".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("contested".into()),
                author: "carol".into(),
                body: "People disagree".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("defeated".into()),
                author: "dave".into(),
                body: "This will be attacked".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("cycle_a".into()),
                author: "alice".into(),
                body: "Cycle node A".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("cycle_b".into()),
                author: "bob".into(),
                body: "Cycle node B".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            // "established" attacks "defeated" → defeated becomes OUT
            Entry::Relation {
                source_id: ClaimId("established".into()),
                target_id: ClaimId("defeated".into()),
                kind: RelationKind::Attacks,
                author: "alice".into(),
            },
            // cycle_a ↔ cycle_b → both UNDEC
            Entry::Relation {
                source_id: ClaimId("cycle_a".into()),
                target_id: ClaimId("cycle_b".into()),
                kind: RelationKind::Attacks,
                author: "alice".into(),
            },
            Entry::Relation {
                source_id: ClaimId("cycle_b".into()),
                target_id: ClaimId("cycle_a".into()),
                kind: RelationKind::Attacks,
                author: "bob".into(),
            },
            // Stances: established has positive, contested has negative
            Entry::Stance {
                target_id: ClaimId("established".into()),
                author: "bob".into(),
                position: Position::Support,
            },
            Entry::Stance {
                target_id: ClaimId("contested".into()),
                author: "alice".into(),
                position: Position::Support,
            },
            Entry::Stance {
                target_id: ClaimId("contested".into()),
                author: "dave".into(),
                position: Position::Block,
            },
        ];

        let state = replay(&entries);
        let (graph, index) = to_graph(&state);
        let labels = grounded_labelling(&graph);
        let statuses = compute_all(&state, &labels, &index);

        assert_eq!(
            statuses[&ClaimId("established".into())],
            EpistemicStatus::Established
        );
        assert_eq!(
            statuses[&ClaimId("unexamined".into())],
            EpistemicStatus::Unexamined
        );
        assert_eq!(
            statuses[&ClaimId("contested".into())],
            EpistemicStatus::Contested
        );
        assert_eq!(
            statuses[&ClaimId("defeated".into())],
            EpistemicStatus::Defeated
        );
        assert_eq!(
            statuses[&ClaimId("cycle_a".into())],
            EpistemicStatus::Unresolved
        );
        assert_eq!(
            statuses[&ClaimId("cycle_b".into())],
            EpistemicStatus::Unresolved
        );
    }

    // -- Property tests --

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_position() -> impl Strategy<Value = Position> {
            prop_oneof![
                Just(Position::Block),
                Just(Position::Object),
                Just(Position::StandAside),
                Just(Position::Abstain),
                Just(Position::Consent),
                Just(Position::Support),
                Just(Position::Champion),
            ]
        }

        fn arb_stances() -> impl Strategy<Value = Vec<StanceState>> {
            proptest::collection::vec(
                arb_position().prop_map(|pos| make_stance("c1", "author", pos)),
                0..10,
            )
        }

        proptest! {
            /// P1: OUT always → Defeated regardless of stances.
            #[test]
            fn out_always_defeated(stances in arb_stances()) {
                let refs: Vec<&StanceState> = stances.iter().collect();
                prop_assert_eq!(epistemic_status(Label::Out, &refs), EpistemicStatus::Defeated);
            }

            /// P2: UNDEC always → Unresolved regardless of stances.
            #[test]
            fn undec_always_unresolved(stances in arb_stances()) {
                let refs: Vec<&StanceState> = stances.iter().collect();
                prop_assert_eq!(epistemic_status(Label::Undec, &refs), EpistemicStatus::Unresolved);
            }

            /// P3: IN + empty stances always → Unexamined.
            #[test]
            fn in_empty_always_unexamined(_dummy in 0..100u32) {
                prop_assert_eq!(epistemic_status(Label::In, &[]), EpistemicStatus::Unexamined);
            }

            /// P4: IN + all non-negative stances → Established.
            #[test]
            fn in_all_nonnegative_is_established(
                positions in proptest::collection::vec(
                    prop_oneof![
                        Just(Position::StandAside),
                        Just(Position::Abstain),
                        Just(Position::Consent),
                        Just(Position::Support),
                        Just(Position::Champion),
                    ],
                    1..10,
                )
            ) {
                let stances: Vec<StanceState> = positions.iter().map(|p|
                    make_stance("c1", "author", *p)
                ).collect();
                let refs: Vec<&StanceState> = stances.iter().collect();
                prop_assert_eq!(epistemic_status(Label::In, &refs), EpistemicStatus::Established);
            }

            /// P5: IN + any negative stance → Contested.
            #[test]
            fn in_any_negative_is_contested(
                negative in prop_oneof![Just(Position::Block), Just(Position::Object)],
                others in proptest::collection::vec(arb_position(), 0..10),
            ) {
                let mut stances = vec![make_stance("c1", "blocker", negative)];
                for (i, pos) in others.iter().enumerate() {
                    stances.push(make_stance("c1", &format!("author_{i}"), *pos));
                }
                let refs: Vec<&StanceState> = stances.iter().collect();
                prop_assert_eq!(epistemic_status(Label::In, &refs), EpistemicStatus::Contested);
            }
        }
    }
}
