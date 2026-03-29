//! Stateful consensus engine.
//!
//! Owns the entry log and runs the full pipeline (reduce → graph → solve →
//! status → render) on every query. Manages a draft buffer for accumulating
//! proposed entries before submission.

use std::collections::HashMap;

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

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

/// Reference to a claim from draft-local state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClaimRef {
    Committed(ClaimId),
    Draft(DraftId),
}

impl Serialize for ClaimRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            ClaimRef::Committed(claim_id) => map.serialize_entry("claim_id", claim_id)?,
            ClaimRef::Draft(draft_id) => map.serialize_entry("draft_id", draft_id)?,
        }
        map.end()
    }
}

/// Draft-local content awaiting submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DraftContent {
    Claim {
        body: String,
        claim_kind: ClaimKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<ClaimRef>,
    },
    Relation {
        source: ClaimRef,
        target: ClaimRef,
        kind: RelationKind,
    },
    Stance {
        target: ClaimRef,
        position: Position,
    },
    Resolve {
        claim: ClaimRef,
        outcome: Outcome,
    },
    Comment {
        #[serde(skip_serializing_if = "Option::is_none")]
        claim: Option<ClaimRef>,
        body: String,
    },
}

/// A draft entry awaiting submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DraftEntry {
    pub id: DraftId,
    pub entry: DraftContent,
}

/// Errors that the engine can produce.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("draft not found: {0:?}")]
    DraftNotFound(DraftId),
    #[error("draft reference must target a claim: {0:?}")]
    DraftReferenceMustTargetClaim(DraftId),
    #[error("cannot remove draft {draft_id:?} because draft {referenced_by:?} depends on it")]
    DraftReferenced {
        draft_id: DraftId,
        referenced_by: DraftId,
    },
}

/// Summary of a newly introduced claim in impact analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactNewClaim {
    pub draft_id: DraftId,
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
    pub draft_id: DraftId,
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
    draft_author: String,
}

impl ConsensusEngine {
    /// Create an empty engine.
    pub fn new(draft_author: String) -> Self {
        Self {
            log: Vec::new(),
            drafts: Vec::new(),
            next_draft_id: 0,
            draft_author,
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
    pub fn draft_claim(
        &mut self,
        body: String,
        kind: ClaimKind,
        parent: Option<ClaimRef>,
    ) -> Result<DraftId, EngineError> {
        self.validate_optional_claim_ref(parent.as_ref())?;
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: DraftContent::Claim {
                body,
                claim_kind: kind,
                parent,
            },
        });
        Ok(id)
    }

    /// Draft a relation between two claims.
    pub fn draft_relation(
        &mut self,
        source: ClaimRef,
        target: ClaimRef,
        kind: RelationKind,
    ) -> Result<DraftId, EngineError> {
        self.validate_claim_ref(&source)?;
        self.validate_claim_ref(&target)?;
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: DraftContent::Relation {
                source,
                target,
                kind,
            },
        });
        Ok(id)
    }

    /// Draft a stance on a claim.
    pub fn draft_stance(
        &mut self,
        target: ClaimRef,
        position: Position,
    ) -> Result<DraftId, EngineError> {
        self.validate_claim_ref(&target)?;
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: DraftContent::Stance { target, position },
        });
        Ok(id)
    }

    /// Draft a resolution for a claim.
    pub fn draft_resolve(
        &mut self,
        claim: ClaimRef,
        outcome: Outcome,
    ) -> Result<DraftId, EngineError> {
        self.validate_claim_ref(&claim)?;
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: DraftContent::Resolve { claim, outcome },
        });
        Ok(id)
    }

    /// Draft a freeform comment, optionally attached to a claim.
    pub fn draft_comment(
        &mut self,
        body: String,
        claim: Option<ClaimRef>,
    ) -> Result<DraftId, EngineError> {
        self.validate_optional_claim_ref(claim.as_ref())?;
        let id = self.alloc_draft_id();
        self.drafts.push(DraftEntry {
            id,
            entry: DraftContent::Comment { claim, body },
        });
        Ok(id)
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
        let removed_is_claim = matches!(self.drafts[pos].entry, DraftContent::Claim { .. });
        if removed_is_claim
            && let Some(referenced_by) = self
                .drafts
                .iter()
                .filter(|draft| draft.id != id)
                .find(|draft| Self::draft_references_draft(&draft.entry, id))
                .map(|draft| draft.id)
        {
            return Err(EngineError::DraftReferenced {
                draft_id: id,
                referenced_by,
            });
        }
        self.drafts.remove(pos);
        Ok(())
    }

    /// Drain all drafts and return their entries for submission.
    pub fn submit_drafts(&mut self) -> Vec<Entry> {
        let entries: Vec<Entry> = self
            .drafts
            .iter()
            .map(|draft| {
                self.materialize_draft_for_preview(draft)
                    .expect("draft refs are validated before submission")
            })
            .collect();
        self.drafts.clear();
        entries
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
    pub fn preview_claim_detail(&self, claim: &ClaimRef) -> Option<ClaimDetail> {
        let claim_id = self
            .resolve_claim_ref_for_preview(claim)
            .expect("draft refs are validated before preview");
        let merged = self.merged_entries();
        let (state, statuses) = Self::materialize(&merged);
        render::claim_detail(&state, &statuses, &claim_id)
    }

    /// Compare committed state with committed + drafts.
    pub fn impact_analysis(&self) -> ImpactAnalysis {
        let (committed_state, committed_statuses) = Self::materialize(&self.log);
        let merged = self.merged_entries();
        let (preview_state, preview_statuses) = Self::materialize(&merged);
        Self::build_impact_analysis(
            &self.drafts,
            &self.draft_author,
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
        let mut claim_id_map: HashMap<DraftId, ClaimId> = HashMap::new();

        for draft in &self.drafts {
            if matches!(draft.entry, DraftContent::Claim { .. }) {
                claim_id_map
                    .entry(draft.id)
                    .or_insert_with(&mut next_claim_id);
            }
        }

        let mut draft_ids = Vec::with_capacity(self.drafts.len());
        let entries = self
            .drafts
            .iter()
            .map(|draft| {
                draft_ids.push(draft.id);
                self.materialize_draft_for_submission(draft, &claim_id_map)
                    .expect("draft refs are validated before submission")
            })
            .collect();

        let mut claim_id_map: Vec<ClaimIdMapping> = claim_id_map
            .into_iter()
            .map(|(draft_id, final_id)| ClaimIdMapping { draft_id, final_id })
            .collect();
        claim_id_map.sort_by_key(|mapping| mapping.draft_id.0);

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
        let mut merged = self.log.clone();
        merged.extend(
            self.drafts
                .iter()
                .map(|draft| {
                    self.materialize_draft_for_preview(draft)
                        .expect("draft refs are validated before preview")
                })
                .collect::<Vec<_>>(),
        );
        merged
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
        drafts: &[DraftEntry],
        draft_author: &str,
        committed_state: &MaterializedState,
        committed_statuses: &HashMap<ClaimId, EpistemicStatus>,
        preview_state: &MaterializedState,
        preview_statuses: &HashMap<ClaimId, EpistemicStatus>,
    ) -> ImpactAnalysis {
        let mut new_claims = Vec::new();
        for draft in drafts {
            if let DraftContent::Claim {
                body, claim_kind, ..
            } = &draft.entry
            {
                let claim_id = Self::draft_claim_preview_id(draft.id);
                new_claims.push(ImpactNewClaim {
                    draft_id: draft.id,
                    body: body.clone(),
                    author: draft_author.to_owned(),
                    kind: *claim_kind,
                    status: preview_statuses.get(&claim_id).copied(),
                });
            }
        }
        new_claims.sort_by_key(|claim| claim.draft_id.0);

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

    fn draft_claim_preview_id(draft_id: DraftId) -> ClaimId {
        ClaimId(format!("draft-{}", draft_id.0))
    }

    fn validate_optional_claim_ref(&self, claim_ref: Option<&ClaimRef>) -> Result<(), EngineError> {
        if let Some(claim_ref) = claim_ref {
            self.validate_claim_ref(claim_ref)?;
        }
        Ok(())
    }

    fn validate_claim_ref(&self, claim_ref: &ClaimRef) -> Result<(), EngineError> {
        match claim_ref {
            ClaimRef::Committed(_) => Ok(()),
            ClaimRef::Draft(draft_id) => match self.find_draft(*draft_id) {
                Some(DraftEntry {
                    entry: DraftContent::Claim { .. },
                    ..
                }) => Ok(()),
                Some(_) => Err(EngineError::DraftReferenceMustTargetClaim(*draft_id)),
                None => Err(EngineError::DraftNotFound(*draft_id)),
            },
        }
    }

    fn resolve_claim_ref_for_preview(&self, claim_ref: &ClaimRef) -> Result<ClaimId, EngineError> {
        match claim_ref {
            ClaimRef::Committed(claim_id) => Ok(claim_id.clone()),
            ClaimRef::Draft(draft_id) => {
                self.validate_claim_ref(claim_ref)?;
                Ok(Self::draft_claim_preview_id(*draft_id))
            }
        }
    }

    fn resolve_claim_ref_for_submission(
        &self,
        claim_ref: &ClaimRef,
        claim_id_map: &HashMap<DraftId, ClaimId>,
    ) -> Result<ClaimId, EngineError> {
        match claim_ref {
            ClaimRef::Committed(claim_id) => Ok(claim_id.clone()),
            ClaimRef::Draft(draft_id) => {
                self.validate_claim_ref(claim_ref)?;
                claim_id_map
                    .get(draft_id)
                    .cloned()
                    .ok_or(EngineError::DraftNotFound(*draft_id))
            }
        }
    }

    fn materialize_draft_for_preview(&self, draft: &DraftEntry) -> Result<Entry, EngineError> {
        self.materialize_draft(
            draft,
            |claim_ref| self.resolve_claim_ref_for_preview(claim_ref),
            |draft_id| Ok(Self::draft_claim_preview_id(draft_id)),
        )
    }

    fn materialize_draft_for_submission(
        &self,
        draft: &DraftEntry,
        claim_id_map: &HashMap<DraftId, ClaimId>,
    ) -> Result<Entry, EngineError> {
        self.materialize_draft(
            draft,
            |claim_ref| self.resolve_claim_ref_for_submission(claim_ref, claim_id_map),
            |draft_id| {
                claim_id_map
                    .get(&draft_id)
                    .cloned()
                    .ok_or(EngineError::DraftNotFound(draft_id))
            },
        )
    }

    fn materialize_draft<F, G>(
        &self,
        draft: &DraftEntry,
        resolve_ref: F,
        resolve_own_claim_id: G,
    ) -> Result<Entry, EngineError>
    where
        F: Fn(&ClaimRef) -> Result<ClaimId, EngineError>,
        G: Fn(DraftId) -> Result<ClaimId, EngineError>,
    {
        match &draft.entry {
            DraftContent::Claim {
                body,
                claim_kind,
                parent,
            } => Ok(Entry::Claim {
                claim_id: resolve_own_claim_id(draft.id)?,
                author: self.draft_author.clone(),
                body: body.clone(),
                claim_kind: *claim_kind,
                parent_id: parent.as_ref().map(&resolve_ref).transpose()?,
            }),
            DraftContent::Relation {
                source,
                target,
                kind,
            } => Ok(Entry::Relation {
                source_id: resolve_ref(source)?,
                target_id: resolve_ref(target)?,
                kind: *kind,
                author: self.draft_author.clone(),
            }),
            DraftContent::Stance { target, position } => Ok(Entry::Stance {
                target_id: resolve_ref(target)?,
                author: self.draft_author.clone(),
                position: *position,
            }),
            DraftContent::Resolve { claim, outcome } => Ok(Entry::Resolve {
                claim_id: resolve_ref(claim)?,
                author: self.draft_author.clone(),
                outcome: *outcome,
            }),
            DraftContent::Comment { claim, body } => Ok(Entry::Comment {
                claim_id: claim.as_ref().map(resolve_ref).transpose()?,
                author: self.draft_author.clone(),
                body: body.clone(),
            }),
        }
    }

    fn find_draft(&self, draft_id: DraftId) -> Option<&DraftEntry> {
        self.drafts.iter().find(|draft| draft.id == draft_id)
    }

    fn draft_references_draft(entry: &DraftContent, draft_id: DraftId) -> bool {
        let references =
            |claim_ref: &ClaimRef| matches!(claim_ref, ClaimRef::Draft(id) if *id == draft_id);
        match entry {
            DraftContent::Claim { parent, .. } => parent.as_ref().is_some_and(references),
            DraftContent::Relation { source, target, .. } => {
                references(source) || references(target)
            }
            DraftContent::Stance { target, .. } => references(target),
            DraftContent::Resolve { claim, .. } => references(claim),
            DraftContent::Comment { claim, .. } => claim.as_ref().is_some_and(references),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::types::*;

    fn engine() -> ConsensusEngine {
        ConsensusEngine::new(String::from("assistant"))
    }

    #[test]
    fn new_engine_is_empty() {
        let engine = engine();
        assert!(engine.log().is_empty());
    }

    #[test]
    fn append_grows_log() {
        let mut engine = engine();
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
        let engine = engine();
        let data = engine.overview();
        assert_eq!(data.total_claims, 0);
        assert_eq!(data.total_relations, 0);
        assert_eq!(data.total_stances, 0);
        assert!(data.participants.is_empty());
    }

    #[test]
    fn overview_categorizes_claims() {
        let mut engine = engine();
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
        let engine = engine();
        assert!(engine.claim_detail(&ClaimId("nope".into())).is_none());
    }

    #[test]
    fn claim_detail_returns_data() {
        let mut engine = engine();
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
    fn draft_claim_assigns_unique_id_and_has_no_committed_fields() {
        let mut engine = engine();
        let id1 = engine
            .draft_claim("Claim A".into(), ClaimKind::Fact, None)
            .unwrap();
        let id2 = engine
            .draft_claim("Claim B".into(), ClaimKind::Proposal, None)
            .unwrap();
        assert_ne!(id1, id2);
        assert_eq!(engine.show_drafts().len(), 2);
        let draft = &engine.show_drafts()[0];
        assert_eq!(draft.id, id1);
        assert!(matches!(
            &draft.entry,
            DraftContent::Claim {
                body,
                claim_kind: ClaimKind::Fact,
                parent: None,
            } if body == "Claim A"
        ));
    }

    #[test]
    fn draft_relation_stance_resolve_accept_claim_refs() {
        let mut engine = engine();
        let draft_claim = engine
            .draft_claim("Claim A".into(), ClaimKind::Fact, None)
            .unwrap();
        engine
            .draft_relation(
                ClaimRef::Draft(draft_claim),
                ClaimRef::Committed(ClaimId("c2".into())),
                RelationKind::Attacks,
            )
            .unwrap();
        engine
            .draft_stance(ClaimRef::Draft(draft_claim), Position::Consent)
            .unwrap();
        engine
            .draft_resolve(ClaimRef::Draft(draft_claim), Outcome::Accepted)
            .unwrap();
        assert_eq!(engine.show_drafts().len(), 4);
    }

    #[test]
    fn draft_comment_adds_comment_entry() {
        let mut engine = engine();
        engine
            .draft_comment(
                "Needs more evidence".into(),
                Some(ClaimRef::Committed(ClaimId("c1".into()))),
            )
            .unwrap();
        assert_eq!(engine.show_drafts().len(), 1);
        assert!(matches!(
            &engine.show_drafts()[0].entry,
            DraftContent::Comment {
                claim: Some(ClaimRef::Committed(claim_id)),
                body,
            } if claim_id.0 == "c1" && body == "Needs more evidence"
        ));
    }

    #[test]
    fn show_drafts_preserves_order() {
        let mut engine = engine();
        let id1 = engine
            .draft_claim("First".into(), ClaimKind::Fact, None)
            .unwrap();
        let id2 = engine
            .draft_claim("Second".into(), ClaimKind::Fact, None)
            .unwrap();
        let ids: Vec<DraftId> = engine.show_drafts().iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![id1, id2]);
    }

    #[test]
    fn remove_draft_succeeds() {
        let mut engine = engine();
        let id1 = engine
            .draft_claim("First".into(), ClaimKind::Fact, None)
            .unwrap();
        let id2 = engine
            .draft_claim("Second".into(), ClaimKind::Fact, None)
            .unwrap();
        engine.remove_draft(id1).unwrap();
        assert_eq!(engine.show_drafts().len(), 1);
        assert_eq!(engine.show_drafts()[0].id, id2);
    }

    #[test]
    fn remove_draft_not_found() {
        let mut engine = engine();
        let result = engine.remove_draft(DraftId(999));
        assert_eq!(result, Err(EngineError::DraftNotFound(DraftId(999))));
    }

    #[test]
    fn remove_draft_rejects_when_other_draft_depends_on_it() {
        let mut engine = engine();
        let claim_draft = engine
            .draft_claim("A".into(), ClaimKind::Fact, None)
            .unwrap();
        let dependent = engine
            .draft_stance(ClaimRef::Draft(claim_draft), Position::Block)
            .unwrap();
        let result = engine.remove_draft(claim_draft);
        assert_eq!(
            result,
            Err(EngineError::DraftReferenced {
                draft_id: claim_draft,
                referenced_by: dependent,
            })
        );
    }

    #[test]
    fn invalid_draft_refs_fail_immediately() {
        let mut engine = engine();
        let comment_draft = engine.draft_comment("Note".into(), None).unwrap();
        assert_eq!(
            engine.draft_stance(ClaimRef::Draft(comment_draft), Position::Consent),
            Err(EngineError::DraftReferenceMustTargetClaim(comment_draft))
        );
        assert_eq!(
            engine.draft_relation(
                ClaimRef::Draft(DraftId(999)),
                ClaimRef::Committed(ClaimId("c1".into())),
                RelationKind::Supports,
            ),
            Err(EngineError::DraftNotFound(DraftId(999)))
        );
    }

    #[test]
    fn submit_drafts_drains_buffer_and_materializes_author() {
        let mut engine = engine();
        let draft_claim = engine
            .draft_claim("A".into(), ClaimKind::Fact, None)
            .unwrap();
        engine
            .draft_stance(ClaimRef::Draft(draft_claim), Position::Block)
            .unwrap();
        let entries = engine.submit_drafts();
        assert_eq!(entries.len(), 2);
        assert!(engine.show_drafts().is_empty());
        assert!(matches!(
            &entries[0],
            Entry::Claim { claim_id, author, body, .. }
                if claim_id == &ClaimId("draft-0".into()) && author == "assistant" && body == "A"
        ));
        assert!(matches!(
            &entries[1],
            Entry::Stance { target_id, author, position }
                if target_id == &ClaimId("draft-0".into())
                    && author == "assistant"
                    && *position == Position::Block
        ));
    }

    #[test]
    fn submit_empty_returns_empty() {
        let mut engine = engine();
        assert!(engine.submit_drafts().is_empty());
    }

    #[test]
    fn clear_drafts_empties_buffer() {
        let mut engine = engine();
        engine
            .draft_claim("A".into(), ClaimKind::Fact, None)
            .unwrap();
        engine
            .draft_claim("B".into(), ClaimKind::Fact, None)
            .unwrap();
        engine.clear_drafts();
        assert!(engine.show_drafts().is_empty());
    }

    #[test]
    fn preview_includes_drafts() {
        let mut engine = engine();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Committed".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine
            .draft_claim("Drafted".into(), ClaimKind::Proposal, None)
            .unwrap();

        let committed = engine.overview();
        let preview = engine.preview_overview();
        assert_eq!(committed.total_claims, 1);
        assert_eq!(preview.total_claims, 2);
        assert_eq!(preview.proposals.len(), 1);
    }

    #[test]
    fn drafts_do_not_leak_into_committed() {
        let mut engine = engine();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Committed".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine
            .draft_claim("Ghost".into(), ClaimKind::Fact, None)
            .unwrap();

        let committed = engine.overview();
        assert_eq!(committed.total_claims, 1);
    }

    #[test]
    fn preview_claim_detail_shows_draft_relations() {
        let mut engine = engine();
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Target".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        let attacker_id = engine
            .draft_claim("Attacker".into(), ClaimKind::Fact, None)
            .unwrap();
        engine
            .draft_relation(
                ClaimRef::Draft(attacker_id),
                ClaimRef::Committed(ClaimId("c1".into())),
                RelationKind::Attacks,
            )
            .unwrap();

        let detail = engine
            .preview_claim_detail(&ClaimRef::Committed(ClaimId("c1".into())))
            .unwrap();
        assert_eq!(detail.attacked_by.len(), 1);
    }

    #[test]
    fn impact_analysis_reports_new_claims_and_status_changes() {
        let mut engine = engine();
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

        let draft_id = engine
            .draft_claim("New fact".into(), ClaimKind::Fact, None)
            .unwrap();
        assert_eq!(draft_id, DraftId(0));
        engine
            .draft_relation(
                ClaimRef::Committed(ClaimId("c2".into())),
                ClaimRef::Committed(ClaimId("c1".into())),
                RelationKind::Attacks,
            )
            .unwrap();

        let impact = engine.impact_analysis();
        assert_eq!(impact.new_claims.len(), 1);
        assert_eq!(impact.new_claims[0].draft_id, draft_id);
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
    fn submission_bundle_rewrites_draft_refs_consistently() {
        let mut engine = engine();
        engine.append(Entry::Claim {
            claim_id: ClaimId("item1".into()),
            author: "alice".into(),
            body: "What should we do?".into(),
            claim_kind: ClaimKind::Item,
            parent_id: None,
        });
        let draft_claim_id = engine
            .draft_claim(
                "Use JWT".into(),
                ClaimKind::Proposal,
                Some(ClaimRef::Committed(ClaimId("item1".into()))),
            )
            .unwrap();
        engine
            .draft_stance(ClaimRef::Draft(draft_claim_id), Position::Consent)
            .unwrap();
        engine
            .draft_comment("Looks good".into(), Some(ClaimRef::Draft(draft_claim_id)))
            .unwrap();

        let mut ids = vec!["final-1", "final-2"].into_iter();
        let bundle = engine.submission_bundle(|| ClaimId(ids.next().unwrap().into()));

        assert_eq!(bundle.claim_id_map.len(), 1);
        assert_eq!(bundle.claim_id_map[0].draft_id, draft_claim_id);
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
        let mut engine = engine();

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
