//! Entry types and materialized state for the consensus protocol.
//!
//! These types define the structured entries that participants submit to the
//! shared append-only log, and the materialized state produced by replaying
//! those entries through the reducer.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::solver::NodeId;

// ---------------------------------------------------------------------------
// Identity types
// ---------------------------------------------------------------------------

/// Unique identifier for a claim in the deliberation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClaimId(pub String);

// ---------------------------------------------------------------------------
// Entry enums
// ---------------------------------------------------------------------------

/// Classification hint for claims. The solver treats all claims identically;
/// this is for rendering and attention routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    Item,
    Proposal,
    Fact,
    Conditional,
    Value,
    Reference,
}

/// The type of relation between two claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Attacks,
    Supports,
}

/// A participant's discrete position on a proposal.
///
/// Derived from Sociocracy + Fist-to-Five + Kaner gradients of agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Position {
    Block,
    Object,
    StandAside,
    Abstain,
    Consent,
    Support,
    Champion,
}

impl Position {
    /// Returns true for positions that indicate disagreement (Block, Object).
    /// Used by epistemic status computation: any negative stance on an IN
    /// claim makes it Contested rather than Established.
    pub fn is_negative(self) -> bool {
        matches!(self, Position::Block | Position::Object)
    }
}

/// The outcome of resolving a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Accepted,
    Rejected,
    Tabled,
    Withdrawn,
}

/// A structured entry in the consensus protocol log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    Claim {
        claim_id: ClaimId,
        author: String,
        body: String,
        claim_kind: ClaimKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_id: Option<ClaimId>,
    },
    Relation {
        source_id: ClaimId,
        target_id: ClaimId,
        kind: RelationKind,
        author: String,
    },
    Stance {
        target_id: ClaimId,
        author: String,
        position: Position,
    },
    Resolve {
        claim_id: ClaimId,
        author: String,
        outcome: Outcome,
    },
    Comment {
        author: String,
        body: String,
    },
}

// ---------------------------------------------------------------------------
// Materialized state types
// ---------------------------------------------------------------------------

/// Resolution status of a claim (set by a `resolve` entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub outcome: Outcome,
    pub author: String,
}

/// A claim as materialized from the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimState {
    pub id: ClaimId,
    pub author: String,
    pub body: String,
    pub kind: ClaimKind,
    pub parent_id: Option<ClaimId>,
    pub resolution: Option<Resolution>,
}

/// A relation as materialized from the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationState {
    pub source_id: ClaimId,
    pub target_id: ClaimId,
    pub kind: RelationKind,
    pub author: String,
}

/// A stance as materialized from the log (latest per author per target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StanceState {
    pub target_id: ClaimId,
    pub author: String,
    pub position: Position,
}

/// The materialized state produced by replaying the log through the reducer.
///
/// Contains all claims, relations, and stances, plus the mapping from
/// `ClaimId` to solver `NodeId` for graph extraction.
#[derive(Debug, Clone)]
pub struct MaterializedState {
    pub claims: HashMap<ClaimId, ClaimState>,
    pub relations: Vec<RelationState>,
    /// Keyed by `(target_id, author)` — latest stance wins.
    pub stances: HashMap<(ClaimId, String), StanceState>,
    /// Maps each claim to a stable solver NodeId.
    pub node_map: HashMap<ClaimId, NodeId>,
    pub next_node_id: u32,
}

impl MaterializedState {
    pub fn new() -> Self {
        Self {
            claims: HashMap::new(),
            relations: Vec::new(),
            stances: HashMap::new(),
            node_map: HashMap::new(),
            next_node_id: 0,
        }
    }
}

impl Default for MaterializedState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    // -- Serde round-trip tests --

    #[test]
    fn claim_entry_round_trip() {
        let entry = Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "We should use JWT".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: Some(ClaimId("item1".into())),
        };
        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: Entry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn claim_entry_json_shape() {
        let entry = Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "We should use JWT".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: None,
        };
        let v: Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["type"], "claim");
        assert_eq!(v["claim_kind"], "proposal");
        assert!(v.get("parent_id").is_none());
    }

    #[test]
    fn claim_entry_with_parent_id_json_shape() {
        let entry = Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Use JWT".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: Some(ClaimId("item1".into())),
        };
        let v: Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["parent_id"], "item1");
    }

    #[test]
    fn relation_entry_round_trip() {
        let entry = Entry::Relation {
            source_id: ClaimId("c2".into()),
            target_id: ClaimId("c1".into()),
            kind: RelationKind::Attacks,
            author: "bob".into(),
        };
        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: Entry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn relation_entry_json_shape() {
        let entry = Entry::Relation {
            source_id: ClaimId("c2".into()),
            target_id: ClaimId("c1".into()),
            kind: RelationKind::Supports,
            author: "bob".into(),
        };
        let v: Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["type"], "relation");
        assert_eq!(v["kind"], "supports");
    }

    #[test]
    fn stance_entry_round_trip() {
        let entry = Entry::Stance {
            target_id: ClaimId("c1".into()),
            author: "carol".into(),
            position: Position::Block,
        };
        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: Entry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn stance_entry_json_shape() {
        let entry = Entry::Stance {
            target_id: ClaimId("c1".into()),
            author: "carol".into(),
            position: Position::StandAside,
        };
        let v: Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["type"], "stance");
        assert_eq!(v["position"], "stand_aside");
    }

    #[test]
    fn resolve_entry_round_trip() {
        let entry = Entry::Resolve {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            outcome: Outcome::Accepted,
        };
        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: Entry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn resolve_entry_json_shape() {
        let entry = Entry::Resolve {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            outcome: Outcome::Withdrawn,
        };
        let v: Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["type"], "resolve");
        assert_eq!(v["outcome"], "withdrawn");
    }

    #[test]
    fn comment_entry_round_trip() {
        let entry = Entry::Comment {
            author: "dave".into(),
            body: "I think we need more context".into(),
        };
        let json_str = serde_json::to_string(&entry).unwrap();
        let parsed: Entry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn comment_entry_json_shape() {
        let entry = Entry::Comment {
            author: "dave".into(),
            body: "Good point".into(),
        };
        let v: Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["type"], "comment");
        assert_eq!(v["author"], "dave");
        assert_eq!(v["body"], "Good point");
    }

    #[test]
    fn unknown_type_fails_deserialization() {
        let input = r#"{"type":"unknown","author":"eve"}"#;
        let result = serde_json::from_str::<Entry>(input);
        assert!(result.is_err());
    }

    #[test]
    fn all_claim_kinds_round_trip() {
        let kinds = [
            ClaimKind::Item,
            ClaimKind::Proposal,
            ClaimKind::Fact,
            ClaimKind::Conditional,
            ClaimKind::Value,
            ClaimKind::Reference,
        ];
        for kind in &kinds {
            let json_str = serde_json::to_string(kind).unwrap();
            let parsed: ClaimKind = serde_json::from_str(&json_str).unwrap();
            assert_eq!(&parsed, kind);
        }
    }

    #[test]
    fn all_positions_round_trip() {
        let positions = [
            Position::Block,
            Position::Object,
            Position::StandAside,
            Position::Abstain,
            Position::Consent,
            Position::Support,
            Position::Champion,
        ];
        for pos in &positions {
            let json_str = serde_json::to_string(pos).unwrap();
            let parsed: Position = serde_json::from_str(&json_str).unwrap();
            assert_eq!(&parsed, pos);
        }
    }

    #[test]
    fn all_outcomes_round_trip() {
        let outcomes = [
            Outcome::Accepted,
            Outcome::Rejected,
            Outcome::Tabled,
            Outcome::Withdrawn,
        ];
        for outcome in &outcomes {
            let json_str = serde_json::to_string(outcome).unwrap();
            let parsed: Outcome = serde_json::from_str(&json_str).unwrap();
            assert_eq!(&parsed, outcome);
        }
    }

    #[test]
    fn entry_from_json_claim() {
        let input = r#"{"type":"claim","claim_id":"c1","author":"alice","body":"Use JWT","claim_kind":"proposal"}"#;
        let parsed: Entry = serde_json::from_str(input).unwrap();
        assert!(matches!(
            parsed,
            Entry::Claim {
                claim_kind: ClaimKind::Proposal,
                ..
            }
        ));
    }

    #[test]
    fn entry_from_json_stance_block() {
        let input = r#"{"type":"stance","target_id":"c1","author":"bob","position":"block"}"#;
        let parsed: Entry = serde_json::from_str(input).unwrap();
        assert!(matches!(
            parsed,
            Entry::Stance {
                position: Position::Block,
                ..
            }
        ));
    }

    #[test]
    fn claim_id_from_json() {
        let v = json!("my-claim-id");
        let id: ClaimId = serde_json::from_value(v).unwrap();
        assert_eq!(id, ClaimId("my-claim-id".into()));
    }
}
