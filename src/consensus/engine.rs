//! Stateful consensus engine.
//!
//! Owns the entry log and runs the full pipeline (reduce → graph → solve →
//! status → render) on every query. Manages a draft buffer for accumulating
//! proposed entries before submission.

use std::collections::HashMap;

use serde::Serialize;

use super::reducer::{replay, to_graph};
use super::render::{self, ClaimDetail, OverviewData};
use super::solver::grounded_labelling;
use super::status::{EpistemicStatus, compute_all};
use super::types::{ClaimId, ClaimKind, Entry, MaterializedState, Outcome, Position, RelationKind};

// ---------------------------------------------------------------------------
// Draft types
// ---------------------------------------------------------------------------

/// Opaque identifier for a draft entry. Monotonically increasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct DraftId(pub u64);

/// A draft entry awaiting submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftEntry {
    pub id: DraftId,
    pub entry: Entry,
}

/// Errors that the engine can produce.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("draft not found: {0:?}")]
    DraftNotFound(DraftId),
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// The consensus engine: owns the log and draft buffer, materializes state on demand.
pub struct ConsensusEngine {
    log: Vec<Entry>,
    drafts: Vec<DraftEntry>,
    next_draft_id: u64,
}

impl Default for ConsensusEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ConsensusEngine {
    /// Create an empty engine.
    pub fn new() -> Self {
        Self {
            log: Vec::new(),
            drafts: Vec::new(),
            next_draft_id: 0,
        }
    }

    // -- Log management -----------------------------------------------------

    /// Append a committed entry (e.g. received from session WebSocket).
    pub fn append(&mut self, entry: Entry) {
        self.log.push(entry);
    }

    /// The committed log entries.
    pub fn log(&self) -> &[Entry] {
        &self.log
    }

    // -- Query committed state ----------------------------------------------

    /// High-level overview of the committed deliberation state.
    pub fn overview(&self) -> OverviewData {
        let (state, statuses) = Self::materialize(&self.log);
        render::overview(&state, &statuses)
    }

    /// Detailed view of a single claim in committed state.
    pub fn claim_detail(&self, claim_id: &ClaimId) -> Option<ClaimDetail> {
        let (state, statuses) = Self::materialize(&self.log);
        render::claim_detail(&state, &statuses, claim_id)
    }

    // -- Draft creation -----------------------------------------------------

    /// Draft a new claim. Returns the assigned DraftId.
    /// Generates a provisional `ClaimId("draft-{N}")`.
    pub fn draft_claim(
        &mut self,
        author: String,
        body: String,
        kind: ClaimKind,
        parent_id: Option<ClaimId>,
    ) -> DraftId {
        let id = self.alloc_draft_id();
        let claim_id = ClaimId(format!("draft-{}", id.0));
        self.drafts.push(DraftEntry {
            id,
            entry: Entry::Claim {
                claim_id,
                author,
                body,
                claim_kind: kind,
                parent_id,
            },
        });
        id
    }

    /// Draft a relation between two claims.
    pub fn draft_relation(
        &mut self,
        source_id: ClaimId,
        target_id: ClaimId,
        kind: RelationKind,
        author: String,
    ) -> DraftId {
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: Entry::Relation {
                source_id,
                target_id,
                kind,
                author,
            },
        });
        id
    }

    /// Draft a stance on a claim.
    pub fn draft_stance(
        &mut self,
        target_id: ClaimId,
        author: String,
        position: Position,
    ) -> DraftId {
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: Entry::Stance {
                target_id,
                author,
                position,
            },
        });
        id
    }

    /// Draft a resolution for a claim.
    pub fn draft_resolve(
        &mut self,
        claim_id: ClaimId,
        author: String,
        outcome: Outcome,
    ) -> DraftId {
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: Entry::Resolve {
                claim_id,
                author,
                outcome,
            },
        });
        id
    }

    // -- Draft management ---------------------------------------------------

    /// All pending drafts, in creation order.
    pub fn show_drafts(&self) -> &[DraftEntry] {
        &self.drafts
    }

    /// Remove a draft by id. Returns error if not found.
    pub fn remove_draft(&mut self, id: DraftId) -> Result<(), EngineError> {
        let pos = self
            .drafts
            .iter()
            .position(|d| d.id == id)
            .ok_or(EngineError::DraftNotFound(id))?;
        self.drafts.remove(pos);
        Ok(())
    }

    /// Drain all drafts and return their entries for submission.
    pub fn submit_drafts(&mut self) -> Vec<Entry> {
        self.drafts.drain(..).map(|d| d.entry).collect()
    }

    /// Discard all drafts.
    pub fn clear_drafts(&mut self) {
        self.drafts.clear();
    }

    // -- Preview (committed + drafts) ---------------------------------------

    /// Overview including uncommitted drafts.
    pub fn preview_overview(&self) -> OverviewData {
        let merged = self.merged_entries();
        let (state, statuses) = Self::materialize(&merged);
        render::overview(&state, &statuses)
    }

    /// Claim detail including uncommitted drafts.
    pub fn preview_claim_detail(&self, claim_id: &ClaimId) -> Option<ClaimDetail> {
        let merged = self.merged_entries();
        let (state, statuses) = Self::materialize(&merged);
        render::claim_detail(&state, &statuses, claim_id)
    }

    // -- Internal -----------------------------------------------------------

    fn alloc_draft_id(&mut self) -> DraftId {
        let id = DraftId(self.next_draft_id);
        self.next_draft_id += 1;
        id
    }

    fn merged_entries(&self) -> Vec<Entry> {
        self.log
            .iter()
            .chain(self.drafts.iter().map(|d| &d.entry))
            .cloned()
            .collect()
    }

    /// Run the full pipeline: replay → graph → solve → status.
    fn materialize(entries: &[Entry]) -> (MaterializedState, HashMap<ClaimId, EpistemicStatus>) {
        let state = replay(entries);
        let (graph, index) = to_graph(&state);
        let labels = grounded_labelling(&graph);
        let statuses = compute_all(&state, &labels, &index);
        (state, statuses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::types::*;

    #[test]
    fn new_engine_is_empty() {
        let engine = ConsensusEngine::new();
        assert!(engine.log().is_empty());
    }

    #[test]
    fn append_grows_log() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "A claim".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        assert_eq!(engine.log().len(), 1);
        engine.append(Entry::Comment {
            author: "bob".into(),
            body: "Interesting".into(),
        });
        assert_eq!(engine.log().len(), 2);
    }

    #[test]
    fn overview_empty() {
        let engine = ConsensusEngine::new();
        let data = engine.overview();
        assert_eq!(data.total_claims, 0);
        assert_eq!(data.total_relations, 0);
        assert_eq!(data.total_stances, 0);
        assert!(data.participants.is_empty());
    }

    #[test]
    fn overview_categorizes_claims() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("item1".into()),
            author: "alice".into(),
            body: "Auth approach?".into(),
            claim_kind: ClaimKind::Item,
            parent_id: None,
        });
        engine.append(Entry::Claim {
            claim_id: ClaimId("p1".into()),
            author: "bob".into(),
            body: "Use JWT".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: Some(ClaimId("item1".into())),
        });

        let data = engine.overview();
        assert_eq!(data.total_claims, 2);
        assert_eq!(data.items.len(), 1);
        assert_eq!(data.proposals.len(), 1);
    }

    #[test]
    fn claim_detail_unknown_returns_none() {
        let engine = ConsensusEngine::new();
        assert!(engine.claim_detail(&ClaimId("nope".into())).is_none());
    }

    #[test]
    fn claim_detail_returns_data() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Main claim".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine.append(Entry::Claim {
            claim_id: ClaimId("c2".into()),
            author: "bob".into(),
            body: "Counter".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine.append(Entry::Relation {
            source_id: ClaimId("c2".into()),
            target_id: ClaimId("c1".into()),
            kind: RelationKind::Attacks,
            author: "bob".into(),
        });

        let detail = engine.claim_detail(&ClaimId("c1".into())).unwrap();
        assert_eq!(detail.claim.id, ClaimId("c1".into()));
        assert_eq!(detail.attacked_by.len(), 1);
    }

    // -- Draft tests --------------------------------------------------------

    #[test]
    fn draft_claim_assigns_unique_id() {
        let mut engine = ConsensusEngine::new();
        let id1 = engine.draft_claim("alice".into(), "Claim A".into(), ClaimKind::Fact, None);
        let id2 = engine.draft_claim("bob".into(), "Claim B".into(), ClaimKind::Proposal, None);
        assert_ne!(id1, id2);
        assert_eq!(engine.show_drafts().len(), 2);
    }

    #[test]
    fn draft_claim_generates_provisional_id() {
        let mut engine = ConsensusEngine::new();
        let draft_id = engine.draft_claim("alice".into(), "A claim".into(), ClaimKind::Fact, None);
        let draft = &engine.show_drafts()[0];
        assert_eq!(draft.id, draft_id);
        if let Entry::Claim { ref claim_id, .. } = draft.entry {
            assert_eq!(claim_id.0, format!("draft-{}", draft_id.0));
        } else {
            panic!("expected Claim entry");
        }
    }

    #[test]
    fn draft_relation_stance_resolve() {
        let mut engine = ConsensusEngine::new();
        engine.draft_relation(
            ClaimId("c1".into()),
            ClaimId("c2".into()),
            RelationKind::Attacks,
            "alice".into(),
        );
        engine.draft_stance(ClaimId("c1".into()), "bob".into(), Position::Consent);
        engine.draft_resolve(ClaimId("c1".into()), "alice".into(), Outcome::Accepted);
        assert_eq!(engine.show_drafts().len(), 3);
    }

    #[test]
    fn show_drafts_preserves_order() {
        let mut engine = ConsensusEngine::new();
        let id1 = engine.draft_claim("a".into(), "First".into(), ClaimKind::Fact, None);
        let id2 = engine.draft_claim("b".into(), "Second".into(), ClaimKind::Fact, None);
        let ids: Vec<DraftId> = engine.show_drafts().iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![id1, id2]);
    }

    #[test]
    fn remove_draft_succeeds() {
        let mut engine = ConsensusEngine::new();
        let id1 = engine.draft_claim("a".into(), "First".into(), ClaimKind::Fact, None);
        let _id2 = engine.draft_claim("b".into(), "Second".into(), ClaimKind::Fact, None);
        engine.remove_draft(id1).unwrap();
        assert_eq!(engine.show_drafts().len(), 1);
        assert_eq!(engine.show_drafts()[0].id, _id2);
    }

    #[test]
    fn remove_draft_not_found() {
        let mut engine = ConsensusEngine::new();
        let result = engine.remove_draft(DraftId(999));
        assert_eq!(result, Err(EngineError::DraftNotFound(DraftId(999))));
    }

    #[test]
    fn submit_drafts_drains_buffer() {
        let mut engine = ConsensusEngine::new();
        engine.draft_claim("alice".into(), "A".into(), ClaimKind::Fact, None);
        engine.draft_stance(ClaimId("c1".into()), "bob".into(), Position::Block);
        let entries = engine.submit_drafts();
        assert_eq!(entries.len(), 2);
        assert!(engine.show_drafts().is_empty());
    }

    #[test]
    fn submit_empty_returns_empty() {
        let mut engine = ConsensusEngine::new();
        assert!(engine.submit_drafts().is_empty());
    }

    #[test]
    fn clear_drafts_empties_buffer() {
        let mut engine = ConsensusEngine::new();
        engine.draft_claim("a".into(), "A".into(), ClaimKind::Fact, None);
        engine.draft_claim("b".into(), "B".into(), ClaimKind::Fact, None);
        engine.clear_drafts();
        assert!(engine.show_drafts().is_empty());
    }

    #[test]
    fn preview_includes_drafts() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Committed".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine.draft_claim("bob".into(), "Drafted".into(), ClaimKind::Proposal, None);

        let committed = engine.overview();
        let preview = engine.preview_overview();
        assert_eq!(committed.total_claims, 1);
        assert_eq!(preview.total_claims, 2);
        assert_eq!(preview.proposals.len(), 1);
    }

    #[test]
    fn drafts_do_not_leak_into_committed() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Committed".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine.draft_claim("bob".into(), "Ghost".into(), ClaimKind::Fact, None);

        let committed = engine.overview();
        assert_eq!(committed.total_claims, 1);
    }

    #[test]
    fn preview_claim_detail_shows_draft_relations() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Target".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        let attacker_id =
            engine.draft_claim("bob".into(), "Attacker".into(), ClaimKind::Fact, None);
        // Get the provisional claim id
        let attacker_claim_id = if let Entry::Claim { ref claim_id, .. } = engine
            .show_drafts()
            .iter()
            .find(|d| d.id == attacker_id)
            .unwrap()
            .entry
        {
            claim_id.clone()
        } else {
            panic!("expected claim");
        };
        engine.draft_relation(
            attacker_claim_id,
            ClaimId("c1".into()),
            RelationKind::Attacks,
            "bob".into(),
        );

        let detail = engine.preview_claim_detail(&ClaimId("c1".into())).unwrap();
        assert_eq!(detail.attacked_by.len(), 1);
    }

    // -- Integration tests --------------------------------------------------

    #[test]
    fn integration_full_deliberation() {
        let mut engine = ConsensusEngine::new();

        // Item
        engine.append(Entry::Claim {
            claim_id: ClaimId("item1".into()),
            author: "alice".into(),
            body: "Auth approach?".into(),
            claim_kind: ClaimKind::Item,
            parent_id: None,
        });

        // Two proposals
        engine.append(Entry::Claim {
            claim_id: ClaimId("p1".into()),
            author: "alice".into(),
            body: "Use JWT".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: Some(ClaimId("item1".into())),
        });
        engine.append(Entry::Claim {
            claim_id: ClaimId("p2".into()),
            author: "bob".into(),
            body: "Use cookies".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: Some(ClaimId("item1".into())),
        });

        // Stances on p1
        engine.append(Entry::Stance {
            target_id: ClaimId("p1".into()),
            author: "bob".into(),
            position: Position::Consent,
        });
        engine.append(Entry::Stance {
            target_id: ClaimId("p1".into()),
            author: "carol".into(),
            position: Position::Consent,
        });

        // Contested p2
        engine.append(Entry::Stance {
            target_id: ClaimId("p2".into()),
            author: "alice".into(),
            position: Position::Block,
        });

        // Resolve p1
        engine.append(Entry::Resolve {
            claim_id: ClaimId("p1".into()),
            author: "alice".into(),
            outcome: Outcome::Accepted,
        });

        let data = engine.overview();
        assert_eq!(data.total_claims, 3);
        assert_eq!(data.items.len(), 1);
        assert_eq!(data.proposals.len(), 1); // p2 still active
        assert_eq!(data.resolved.len(), 1); // p1 resolved
        assert!(data.participants.len() >= 3); // alice, bob, carol
    }
}
