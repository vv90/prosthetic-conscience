//! Pure browser-facing app boundary for the local consensus slice.
//!
//! The app composes two lower-level pure state machines:
//! - `coordinator` owns committed-entry sequencing
//! - `drafts` owns local draft creation/removal and notices

use serde::Serialize;

use crate::conversation;
use crate::coordinator;
use crate::drafts;
use crate::engine::{DraftEntry, overview_from_entries};
use crate::render::OverviewData;
use crate::types::Entry;

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

pub fn view(state: &State) -> View {
    let draft_view = drafts::view(&state.drafts);
    View {
        overview: overview_from_entries(state.coordinator.committed_prefix()),
        drafts: draft_view.drafts,
        notice: draft_view.notice,
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
    effects.into_iter().map(|effect| match effect {}).collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use serde_json::json;

    use super::*;
    use crate::conversation;
    use crate::drafts::Notice;
    use crate::engine::{ClaimRef, DraftId};
    use crate::types::{ClaimId, ClaimKind};

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

    fn received_event(index: usize, entry: Entry) -> Event {
        Event::CoordinatorEvent {
            event: coordinator::Event::Received { index, entry },
        }
    }

    fn init_state() -> State {
        init(String::from("alice"), None).state
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
