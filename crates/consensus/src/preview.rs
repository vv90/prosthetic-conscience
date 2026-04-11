//! Shared pure helpers for previewing committed state plus local drafts.

use std::collections::HashMap;

use crate::engine::{
    ClaimRef, DraftContent, DraftEntry, DraftId, EngineError, ImpactAnalysis, ImpactNewClaim,
    ImpactStatusChange, materialize_entries, overview_from_entries,
};
use crate::render::{self, ClaimDetail, OverviewData};
use crate::status::EpistemicStatus;
use crate::types::{ClaimId, Entry};

pub(crate) fn materialize_drafts_for_preview(
    drafts: &[DraftEntry],
    draft_author: &str,
) -> Result<Vec<Entry>, EngineError> {
    drafts
        .iter()
        .map(|draft| materialize_draft_for_preview(drafts, draft_author, draft))
        .collect()
}

pub(crate) fn preview_overview<'a>(
    committed_entries: impl IntoIterator<Item = &'a Entry>,
    drafts: &[DraftEntry],
    draft_author: &str,
) -> Result<OverviewData, EngineError> {
    let merged = merged_entries(committed_entries, drafts, draft_author)?;
    Ok(overview_from_entries(merged.iter()))
}

pub(crate) fn preview_claim_detail<'a>(
    committed_entries: impl IntoIterator<Item = &'a Entry>,
    drafts: &[DraftEntry],
    draft_author: &str,
    claim: &ClaimRef,
) -> Result<Option<ClaimDetail>, EngineError> {
    let claim_id = resolve_claim_ref_for_preview(drafts, claim)?;
    let merged = merged_entries(committed_entries, drafts, draft_author)?;
    let (state, statuses) = materialize_entries(merged.iter());
    Ok(render::claim_detail(&state, &statuses, &claim_id))
}

pub(crate) fn impact_analysis<'a>(
    committed_entries: impl IntoIterator<Item = &'a Entry>,
    drafts: &[DraftEntry],
    draft_author: &str,
) -> Result<ImpactAnalysis, EngineError> {
    let committed_entries = committed_entries.into_iter().cloned().collect::<Vec<_>>();
    let (committed_state, committed_statuses) = materialize_entries(committed_entries.iter());
    let merged = merged_entries(committed_entries.iter(), drafts, draft_author)?;
    let (preview_state, preview_statuses) = materialize_entries(merged.iter());

    Ok(build_impact_analysis(
        drafts,
        draft_author,
        &committed_state,
        &committed_statuses,
        &preview_state,
        &preview_statuses,
    ))
}

fn merged_entries<'a>(
    committed_entries: impl IntoIterator<Item = &'a Entry>,
    drafts: &[DraftEntry],
    draft_author: &str,
) -> Result<Vec<Entry>, EngineError> {
    let mut merged = committed_entries.into_iter().cloned().collect::<Vec<_>>();
    merged.extend(materialize_drafts_for_preview(drafts, draft_author)?);
    Ok(merged)
}

fn build_impact_analysis(
    drafts: &[DraftEntry],
    draft_author: &str,
    committed_state: &crate::types::MaterializedState,
    committed_statuses: &HashMap<ClaimId, EpistemicStatus>,
    preview_state: &crate::types::MaterializedState,
    preview_statuses: &HashMap<ClaimId, EpistemicStatus>,
) -> ImpactAnalysis {
    let mut new_claims = Vec::new();
    for draft in drafts {
        if let DraftContent::Claim {
            body, claim_kind, ..
        } = &draft.entry
        {
            let claim_id = draft_claim_preview_id(draft.id);
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

fn validate_claim_ref(drafts: &[DraftEntry], claim_ref: &ClaimRef) -> Result<(), EngineError> {
    match claim_ref {
        ClaimRef::Committed(_) => Ok(()),
        ClaimRef::Draft(draft_id) => match find_draft(drafts, *draft_id) {
            Some(DraftEntry {
                entry: DraftContent::Claim { .. },
                ..
            }) => Ok(()),
            Some(_) => Err(EngineError::DraftReferenceMustTargetClaim(*draft_id)),
            None => Err(EngineError::DraftNotFound(*draft_id)),
        },
    }
}

fn resolve_claim_ref_for_preview(
    drafts: &[DraftEntry],
    claim_ref: &ClaimRef,
) -> Result<ClaimId, EngineError> {
    match claim_ref {
        ClaimRef::Committed(claim_id) => Ok(claim_id.clone()),
        ClaimRef::Draft(draft_id) => {
            validate_claim_ref(drafts, claim_ref)?;
            Ok(draft_claim_preview_id(*draft_id))
        }
    }
}

fn materialize_draft_for_preview(
    drafts: &[DraftEntry],
    draft_author: &str,
    draft: &DraftEntry,
) -> Result<Entry, EngineError> {
    match &draft.entry {
        DraftContent::Claim {
            body,
            claim_kind,
            parent,
        } => Ok(Entry::Claim {
            claim_id: draft_claim_preview_id(draft.id),
            author: draft_author.to_owned(),
            body: body.clone(),
            claim_kind: *claim_kind,
            parent_id: parent
                .as_ref()
                .map(|claim_ref| resolve_claim_ref_for_preview(drafts, claim_ref))
                .transpose()?,
        }),
        DraftContent::Relation {
            source,
            target,
            kind,
        } => Ok(Entry::Relation {
            source_id: resolve_claim_ref_for_preview(drafts, source)?,
            target_id: resolve_claim_ref_for_preview(drafts, target)?,
            kind: *kind,
            author: draft_author.to_owned(),
        }),
        DraftContent::Stance { target, position } => Ok(Entry::Stance {
            target_id: resolve_claim_ref_for_preview(drafts, target)?,
            author: draft_author.to_owned(),
            position: *position,
        }),
        DraftContent::Resolve { claim, outcome } => Ok(Entry::Resolve {
            claim_id: resolve_claim_ref_for_preview(drafts, claim)?,
            author: draft_author.to_owned(),
            outcome: *outcome,
        }),
        DraftContent::Comment { claim, body } => Ok(Entry::Comment {
            claim_id: claim
                .as_ref()
                .map(|claim_ref| resolve_claim_ref_for_preview(drafts, claim_ref))
                .transpose()?,
            author: draft_author.to_owned(),
            body: body.clone(),
        }),
    }
}

fn find_draft(drafts: &[DraftEntry], draft_id: DraftId) -> Option<&DraftEntry> {
    drafts.iter().find(|draft| draft.id == draft_id)
}

#[cfg(test)]
mod tests {
    use crate::engine::{ClaimRef, DraftContent, DraftEntry, DraftId, ImpactStatusChange};
    use crate::status::EpistemicStatus;
    use crate::types::{ClaimId, ClaimKind, Entry, RelationKind};

    use super::*;

    #[test]
    fn preview_overview_includes_uncommitted_claims() {
        let committed = vec![Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Committed".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        }];
        let drafts = vec![DraftEntry {
            id: DraftId(0),
            entry: DraftContent::Claim {
                body: "Draft".into(),
                claim_kind: ClaimKind::Proposal,
                parent: None,
            },
        }];

        let overview = preview_overview(committed.iter(), &drafts, "assistant").unwrap();

        assert_eq!(overview.total_claims, 2);
        assert_eq!(overview.proposals.len(), 1);
        assert_eq!(overview.proposals[0].id, ClaimId("draft-0".into()));
    }

    #[test]
    fn preview_claim_detail_accepts_draft_reference() {
        let drafts = vec![DraftEntry {
            id: DraftId(0),
            entry: DraftContent::Claim {
                body: "Draft".into(),
                claim_kind: ClaimKind::Fact,
                parent: None,
            },
        }];

        let detail = preview_claim_detail(
            std::iter::empty(),
            &drafts,
            "assistant",
            &ClaimRef::Draft(DraftId(0)),
        )
        .unwrap()
        .unwrap();

        assert_eq!(detail.claim.id, ClaimId("draft-0".into()));
        assert_eq!(detail.claim.body, "Draft");
    }

    #[test]
    fn impact_analysis_reports_new_claims_and_status_changes() {
        let committed = vec![
            Entry::Claim {
                claim_id: ClaimId("c1".into()),
                author: "alice".into(),
                body: "Target".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
            Entry::Claim {
                claim_id: ClaimId("c2".into()),
                author: "bob".into(),
                body: "Attacker".into(),
                claim_kind: ClaimKind::Fact,
                parent_id: None,
            },
        ];
        let drafts = vec![
            DraftEntry {
                id: DraftId(0),
                entry: DraftContent::Claim {
                    body: "New fact".into(),
                    claim_kind: ClaimKind::Fact,
                    parent: None,
                },
            },
            DraftEntry {
                id: DraftId(1),
                entry: DraftContent::Relation {
                    source: ClaimRef::Committed(ClaimId("c2".into())),
                    target: ClaimRef::Committed(ClaimId("c1".into())),
                    kind: RelationKind::Attacks,
                },
            },
        ];

        let impact = impact_analysis(committed.iter(), &drafts, "assistant").unwrap();

        assert_eq!(impact.new_claims.len(), 1);
        assert_eq!(impact.new_claims[0].draft_id, DraftId(0));
        assert_eq!(
            impact.status_changes,
            vec![ImpactStatusChange {
                claim_id: ClaimId("c1".into()),
                body: "Target".into(),
                before: Some(EpistemicStatus::Unexamined),
                after: Some(EpistemicStatus::Defeated),
            }]
        );
    }
}
