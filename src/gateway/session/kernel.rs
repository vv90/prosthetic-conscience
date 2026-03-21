use im::HashMap;

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

    pub fn into_entries(self) -> Vec<Value> {
        self.entries
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
    pub subscribers: HashMap<SubId, u64>,
    pub subscriber_ttl: u64,
}

impl<SubId: Clone + Eq + std::hash::Hash> Default for State<SubId> {
    fn default() -> Self {
        Self {
            entries: AppendLog::new(),
            subscribers: HashMap::new(),
            subscriber_ttl: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event<SubId> {
    EntryAppended { payload: Value },
    Subscribed { subscriber_id: SubId, tick: u64 },
    Unsubscribed { subscriber_id: SubId },
    Tick { tick: u64 },
    SubscriberHeartbeat { subscriber_id: SubId, tick: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect<SubId> {
    NotifySubscribers {
        entry_index: usize,
        payload: Value,
        subscribers: Vec<SubId>,
    },
    SubscriberRemoved {
        subscriber_id: SubId,
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
                    subscribers: state.subscribers.keys().cloned().collect(),
                }]
            };
            Transition {
                state: State { entries, ..state },
                effects,
            }
        }
        Event::Subscribed {
            subscriber_id,
            tick,
        } => {
            let deadline = tick + state.subscriber_ttl;
            Transition {
                state: State {
                    subscribers: state.subscribers.update(subscriber_id, deadline),
                    ..state
                },
                effects: Vec::new(),
            }
        }
        Event::Unsubscribed { subscriber_id } => {
            if state.subscribers.contains_key(&subscriber_id) {
                Transition {
                    state: State {
                        subscribers: state.subscribers.without(&subscriber_id),
                        ..state
                    },
                    effects: vec![Effect::SubscriberRemoved { subscriber_id }],
                }
            } else {
                Transition {
                    state,
                    effects: Vec::new(),
                }
            }
        }
        Event::Tick { tick } => {
            let mut expired = Vec::new();
            let mut surviving = state.subscribers.clone();
            for (sub_id, deadline) in state.subscribers.iter() {
                if *deadline <= tick {
                    expired.push(sub_id.clone());
                    surviving = surviving.without(sub_id);
                }
            }
            let effects = expired
                .into_iter()
                .map(|subscriber_id| Effect::SubscriberRemoved { subscriber_id })
                .collect();
            Transition {
                state: State {
                    subscribers: surviving,
                    ..state
                },
                effects,
            }
        }
        Event::SubscriberHeartbeat {
            subscriber_id,
            tick,
        } => {
            if state.subscribers.contains_key(&subscriber_id) {
                let deadline = tick + state.subscriber_ttl;
                Transition {
                    state: State {
                        subscribers: state.subscribers.update(subscriber_id, deadline),
                        ..state
                    },
                    effects: Vec::new(),
                }
            } else {
                Transition {
                    state,
                    effects: Vec::new(),
                }
            }
        }
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

    // T1: Tick removes subscriber at deadline (deadline <= tick)
    #[test]
    fn t1_tick_removes_subscriber_at_deadline() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 10),
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(state, Event::Tick { tick: 10 });
        assert!(transition.state.subscribers.is_empty());
    }

    // T2: Tick keeps subscriber before deadline (deadline > tick)
    #[test]
    fn t2_tick_keeps_subscriber_before_deadline() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 10),
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(state, Event::Tick { tick: 9 });
        assert!(
            transition
                .state
                .subscribers
                .contains_key(&"sub1".to_string())
        );
    }

    // T3: Tick with no subscribers produces no effects
    #[test]
    fn t3_tick_no_subscribers_no_effects() {
        let state: State<String> = State::default();
        let transition = reduce(state, Event::Tick { tick: 10 });
        assert!(transition.effects.is_empty());
    }

    // T4: Tick emits SubscriberRemoved for each expired subscriber
    #[test]
    fn t4_tick_emits_subscriber_removed_for_expired() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 5).update("sub2".to_string(), 5),
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(state, Event::Tick { tick: 5 });
        let mut removed: Vec<String> = transition
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::SubscriberRemoved { subscriber_id } => Some(subscriber_id.clone()),
                _ => None,
            })
            .collect();
        removed.sort();
        assert_eq!(removed, vec!["sub1".to_string(), "sub2".to_string()]);
    }

    // T5: SubscriberHeartbeat resets deadline for known subscriber
    #[test]
    fn t5_heartbeat_resets_deadline() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 10),
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(
            state,
            Event::SubscriberHeartbeat {
                subscriber_id: "sub1".to_string(),
                tick: 20,
            },
        );
        assert_eq!(
            transition.state.subscribers.get(&"sub1".to_string()),
            Some(&25) // tick 20 + ttl 5
        );
    }

    // T6: SubscriberHeartbeat for unknown subscriber leaves state unchanged
    #[test]
    fn t6_heartbeat_unknown_subscriber_no_change() {
        let state: State<String> = State {
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(
            state.clone(),
            Event::SubscriberHeartbeat {
                subscriber_id: "unknown".to_string(),
                tick: 10,
            },
        );
        assert_eq!(transition.state.subscribers, state.subscribers);
        assert!(transition.effects.is_empty());
    }

    // T7: Subscribed sets deadline to tick + subscriber_ttl
    #[test]
    fn t7_subscribed_sets_deadline() {
        let state: State<String> = State {
            subscriber_ttl: 7,
            ..State::default()
        };
        let transition = reduce(
            state,
            Event::Subscribed {
                subscriber_id: "sub1".to_string(),
                tick: 3,
            },
        );
        assert_eq!(
            transition.state.subscribers.get(&"sub1".to_string()),
            Some(&10) // tick 3 + ttl 7
        );
    }

    // T8: SubscriberHeartbeat for known subscriber sets deadline to tick + subscriber_ttl
    #[test]
    fn t8_heartbeat_sets_correct_deadline() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 5),
            subscriber_ttl: 10,
            ..State::default()
        };
        let transition = reduce(
            state,
            Event::SubscriberHeartbeat {
                subscriber_id: "sub1".to_string(),
                tick: 15,
            },
        );
        assert_eq!(
            transition.state.subscribers.get(&"sub1".to_string()),
            Some(&25) // tick 15 + ttl 10
        );
    }

    // T9: Heartbeat prevents expiry (heartbeat then tick past original deadline)
    #[test]
    fn t9_heartbeat_prevents_expiry() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 10),
            subscriber_ttl: 5,
            ..State::default()
        };
        // Heartbeat at tick 12 resets deadline to 17
        let transition = reduce(
            state,
            Event::SubscriberHeartbeat {
                subscriber_id: "sub1".to_string(),
                tick: 12,
            },
        );
        // Tick at 15 (past original deadline 10 but before new deadline 17)
        let transition = reduce(transition.state, Event::Tick { tick: 15 });
        assert!(
            transition
                .state
                .subscribers
                .contains_key(&"sub1".to_string())
        );
        assert!(transition.effects.is_empty());
    }

    // T10: subscriber_ttl = 0 expires subscriber on next tick
    #[test]
    fn t10_zero_ttl_expires_on_next_tick() {
        let state: State<String> = State {
            subscriber_ttl: 0,
            ..State::default()
        };
        let transition = reduce(
            state,
            Event::Subscribed {
                subscriber_id: "sub1".to_string(),
                tick: 5,
            },
        );
        // deadline = tick + ttl = 5 + 0 = 5, so tick 5 should expire it
        let transition = reduce(transition.state, Event::Tick { tick: 5 });
        assert!(transition.state.subscribers.is_empty());
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::SubscriberRemoved { subscriber_id } if subscriber_id == "sub1"
        ));
    }

    // T11: Unsubscribed emits SubscriberRemoved for known subscriber
    #[test]
    fn t11_unsubscribe_emits_subscriber_removed() {
        let state = State {
            subscribers: HashMap::unit("sub1".to_string(), 10),
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(
            state,
            Event::Unsubscribed {
                subscriber_id: "sub1".to_string(),
            },
        );
        assert!(transition.state.subscribers.is_empty());
        assert_eq!(transition.effects.len(), 1);
        assert!(matches!(
            &transition.effects[0],
            Effect::SubscriberRemoved { subscriber_id } if subscriber_id == "sub1"
        ));
    }

    // T12: Unsubscribed for unknown subscriber produces no effects
    #[test]
    fn t12_unsubscribe_unknown_no_effects() {
        let state: State<String> = State {
            subscriber_ttl: 5,
            ..State::default()
        };
        let transition = reduce(
            state,
            Event::Unsubscribed {
                subscriber_id: "unknown".to_string(),
            },
        );
        assert!(transition.effects.is_empty());
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
                    subscriber_id: String::from(id),
                    tick: 0,
                }),
                prop::sample::select(SUB_IDS).prop_map(|id| Event::Unsubscribed {
                    subscriber_id: String::from(id)
                }),
                (0..100u64).prop_map(|tick| Event::Tick { tick }),
                (
                    prop::sample::select(SUB_IDS).prop_map(String::from),
                    0..100u64,
                )
                    .prop_map(|(id, tick)| Event::SubscriberHeartbeat {
                        subscriber_id: id,
                        tick,
                    }),
            ]
        }

        fn arb_subscribers() -> impl Strategy<Value = HashMap<String, u64>> {
            proptest::collection::btree_map(
                prop::sample::select(SUB_IDS).prop_map(String::from),
                0..100u64,
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
            (arb_entries(), arb_subscribers(), 0..100u64).prop_map(
                |(entries, subscribers, subscriber_ttl)| State {
                    entries,
                    subscribers,
                    subscriber_ttl,
                },
            )
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
                    if let Effect::NotifySubscribers { entry_index, payload: notified_payload, .. } = effect {
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
            }

            // S8: Notification subscriber list matches subscriber set at append time
            #[test]
            fn s8_notification_subscriber_correctness(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let mut expected_subs: Vec<String> = state.subscribers.keys().cloned().collect();
                expected_subs.sort();
                let event = Event::EntryAppended { payload };
                let transition = reduce(state, event);

                for effect in &transition.effects {
                    if let Effect::NotifySubscribers { subscribers, .. } = effect {
                        let mut sorted_subs = subscribers.clone();
                        sorted_subs.sort();
                        prop_assert_eq!(
                            sorted_subs, expected_subs.clone(),
                            "notification subscribers should match subscriber set"
                        );
                    }
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
                    Event::Tick { .. } | Event::SubscriberHeartbeat { .. } => {
                        prop_assert_eq!(
                            &transition.state.entries, &state.entries,
                            "tick/heartbeat must not modify entries"
                        );
                    }
                }
            }

            // S10: Subscribe produces no effects
            #[test]
            fn s10_subscribe_no_effects(
                state in arb_state(),
                sub_id in prop::sample::select(SUB_IDS).prop_map(String::from),
            ) {
                let event = Event::Subscribed { subscriber_id: sub_id, tick: 0 };
                let transition = reduce(state, event);
                prop_assert!(
                    transition.effects.is_empty(),
                    "subscribe must produce no effects, got {:?}",
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
                let event = Event::Subscribed { subscriber_id: sub_id.clone(), tick: 0 };
                let transition = reduce(state, event);
                prop_assert!(
                    transition.state.subscribers.contains_key(&sub_id),
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
                    !transition.state.subscribers.contains_key(&sub_id),
                    "subscriber must not be present after unsubscribe"
                );
            }

            // P1: Tick never adds subscribers (count can only decrease or stay same)
            #[test]
            fn p1_tick_never_adds_subscribers(
                state in arb_state(),
                tick in 0..200u64,
            ) {
                let before = state.subscribers.len();
                let transition = reduce(state, Event::Tick { tick });
                prop_assert!(
                    transition.state.subscribers.len() <= before,
                    "Tick increased subscriber count from {} to {}",
                    before, transition.state.subscribers.len()
                );
            }

            // P2: Tick never modifies entries or subscriber_ttl
            #[test]
            fn p2_tick_preserves_entries_and_ttl(
                state in arb_state(),
                tick in 0..200u64,
            ) {
                let transition = reduce(state.clone(), Event::Tick { tick });
                prop_assert_eq!(
                    &transition.state.entries, &state.entries,
                    "Tick must not modify entries"
                );
                prop_assert_eq!(
                    transition.state.subscriber_ttl, state.subscriber_ttl,
                    "Tick must not modify subscriber_ttl"
                );
            }

            // P3: SubscriberHeartbeat produces no effects
            #[test]
            fn p3_heartbeat_no_effects(
                state in arb_state(),
                sub_id in prop::sample::select(SUB_IDS).prop_map(String::from),
                tick in 0..200u64,
            ) {
                let transition = reduce(state, Event::SubscriberHeartbeat {
                    subscriber_id: sub_id,
                    tick,
                });
                prop_assert!(
                    transition.effects.is_empty(),
                    "SubscriberHeartbeat must produce no effects, got {:?}",
                    transition.effects
                );
            }

            // P4: SubscriberHeartbeat for unknown subscriber is a no-op (state unchanged)
            #[test]
            fn p4_heartbeat_unknown_is_noop(
                state in arb_state(),
                tick in 0..200u64,
            ) {
                // Use a subscriber ID that's never in arb_state
                let transition = reduce(state.clone(), Event::SubscriberHeartbeat {
                    subscriber_id: "never_exists".to_string(),
                    tick,
                });
                prop_assert!(
                    transition.state.subscribers == state.subscribers,
                    "SubscriberHeartbeat for unknown subscriber must not change state"
                );
                prop_assert!(
                    transition.state.entries == state.entries,
                    "SubscriberHeartbeat for unknown subscriber must not change entries"
                );
                prop_assert!(
                    transition.state.subscriber_ttl == state.subscriber_ttl,
                    "SubscriberHeartbeat for unknown subscriber must not change ttl"
                );
            }

            // P5: All surviving subscribers after Tick have deadline > tick
            #[test]
            fn p5_surviving_subscribers_have_valid_deadlines(
                state in arb_state(),
                tick in 0..200u64,
            ) {
                let transition = reduce(state, Event::Tick { tick });
                for (sub_id, deadline) in transition.state.subscribers.iter() {
                    prop_assert!(
                        *deadline > tick,
                        "Subscriber {} has deadline {} <= tick {}",
                        sub_id, deadline, tick
                    );
                }
            }

            // P6: EntryAppended does not modify subscribers (extended for HashMap)
            #[test]
            fn p6_append_preserves_subscribers(
                state in arb_state(),
                payload in arb_payload(),
            ) {
                let transition = reduce(state.clone(), Event::EntryAppended { payload });
                prop_assert!(
                    transition.state.subscribers == state.subscribers,
                    "EntryAppended must not modify subscribers"
                );
            }

            // P7: Every subscriber that enters a session eventually receives a SubscriberRemoved
            // effect (drain with ticks)
            #[test]
            fn p7_every_subscriber_eventually_removed(
                sub_ids in proptest::collection::vec(
                    prop::sample::select(SUB_IDS).prop_map(String::from),
                    1..=3,
                ),
                subscriber_ttl in 1..50u64,
            ) {
                let mut state: State<String> = State {
                    subscriber_ttl,
                    ..State::default()
                };

                // Subscribe all subscribers at tick 0
                for sub_id in &sub_ids {
                    let transition = reduce(state, Event::Subscribed {
                        subscriber_id: sub_id.clone(),
                        tick: 0,
                    });
                    state = transition.state;
                }

                let subscribed: Vec<String> = state.subscribers.keys().cloned().collect();

                // Drain with a single large tick
                let transition = reduce(state, Event::Tick { tick: subscriber_ttl + 1 });
                let mut removed: Vec<String> = transition
                    .effects
                    .iter()
                    .filter_map(|e| match e {
                        Effect::SubscriberRemoved { subscriber_id } => Some(subscriber_id.clone()),
                        _ => None,
                    })
                    .collect();
                removed.sort();
                let mut expected = subscribed;
                expected.sort();
                prop_assert_eq!(
                    removed, expected,
                    "Every subscribed subscriber must receive SubscriberRemoved"
                );
            }

            // P8: All subscribers are eventually removed from the session (given enough ticks
            // without heartbeats)
            #[test]
            fn p8_all_subscribers_eventually_removed(
                state in arb_state(),
            ) {
                // Find the max deadline across all subscribers
                let max_deadline = state
                    .subscribers
                    .values()
                    .copied()
                    .max()
                    .unwrap_or(0);

                // A tick past all deadlines should remove everyone
                let transition = reduce(state, Event::Tick { tick: max_deadline });
                prop_assert!(
                    transition.state.subscribers.is_empty(),
                    "All subscribers should be removed after tick past max deadline, but {} remain",
                    transition.state.subscribers.len()
                );
            }
        }
    }
}
