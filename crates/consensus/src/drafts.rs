//! Pure local draft state machine.

use serde::Serialize;

use crate::engine::{ClaimRef, DraftContent, DraftEntry, DraftId, EngineError};
use crate::types::{ClaimKind, Entry};

/// Read-only committed context available to draft decisions.
///
/// The current local-only slice preserves the engine's existing semantics, so
/// committed references are not validated against this context yet.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub committed_entries: &'a [&'a Entry],
}

impl<'a> Default for Context<'a> {
    fn default() -> Self {
        Self {
            committed_entries: &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    drafts: Vec<DraftEntry>,
    next_draft_id: u64,
    last_notice: Option<Notice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    DraftClaimRequested {
        body: String,
        claim_kind: ClaimKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent: Option<ClaimRef>,
    },
    RemoveDraftRequested {
        draft_id: DraftId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Effect {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Notice {
    DraftNotFound {
        draft_id: DraftId,
    },
    DraftReferenceMustTargetClaim {
        draft_id: DraftId,
    },
    DraftReferenced {
        draft_id: DraftId,
        referenced_by: DraftId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct View {
    pub drafts: Vec<DraftEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<Notice>,
}

pub fn init() -> State {
    State {
        drafts: Vec::new(),
        next_draft_id: 0,
        last_notice: None,
    }
}

pub fn reduce(mut state: State, event: Event, context: Context<'_>) -> Transition {
    match event {
        Event::DraftClaimRequested {
            body,
            claim_kind,
            parent,
        } => match draft_claim(state.clone(), body, claim_kind, parent, context) {
            Ok((next_state, _draft_id)) => {
                state = next_state;
                state.last_notice = None;
            }
            Err(error) => {
                state.last_notice = Some(map_notice(error));
            }
        },
        Event::RemoveDraftRequested { draft_id } => {
            match remove_draft(state.clone(), draft_id, context) {
                Ok(next_state) => {
                    state = next_state;
                    state.last_notice = None;
                }
                Err(error) => {
                    state.last_notice = Some(map_notice(error));
                }
            }
        }
    }

    Transition {
        state,
        effects: Vec::new(),
    }
}

pub(crate) fn draft_claim(
    mut state: State,
    body: String,
    claim_kind: ClaimKind,
    parent: Option<ClaimRef>,
    context: Context<'_>,
) -> Result<(State, DraftId), EngineError> {
    let _ = context.committed_entries;
    validate_optional_claim_ref(&state, parent.as_ref())?;
    let id = alloc_draft_id(&mut state);
    state.drafts.push(DraftEntry {
        id,
        entry: DraftContent::Claim {
            body,
            claim_kind,
            parent,
        },
    });
    Ok((state, id))
}

pub(crate) fn remove_draft(
    mut state: State,
    draft_id: DraftId,
    context: Context<'_>,
) -> Result<State, EngineError> {
    let _ = context.committed_entries;
    remove_draft_in_place(&mut state, draft_id)?;
    Ok(state)
}

pub fn view(state: &State) -> View {
    View {
        drafts: state.drafts.clone(),
        notice: state.last_notice.clone(),
    }
}

pub fn show_drafts(state: &State) -> &[DraftEntry] {
    &state.drafts
}

pub fn notice(state: &State) -> Option<&Notice> {
    state.last_notice.as_ref()
}

#[cfg(test)]
pub(crate) fn state_with_drafts(drafts: Vec<DraftEntry>) -> State {
    let next_draft_id = drafts
        .iter()
        .map(|draft| draft.id.0)
        .max()
        .map_or(0, |id| id + 1);
    State {
        drafts,
        next_draft_id,
        last_notice: None,
    }
}

fn alloc_draft_id(state: &mut State) -> DraftId {
    let id = DraftId(state.next_draft_id);
    state.next_draft_id += 1;
    id
}

fn validate_optional_claim_ref(
    state: &State,
    claim_ref: Option<&ClaimRef>,
) -> Result<(), EngineError> {
    if let Some(claim_ref) = claim_ref {
        validate_claim_ref(state, claim_ref)?;
    }
    Ok(())
}

fn validate_claim_ref(state: &State, claim_ref: &ClaimRef) -> Result<(), EngineError> {
    match claim_ref {
        ClaimRef::Committed(_) => Ok(()),
        ClaimRef::Draft(draft_id) => match find_draft(state, *draft_id) {
            Some(DraftEntry {
                entry: DraftContent::Claim { .. },
                ..
            }) => Ok(()),
            Some(_) => Err(EngineError::DraftReferenceMustTargetClaim(*draft_id)),
            None => Err(EngineError::DraftNotFound(*draft_id)),
        },
    }
}

fn remove_draft_in_place(state: &mut State, draft_id: DraftId) -> Result<(), EngineError> {
    let pos = state
        .drafts
        .iter()
        .position(|draft| draft.id == draft_id)
        .ok_or(EngineError::DraftNotFound(draft_id))?;
    let removed_is_claim = state
        .drafts
        .get(pos)
        .is_some_and(|draft| matches!(draft.entry, DraftContent::Claim { .. }));
    if removed_is_claim
        && let Some(referenced_by) = state
            .drafts
            .iter()
            .filter(|draft| draft.id != draft_id)
            .find(|draft| draft_references_draft(&draft.entry, draft_id))
            .map(|draft| draft.id)
    {
        return Err(EngineError::DraftReferenced {
            draft_id,
            referenced_by,
        });
    }
    state.drafts.remove(pos);
    Ok(())
}

fn find_draft(state: &State, draft_id: DraftId) -> Option<&DraftEntry> {
    state.drafts.iter().find(|draft| draft.id == draft_id)
}

fn draft_references_draft(entry: &DraftContent, draft_id: DraftId) -> bool {
    let references =
        |claim_ref: &ClaimRef| matches!(claim_ref, ClaimRef::Draft(id) if *id == draft_id);
    match entry {
        DraftContent::Claim { parent, .. } => parent.as_ref().is_some_and(references),
        DraftContent::Relation { source, target, .. } => references(source) || references(target),
        DraftContent::Stance { target, .. } => references(target),
        DraftContent::Resolve { claim, .. } => references(claim),
        DraftContent::Comment { claim, .. } => claim.as_ref().is_some_and(references),
    }
}

fn map_notice(error: EngineError) -> Notice {
    match error {
        EngineError::DraftNotFound(draft_id) => Notice::DraftNotFound { draft_id },
        EngineError::DraftReferenceMustTargetClaim(draft_id) => {
            Notice::DraftReferenceMustTargetClaim { draft_id }
        }
        EngineError::DraftReferenced {
            draft_id,
            referenced_by,
        } => Notice::DraftReferenced {
            draft_id,
            referenced_by,
        },
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::types::Position;

    #[derive(Debug, Clone)]
    enum LocalOp {
        Add { body: String, claim_kind: ClaimKind },
        Remove { draft_id: u8 },
    }

    fn body_strategy() -> impl Strategy<Value = String> {
        prop::string::string_regex("[a-z]{0,12}").expect("body regex should compile")
    }

    fn claim_kind_strategy() -> impl Strategy<Value = ClaimKind> {
        prop_oneof![
            Just(ClaimKind::Item),
            Just(ClaimKind::Proposal),
            Just(ClaimKind::Fact),
            Just(ClaimKind::Conditional),
            Just(ClaimKind::Value),
            Just(ClaimKind::Reference),
        ]
    }

    fn local_op_strategy() -> impl Strategy<Value = LocalOp> {
        prop_oneof![
            (body_strategy(), claim_kind_strategy())
                .prop_map(|(body, claim_kind)| LocalOp::Add { body, claim_kind }),
            any::<u8>().prop_map(|draft_id| LocalOp::Remove { draft_id }),
        ]
    }

    fn draft_ids(state: &State) -> Vec<DraftId> {
        state.drafts.iter().map(|draft| draft.id).collect()
    }

    #[test]
    fn initial_state_has_empty_view_and_reducer_emits_no_effects() {
        let state = init();
        let draft_view = view(&state);

        assert!(draft_view.drafts.is_empty());
        assert!(draft_view.notice.is_none());

        let transition = reduce(
            state,
            Event::RemoveDraftRequested {
                draft_id: DraftId(0),
            },
            Context::default(),
        );
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn successful_claim_draft_appends_one_draft_and_clears_notice() {
        let state = init();
        let transition = reduce(
            state,
            Event::RemoveDraftRequested {
                draft_id: DraftId(999),
            },
            Context::default(),
        );
        assert_eq!(
            transition.state.last_notice,
            Some(Notice::DraftNotFound {
                draft_id: DraftId(999),
            })
        );

        let transition = reduce(
            transition.state,
            Event::DraftClaimRequested {
                body: String::from("Use session cookies"),
                claim_kind: ClaimKind::Proposal,
                parent: None,
            },
            Context::default(),
        );

        assert!(transition.effects.is_empty());
        assert_eq!(transition.state.last_notice, None);
        assert_eq!(transition.state.drafts.len(), 1);
        assert!(matches!(
            &transition.state.drafts[0],
            DraftEntry {
                entry: DraftContent::Claim {
                    body,
                    claim_kind: ClaimKind::Proposal,
                    parent: None,
                },
                ..
            } if body == "Use session cookies"
        ));
    }

    #[test]
    fn notice_free_draft_claim_preserves_notice_and_returns_new_id() {
        let mut state = init();
        state.last_notice = Some(Notice::DraftNotFound {
            draft_id: DraftId(999),
        });

        let (state, draft_id) = draft_claim(
            state,
            String::from("Use session cookies"),
            ClaimKind::Proposal,
            None,
            Context::default(),
        )
        .unwrap();

        assert_eq!(draft_id, DraftId(0));
        assert_eq!(
            state.last_notice,
            Some(Notice::DraftNotFound {
                draft_id: DraftId(999),
            })
        );
        assert_eq!(state.drafts.len(), 1);
    }

    #[test]
    fn removing_existing_middle_draft_removes_only_that_draft_and_preserves_order() {
        let mut state = init();
        for body in ["first", "second", "third"] {
            state = reduce(
                state,
                Event::DraftClaimRequested {
                    body: body.to_owned(),
                    claim_kind: ClaimKind::Fact,
                    parent: None,
                },
                Context::default(),
            )
            .state;
        }

        let before_ids = draft_ids(&state);
        let middle_id = before_ids[1];

        let transition = reduce(
            state,
            Event::RemoveDraftRequested {
                draft_id: middle_id,
            },
            Context::default(),
        );

        assert!(transition.effects.is_empty());
        assert_eq!(transition.state.last_notice, None);
        assert_eq!(
            draft_ids(&transition.state),
            vec![before_ids[0], before_ids[2]]
        );
    }

    #[test]
    fn notice_free_remove_draft_preserves_notice_and_removes_one_draft() {
        let mut state = init();
        state = reduce(
            state,
            Event::DraftClaimRequested {
                body: String::from("first"),
                claim_kind: ClaimKind::Fact,
                parent: None,
            },
            Context::default(),
        )
        .state;
        state = reduce(
            state,
            Event::DraftClaimRequested {
                body: String::from("second"),
                claim_kind: ClaimKind::Fact,
                parent: None,
            },
            Context::default(),
        )
        .state;
        state.last_notice = Some(Notice::DraftNotFound {
            draft_id: DraftId(999),
        });

        let state = remove_draft(state, DraftId(0), Context::default()).unwrap();

        assert_eq!(draft_ids(&state), vec![DraftId(1)]);
        assert_eq!(
            state.last_notice,
            Some(Notice::DraftNotFound {
                draft_id: DraftId(999),
            })
        );
    }

    #[test]
    fn removing_unknown_draft_preserves_drafts_and_sets_notice() {
        let mut state = init();
        state = reduce(
            state,
            Event::DraftClaimRequested {
                body: String::from("known"),
                claim_kind: ClaimKind::Fact,
                parent: None,
            },
            Context::default(),
        )
        .state;
        let before_drafts = view(&state).drafts;

        let transition = reduce(
            state,
            Event::RemoveDraftRequested {
                draft_id: DraftId(999),
            },
            Context::default(),
        );

        assert!(transition.effects.is_empty());
        assert_eq!(view(&transition.state).drafts, before_drafts);
        assert_eq!(
            transition.state.last_notice,
            Some(Notice::DraftNotFound {
                draft_id: DraftId(999),
            })
        );
    }

    #[test]
    fn removing_referenced_claim_draft_preserves_drafts_and_sets_notice() {
        let mut state = init();
        let claim_draft = alloc_draft_id(&mut state);
        state.drafts.push(DraftEntry {
            id: claim_draft,
            entry: DraftContent::Claim {
                body: String::from("A"),
                claim_kind: ClaimKind::Fact,
                parent: None,
            },
        });
        let dependent = alloc_draft_id(&mut state);
        state.drafts.push(DraftEntry {
            id: dependent,
            entry: DraftContent::Stance {
                target: ClaimRef::Draft(claim_draft),
                position: Position::Block,
            },
        });
        let before_drafts = view(&state).drafts;

        let transition = reduce(
            state,
            Event::RemoveDraftRequested {
                draft_id: claim_draft,
            },
            Context::default(),
        );

        assert!(transition.effects.is_empty());
        assert_eq!(view(&transition.state).drafts, before_drafts);
        assert_eq!(
            transition.state.last_notice,
            Some(Notice::DraftReferenced {
                draft_id: claim_draft,
                referenced_by: dependent,
            })
        );
    }

    #[test]
    fn invalid_parent_claim_request_preserves_drafts_and_sets_notice() {
        let mut state = init();
        let comment_draft = alloc_draft_id(&mut state);
        state.drafts.push(DraftEntry {
            id: comment_draft,
            entry: DraftContent::Comment {
                claim: None,
                body: String::from("note"),
            },
        });
        let before_drafts = view(&state).drafts;

        let transition = reduce(
            state,
            Event::DraftClaimRequested {
                body: String::from("child"),
                claim_kind: ClaimKind::Fact,
                parent: Some(ClaimRef::Draft(comment_draft)),
            },
            Context::default(),
        );

        assert!(transition.effects.is_empty());
        assert_eq!(view(&transition.state).drafts, before_drafts);
        assert_eq!(
            transition.state.last_notice,
            Some(Notice::DraftReferenceMustTargetClaim {
                draft_id: comment_draft,
            })
        );
    }

    #[test]
    fn notice_free_helpers_leave_state_unchanged_on_failure() {
        let mut state = init();
        let comment_draft = alloc_draft_id(&mut state);
        state.drafts.push(DraftEntry {
            id: comment_draft,
            entry: DraftContent::Comment {
                claim: None,
                body: String::from("note"),
            },
        });
        state.last_notice = Some(Notice::DraftNotFound {
            draft_id: DraftId(777),
        });
        let original = state.clone();

        assert_eq!(
            draft_claim(
                state.clone(),
                String::from("child"),
                ClaimKind::Fact,
                Some(ClaimRef::Draft(comment_draft)),
                Context::default(),
            ),
            Err(EngineError::DraftReferenceMustTargetClaim(comment_draft))
        );
        assert_eq!(
            remove_draft(state.clone(), DraftId(999), Context::default()),
            Err(EngineError::DraftNotFound(DraftId(999)))
        );
        assert_eq!(state, original);
    }

    proptest! {
        #[test]
        fn local_traces_keep_view_drafts_exact(
            ops in prop::collection::vec(local_op_strategy(), 0..40)
        ) {
            let mut state = init();

            for op in ops {
                let event = match op {
                    LocalOp::Add { body, claim_kind } => Event::DraftClaimRequested {
                        body,
                        claim_kind,
                        parent: None,
                    },
                    LocalOp::Remove { draft_id } => Event::RemoveDraftRequested {
                        draft_id: DraftId(u64::from(draft_id)),
                    },
                };

                let transition = reduce(state, event, Context::default());
                prop_assert!(transition.effects.is_empty());
                state = transition.state;

                let draft_view = view(&state);
                prop_assert_eq!(&draft_view.drafts, &state.drafts);
                prop_assert_eq!(
                    draft_view.drafts.iter().map(|draft| draft.id).collect::<Vec<_>>(),
                    state.drafts.iter().map(|draft| draft.id).collect::<Vec<_>>(),
                );
            }
        }

        #[test]
        fn removing_existing_generated_draft_removes_exactly_one_and_preserves_relative_order(
            draft_specs in prop::collection::vec((body_strategy(), claim_kind_strategy()), 1..16),
            remove_index in any::<usize>(),
        ) {
            let mut state = init();

            for (body, claim_kind) in draft_specs {
                state = reduce(
                    state,
                    Event::DraftClaimRequested {
                        body,
                        claim_kind,
                        parent: None,
                    },
                    Context::default(),
                ).state;
            }

            let before_ids = draft_ids(&state);
            let remove_index = remove_index % before_ids.len();
            let removed_id = before_ids[remove_index];

            state = reduce(
                state,
                Event::RemoveDraftRequested { draft_id: removed_id },
                Context::default(),
            ).state;

            let after_ids = draft_ids(&state);
            let expected_ids = before_ids
                .iter()
                .enumerate()
                .filter_map(|(index, draft_id)| {
                    if index == remove_index {
                        None
                    } else {
                        Some(*draft_id)
                    }
                })
                .collect::<Vec<_>>();

            prop_assert_eq!(after_ids.len() + 1, before_ids.len());
            prop_assert_eq!(after_ids, expected_ids);
            prop_assert_eq!(state.last_notice, None);
        }
    }
}
