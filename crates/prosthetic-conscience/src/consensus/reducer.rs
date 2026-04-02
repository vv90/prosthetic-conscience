//! Pure reducer: replays a log of consensus protocol entries into materialized state.
//!
//! The reducer is a fold: `entries.fold(MaterializedState::new(), reduce)`.
//! It is deterministic, side-effect-free, and produces identical output for
//! identical input regardless of when or where it runs.

use super::solver::{Graph, NodeId};
use super::types::{
    ClaimId, ClaimState, Entry, MaterializedState, RelationKind, RelationState, Resolution,
    StanceState,
};

/// Apply a single log entry to the materialized state, producing a new state.
pub fn reduce(mut state: MaterializedState, entry: &Entry) -> MaterializedState {
    match entry {
        Entry::Claim {
            claim_id,
            author,
            body,
            claim_kind,
            parent_id,
        } => {
            // First writer wins: ignore duplicate claim_ids.
            if state.claims.contains_key(claim_id) {
                return state;
            }

            // Assign a stable NodeId for this claim.
            let node_id = NodeId(state.next_node_id);
            state.next_node_id += 1;
            state.node_map.insert(claim_id.clone(), node_id);

            state.claims.insert(
                claim_id.clone(),
                ClaimState {
                    id: claim_id.clone(),
                    author: author.clone(),
                    body: body.clone(),
                    kind: *claim_kind,
                    parent_id: parent_id.clone(),
                    resolution: None,
                },
            );
        }

        Entry::Relation {
            source_id,
            target_id,
            kind,
            author,
        } => {
            state.relations.push(RelationState {
                source_id: source_id.clone(),
                target_id: target_id.clone(),
                kind: *kind,
                author: author.clone(),
            });
        }

        Entry::Stance {
            target_id,
            author,
            position,
        } => {
            let key = (target_id.clone(), author.clone());
            state.stances.insert(
                key,
                StanceState {
                    target_id: target_id.clone(),
                    author: author.clone(),
                    position: *position,
                },
            );
        }

        Entry::Resolve {
            claim_id,
            author,
            outcome,
        } => {
            if let Some(claim) = state.claims.get_mut(claim_id) {
                claim.resolution = Some(Resolution {
                    outcome: *outcome,
                    author: author.clone(),
                });
            }
        }

        Entry::Comment { .. } => {
            // Comments don't affect materialized state.
        }
    }

    state
}

/// Replay a full log of entries into a materialized state.
pub fn replay(entries: &[Entry]) -> MaterializedState {
    entries
        .iter()
        .fold(MaterializedState::new(), |state, entry| {
            reduce(state, entry)
        })
}

/// Build a solver graph from the materialized state.
///
/// Returns `(graph, index_to_claim)` where `index_to_claim[node_id]` maps
/// solver NodeIds back to ClaimIds. Only active (non-resolved) claims and
/// relations with both endpoints active and known are included.
pub fn to_graph(state: &MaterializedState) -> (Graph, Vec<ClaimId>) {
    // Collect active claims and assign dense NodeIds for the solver graph.
    let mut active_node_map: std::collections::HashMap<&ClaimId, NodeId> =
        std::collections::HashMap::new();
    let mut index_to_claim: Vec<ClaimId> = Vec::new();

    for (claim_id, claim) in &state.claims {
        if claim.resolution.is_some() {
            continue;
        }
        let node_id = NodeId(index_to_claim.len() as u32);
        active_node_map.insert(claim_id, node_id);
        index_to_claim.push(claim_id.clone());
    }

    let node_count = index_to_claim.len() as u32;
    let mut builder = Graph::builder(node_count);

    for relation in &state.relations {
        let source = active_node_map.get(&relation.source_id);
        let target = active_node_map.get(&relation.target_id);

        if let (Some(&src_node), Some(&tgt_node)) = (source, target) {
            match relation.kind {
                RelationKind::Attacks => {
                    builder.attack(src_node, tgt_node);
                }
                RelationKind::Supports => {
                    builder.support(src_node, tgt_node);
                }
            }
        }
    }

    (builder.build(), index_to_claim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::solver::{Label, grounded_labelling};
    use crate::consensus::types::*;

    fn claim(id: &str, author: &str, body: &str, kind: ClaimKind) -> Entry {
        Entry::Claim {
            claim_id: ClaimId(id.into()),
            author: author.into(),
            body: body.into(),
            claim_kind: kind,
            parent_id: None,
        }
    }

    fn proposal(id: &str, author: &str, body: &str, parent: &str) -> Entry {
        Entry::Claim {
            claim_id: ClaimId(id.into()),
            author: author.into(),
            body: body.into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: Some(ClaimId(parent.into())),
        }
    }

    fn attacks(source: &str, target: &str, author: &str) -> Entry {
        Entry::Relation {
            source_id: ClaimId(source.into()),
            target_id: ClaimId(target.into()),
            kind: RelationKind::Attacks,
            author: author.into(),
        }
    }

    fn supports(source: &str, target: &str, author: &str) -> Entry {
        Entry::Relation {
            source_id: ClaimId(source.into()),
            target_id: ClaimId(target.into()),
            kind: RelationKind::Supports,
            author: author.into(),
        }
    }

    fn stance(target: &str, author: &str, position: Position) -> Entry {
        Entry::Stance {
            target_id: ClaimId(target.into()),
            author: author.into(),
            position,
        }
    }

    fn resolve(claim: &str, author: &str, outcome: Outcome) -> Entry {
        Entry::Resolve {
            claim_id: ClaimId(claim.into()),
            author: author.into(),
            outcome,
        }
    }

    // -- Empty state --

    #[test]
    fn empty_log_produces_empty_state() {
        let state = replay(&[]);
        assert!(state.claims.is_empty());
        assert!(state.relations.is_empty());
        assert!(state.stances.is_empty());
    }

    // -- Claim reduction --

    #[test]
    fn single_claim_creates_entry() {
        let state = replay(&[claim("c1", "alice", "Use JWT", ClaimKind::Proposal)]);
        assert_eq!(state.claims.len(), 1);
        let c = &state.claims[&ClaimId("c1".into())];
        assert_eq!(c.author, "alice");
        assert_eq!(c.body, "Use JWT");
        assert_eq!(c.kind, ClaimKind::Proposal);
        assert!(c.resolution.is_none());
    }

    #[test]
    fn duplicate_claim_id_first_writer_wins() {
        let state = replay(&[
            claim("c1", "alice", "First version", ClaimKind::Fact),
            claim("c1", "bob", "Second version", ClaimKind::Fact),
        ]);
        assert_eq!(state.claims.len(), 1);
        assert_eq!(state.claims[&ClaimId("c1".into())].author, "alice");
        assert_eq!(state.claims[&ClaimId("c1".into())].body, "First version");
    }

    #[test]
    fn claim_gets_node_id() {
        let state = replay(&[claim("c1", "alice", "Claim", ClaimKind::Fact)]);
        assert!(state.node_map.contains_key(&ClaimId("c1".into())));
    }

    #[test]
    fn proposal_with_parent() {
        let state = replay(&[
            claim("item1", "alice", "Auth approach?", ClaimKind::Item),
            proposal("p1", "bob", "Use JWT", "item1"),
        ]);
        let p = &state.claims[&ClaimId("p1".into())];
        assert_eq!(p.parent_id, Some(ClaimId("item1".into())));
    }

    // -- Relation reduction --

    #[test]
    fn relation_between_existing_claims() {
        let state = replay(&[
            claim("c1", "alice", "A", ClaimKind::Fact),
            claim("c2", "bob", "B", ClaimKind::Fact),
            attacks("c2", "c1", "bob"),
        ]);
        assert_eq!(state.relations.len(), 1);
        assert_eq!(state.relations[0].source_id, ClaimId("c2".into()));
        assert_eq!(state.relations[0].target_id, ClaimId("c1".into()));
        assert_eq!(state.relations[0].kind, RelationKind::Attacks);
    }

    #[test]
    fn support_relation_stored() {
        let state = replay(&[
            claim("c1", "alice", "A", ClaimKind::Fact),
            claim("c2", "bob", "B supports A", ClaimKind::Fact),
            supports("c2", "c1", "bob"),
        ]);
        assert_eq!(state.relations.len(), 1);
        assert_eq!(state.relations[0].kind, RelationKind::Supports);
    }

    #[test]
    fn relation_with_unknown_target_stored_but_excluded_from_graph() {
        let state = replay(&[
            claim("c1", "alice", "A", ClaimKind::Fact),
            attacks("c1", "nonexistent", "alice"),
        ]);
        // Stored in relations list
        assert_eq!(state.relations.len(), 1);
        // But excluded from graph (nonexistent target)
        let (graph, _) = to_graph(&state);
        assert_eq!(graph.node_count(), 1);
        assert!(graph.attackers(NodeId(0)).is_empty());
    }

    // -- Stance reduction --

    #[test]
    fn stance_stored_by_target_and_author() {
        let state = replay(&[
            claim("c1", "alice", "Proposal", ClaimKind::Proposal),
            stance("c1", "bob", Position::Support),
        ]);
        let key = (ClaimId("c1".into()), "bob".into());
        assert!(state.stances.contains_key(&key));
        assert_eq!(state.stances[&key].position, Position::Support);
    }

    #[test]
    fn stance_supersession_latest_wins() {
        let state = replay(&[
            claim("c1", "alice", "Proposal", ClaimKind::Proposal),
            stance("c1", "bob", Position::Support),
            stance("c1", "bob", Position::Block),
        ]);
        let key = (ClaimId("c1".into()), "bob".into());
        assert_eq!(state.stances[&key].position, Position::Block);
    }

    #[test]
    fn different_authors_independent_stances() {
        let state = replay(&[
            claim("c1", "alice", "Proposal", ClaimKind::Proposal),
            stance("c1", "bob", Position::Support),
            stance("c1", "carol", Position::Object),
        ]);
        assert_eq!(state.stances.len(), 2);
        let bob_key = (ClaimId("c1".into()), "bob".into());
        let carol_key = (ClaimId("c1".into()), "carol".into());
        assert_eq!(state.stances[&bob_key].position, Position::Support);
        assert_eq!(state.stances[&carol_key].position, Position::Object);
    }

    // -- Resolve reduction --

    #[test]
    fn resolve_marks_claim_resolved() {
        let state = replay(&[
            claim("c1", "alice", "Proposal", ClaimKind::Proposal),
            resolve("c1", "alice", Outcome::Accepted),
        ]);
        let c = &state.claims[&ClaimId("c1".into())];
        assert!(c.resolution.is_some());
        assert_eq!(c.resolution.as_ref().unwrap().outcome, Outcome::Accepted);
    }

    #[test]
    fn resolve_unknown_claim_is_ignored() {
        let state = replay(&[resolve("nonexistent", "alice", Outcome::Rejected)]);
        assert!(state.claims.is_empty());
    }

    // -- Comment reduction --

    #[test]
    fn comment_does_not_affect_state() {
        let state = replay(&[Entry::Comment {
            claim_id: None,
            author: "dave".into(),
            body: "Interesting discussion".into(),
        }]);
        assert!(state.claims.is_empty());
        assert!(state.relations.is_empty());
        assert!(state.stances.is_empty());
    }

    // -- Graph extraction --

    #[test]
    fn resolved_claims_excluded_from_graph() {
        let state = replay(&[
            claim("c1", "alice", "Active", ClaimKind::Fact),
            claim("c2", "bob", "Resolved", ClaimKind::Proposal),
            resolve("c2", "bob", Outcome::Withdrawn),
        ]);
        let (graph, index) = to_graph(&state);
        assert_eq!(graph.node_count(), 1);
        assert_eq!(index.len(), 1);
        assert_eq!(index[0], ClaimId("c1".into()));
    }

    #[test]
    fn relations_touching_resolved_claims_excluded() {
        let state = replay(&[
            claim("c1", "alice", "Active", ClaimKind::Fact),
            claim("c2", "bob", "Will be resolved", ClaimKind::Proposal),
            attacks("c1", "c2", "alice"),
            resolve("c2", "bob", Outcome::Withdrawn),
        ]);
        let (graph, _) = to_graph(&state);
        // c2 is resolved, so the attack c1→c2 is excluded
        assert_eq!(graph.node_count(), 1);
        assert!(graph.attackers(NodeId(0)).is_empty());
    }

    #[test]
    fn to_graph_simple_attack() {
        let state = replay(&[
            claim("c1", "alice", "A", ClaimKind::Fact),
            claim("c2", "bob", "B attacks A", ClaimKind::Fact),
            attacks("c2", "c1", "bob"),
        ]);
        let (graph, index) = to_graph(&state);
        assert_eq!(graph.node_count(), 2);

        // Find which NodeId corresponds to c1 and c2
        let c1_pos = index
            .iter()
            .position(|id| *id == ClaimId("c1".into()))
            .unwrap();
        let c2_pos = index
            .iter()
            .position(|id| *id == ClaimId("c2".into()))
            .unwrap();

        // c1 should be attacked by c2
        let c1_attackers = graph.attackers(NodeId(c1_pos as u32));
        assert_eq!(c1_attackers.len(), 1);
        assert_eq!(c1_attackers[0], NodeId(c2_pos as u32));

        // c2 should have no attackers
        assert!(graph.attackers(NodeId(c2_pos as u32)).is_empty());
    }

    // -- Reducer + solver integration --

    #[test]
    fn reducer_solver_integration_reinstatement() {
        // A (unattacked fact), B attacks A, C attacks B
        // Expected: A=IN (reinstated), B=OUT, C=IN
        let state = replay(&[
            claim("a", "alice", "Fact A", ClaimKind::Fact),
            claim("b", "bob", "B attacks A", ClaimKind::Fact),
            claim("c", "carol", "C attacks B", ClaimKind::Fact),
            attacks("b", "a", "bob"),
            attacks("c", "b", "carol"),
        ]);
        let (graph, index) = to_graph(&state);
        let labels = grounded_labelling(&graph);

        let label_for = |id: &str| -> Label {
            let pos = index
                .iter()
                .position(|cid| *cid == ClaimId(id.into()))
                .unwrap();
            labels[pos]
        };

        assert_eq!(label_for("a"), Label::In);
        assert_eq!(label_for("b"), Label::Out);
        assert_eq!(label_for("c"), Label::In);
    }

    #[test]
    fn reducer_solver_integration_mutual_attack() {
        // A ↔ B: both UNDEC, C (unattacked): IN
        let state = replay(&[
            claim("a", "alice", "A", ClaimKind::Fact),
            claim("b", "bob", "B", ClaimKind::Fact),
            claim("c", "carol", "C", ClaimKind::Fact),
            attacks("a", "b", "alice"),
            attacks("b", "a", "bob"),
        ]);
        let (graph, index) = to_graph(&state);
        let labels = grounded_labelling(&graph);

        let label_for = |id: &str| -> Label {
            let pos = index
                .iter()
                .position(|cid| *cid == ClaimId(id.into()))
                .unwrap();
            labels[pos]
        };

        assert_eq!(label_for("a"), Label::Undec);
        assert_eq!(label_for("b"), Label::Undec);
        assert_eq!(label_for("c"), Label::In);
    }

    // -- Property tests --

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        const AUTHORS: &[&str] = &["alice", "bob", "carol", "dave"];

        fn arb_claim_id() -> impl Strategy<Value = String> {
            prop::sample::select(&["c1", "c2", "c3", "c4", "c5"][..]).prop_map(String::from)
        }

        fn arb_claim_kind() -> impl Strategy<Value = ClaimKind> {
            prop_oneof![
                Just(ClaimKind::Item),
                Just(ClaimKind::Proposal),
                Just(ClaimKind::Fact),
            ]
        }

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

        fn arb_outcome() -> impl Strategy<Value = Outcome> {
            prop_oneof![
                Just(Outcome::Accepted),
                Just(Outcome::Rejected),
                Just(Outcome::Tabled),
                Just(Outcome::Withdrawn),
            ]
        }

        fn arb_entry() -> impl Strategy<Value = Entry> {
            prop_oneof![
                // Claims
                (
                    arb_claim_id(),
                    prop::sample::select(AUTHORS),
                    arb_claim_kind()
                )
                    .prop_map(|(id, author, kind)| Entry::Claim {
                        claim_id: ClaimId(id.clone()),
                        author: String::from(author),
                        body: format!("Body of {id}"),
                        claim_kind: kind,
                        parent_id: None,
                    }),
                // Relations
                (
                    arb_claim_id(),
                    arb_claim_id(),
                    prop::sample::select(AUTHORS)
                )
                    .prop_map(|(src, tgt, author)| Entry::Relation {
                        source_id: ClaimId(src),
                        target_id: ClaimId(tgt),
                        kind: RelationKind::Attacks,
                        author: String::from(author),
                    }),
                // Stances
                (
                    arb_claim_id(),
                    prop::sample::select(AUTHORS),
                    arb_position()
                )
                    .prop_map(|(target, author, position)| Entry::Stance {
                        target_id: ClaimId(target),
                        author: String::from(author),
                        position,
                    }),
                // Resolves
                (arb_claim_id(), prop::sample::select(AUTHORS), arb_outcome()).prop_map(
                    |(id, author, outcome)| Entry::Resolve {
                        claim_id: ClaimId(id),
                        author: String::from(author),
                        outcome,
                    }
                ),
                // Comments
                prop::sample::select(AUTHORS).prop_map(|author| Entry::Comment {
                    claim_id: None,
                    author: String::from(author),
                    body: "A comment".into(),
                }),
            ]
        }

        fn arb_log() -> impl Strategy<Value = Vec<Entry>> {
            proptest::collection::vec(arb_entry(), 0..30)
        }

        proptest! {
            /// P1: Deterministic replay — same log always produces same claim set.
            #[test]
            fn deterministic_replay(log in arb_log()) {
                let state1 = replay(&log);
                let state2 = replay(&log);
                prop_assert_eq!(state1.claims.len(), state2.claims.len());
                prop_assert_eq!(state1.relations.len(), state2.relations.len());
                prop_assert_eq!(state1.stances.len(), state2.stances.len());
                for (k, v) in &state1.claims {
                    prop_assert_eq!(v, &state2.claims[k]);
                }
                for (k, v) in &state1.stances {
                    prop_assert_eq!(v, &state2.stances[k]);
                }
            }

            /// P2: Claim count never decreases as entries are appended.
            #[test]
            fn claim_count_monotonic(log in arb_log()) {
                let mut state = MaterializedState::new();
                let mut prev_count = 0;
                for entry in &log {
                    state = reduce(state, entry);
                    prop_assert!(
                        state.claims.len() >= prev_count,
                        "Claim count decreased from {} to {}",
                        prev_count, state.claims.len()
                    );
                    prev_count = state.claims.len();
                }
            }

            /// P3: Stance latest-wins — state matches last stance per (target, author).
            #[test]
            fn stance_latest_wins(log in arb_log()) {
                let state = replay(&log);

                // Compute expected stances by scanning log in order
                let mut expected: std::collections::HashMap<(ClaimId, String), Position> =
                    std::collections::HashMap::new();
                for entry in &log {
                    if let Entry::Stance { target_id, author, position } = entry {
                        expected.insert((target_id.clone(), author.clone()), *position);
                    }
                }

                for (key, expected_pos) in &expected {
                    if let Some(stance) = state.stances.get(key) {
                        prop_assert_eq!(
                            &stance.position, expected_pos,
                            "Stance for {:?} doesn't match last entry", key
                        );
                    }
                }
            }

            /// P4: Graph node count ≤ number of active (non-resolved) claims.
            #[test]
            fn graph_node_count_le_active_claims(log in arb_log()) {
                let state = replay(&log);
                let active = state.claims.values().filter(|c| c.resolution.is_none()).count();
                let (graph, _) = to_graph(&state);
                prop_assert!(
                    graph.node_count() as usize <= active,
                    "Graph has {} nodes but only {} active claims",
                    graph.node_count(), active
                );
            }

            /// P5: All graph edges reference valid nodes.
            #[test]
            fn graph_edges_valid(log in arb_log()) {
                let state = replay(&log);
                let (graph, _) = to_graph(&state);
                let n = graph.node_count();
                for node in 0..n {
                    for attacker in graph.attackers(NodeId(node)) {
                        prop_assert!(
                            attacker.0 < n,
                            "Edge references node {} but graph has only {} nodes",
                            attacker.0, n
                        );
                    }
                }
            }
        }
    }
}
