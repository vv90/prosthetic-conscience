//! Pure session coordination reducer.
//!
//! This module defines a small reducer-shaped boundary for:
//! - bootstrapping from the latest known indexed entry
//! - receiving live or fetched stream entries
//! - planning fetches for missing ranges
//! - emitting submission requests for locally created entries
//!
//! The coordinator is synchronous, performs no I/O, and must never panic in
//! non-test code.

/// A latest known indexed entry used for bootstrap or resync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestEntry<T> {
    pub index: usize,
    pub entry: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot<T> {
    Requested,
    Received(T),
}

/// Opaque reducer state for stream coordination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorState<T> {
    slots: Vec<Slot<T>>,
    page_limit: usize,
}

/// Inputs to the coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<T> {
    Received { index: usize, entry: T },
    EntryCreated(T),
}

/// Outputs requested by the coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect<T> {
    FetchMissing { from: usize, limit: usize },
    SubmitEntry(T),
}

/// Reducer result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition<T> {
    pub state: CoordinatorState<T>,
    pub effects: Vec<Effect<T>>,
}

/// Errors from coordinator bootstrap.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InitError {
    #[error("page_limit must be > 0")]
    InvalidPageLimit,
}

impl<T> CoordinatorState<T> {
    /// The first missing slot, or the known upper bound if no slots are missing.
    pub fn next_expected(&self) -> usize {
        self.slots
            .iter()
            .position(|slot| matches!(slot, Slot::Requested))
            .unwrap_or(self.slots.len())
    }
}

/// Create coordinator state from a page limit and optional latest known entry.
pub fn init<T>(
    page_limit: usize,
    latest: Option<LatestEntry<T>>,
) -> Result<Transition<T>, InitError> {
    if page_limit == 0 {
        return Err(InitError::InvalidPageLimit);
    }

    let state = CoordinatorState {
        slots: Vec::new(),
        page_limit,
    };

    Ok(sync_to_latest(state, latest))
}

/// Update coordinator state to the latest known entry without shrinking state.
pub fn sync_to_latest<T>(
    mut state: CoordinatorState<T>,
    latest: Option<LatestEntry<T>>,
) -> Transition<T> {
    if let Some(latest) = latest {
        apply_latest(&mut state, latest);
        let effects = plan_missing_fetches(&state);
        return Transition { state, effects };
    }

    Transition {
        state,
        effects: Vec::new(),
    }
}

/// Reduce one event into a new state and requested effects.
pub fn reduce<T>(mut state: CoordinatorState<T>, event: Event<T>) -> Transition<T> {
    match event {
        Event::Received { index, entry } => {
            if let Some(slot) = state.slots.get_mut(index) {
                return match slot {
                    Slot::Received(_) => Transition {
                        state,
                        effects: Vec::new(),
                    },
                    Slot::Requested => {
                        *slot = Slot::Received(entry);
                        Transition {
                            state,
                            effects: Vec::new(),
                        }
                    }
                };
            }

            state.slots.resize_with(index + 1, || Slot::Requested);
            if let Some(slot) = state.slots.get_mut(index) {
                *slot = Slot::Received(entry);
            }
            let effects = plan_missing_fetches(&state);
            Transition { state, effects }
        }
        Event::EntryCreated(entry) => Transition {
            state,
            effects: vec![Effect::SubmitEntry(entry)],
        },
    }
}

fn apply_latest<T>(state: &mut CoordinatorState<T>, latest: LatestEntry<T>) {
    if latest.index >= state.slots.len() {
        state
            .slots
            .resize_with(latest.index + 1, || Slot::Requested);
    }

    if matches!(state.slots.get(latest.index), Some(Slot::Requested))
        && let Some(slot) = state.slots.get_mut(latest.index)
    {
        *slot = Slot::Received(latest.entry);
    }
}

fn plan_missing_fetches<T>(state: &CoordinatorState<T>) -> Vec<Effect<T>> {
    let mut effects = Vec::new();
    let mut index = 0;

    while index < state.slots.len() {
        if !matches!(state.slots.get(index), Some(Slot::Requested)) {
            index += 1;
            continue;
        }

        let start = index;
        while index < state.slots.len() && matches!(state.slots.get(index), Some(Slot::Requested)) {
            index += 1;
        }

        let end = index;
        let mut from = start;
        while from < end {
            let limit = (end - from).min(state.page_limit);
            effects.push(Effect::FetchMissing { from, limit });
            from += limit;
        }
    }

    effects
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Dummy(usize);

    #[derive(Debug, Clone)]
    enum Op {
        Receive { index: usize, value: usize },
        SyncLatest(Option<(usize, usize)>),
    }

    fn latest(index: usize, value: usize) -> LatestEntry<Dummy> {
        LatestEntry {
            index,
            entry: Dummy(value),
        }
    }

    fn fetch_ranges(effects: &[Effect<Dummy>]) -> Vec<(usize, usize)> {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::FetchMissing { from, limit } => Some((*from, *limit)),
                Effect::SubmitEntry(_) => None,
            })
            .collect()
    }

    fn requested_indices(state: &CoordinatorState<Dummy>) -> Vec<usize> {
        state
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Requested => Some(index),
                Slot::Received(_) => None,
            })
            .collect()
    }

    fn received_values(state: &CoordinatorState<Dummy>) -> BTreeMap<usize, Dummy> {
        state
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match slot {
                Slot::Requested => None,
                Slot::Received(value) => Some((index, value.clone())),
            })
            .collect()
    }

    fn is_covered(index: usize, effects: &[Effect<Dummy>]) -> bool {
        effects.iter().any(|effect| match effect {
            Effect::FetchMissing { from, limit } => {
                *from <= index && index < from.saturating_add(*limit)
            }
            Effect::SubmitEntry(_) => false,
        })
    }

    fn apply_op(state: CoordinatorState<Dummy>, op: Op) -> Transition<Dummy> {
        match op {
            Op::Receive { index, value } => reduce(
                state,
                Event::Received {
                    index,
                    entry: Dummy(value),
                },
            ),
            Op::SyncLatest(Some((index, value))) => {
                sync_to_latest(state, Some(latest(index, value)))
            }
            Op::SyncLatest(None) => sync_to_latest(state, None),
        }
    }

    #[test]
    fn init_without_latest_returns_empty_state_and_no_effects() {
        let transition = init::<Dummy>(3, None);

        assert!(transition.state.slots.is_empty());
        assert_eq!(transition.state.next_expected(), 0);
        assert!(transition.effects.is_empty());
    }

    #[test]
    #[should_panic(expected = "page_limit must be > 0")]
    fn init_with_zero_page_limit_panics() {
        let _ = init::<Dummy>(0, None);
    }

    #[test]
    fn init_with_latest_creates_requested_holes_and_fetches_them() {
        let transition = init(2, Some(latest(3, 30)));

        assert_eq!(
            transition.state.slots,
            vec![
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
                Slot::Received(Dummy(30)),
            ]
        );
        assert_eq!(transition.state.next_expected(), 0);
        assert_eq!(
            transition.effects,
            vec![
                Effect::FetchMissing { from: 0, limit: 2 },
                Effect::FetchMissing { from: 2, limit: 1 },
            ]
        );
    }

    #[test]
    fn sync_to_latest_none_is_noop() {
        let initial = init(3, Some(latest(2, 20))).state;

        let transition = sync_to_latest(initial.clone(), None::<LatestEntry<Dummy>>);

        assert_eq!(transition.state, initial);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn sync_to_latest_never_shrinks_state() {
        let initial = init(3, Some(latest(5, 50))).state;

        let transition = sync_to_latest(initial.clone(), Some(latest(2, 20)));

        assert_eq!(transition.state.slots.len(), initial.slots.len());
        assert_eq!(transition.state.slots[5], Slot::Received(Dummy(50)));
    }

    #[test]
    fn sync_to_latest_fills_requested_latest_slot() {
        let initial = init(4, Some(latest(5, 50))).state;

        let transition = sync_to_latest(initial, Some(latest(3, 30)));

        assert_eq!(transition.state.slots[3], Slot::Received(Dummy(30)));
        assert_eq!(
            transition.effects,
            vec![
                Effect::FetchMissing { from: 0, limit: 3 },
                Effect::FetchMissing { from: 4, limit: 1 },
            ]
        );
    }

    #[test]
    fn sync_to_latest_preserves_existing_received_value() {
        let initial = reduce(
            init::<Dummy>(4, None).state,
            Event::Received {
                index: 3,
                entry: Dummy(30),
            },
        )
        .state;

        let transition = sync_to_latest(initial, Some(latest(3, 999)));

        assert_eq!(transition.state.slots[3], Slot::Received(Dummy(30)));
    }

    #[test]
    fn duplicate_received_preserves_first_value_and_emits_no_effects() {
        let first = reduce(
            init::<Dummy>(4, None).state,
            Event::Received {
                index: 2,
                entry: Dummy(20),
            },
        );

        let duplicate = reduce(
            first.state.clone(),
            Event::Received {
                index: 2,
                entry: Dummy(999),
            },
        );

        assert_eq!(duplicate.state, first.state);
        assert!(duplicate.effects.is_empty());
        assert_eq!(duplicate.state.slots[2], Slot::Received(Dummy(20)));
    }

    #[test]
    fn received_requested_slot_fills_without_effects() {
        let initial = init(4, Some(latest(3, 30))).state;

        let transition = reduce(
            initial,
            Event::Received {
                index: 1,
                entry: Dummy(10),
            },
        );

        assert_eq!(transition.state.slots[1], Slot::Received(Dummy(10)));
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn future_received_extends_state_and_emits_eager_fetches() {
        let transition = reduce(
            init::<Dummy>(2, None).state,
            Event::Received {
                index: 4,
                entry: Dummy(40),
            },
        );

        assert_eq!(
            transition.state.slots,
            vec![
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
                Slot::Received(Dummy(40)),
            ]
        );
        assert_eq!(
            transition.effects,
            vec![
                Effect::FetchMissing { from: 0, limit: 2 },
                Effect::FetchMissing { from: 2, limit: 2 },
            ]
        );
    }

    #[test]
    fn entry_created_emits_exactly_one_submit_entry() {
        let transition = reduce(init::<Dummy>(4, None).state, Event::EntryCreated(Dummy(42)));

        assert_eq!(transition.effects, vec![Effect::SubmitEntry(Dummy(42))]);
    }

    #[test]
    fn next_expected_matches_first_requested_slot_or_len() {
        let mut state = init(4, Some(latest(3, 30))).state;
        assert_eq!(state.next_expected(), 0);

        state = reduce(
            state,
            Event::Received {
                index: 0,
                entry: Dummy(0),
            },
        )
        .state;
        assert_eq!(state.next_expected(), 1);

        state = reduce(
            state,
            Event::Received {
                index: 1,
                entry: Dummy(1),
            },
        )
        .state;
        assert_eq!(state.next_expected(), 2);

        state = reduce(
            state,
            Event::Received {
                index: 2,
                entry: Dummy(2),
            },
        )
        .state;
        assert_eq!(state.next_expected(), 4);
    }

    #[test]
    fn type_shapes_cover_all_public_variants() {
        let latest_entry = latest(4, 40);
        assert_eq!(latest_entry.index, 4);
        assert_eq!(latest_entry.entry, Dummy(40));

        let all_events = vec![
            Event::Received {
                index: 3,
                entry: Dummy(30),
            },
            Event::EntryCreated(Dummy(50)),
        ];

        for event in all_events {
            match event {
                Event::Received { index, entry } => {
                    assert_eq!(index, 3);
                    assert_eq!(entry, Dummy(30));
                }
                Event::EntryCreated(entry) => assert_eq!(entry, Dummy(50)),
            }
        }

        let all_effects = vec![
            Effect::FetchMissing { from: 2, limit: 3 },
            Effect::SubmitEntry(Dummy(60)),
        ];

        for effect in all_effects {
            match effect {
                Effect::FetchMissing { from, limit } => {
                    assert_eq!(from, 2);
                    assert_eq!(limit, 3);
                }
                Effect::SubmitEntry(entry) => assert_eq!(entry, Dummy(60)),
            }
        }
    }

    proptest! {
        #[test]
        fn first_writer_wins_for_duplicate_indices(
            events in prop::collection::vec((0usize..32, 0usize..1000), 0..64)
        ) {
            let mut state = init::<Dummy>(5, None).state;
            let mut expected = BTreeMap::new();

            for (index, value) in events {
                let transition = reduce(state, Event::Received { index, entry: Dummy(value) });
                expected.entry(index).or_insert(Dummy(value));
                prop_assert_eq!(received_values(&transition.state), expected.clone());
                state = transition.state;
            }
        }

        #[test]
        fn future_received_covers_all_missing_slots_below_upper_bound(
            events in prop::collection::vec((0usize..32, 0usize..1000), 0..64)
        ) {
            let mut state = init::<Dummy>(5, None).state;

            for (index, value) in events {
                let len_before = state.slots.len();
                let transition = reduce(state, Event::Received { index, entry: Dummy(value) });

                if index >= len_before {
                    for missing in requested_indices(&transition.state) {
                        prop_assert!(
                            is_covered(missing, &transition.effects),
                            "missing slot {missing} was not covered by fetch effects"
                        );
                    }
                }

                state = transition.state;
            }
        }

        #[test]
        fn fetch_effects_are_ascending_non_overlapping_and_bounded(
            ops in prop::collection::vec(
                prop_oneof![
                    (0usize..32, 0usize..1000).prop_map(|(index, value)| Op::Receive { index, value }),
                    prop::option::of((0usize..32, 0usize..1000))
                        .prop_map(Op::SyncLatest),
                ],
                0..64,
            )
        ) {
            let page_limit = 5;
            let mut state = init::<Dummy>(page_limit, None).state;

            for op in ops {
                let transition = apply_op(state, op);
                let fetches = fetch_ranges(&transition.effects);

                let mut prev_end = 0usize;
                let mut first = true;
                for (from, limit) in fetches {
                    prop_assert!(limit > 0);
                    prop_assert!(limit <= page_limit);
                    if !first {
                        prop_assert!(from >= prev_end);
                    }
                    prev_end = from + limit;
                    first = false;
                }

                state = transition.state;
            }
        }

        #[test]
        fn next_expected_monotonic_non_decreasing_across_received_events(
            events in prop::collection::vec((0usize..32, 0usize..1000), 0..64)
        ) {
            let mut state = init::<Dummy>(5, None).state;
            let mut previous = state.next_expected();

            for (index, value) in events {
                let transition = reduce(state, Event::Received { index, entry: Dummy(value) });
                let next = transition.state.next_expected();
                prop_assert!(next >= previous);
                previous = next;
                state = transition.state;
            }
        }

        #[test]
        fn next_expected_matches_slot_layout_after_ops(
            ops in prop::collection::vec(
                prop_oneof![
                    (0usize..32, 0usize..1000).prop_map(|(index, value)| Op::Receive { index, value }),
                    prop::option::of((0usize..32, 0usize..1000))
                        .prop_map(Op::SyncLatest),
                ],
                0..64,
            )
        ) {
            let mut state = init::<Dummy>(5, None).state;

            for op in ops {
                let transition = apply_op(state, op);
                let next_expected = transition.state.next_expected();

                for slot in &transition.state.slots[..next_expected] {
                    prop_assert!(matches!(slot, Slot::Received(_)));
                }

                if next_expected < transition.state.slots.len() {
                    prop_assert!(matches!(transition.state.slots[next_expected], Slot::Requested));
                }

                state = transition.state;
            }
        }
    }
}
