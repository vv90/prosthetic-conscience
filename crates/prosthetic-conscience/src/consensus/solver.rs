//! Grounded semantics solver for abstract argumentation frameworks.
//!
//! Operates on an abstract graph of node IDs and directed attack edges.
//! Content-opaque: the solver never sees natural language, only graph structure.

/// A node in the argumentation graph. Newtype over u32 for type safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// The label assigned to a node by the solver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// Defensible: all attackers are OUT (including vacuously, when there are none).
    In,
    /// Defeated: at least one attacker is IN.
    Out,
    /// Unresolved: involved in cycles or mutual attacks that prevent determination.
    Undec,
}

/// An argumentation graph: nodes (implicit 0..node_count) and incoming attack edges.
///
/// The `supported_by` field is reserved for future BAF extension and is
/// currently ignored by the solver.
#[derive(Debug, Clone)]
pub struct Graph {
    node_count: u32,
    /// `attacked_by[target]` = list of nodes that attack `target`.
    attacked_by: Vec<Vec<NodeId>>,
    /// Reserved for BAF support propagation (not yet used by solver).
    #[allow(dead_code)]
    supported_by: Vec<Vec<NodeId>>,
}

impl Graph {
    pub fn builder(node_count: u32) -> GraphBuilder {
        GraphBuilder {
            node_count,
            attacks: Vec::new(),
            supports: Vec::new(),
        }
    }

    pub fn node_count(&self) -> u32 {
        self.node_count
    }

    pub fn attackers(&self, target: NodeId) -> &[NodeId] {
        &self.attacked_by[target.0 as usize]
    }
}

/// Incremental builder for constructing argumentation graphs.
pub struct GraphBuilder {
    node_count: u32,
    attacks: Vec<(NodeId, NodeId)>,
    supports: Vec<(NodeId, NodeId)>,
}

impl GraphBuilder {
    /// Add an attack edge: `source` attacks `target`.
    pub fn attack(&mut self, source: NodeId, target: NodeId) -> &mut Self {
        self.attacks.push((source, target));
        self
    }

    /// Add a support edge: `source` supports `target`.
    /// Reserved for future BAF extension.
    pub fn support(&mut self, source: NodeId, target: NodeId) -> &mut Self {
        self.supports.push((source, target));
        self
    }

    pub fn build(self) -> Graph {
        let n = self.node_count as usize;
        let mut attacked_by = vec![Vec::new(); n];
        let mut supported_by = vec![Vec::new(); n];

        for (source, target) in self.attacks {
            attacked_by[target.0 as usize].push(source);
        }
        for (source, target) in self.supports {
            supported_by[target.0 as usize].push(source);
        }

        Graph {
            node_count: self.node_count,
            attacked_by,
            supported_by,
        }
    }
}

/// Compute the grounded labelling for an argumentation graph.
///
/// Uses Dung's iterative fixpoint algorithm:
/// 1. Start with all nodes UNDEC.
/// 2. Any UNDEC node whose attackers are all OUT → IN.
/// 3. Any UNDEC node with at least one IN attacker → OUT.
/// 4. Repeat until no labels change.
///
/// Unattacked nodes are IN on the first pass (vacuously, all zero attackers are OUT).
pub fn grounded_labelling(graph: &Graph) -> Vec<Label> {
    let n = graph.node_count as usize;
    let mut labels = vec![Label::Undec; n];

    loop {
        let mut changed = false;

        for node in 0..n {
            if labels[node] != Label::Undec {
                continue;
            }

            let attackers = &graph.attacked_by[node];

            let all_attackers_out = attackers.iter().all(|a| labels[a.0 as usize] == Label::Out);

            if all_attackers_out {
                labels[node] = Label::In;
                changed = true;
                continue;
            }

            let any_attacker_in = attackers.iter().any(|a| labels[a.0 as usize] == Label::In);

            if any_attacker_in {
                labels[node] = Label::Out;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: node IDs for readability
    const A: NodeId = NodeId(0);
    const B: NodeId = NodeId(1);
    const C: NodeId = NodeId(2);
    const D: NodeId = NodeId(3);

    // -- Empty and trivial graphs --

    #[test]
    fn empty_graph() {
        let graph = Graph::builder(0).build();
        let labels = grounded_labelling(&graph);
        assert!(labels.is_empty());
    }

    #[test]
    fn single_unattacked_node_is_in() {
        let graph = Graph::builder(1).build();
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
    }

    #[test]
    fn two_unattacked_nodes_both_in() {
        let graph = Graph::builder(2).build();
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::In);
    }

    // -- Linear chains --

    /// Helper: build a graph from a list of attack edges.
    fn graph_with_attacks(node_count: u32, attacks: &[(NodeId, NodeId)]) -> Graph {
        let mut builder = Graph::builder(node_count);
        for &(src, tgt) in attacks {
            builder.attack(src, tgt);
        }
        builder.build()
    }

    #[test]
    fn a_attacks_b() {
        // A → B: A is IN (unattacked), B is OUT (attacked by IN)
        let graph = graph_with_attacks(2, &[(A, B)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::Out);
    }

    #[test]
    fn reinstatement_a_attacks_b_attacks_c() {
        // A → B → C: A=IN, B=OUT, C=IN (reinstated)
        let graph = graph_with_attacks(3, &[(A, B), (B, C)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::Out);
        assert_eq!(labels[C.0 as usize], Label::In);
    }

    #[test]
    fn chain_of_four() {
        // A → B → C → D: A=IN, B=OUT, C=IN, D=OUT
        let graph = graph_with_attacks(4, &[(A, B), (B, C), (C, D)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::Out);
        assert_eq!(labels[C.0 as usize], Label::In);
        assert_eq!(labels[D.0 as usize], Label::Out);
    }

    // -- Cycles --

    #[test]
    fn mutual_attack_both_undec() {
        // A ↔ B: both UNDEC (neither can be grounded)
        let graph = graph_with_attacks(2, &[(A, B), (B, A)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::Undec);
        assert_eq!(labels[B.0 as usize], Label::Undec);
    }

    #[test]
    fn self_attack_is_undec() {
        // A attacks itself: UNDEC (cannot be IN because attacker not OUT,
        // cannot be OUT because no attacker is IN)
        let graph = graph_with_attacks(1, &[(A, A)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::Undec);
    }

    #[test]
    fn odd_cycle_all_undec() {
        // A → B → C → A: all UNDEC
        let graph = graph_with_attacks(3, &[(A, B), (B, C), (C, A)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::Undec);
        assert_eq!(labels[B.0 as usize], Label::Undec);
        assert_eq!(labels[C.0 as usize], Label::Undec);
    }

    // -- Mixed: grounded nodes + cycles --

    #[test]
    fn grounded_node_defeats_into_cycle() {
        // C (unattacked) attacks A, which is in a cycle A ↔ B
        // C=IN, A=OUT (attacked by IN C), B=IN (A is OUT, B's only attacker is OUT)
        let graph = graph_with_attacks(3, &[(A, B), (B, A), (C, A)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[C.0 as usize], Label::In);
        assert_eq!(labels[A.0 as usize], Label::Out);
        assert_eq!(labels[B.0 as usize], Label::In);
    }

    #[test]
    fn disconnected_components() {
        // A → B (separate), C → D (separate)
        // A=IN, B=OUT, C=IN, D=OUT
        let graph = graph_with_attacks(4, &[(A, B), (C, D)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::Out);
        assert_eq!(labels[C.0 as usize], Label::In);
        assert_eq!(labels[D.0 as usize], Label::Out);
    }

    #[test]
    fn multiple_attackers_all_must_be_out_for_in() {
        // A → C, B → C: C is IN only if both A and B are OUT.
        // A and B are unattacked → both IN → C is OUT.
        let graph = graph_with_attacks(3, &[(A, C), (B, C)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::In);
        assert_eq!(labels[C.0 as usize], Label::Out);
    }

    #[test]
    fn multiple_attackers_one_undec_prevents_in() {
        // A ↔ B (cycle, both UNDEC), A → C
        // C has attacker A which is UNDEC → C cannot be IN.
        // C has no IN attacker → C cannot be OUT. → C is UNDEC.
        let graph = graph_with_attacks(3, &[(A, B), (B, A), (A, C)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::Undec);
        assert_eq!(labels[B.0 as usize], Label::Undec);
        assert_eq!(labels[C.0 as usize], Label::Undec);
    }

    // -- Duplicate edges --

    #[test]
    fn duplicate_attack_edge_same_result() {
        // A → B, A → B (duplicate): same as single A → B
        let graph = graph_with_attacks(2, &[(A, B), (A, B)]);
        let labels = grounded_labelling(&graph);
        assert_eq!(labels[A.0 as usize], Label::In);
        assert_eq!(labels[B.0 as usize], Label::Out);
    }

    // -- Property tests --

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_graph() -> impl Strategy<Value = Graph> {
            (1..20u32)
                .prop_flat_map(|n| {
                    let max_edges = (n as usize) * (n as usize);
                    let edge_count = 0..max_edges.min(40);
                    (Just(n), proptest::collection::vec((0..n, 0..n), edge_count))
                })
                .prop_map(|(n, edges)| {
                    let mut builder = Graph::builder(n);
                    for (src, tgt) in edges {
                        builder.attack(NodeId(src), NodeId(tgt));
                    }
                    builder.build()
                })
        }

        proptest! {
            /// P1: Unattacked nodes are always IN.
            #[test]
            fn unattacked_nodes_are_in(graph in arb_graph()) {
                let labels = grounded_labelling(&graph);
                for (node, label) in labels.iter().enumerate() {
                    if graph.attackers(NodeId(node as u32)).is_empty() {
                        prop_assert_eq!(
                            *label, Label::In,
                            "Unattacked node {} should be IN", node
                        );
                    }
                }
            }

            /// P2: If any attacker of N is IN, then N is OUT.
            #[test]
            fn attacked_by_in_is_out(graph in arb_graph()) {
                let labels = grounded_labelling(&graph);
                for node in 0..graph.node_count() as usize {
                    let any_in_attacker = graph.attackers(NodeId(node as u32))
                        .iter()
                        .any(|a| labels[a.0 as usize] == Label::In);
                    if any_in_attacker {
                        prop_assert_eq!(
                            labels[node], Label::Out,
                            "Node {} has an IN attacker but is not OUT", node
                        );
                    }
                }
            }

            /// P3: If N is IN, then all of N's attackers are OUT.
            #[test]
            fn in_nodes_have_all_attackers_out(graph in arb_graph()) {
                let labels = grounded_labelling(&graph);
                for node in 0..graph.node_count() as usize {
                    if labels[node] == Label::In {
                        for attacker in graph.attackers(NodeId(node as u32)) {
                            prop_assert_eq!(
                                labels[attacker.0 as usize], Label::Out,
                                "IN node {} has attacker {} which is not OUT",
                                node, attacker.0
                            );
                        }
                    }
                }
            }

            /// P4: Idempotency — running the solver twice gives the same result.
            #[test]
            fn idempotency(graph in arb_graph()) {
                let labels1 = grounded_labelling(&graph);
                let labels2 = grounded_labelling(&graph);
                prop_assert_eq!(labels1, labels2);
            }

            /// P5: OUT nodes always have at least one attacker.
            #[test]
            fn out_nodes_have_attackers(graph in arb_graph()) {
                let labels = grounded_labelling(&graph);
                for (node, label) in labels.iter().enumerate() {
                    if *label == Label::Out {
                        prop_assert!(
                            !graph.attackers(NodeId(node as u32)).is_empty(),
                            "OUT node {} has no attackers", node
                        );
                    }
                }
            }

            /// P6: UNDEC nodes are never unattacked (unattacked → IN, not UNDEC).
            #[test]
            fn undec_nodes_have_attackers(graph in arb_graph()) {
                let labels = grounded_labelling(&graph);
                for (node, label) in labels.iter().enumerate() {
                    if *label == Label::Undec {
                        prop_assert!(
                            !graph.attackers(NodeId(node as u32)).is_empty(),
                            "UNDEC node {} has no attackers", node
                        );
                    }
                }
            }

            /// P7: Adding an isolated node doesn't change existing labels.
            #[test]
            fn isolated_node_addition_preserves_labels(graph in arb_graph()) {
                let original_labels = grounded_labelling(&graph);

                // Build a new graph with one extra node, no new edges
                let n = graph.node_count();
                let mut builder = Graph::builder(n + 1);
                for target in 0..n {
                    for attacker in graph.attackers(NodeId(target)) {
                        builder.attack(*attacker, NodeId(target));
                    }
                }
                let extended = builder.build();
                let extended_labels = grounded_labelling(&extended);

                // Original nodes unchanged
                for node in 0..n as usize {
                    prop_assert_eq!(
                        original_labels[node], extended_labels[node],
                        "Node {} label changed after adding isolated node", node
                    );
                }
                // New node is IN (unattacked)
                prop_assert_eq!(
                    extended_labels[n as usize], Label::In,
                    "Newly added isolated node should be IN"
                );
            }
        }
    }
}
