//! Pure browser-facing app boundary for the local consensus slice.
//!
//! The app composes two lower-level pure state machines:
//! - `coordinator` owns committed-entry sequencing
//! - `drafts` owns local draft creation/removal and notices

use serde::Serialize;

use crate::conversation;
use crate::coordinator;
use crate::drafts;
use crate::engine::{
    ClaimRef, DraftEntry, EngineError, ImpactAnalysis, materialize_entries, overview_from_entries,
};
use crate::preview;
use crate::render::{ClaimDetail, OverviewData, claim_detail as render_claim_detail};
use crate::types::{ClaimId, Entry};

const COORDINATOR_PAGE_LIMIT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    participant: String,
    conversation: conversation::State,
    coordinator: coordinator::State<Entry>,
    drafts: drafts::State,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ConversationEvent { event: conversation::Event },
    DraftsEvent { event: drafts::Event },
    CoordinatorEvent { event: coordinator::Event<Entry> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    ConversationEffect { effect: conversation::Effect },
    CoordinatorEffect { effect: coordinator::Effect<Entry> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct View {
    pub overview: OverviewData,
    pub drafts: Vec<DraftEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<drafts::Notice>,
}

pub fn init(participant: String, latest_entry_index: Option<usize>) -> Transition {
    let coordinator = coordinator::sync_to_latest(
        coordinator::State::empty(COORDINATOR_PAGE_LIMIT),
        latest_entry_index,
    );

    Transition {
        state: State {
            participant,
            conversation: conversation::init(),
            coordinator: coordinator.state,
            drafts: drafts::init(),
        },
        effects: coordinator
            .effects
            .into_iter()
            .map(|effect| Effect::CoordinatorEffect { effect })
            .collect(),
    }
}

pub fn reduce(state: State, event: Event) -> Transition {
    match event {
        Event::ConversationEvent { event } => reduce_conversation_event(state, event),
        Event::DraftsEvent { event } => reduce_drafts_event(state, event),
        Event::CoordinatorEvent { event } => reduce_coordinator_event(state, event),
    }
}

pub fn participant(state: &State) -> &str {
    &state.participant
}

pub fn overview(state: &State) -> OverviewData {
    overview_from_entries(state.coordinator.committed_prefix())
}

pub fn show_drafts(state: &State) -> Vec<DraftEntry> {
    drafts::show_drafts(&state.drafts).to_vec()
}

pub fn claim_detail(state: &State, claim_id: &ClaimId) -> Option<ClaimDetail> {
    let (materialized, statuses) = materialize_entries(state.coordinator.committed_prefix());
    render_claim_detail(&materialized, &statuses, claim_id)
}

pub fn preview_overview(state: &State) -> Result<OverviewData, EngineError> {
    preview::preview_overview(
        state.coordinator.committed_prefix(),
        drafts::show_drafts(&state.drafts),
        &state.participant,
    )
}

pub fn preview_claim_detail(
    state: &State,
    claim: &ClaimRef,
) -> Result<Option<ClaimDetail>, EngineError> {
    preview::preview_claim_detail(
        state.coordinator.committed_prefix(),
        drafts::show_drafts(&state.drafts),
        &state.participant,
        claim,
    )
}

pub fn impact_analysis(state: &State) -> Result<ImpactAnalysis, EngineError> {
    preview::impact_analysis(
        state.coordinator.committed_prefix(),
        drafts::show_drafts(&state.drafts),
        &state.participant,
    )
}

pub fn view(state: &State) -> View {
    View {
        overview: overview(state),
        drafts: show_drafts(state),
        notice: drafts::notice(&state.drafts).cloned(),
    }
}

fn reduce_drafts_event(state: State, event: drafts::Event) -> Transition {
    let committed_entries = state.coordinator.committed_prefix().collect::<Vec<_>>();
    let transition = drafts::reduce(
        state.drafts,
        event,
        drafts::Context {
            committed_entries: &committed_entries,
        },
    );

    Transition {
        state: State {
            participant: state.participant,
            conversation: state.conversation,
            coordinator: state.coordinator,
            drafts: transition.state,
        },
        effects: map_draft_effects(transition.effects),
    }
}

fn reduce_conversation_event(state: State, event: conversation::Event) -> Transition {
    let transition = conversation::reduce(state.conversation, event);

    Transition {
        state: State {
            participant: state.participant,
            conversation: transition.state,
            coordinator: state.coordinator,
            drafts: state.drafts,
        },
        effects: map_conversation_effects(transition.effects),
    }
}

fn reduce_coordinator_event(state: State, event: coordinator::Event<Entry>) -> Transition {
    let transition = coordinator::reduce(state.coordinator, event);

    Transition {
        state: State {
            participant: state.participant,
            conversation: state.conversation,
            coordinator: transition.state,
            drafts: state.drafts,
        },
        effects: transition
            .effects
            .into_iter()
            .map(|effect| Effect::CoordinatorEffect { effect })
            .collect(),
    }
}

fn map_draft_effects(effects: Vec<drafts::Effect>) -> Vec<Effect> {
    effects.into_iter().map(|effect| match effect {}).collect()
}

fn map_conversation_effects(effects: Vec<conversation::Effect>) -> Vec<Effect> {
    effects
        .into_iter()
        .map(|effect| Effect::ConversationEffect { effect })
        .collect()
}

#[cfg(test)]
pub(crate) fn state_for_tests(
    participant: &str,
    committed_entries: Vec<Entry>,
    draft_entries: Vec<DraftEntry>,
) -> State {
    let mut state = init(participant.to_owned(), None).state;
    for (index, entry) in committed_entries.into_iter().enumerate() {
        state = reduce(
            state,
            Event::CoordinatorEvent {
                event: coordinator::Event::Received { index, entry },
            },
        )
        .state;
    }
    state.drafts = drafts::state_with_drafts(draft_entries);
    state
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;
    use crate::conversation;
    use crate::drafts::Notice;
    use crate::engine::{ClaimRef, DraftContent, DraftId};
    use crate::status::EpistemicStatus;
    use crate::types::{ClaimId, ClaimKind, RelationKind};

    #[derive(Debug, Clone)]
    enum LocalOp {
        Add { body: String, claim_kind: ClaimKind },
        Remove { draft_id: u8 },
    }

    #[derive(Debug, Clone)]
    enum AppOp {
        Local(LocalOp),
        Receive { index: usize },
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

    fn app_op_strategy() -> impl Strategy<Value = AppOp> {
        prop_oneof![
            local_op_strategy().prop_map(AppOp::Local),
            (0usize..24).prop_map(|index| AppOp::Receive { index }),
        ]
    }

    fn claim_entry(id: &str, body: &str) -> Entry {
        Entry::Claim {
            claim_id: ClaimId(id.to_owned()),
            author: String::from("alice"),
            body: body.to_owned(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        }
    }

    fn draft_claim_event(body: impl Into<String>, claim_kind: ClaimKind) -> Event {
        Event::DraftsEvent {
            event: drafts::Event::DraftClaimRequested {
                body: body.into(),
                claim_kind,
                parent: None,
            },
        }
    }

    fn remove_draft_event(draft_id: DraftId) -> Event {
        Event::DraftsEvent {
            event: drafts::Event::RemoveDraftRequested { draft_id },
        }
    }

    fn chat_completion_received_event(chunks: Vec<serde_json::Value>) -> Event {
        Event::ConversationEvent {
            event: conversation::Event::ChatCompletionReceived { chunks },
        }
    }

    fn received_event(index: usize, entry: Entry) -> Event {
        Event::CoordinatorEvent {
            event: coordinator::Event::Received { index, entry },
        }
    }

    fn role_chunk() -> serde_json::Value {
        json!({"choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]})
    }

    fn content_chunk(content: &str) -> serde_json::Value {
        json!({"choices": [{"index": 0, "delta": {"content": content}, "finish_reason": null}]})
    }

    fn init_state() -> State {
        init(String::from("alice"), None).state
    }

    fn draft_claim_entry(id: u64, body: &str, claim_kind: ClaimKind) -> DraftEntry {
        DraftEntry {
            id: DraftId(id),
            entry: DraftContent::Claim {
                body: body.to_owned(),
                claim_kind,
                parent: None,
            },
        }
    }

    #[test]
    fn init_without_latest_entry_index_has_empty_view_and_no_effects() {
        let transition = init(String::from("alice"), None);
        let state = transition.state;
        let app_view = view(&state);

        assert!(transition.effects.is_empty());
        assert_eq!(participant(&state), "alice");
        assert!(conversation::view(&state.conversation).history.is_empty());
        assert!(app_view.drafts.is_empty());
        assert!(app_view.notice.is_none());
        assert_eq!(app_view.overview.total_claims, 0);
        assert_eq!(app_view.overview.total_relations, 0);
        assert_eq!(app_view.overview.total_stances, 0);
        assert!(app_view.overview.attention.is_empty());

        let transition = reduce(state, remove_draft_event(DraftId(0)));
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn draft_and_coordinator_events_preserve_conversation_state() {
        let state = init_state();
        let conversation_before = conversation::view(&state.conversation);

        let state = reduce(state, draft_claim_event("local draft", ClaimKind::Proposal)).state;
        assert_eq!(conversation::view(&state.conversation), conversation_before);

        let state = reduce(state, received_event(0, claim_entry("c1", "committed"))).state;
        assert_eq!(conversation::view(&state.conversation), conversation_before);
    }

    #[test]
    fn chat_completion_received_changes_only_conversation_history() {
        let mut state = init_state();
        state = reduce(state, draft_claim_event("local draft", ClaimKind::Proposal)).state;
        state = reduce(state, received_event(0, claim_entry("c1", "committed"))).state;

        let participant_before = state.participant.clone();
        let coordinator_before = state.coordinator.clone();
        let drafts_before = state.drafts.clone();

        let transition = reduce(
            state,
            chat_completion_received_event(vec![
                role_chunk(),
                content_chunk("hello"),
                content_chunk(" world"),
            ]),
        );

        assert!(transition.effects.is_empty());
        assert_eq!(transition.state.participant, participant_before);
        assert_eq!(transition.state.coordinator, coordinator_before);
        assert_eq!(transition.state.drafts, drafts_before);
        assert_eq!(
            conversation::view(&transition.state.conversation).history,
            vec![conversation::Message::Assistant {
                content: Some(String::from("hello world")),
                tool_calls: Vec::new(),
            }]
        );
    }

    #[test]
    fn chat_completion_decode_failure_surfaces_as_wrapped_conversation_effect() {
        let state = init_state();
        let participant_before = state.participant.clone();
        let coordinator_before = state.coordinator.clone();
        let drafts_before = state.drafts.clone();
        let conversation_before = state.conversation.clone();

        let transition = reduce(state, chat_completion_received_event(vec![]));

        assert_eq!(transition.state.participant, participant_before);
        assert_eq!(transition.state.coordinator, coordinator_before);
        assert_eq!(transition.state.drafts, drafts_before);
        assert_eq!(transition.state.conversation, conversation_before);
        assert_eq!(
            transition.effects,
            vec![Effect::ConversationEffect {
                effect: conversation::Effect::ChatCompletionDecodeFailed {
                    error: String::from("no chunks to assemble"),
                },
            }]
        );
    }

    #[test]
    fn init_with_latest_entry_index_requests_missing_history() {
        let transition = init(String::from("alice"), Some(3));

        assert_eq!(
            transition.effects,
            vec![Effect::CoordinatorEffect {
                effect: coordinator::Effect::fetch_missing(0, 4),
            }]
        );
        assert_eq!(view(&transition.state).overview.total_claims, 0);
    }

    #[test]
    fn session_entry_observed_updates_overview_via_coordinator() {
        let state = init_state();
        let transition = reduce(state, received_event(0, claim_entry("c1", "Use JWT")));

        assert!(transition.effects.is_empty());
        let app_view = view(&transition.state);
        assert_eq!(app_view.overview.total_claims, 1);
        assert_eq!(app_view.overview.total_relations, 0);
        assert_eq!(app_view.overview.total_stances, 0);
    }

    #[test]
    fn future_out_of_order_entries_do_not_affect_overview_until_gap_is_filled() {
        let state = init_state();
        let transition = reduce(state, received_event(2, claim_entry("c3", "third")));

        assert_eq!(
            transition.effects,
            vec![Effect::CoordinatorEffect {
                effect: coordinator::Effect::fetch_missing(0, 2),
            }]
        );
        assert_eq!(view(&transition.state).overview.total_claims, 0);

        let transition = reduce(
            transition.state,
            received_event(0, claim_entry("c1", "first")),
        );
        assert!(transition.effects.is_empty());
        assert_eq!(view(&transition.state).overview.total_claims, 1);

        let transition = reduce(
            transition.state,
            received_event(1, claim_entry("c2", "second")),
        );
        assert!(transition.effects.is_empty());
        assert_eq!(view(&transition.state).overview.total_claims, 3);
    }

    #[test]
    fn coordinator_fetch_requests_surface_as_wrapped_child_effects() {
        let state = init_state();
        let transition = reduce(state, received_event(4, claim_entry("c5", "fifth")));

        assert_eq!(
            transition.effects,
            vec![Effect::CoordinatorEffect {
                effect: coordinator::Effect::fetch_missing(0, 4),
            }]
        );
    }

    #[test]
    fn drafts_event_changes_only_drafts_state_and_preserves_committed_overview() {
        let state = init_state();
        let state = reduce(state, received_event(0, claim_entry("c1", "committed"))).state;
        let overview_before = view(&state).overview;

        let transition = reduce(state, draft_claim_event("local draft", ClaimKind::Proposal));

        assert!(transition.effects.is_empty());
        assert_eq!(view(&transition.state).overview, overview_before);
        assert_eq!(view(&transition.state).drafts.len(), 1);
    }

    #[test]
    fn coordinator_event_preserves_existing_draft_notice_and_drafts() {
        let mut state = init_state();
        state = reduce(state, draft_claim_event("draft", ClaimKind::Fact)).state;
        state = reduce(state, remove_draft_event(DraftId(999))).state;
        let before_view = view(&state);

        let transition = reduce(state, received_event(0, claim_entry("c1", "committed")));
        let after_view = view(&transition.state);

        assert_eq!(after_view.drafts, before_view.drafts);
        assert_eq!(after_view.notice, before_view.notice);
        assert_eq!(after_view.overview.total_claims, 1);
    }

    #[test]
    fn overview_query_reads_committed_state_only() {
        let state = state_for_tests(
            "alice",
            vec![claim_entry("c1", "committed")],
            vec![draft_claim_entry(0, "draft", ClaimKind::Proposal)],
        );

        let query = overview(&state);

        assert_eq!(query.total_claims, 1);
        assert!(query.proposals.is_empty());
    }

    #[test]
    fn show_drafts_query_reads_local_drafts_directly() {
        let state = state_for_tests(
            "alice",
            vec![],
            vec![draft_claim_entry(0, "draft", ClaimKind::Proposal)],
        );

        let drafts = show_drafts(&state);

        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].id, DraftId(0));
    }

    #[test]
    fn claim_detail_query_reads_committed_materialized_state() {
        let state = state_for_tests("alice", vec![claim_entry("c1", "committed")], vec![]);

        let detail = claim_detail(&state, &ClaimId("c1".into())).unwrap();

        assert_eq!(detail.claim.id, ClaimId("c1".into()));
        assert_eq!(detail.claim.body, "committed");
    }

    #[test]
    fn preview_queries_include_local_drafts() {
        let state = state_for_tests(
            "alice",
            vec![claim_entry("c1", "committed")],
            vec![draft_claim_entry(0, "draft", ClaimKind::Proposal)],
        );

        let overview = preview_overview(&state).unwrap();
        let detail = preview_claim_detail(&state, &ClaimRef::Draft(DraftId(0)))
            .unwrap()
            .unwrap();

        assert_eq!(overview.total_claims, 2);
        assert_eq!(detail.claim.id, ClaimId("draft-0".into()));
    }

    #[test]
    fn impact_analysis_query_matches_engine_preview_behavior() {
        let state = state_for_tests(
            "assistant",
            vec![
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
            ],
            vec![
                draft_claim_entry(0, "Draft proposal", ClaimKind::Proposal),
                DraftEntry {
                    id: DraftId(1),
                    entry: DraftContent::Relation {
                        source: ClaimRef::Committed(ClaimId("c2".into())),
                        target: ClaimRef::Committed(ClaimId("c1".into())),
                        kind: RelationKind::Attacks,
                    },
                },
            ],
        );

        let impact = impact_analysis(&state).unwrap();

        assert_eq!(impact.new_claims.len(), 1);
        assert_eq!(impact.new_claims[0].draft_id, DraftId(0));
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
    fn app_preview_queries_match_engine_preview_methods_for_same_state() {
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
            draft_claim_entry(0, "Draft proposal", ClaimKind::Proposal),
            DraftEntry {
                id: DraftId(1),
                entry: DraftContent::Relation {
                    source: ClaimRef::Committed(ClaimId("c2".into())),
                    target: ClaimRef::Committed(ClaimId("c1".into())),
                    kind: RelationKind::Attacks,
                },
            },
        ];
        let state = state_for_tests("assistant", committed.clone(), drafts.clone());

        let mut engine = crate::engine::ConsensusEngine::new(String::from("assistant"));
        for entry in committed {
            engine.append(entry);
        }
        engine
            .draft_claim("Draft proposal".into(), ClaimKind::Proposal, None)
            .unwrap();
        engine
            .draft_relation(
                ClaimRef::Committed(ClaimId("c2".into())),
                ClaimRef::Committed(ClaimId("c1".into())),
                RelationKind::Attacks,
            )
            .unwrap();

        assert_eq!(
            preview_overview(&state).unwrap(),
            engine.preview_overview().unwrap()
        );
        assert_eq!(
            preview_claim_detail(&state, &ClaimRef::Committed(ClaimId("c1".into()))).unwrap(),
            engine
                .preview_claim_detail(&ClaimRef::Committed(ClaimId("c1".into())))
                .unwrap()
        );
        assert_eq!(
            impact_analysis(&state).unwrap(),
            engine.impact_analysis().unwrap()
        );
        assert_eq!(show_drafts(&state), drafts);
    }

    #[test]
    fn wrapped_drafts_event_serde_shape() {
        let value = serde_json::to_value(Event::DraftsEvent {
            event: drafts::Event::DraftClaimRequested {
                body: String::from("Use JWT"),
                claim_kind: ClaimKind::Proposal,
                parent: Some(ClaimRef::Committed(ClaimId(String::from("root")))),
            },
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "drafts_event",
                "event": {
                    "type": "draft_claim_requested",
                    "body": "Use JWT",
                    "claim_kind": "proposal",
                    "parent": { "claim_id": "root" }
                }
            })
        );
    }

    #[test]
    fn wrapped_conversation_effect_serde_shape() {
        let value = serde_json::to_value(Effect::ConversationEffect {
            effect: conversation::Effect::ChatCompletionDecodeFailed {
                error: String::from("no chunks to assemble"),
            },
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "conversation_effect",
                "effect": {
                    "type": "chat_completion_decode_failed",
                    "error": "no chunks to assemble"
                }
            })
        );
    }

    #[test]
    fn wrapped_coordinator_event_serde_shape() {
        let value = serde_json::to_value(received_event(3, claim_entry("c3", "third"))).unwrap();

        assert_eq!(
            value,
            json!({
                "type": "coordinator_event",
                "event": {
                    "type": "received",
                    "data": {
                        "index": 3,
                        "entry": {
                            "type": "claim",
                            "claim_id": "c3",
                            "author": "alice",
                            "body": "third",
                            "claim_kind": "fact"
                        }
                    }
                }
            })
        );
    }

    #[test]
    fn wrapped_coordinator_effect_serde_shape() {
        let value = serde_json::to_value(Effect::CoordinatorEffect {
            effect: coordinator::Effect::fetch_missing(2, 3),
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "coordinator_effect",
                "effect": {
                    "type": "fetch_missing",
                    "data": {
                        "from": 2,
                        "limit": 3
                    }
                }
            })
        );
    }

    proptest! {
        #[test]
        fn mixed_reducer_traces_preserve_participant(
            participant_name in body_strategy(),
            ops in prop::collection::vec(app_op_strategy(), 0..40)
        ) {
            let mut state = init(participant_name.clone(), None).state;
            prop_assert_eq!(participant(&state), participant_name.as_str());

            for (step, op) in ops.into_iter().enumerate() {
                let event = match op {
                    AppOp::Local(LocalOp::Add { body, claim_kind }) => {
                        draft_claim_event(body, claim_kind)
                    }
                    AppOp::Local(LocalOp::Remove { draft_id }) => {
                        remove_draft_event(DraftId(u64::from(draft_id)))
                    }
                    AppOp::Receive { index } => {
                        received_event(index, claim_entry(&format!("c{step}"), &format!("body{step}")))
                    }
                };

                state = reduce(state, event).state;
                prop_assert_eq!(participant(&state), participant_name.as_str());
            }
        }

        #[test]
        fn draft_only_traces_preserve_coordinator_derived_committed_overview(
            ops in prop::collection::vec(local_op_strategy(), 0..40)
        ) {
            let mut state = init_state();
            state = reduce(state, received_event(0, claim_entry("c1", "first"))).state;
            state = reduce(state, received_event(1, claim_entry("c2", "second"))).state;

            let initial_overview = view(&state).overview;

            for op in ops {
                let event = match op {
                    LocalOp::Add { body, claim_kind } => draft_claim_event(body, claim_kind),
                    LocalOp::Remove { draft_id } => remove_draft_event(DraftId(u64::from(draft_id))),
                };

                let transition = reduce(state, event);
                prop_assert!(transition.effects.is_empty());
                state = transition.state;
                prop_assert_eq!(&view(&state).overview, &initial_overview);
            }
        }

        #[test]
        fn entry_only_traces_do_not_change_draft_list_or_notice(
            events in prop::collection::vec(0usize..24, 0..40)
        ) {
            let mut state = init_state();
            state = reduce(state, draft_claim_event("draft", ClaimKind::Fact)).state;
            state = reduce(state, remove_draft_event(DraftId(999))).state;

            let initial_view = view(&state);

            for (step, index) in events.into_iter().enumerate() {
                state = reduce(
                    state,
                    received_event(index, claim_entry(&format!("c{step}"), &format!("body{step}"))),
                ).state;

                let current_view = view(&state);
                prop_assert_eq!(&current_view.drafts, &initial_view.drafts);
                prop_assert_eq!(&current_view.notice, &initial_view.notice);
            }
        }
    }

    #[test]
    fn entry_only_events_preserve_existing_draft_notice() {
        let mut state = init_state();
        state = reduce(state, draft_claim_event("draft", ClaimKind::Fact)).state;
        state = reduce(state, remove_draft_event(DraftId(999))).state;

        let transition = reduce(state, received_event(0, claim_entry("c1", "committed")));
        assert_eq!(view(&transition.state).drafts.len(), 1);
        assert_eq!(
            view(&transition.state).notice,
            Some(Notice::DraftNotFound {
                draft_id: DraftId(999),
            })
        );
    }
}
