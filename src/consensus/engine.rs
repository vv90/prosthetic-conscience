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

/// Summary of a newly introduced claim in impact analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactNewClaim {
    pub claim_id: ClaimId,
    pub body: String,
    pub author: String,
    pub kind: ClaimKind,
    pub status: Option<EpistemicStatus>,
}

/// A before/after status transition caused by the current drafts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactStatusChange {
    pub claim_id: ClaimId,
    pub body: String,
    pub before: Option<EpistemicStatus>,
    pub after: Option<EpistemicStatus>,
}

/// Impact of applying the current drafts to the committed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactAnalysis {
    pub new_claims: Vec<ImpactNewClaim>,
    pub status_changes: Vec<ImpactStatusChange>,
}

/// Mapping from a provisional draft claim id to its final committed id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaimIdMapping {
    pub provisional: ClaimId,
    pub final_id: ClaimId,
}

/// Finalized draft entries ready for network submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmissionBundle {
    pub draft_ids: Vec<DraftId>,
    pub entries: Vec<Entry>,
    pub claim_id_map: Vec<ClaimIdMapping>,
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

    /// Draft a freeform comment, optionally attached to a claim.
    pub fn draft_comment(
        &mut self,
        author: String,
        body: String,
        claim_id: Option<ClaimId>,
    ) -> DraftId {
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: Entry::Comment {
                claim_id,
                author,
                body,
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

    /// Compare committed state with committed + drafts.
    pub fn impact_analysis(&self) -> ImpactAnalysis {
        let (committed_state, committed_statuses) = Self::materialize(&self.log);
        let merged = self.merged_entries();
        let (preview_state, preview_statuses) = Self::materialize(&merged);
        Self::build_impact_analysis(
            &committed_state,
            &committed_statuses,
            &preview_state,
            &preview_statuses,
        )
    }

    /// Clone the current drafts into a finalized submission bundle.
    ///
    /// Draft claim ids are rewritten using `next_claim_id`, while references to
    /// existing committed claim ids are preserved.
    pub fn submission_bundle<F>(&self, mut next_claim_id: F) -> SubmissionBundle
    where
        F: FnMut() -> ClaimId,
    {
        let mut claim_id_map: HashMap<ClaimId, ClaimId> = HashMap::new();

        for draft in &self.drafts {
            if let Entry::Claim { claim_id, .. } = &draft.entry {
                claim_id_map
                    .entry(claim_id.clone())
                    .or_insert_with(&mut next_claim_id);
            }
        }

        let mut draft_ids = Vec::with_capacity(self.drafts.len());
        let entries = self
            .drafts
            .iter()
            .map(|draft| {
                draft_ids.push(draft.id);
                Self::rewrite_entry_claim_ids(&draft.entry, &claim_id_map)
            })
            .collect();

        let mut claim_id_map: Vec<ClaimIdMapping> = claim_id_map
            .into_iter()
            .map(|(provisional, final_id)| ClaimIdMapping {
                provisional,
                final_id,
            })
            .collect();
        claim_id_map.sort_by(|a, b| a.provisional.0.cmp(&b.provisional.0));

        SubmissionBundle {
            draft_ids,
            entries,
            claim_id_map,
        }
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

    fn build_impact_analysis(
        committed_state: &MaterializedState,
        committed_statuses: &HashMap<ClaimId, EpistemicStatus>,
        preview_state: &MaterializedState,
        preview_statuses: &HashMap<ClaimId, EpistemicStatus>,
    ) -> ImpactAnalysis {
        let mut new_claims = Vec::new();
        for (claim_id, claim) in &preview_state.claims {
            if !committed_state.claims.contains_key(claim_id) {
                new_claims.push(ImpactNewClaim {
                    claim_id: claim_id.clone(),
                    body: claim.body.clone(),
                    author: claim.author.clone(),
                    kind: claim.kind,
                    status: preview_statuses.get(claim_id).copied(),
                });
            }
        }
        new_claims.sort_by(|a, b| a.claim_id.0.cmp(&b.claim_id.0));

        let mut claim_ids: Vec<ClaimId> = committed_state
            .claims
            .keys()
            .chain(preview_state.claims.keys())
            .cloned()
            .collect();
        claim_ids.sort_by(|a, b| a.0.cmp(&b.0));
        claim_ids.dedup_by(|a, b| a.0 == b.0);

        let mut status_changes = Vec::new();
        for claim_id in claim_ids {
            if !committed_state.claims.contains_key(&claim_id) {
                continue;
            }

            let before = committed_statuses.get(&claim_id).copied();
            let after = preview_statuses.get(&claim_id).copied();
            if before != after {
                let body = preview_state
                    .claims
                    .get(&claim_id)
                    .or_else(|| committed_state.claims.get(&claim_id))
                    .map(|claim| claim.body.clone())
                    .unwrap_or_default();
                status_changes.push(ImpactStatusChange {
                    claim_id,
                    body,
                    before,
                    after,
                });
            }
        }
        status_changes.sort_by(|a, b| a.claim_id.0.cmp(&b.claim_id.0));

        ImpactAnalysis {
            new_claims,
            status_changes,
        }
    }

    fn rewrite_claim_id(claim_id: &ClaimId, claim_id_map: &HashMap<ClaimId, ClaimId>) -> ClaimId {
        claim_id_map
            .get(claim_id)
            .cloned()
            .unwrap_or_else(|| claim_id.clone())
    }

    fn rewrite_entry_claim_ids(entry: &Entry, claim_id_map: &HashMap<ClaimId, ClaimId>) -> Entry {
        match entry {
            Entry::Claim {
                claim_id,
                author,
                body,
                claim_kind,
                parent_id,
            } => Entry::Claim {
                claim_id: Self::rewrite_claim_id(claim_id, claim_id_map),
                author: author.clone(),
                body: body.clone(),
                claim_kind: *claim_kind,
                parent_id: parent_id
                    .as_ref()
                    .map(|id| Self::rewrite_claim_id(id, claim_id_map)),
            },
            Entry::Relation {
                source_id,
                target_id,
                kind,
                author,
            } => Entry::Relation {
                source_id: Self::rewrite_claim_id(source_id, claim_id_map),
                target_id: Self::rewrite_claim_id(target_id, claim_id_map),
                kind: *kind,
                author: author.clone(),
            },
            Entry::Stance {
                target_id,
                author,
                position,
            } => Entry::Stance {
                target_id: Self::rewrite_claim_id(target_id, claim_id_map),
                author: author.clone(),
                position: *position,
            },
            Entry::Resolve {
                claim_id,
                author,
                outcome,
            } => Entry::Resolve {
                claim_id: Self::rewrite_claim_id(claim_id, claim_id_map),
                author: author.clone(),
                outcome: *outcome,
            },
            Entry::Comment {
                claim_id,
                author,
                body,
            } => Entry::Comment {
                claim_id: claim_id
                    .as_ref()
                    .map(|id| Self::rewrite_claim_id(id, claim_id_map)),
                author: author.clone(),
                body: body.clone(),
            },
        }
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
            claim_id: None,
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
    fn draft_comment_adds_comment_entry() {
        let mut engine = ConsensusEngine::new();
        engine.draft_comment(
            "alice".into(),
            "Needs more evidence".into(),
            Some(ClaimId("c1".into())),
        );
        assert_eq!(engine.show_drafts().len(), 1);
        assert!(matches!(
            &engine.show_drafts()[0].entry,
            Entry::Comment {
                claim_id: Some(claim_id),
                author,
                body,
            } if claim_id.0 == "c1" && author == "alice" && body == "Needs more evidence"
        ));
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

    #[test]
    fn impact_analysis_reports_new_claims_and_status_changes() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Target".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine.append(Entry::Claim {
            claim_id: ClaimId("c2".into()),
            author: "bob".into(),
            body: "Attacker".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });

        let draft_id = engine.draft_claim("carol".into(), "New fact".into(), ClaimKind::Fact, None);
        let draft_claim_id = match &engine.show_drafts()[0].entry {
            Entry::Claim { claim_id, .. } => claim_id.clone(),
            other => panic!("expected claim draft, got {other:?}"),
        };
        assert_eq!(draft_id, DraftId(0));
        engine.draft_relation(
            ClaimId("c2".into()),
            ClaimId("c1".into()),
            RelationKind::Attacks,
            "bob".into(),
        );

        let impact = engine.impact_analysis();
        assert_eq!(impact.new_claims.len(), 1);
        assert_eq!(impact.new_claims[0].claim_id, draft_claim_id);
        assert_eq!(impact.status_changes.len(), 1);
        assert_eq!(impact.status_changes[0].claim_id, ClaimId("c1".into()));
        assert_eq!(
            impact.status_changes[0].before,
            Some(EpistemicStatus::Unexamined)
        );
        assert_eq!(
            impact.status_changes[0].after,
            Some(EpistemicStatus::Defeated)
        );
    }

    #[test]
    fn submission_bundle_rewrites_provisional_ids_consistently() {
        let mut engine = ConsensusEngine::new();
        engine.append(Entry::Claim {
            claim_id: ClaimId("item1".into()),
            author: "alice".into(),
            body: "What should we do?".into(),
            claim_kind: ClaimKind::Item,
            parent_id: None,
        });
        let draft_claim_id = engine.draft_claim(
            "bob".into(),
            "Use JWT".into(),
            ClaimKind::Proposal,
            Some(ClaimId("item1".into())),
        );
        let provisional = match engine
            .show_drafts()
            .iter()
            .find(|draft| draft.id == draft_claim_id)
            .map(|draft| &draft.entry)
            .unwrap()
        {
            Entry::Claim { claim_id, .. } => claim_id.clone(),
            other => panic!("expected claim draft, got {other:?}"),
        };
        engine.draft_stance(provisional.clone(), "carol".into(), Position::Consent);
        engine.draft_comment(
            "dave".into(),
            "Looks good".into(),
            Some(provisional.clone()),
        );

        let mut ids = vec!["final-1", "final-2"].into_iter();
        let bundle = engine.submission_bundle(|| ClaimId(ids.next().unwrap().into()));

        assert_eq!(bundle.claim_id_map.len(), 1);
        assert_eq!(bundle.claim_id_map[0].provisional, provisional);
        assert_eq!(bundle.claim_id_map[0].final_id, ClaimId("final-1".into()));
        assert_eq!(bundle.entries.len(), 3);
        assert!(matches!(
            &bundle.entries[0],
            Entry::Claim { claim_id, parent_id, .. }
                if *claim_id == ClaimId("final-1".into())
                && *parent_id == Some(ClaimId("item1".into()))
        ));
        assert!(matches!(
            &bundle.entries[1],
            Entry::Stance { target_id, .. } if *target_id == ClaimId("final-1".into())
        ));
        assert!(matches!(
            &bundle.entries[2],
            Entry::Comment { claim_id, .. }
                if *claim_id == Some(ClaimId("final-1".into()))
        ));
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
