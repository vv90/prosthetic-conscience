use im::HashSet;

use serde_json::Value;

/// Append-only log of JSON entries.
///
/// Structurally enforces two invariants:
/// - **Entry permanence**: existing entries cannot be removed, reordered, or mutated.
/// - **Append-only growth**: length never decreases and increases by at most 1 per operation.
///
/// The only way to produce a longer log is `append`, which returns a new `AppendLog`.
/// No `&mut` access, no removal, no interior mutability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendLog {
    entries: Vec<Value>,
}

impl Default for AppendLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AppendLog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn append(mut self, value: Value) -> Self {
        self.entries.push(value);
        self
    }

    pub fn get(&self, index: usize) -> Option<&Value> {
        self.entries.get(index)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn slice(&self, after: usize, limit: usize) -> &[Value] {
        let start = after.min(self.entries.len());
        let end = (start + limit).min(self.entries.len());
        &self.entries[start..end]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State<SubId: Clone + Eq + std::hash::Hash> {
    pub entries: AppendLog,
    pub subscribers: HashSet<SubId>,
}

impl<SubId: Clone + Eq + std::hash::Hash> Default for State<SubId> {
    fn default() -> Self {
        Self {
            entries: AppendLog::new(),
            subscribers: HashSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event<SubId> {
    EntryAppended { payload: Value },
    Subscribed { subscriber_id: SubId },
    Unsubscribed { subscriber_id: SubId },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect<SubId> {
    NotifySubscribers {
        entry_index: usize,
        payload: Value,
        subscribers: Vec<SubId>,
    },
}

pub struct Transition<SubId: Clone + Eq + std::hash::Hash> {
    pub state: State<SubId>,
    pub effects: Vec<Effect<SubId>>,
}

pub fn reduce<SubId: Clone + Eq + std::hash::Hash>(
    state: State<SubId>,
    event: Event<SubId>,
) -> Transition<SubId> {
    match event {
        Event::EntryAppended { payload } => {
            let entry_index = state.entries.len();
            let entries = state.entries.append(payload.clone());
            let effects = if state.subscribers.is_empty() {
                Vec::new()
            } else {
                vec![Effect::NotifySubscribers {
                    entry_index,
                    payload,
                    subscribers: state.subscribers.iter().cloned().collect(),
                }]
            };
            Transition {
                state: State { entries, ..state },
                effects,
            }
        }
        Event::Subscribed { subscriber_id } => Transition {
            state: State {
                subscribers: state.subscribers.update(subscriber_id),
                ..state
            },
            effects: Vec::new(),
        },
        Event::Unsubscribed { subscriber_id } => Transition {
            state: State {
                subscribers: state.subscribers.without(&subscriber_id),
                ..state
            },
            effects: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn log_with_entries(n: usize) -> AppendLog {
        (0..n).fold(AppendLog::new(), |log, i| log.append(json!(i)))
    }

    #[test]
    fn append_log_new_is_empty() {
        let log = AppendLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn append_log_append_increases_len() {
        let log = AppendLog::new().append(json!("a"));
        assert_eq!(log.len(), 1);
        assert_eq!(log.get(0), Some(&json!("a")));
    }

    #[test]
    fn append_log_preserves_order() {
        let log = AppendLog::new()
            .append(json!("a"))
            .append(json!("b"))
            .append(json!("c"));
        assert_eq!(log.get(0), Some(&json!("a")));
        assert_eq!(log.get(1), Some(&json!("b")));
        assert_eq!(log.get(2), Some(&json!("c")));
    }

    #[test]
    fn append_log_get_out_of_bounds() {
        let log = AppendLog::new().append(json!("a"));
        assert_eq!(log.get(1), None);
    }

    #[test]
    fn slice_empty_log() {
        let log = AppendLog::new();
        assert_eq!(log.slice(0, 10), &[] as &[Value]);
    }

    #[test]
    fn slice_from_start() {
        let log = log_with_entries(5);
        let result = log.slice(0, 3);
        assert_eq!(result, &[json!(0), json!(1), json!(2)]);
    }

    #[test]
    fn slice_with_offset() {
        let log = log_with_entries(5);
        let result = log.slice(2, 2);
        assert_eq!(result, &[json!(2), json!(3)]);
    }

    #[test]
    fn slice_limit_exceeds_remaining() {
        let log = log_with_entries(3);
        let result = log.slice(1, 100);
        assert_eq!(result, &[json!(1), json!(2)]);
    }

    #[test]
    fn slice_after_exceeds_len() {
        let log = log_with_entries(3);
        assert_eq!(log.slice(10, 5), &[] as &[Value]);
    }

    #[test]
    fn slice_zero_limit() {
        let log = log_with_entries(3);
        assert_eq!(log.slice(0, 0), &[] as &[Value]);
    }

    #[test]
    fn slice_entire_log() {
        let log = log_with_entries(3);
        let result = log.slice(0, 3);
        assert_eq!(result, &[json!(0), json!(1), json!(2)]);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;
        use serde_json::json;

        const SUB_IDS: &[&str] = &["sub0", "sub1", "sub2"];

        fn arb_payload() -> impl Strategy<Value = Value> {
            prop_oneof![
                Just(json!({"msg": "hello"})),
                Just(json!(42)),
                Just(json!("text")),
                Just(json!(null)),
            ]
        }

        fn arb_event() -> impl Strategy<Value = Event<String>> {
            prop_oneof![
                arb_payload().prop_map(|payload| Event::EntryAppended { payload }),
                prop::sample::select(SUB_IDS).prop_map(|id| Event::Subscribed {
                    subscriber_id: String::from(id)
                }),
                prop::sample::select(SUB_IDS).prop_map(|id| Event::Unsubscribed {
                    subscriber_id: String::from(id)
                }),
            ]
        }

        fn arb_subscribers() -> impl Strategy<Value = HashSet<String>> {
            proptest::collection::btree_set(
                prop::sample::select(SUB_IDS).prop_map(String::from),
                0..=3,
            )
            .prop_map(|s| s.into_iter().collect())
        }

        fn arb_entries() -> impl Strategy<Value = AppendLog> {
            proptest::collection::vec(arb_payload(), 0..10).prop_map(|payloads| {
                payloads
                    .into_iter()
                    .fold(AppendLog::new(), |log, p| log.append(p))
            })
        }

        fn arb_state() -> impl Strategy<Value = State<String>> {
            (arb_entries(), arb_subscribers()).prop_map(|(entries, subscribers)| State {
                entries,
                subscribers,
            })
        }

        proptest! {
            // S6: NotifySubscribers emitted iff subscribers non-empty at append time
            #[test]
            fn s6_notification_iff_subscribers_nonempty(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let event = Event::EntryAppended { payload };
                let transition = reduce(state.clone(), event);

                let has_notification = transition.effects.iter().any(|e|
                    matches!(e, Effect::NotifySubscribers { .. })
                );
                if state.subscribers.is_empty() {
                    prop_assert!(
                        !has_notification,
                        "NotifySubscribers emitted with no subscribers"
                    );
                } else {
                    prop_assert!(
                        has_notification,
                        "NotifySubscribers not emitted with {} subscribers",
                        state.subscribers.len()
                    );
                }
            }

            // S7: Notification payload matches appended entry and entry_index
            #[test]
            fn s7_notification_payload_correctness(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let expected_index = state.entries.len();
                let event = Event::EntryAppended { payload: payload.clone() };
                let transition = reduce(state, event);

                for effect in &transition.effects {
                    let Effect::NotifySubscribers { entry_index, payload: notified_payload, .. } = effect;
                    prop_assert_eq!(
                        *entry_index, expected_index,
                        "entry_index should equal pre-append log length"
                    );
                    prop_assert_eq!(
                        notified_payload, &payload,
                        "notification payload should match appended entry"
                    );
                }
            }

            // S8: Notification subscriber list matches subscriber set at append time
            #[test]
            fn s8_notification_subscriber_correctness(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let mut expected_subs: Vec<String> = state.subscribers.iter().cloned().collect();
                expected_subs.sort();
                let event = Event::EntryAppended { payload };
                let transition = reduce(state, event);

                for effect in &transition.effects {
                    let Effect::NotifySubscribers { subscribers, .. } = effect;
                    let mut sorted_subs = subscribers.clone();
                    sorted_subs.sort();
                    prop_assert_eq!(
                        sorted_subs, expected_subs.clone(),
                        "notification subscribers should match subscriber set"
                    );
                }
            }

            // S9: Subscribe/unsubscribe don't touch entries, append doesn't touch subscribers
            #[test]
            fn s9_subscriber_entry_independence(
                state in arb_state(),
                event in arb_event(),
            ) {
                let transition = reduce(state.clone(), event.clone());

                match &event {
                    Event::Subscribed { .. } | Event::Unsubscribed { .. } => {
                        prop_assert_eq!(
                            &transition.state.entries, &state.entries,
                            "subscribe/unsubscribe must not modify entries"
                        );
                    }
                    Event::EntryAppended { .. } => {
                        prop_assert_eq!(
                            &transition.state.subscribers, &state.subscribers,
                            "append must not modify subscribers"
                        );
                    }
                }
            }

            // S10: Subscribe/unsubscribe produce no effects
            #[test]
            fn s10_subscribe_unsubscribe_no_effects(
                state in arb_state(),
                sub_id in prop::sample::select(SUB_IDS).prop_map(String::from),
                subscribe in any::<bool>(),
            ) {
                let event = if subscribe {
                    Event::Subscribed { subscriber_id: sub_id }
                } else {
                    Event::Unsubscribed { subscriber_id: sub_id }
                };
                let transition = reduce(state, event);
                prop_assert!(
                    transition.effects.is_empty(),
                    "subscribe/unsubscribe must produce no effects, got {:?}",
                    transition.effects
                );
            }

            // S11: EntryAppended produces at most one effect
            #[test]
            fn s11_append_at_most_one_effect(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let event = Event::EntryAppended { payload };
                let transition = reduce(state, event);
                prop_assert!(
                    transition.effects.len() <= 1,
                    "EntryAppended produced {} effects, expected at most 1",
                    transition.effects.len()
                );
            }

            // Foundational: EntryAppended grows the log by exactly one entry
            #[test]
            fn append_grows_log(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let len_before = state.entries.len();
                let event = Event::EntryAppended { payload: payload.clone() };
                let transition = reduce(state, event);
                prop_assert_eq!(
                    transition.state.entries.len(), len_before + 1,
                    "append must grow log by exactly 1"
                );
                prop_assert_eq!(
                    transition.state.entries.get(len_before),
                    Some(&payload),
                    "appended entry must be at the end of the log"
                );
            }

            // Foundational: Subscribed adds subscriber to set
            #[test]
            fn subscribe_adds_subscriber(
                state in arb_state(),
                sub_id in prop::sample::select(SUB_IDS).prop_map(String::from),
            ) {
                let event = Event::Subscribed { subscriber_id: sub_id.clone() };
                let transition = reduce(state, event);
                prop_assert!(
                    transition.state.subscribers.contains(&sub_id),
                    "subscriber must be present after subscribe"
                );
            }

            // Foundational: Unsubscribed removes subscriber from set
            #[test]
            fn unsubscribe_removes_subscriber(
                state in arb_state(),
                sub_id in prop::sample::select(SUB_IDS).prop_map(String::from),
            ) {
                let event = Event::Unsubscribed { subscriber_id: sub_id.clone() };
                let transition = reduce(state, event);
                prop_assert!(
                    !transition.state.subscribers.contains(&sub_id),
                    "subscriber must not be present after unsubscribe"
                );
            }
        }
    }
}
