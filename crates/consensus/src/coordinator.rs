//! Pure session coordination reducer.
//!
//! This module defines a small reducer-shaped boundary for:
//! - bootstrapping from the latest known entry index
//! - receiving live or fetched stream entries
//! - planning fetches for missing ranges
//!
//! The coordinator is synchronous, performs no I/O, and must never panic in
//! non-test code.

use std::marker::PhantomData;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Slot<T> {
    Requested,
    Received(T),
}

/// Opaque reducer state for stream coordination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State<T> {
    slots: Vec<Slot<T>>,
    page_limit: usize,
}

/// Inputs to the coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event<T> {
    Received { index: usize, entry: T },
}

/// Outputs requested by the coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Effect<T> {
    FetchMissing {
        from: usize,
        limit: usize,
        #[serde(skip)]
        _marker: PhantomData<T>,
    },
}

/// Reducer result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition<T> {
    pub state: State<T>,
    pub effects: Vec<Effect<T>>,
}

/// Errors from coordinator bootstrap.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InitError {
    #[error("page_limit must be > 0")]
    InvalidPageLimit,
}

impl<T> Effect<T> {
    pub(crate) fn fetch_missing(from: usize, limit: usize) -> Self {
        Self::FetchMissing {
            from,
            limit,
            _marker: PhantomData,
        }
    }
}

impl<T> State<T> {
    pub(crate) fn empty(page_limit: usize) -> Self {
        debug_assert!(page_limit > 0);
        Self {
            slots: Vec::new(),
            page_limit,
        }
    }

    /// The first missing slot, or the known upper bound if no slots are missing.
    pub fn next_expected(&self) -> usize {
        self.slots
            .iter()
            .position(|slot| matches!(slot, Slot::Requested))
            .unwrap_or(self.slots.len())
    }

    /// The contiguous committed prefix, excluding any buffered future entries.
    pub fn committed_prefix(&self) -> impl Iterator<Item = &T> {
        let next_expected = self.next_expected();
        self.slots
            .iter()
            .take(next_expected)
            .filter_map(|slot| match slot {
                Slot::Requested => None,
                Slot::Received(entry) => Some(entry),
            })
    }
}

/// Create coordinator state from a page limit and optional latest known entry index.
pub fn init<T>(
    page_limit: usize,
    latest_entry_index: Option<usize>,
) -> Result<Transition<T>, InitError> {
    if page_limit == 0 {
        return Err(InitError::InvalidPageLimit);
    }

    let state = State::empty(page_limit);

    Ok(sync_to_latest(state, latest_entry_index))
}

/// Update coordinator state to the latest known entry index without shrinking state.
pub fn sync_to_latest<T>(mut state: State<T>, latest_entry_index: Option<usize>) -> Transition<T> {
    if let Some(latest_entry_index) = latest_entry_index {
        apply_latest_index(&mut state, latest_entry_index);
        let effects = plan_missing_fetches(&state);
        return Transition { state, effects };
    }

    Transition {
        state,
        effects: Vec::new(),
    }
}

/// Reduce one event into a new state and requested effects.
pub fn reduce<T>(mut state: State<T>, event: Event<T>) -> Transition<T> {
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
    }
}

fn apply_latest_index<T>(state: &mut State<T>, latest_entry_index: usize) {
    if latest_entry_index >= state.slots.len() {
        state
            .slots
            .resize_with(latest_entry_index + 1, || Slot::Requested);
    }
}

fn plan_missing_fetches<T>(state: &State<T>) -> Vec<Effect<T>> {
    let mut effects: Vec<Effect<T>> = Vec::new();
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
            effects.push(Effect::fetch_missing(from, limit));
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
        SyncLatest(Option<usize>),
    }

    /// Unwrapping helper for tests — `init` returns Result now.
    fn test_init(page_limit: usize, latest_entry_index: Option<usize>) -> Transition<Dummy> {
        init(page_limit, latest_entry_index).unwrap()
    }

    fn fetch_ranges(effects: &[Effect<Dummy>]) -> Vec<(usize, usize)> {
        effects
            .iter()
            .map(|effect| match effect {
                Effect::FetchMissing { from, limit, .. } => (*from, *limit),
            })
            .collect()
    }

    fn requested_indices(state: &State<Dummy>) -> Vec<usize> {
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

    fn received_values(state: &State<Dummy>) -> BTreeMap<usize, Dummy> {
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
            Effect::FetchMissing { from, limit, .. } => {
                *from <= index && index < from.saturating_add(*limit)
            }
        })
    }

    fn apply_op(state: State<Dummy>, op: Op) -> Transition<Dummy> {
        match op {
            Op::Receive { index, value } => reduce(
                state,
                Event::Received {
                    index,
                    entry: Dummy(value),
                },
            ),
            Op::SyncLatest(latest_entry_index) => sync_to_latest(state, latest_entry_index),
        }
    }

    #[test]
    fn init_without_latest_returns_empty_state_and_no_effects() {
        let transition = test_init(3, None);

        assert!(transition.state.slots.is_empty());
        assert_eq!(transition.state.next_expected(), 0);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn init_with_zero_page_limit_returns_error() {
        assert!(init::<Dummy>(0, None).is_err());
    }

    #[test]
    fn init_with_latest_entry_index_creates_requested_holes_and_fetches_them() {
        let transition = test_init(2, Some(3));

        assert_eq!(
            transition.state.slots,
            vec![
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
            ]
        );
        assert_eq!(transition.state.next_expected(), 0);
        assert_eq!(
            transition.effects,
            vec![Effect::fetch_missing(0, 2), Effect::fetch_missing(2, 2),]
        );
    }

    #[test]
    fn sync_to_latest_none_is_noop() {
        let initial = test_init(3, Some(2)).state;

        let transition = sync_to_latest(initial.clone(), None);

        assert_eq!(transition.state, initial);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn sync_to_latest_never_shrinks_state() {
        let initial = test_init(3, Some(5)).state;

        let transition = sync_to_latest(initial.clone(), Some(2));

        assert_eq!(transition.state.slots.len(), initial.slots.len());
        assert_eq!(transition.state.slots[5], Slot::Requested);
    }

    #[test]
    fn sync_to_latest_extends_requested_holes_without_shrinking() {
        let initial = test_init(4, Some(1)).state;

        let transition = sync_to_latest(initial, Some(3));

        assert_eq!(
            transition.state.slots,
            vec![
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
                Slot::Requested,
            ]
        );
        assert_eq!(transition.effects, vec![Effect::fetch_missing(0, 4),]);
    }

    #[test]
    fn sync_to_latest_preserves_existing_received_value() {
        let initial = reduce(
            test_init(4, None).state,
            Event::Received {
                index: 3,
                entry: Dummy(30),
            },
        )
        .state;

        let transition = sync_to_latest(initial, Some(3));

        assert_eq!(transition.state.slots[3], Slot::Received(Dummy(30)));
    }

    #[test]
    fn duplicate_received_preserves_first_value_and_emits_no_effects() {
        let first = reduce(
            test_init(4, None).state,
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
        let initial = test_init(4, Some(3)).state;

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
            test_init(2, None).state,
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
            vec![Effect::fetch_missing(0, 2), Effect::fetch_missing(2, 2),]
        );
    }

    #[test]
    fn next_expected_matches_first_requested_slot_or_len() {
        let mut state = test_init(4, Some(3)).state;
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
        assert_eq!(state.next_expected(), 3);
    }

    #[test]
    fn type_shapes_cover_all_public_variants() {
        let all_events = vec![Event::Received {
            index: 3,
            entry: Dummy(30),
        }];

        for event in all_events {
            match event {
                Event::Received { index, entry } => {
                    assert_eq!(index, 3);
                    assert_eq!(entry, Dummy(30));
                }
            }
        }

        let all_effects: Vec<Effect<Dummy>> = vec![Effect::fetch_missing(2, 3)];

        for effect in all_effects {
            match effect {
                Effect::FetchMissing { from, limit, .. } => {
                    assert_eq!(from, 2);
                    assert_eq!(limit, 3);
                }
            }
        }
    }

    #[test]
    fn committed_prefix_omits_buffered_future_entries_until_gaps_are_filled() {
        let state = reduce(
            test_init(4, None).state,
            Event::Received {
                index: 2,
                entry: Dummy(20),
            },
        )
        .state;
        assert_eq!(
            state.committed_prefix().cloned().collect::<Vec<_>>(),
            Vec::<Dummy>::new()
        );

        let state = reduce(
            state,
            Event::Received {
                index: 0,
                entry: Dummy(0),
            },
        )
        .state;
        assert_eq!(
            state.committed_prefix().cloned().collect::<Vec<_>>(),
            vec![Dummy(0)]
        );

        let state = reduce(
            state,
            Event::Received {
                index: 1,
                entry: Dummy(10),
            },
        )
        .state;
        assert_eq!(
            state.committed_prefix().cloned().collect::<Vec<_>>(),
            vec![Dummy(0), Dummy(10), Dummy(20)]
        );
    }

    proptest! {
        #[test]
        fn first_writer_wins_for_duplicate_indices(
            events in prop::collection::vec((0usize..32, 0usize..1000), 0..64)
        ) {
            let mut state = test_init(5, None).state;
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
            let mut state = test_init(5, None).state;

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
                    prop::option::of(0usize..32).prop_map(Op::SyncLatest),
                ],
                0..64,
            )
        ) {
            let page_limit = 5;
            let mut state = test_init(page_limit, None).state;

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
        fn next_expected_matches_slot_layout_after_ops(
            ops in prop::collection::vec(
                prop_oneof![
                    (0usize..32, 0usize..1000).prop_map(|(index, value)| Op::Receive { index, value }),
                    prop::option::of(0usize..32).prop_map(Op::SyncLatest),
                ],
                0..64,
            )
        ) {
            let mut state = test_init(5, None).state;

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

        // S1: Received slots are never overwritten.
        // S2: slots.len() never decreases.
        // N1: next_expected() never decreases.
        #[test]
        fn state_monotonicity_across_all_ops(
            ops in prop::collection::vec(
                prop_oneof![
                    (0usize..32, 0usize..1000).prop_map(|(index, value)| Op::Receive { index, value }),
                    prop::option::of(0usize..32).prop_map(Op::SyncLatest),
                ],
                0..64,
            )
        ) {
            let mut state = test_init(5, None).state;
            let mut prev_len = state.slots.len();
            let mut prev_next = state.next_expected();
            let mut prev_received = received_values(&state);

            for op in ops {
                let transition = apply_op(state, op);

                // S2: slots never shrink.
                prop_assert!(
                    transition.state.slots.len() >= prev_len,
                    "slots.len() decreased from {} to {}",
                    prev_len, transition.state.slots.len()
                );

                // N1: next_expected never decreases.
                let next = transition.state.next_expected();
                prop_assert!(
                    next >= prev_next,
                    "next_expected decreased from {} to {}",
                    prev_next, next
                );

                // S1: previously Received slots keep their values.
                let new_received = received_values(&transition.state);
                for (idx, value) in &prev_received {
                    prop_assert!(
                        new_received.get(idx) == Some(value),
                        "Received slot {} was overwritten", idx
                    );
                }

                prev_len = transition.state.slots.len();
                prev_next = next;
                prev_received = new_received;
                state = transition.state;
            }
        }

        // F1 (bound): every FetchMissing range fits within slots.len().
        // F3: every Requested slot created by any op is covered by a FetchMissing.
        // F4: no FetchMissing when no new Requested slots are created.
        #[test]
        fn fetch_coverage_across_all_ops(
            ops in prop::collection::vec(
                prop_oneof![
                    (0usize..32, 0usize..1000).prop_map(|(index, value)| Op::Receive { index, value }),
                    prop::option::of(0usize..32).prop_map(Op::SyncLatest),
                ],
                0..64,
            )
        ) {
            let page_limit = 5;
            let mut state = test_init(page_limit, None).state;

            for op in ops {
                let prev_len = state.slots.len();
                let is_receive = matches!(op, Op::Receive { .. });
                let prev_requested: Vec<usize> = requested_indices(&state);
                let transition = apply_op(state, op);
                let new_requested: Vec<usize> = requested_indices(&transition.state);

                // Newly created Requested slots = in new but not in prev.
                let prev_set: std::collections::HashSet<usize> = prev_requested.iter().copied().collect();
                let newly_requested: Vec<usize> = new_requested.iter()
                    .filter(|i| !prev_set.contains(i))
                    .copied()
                    .collect();

                // F1 (bound): from + limit <= slots.len().
                for effect in &transition.effects {
                    let Effect::FetchMissing { from, limit, .. } = effect;
                    prop_assert!(
                        from + limit <= transition.state.slots.len(),
                        "FetchMissing {{ from: {from}, limit: {limit} }} exceeds slots.len() {}",
                        transition.state.slots.len()
                    );
                }

                // F3: every newly created Requested slot is covered.
                for index in &newly_requested {
                    prop_assert!(
                        is_covered(*index, &transition.effects),
                        "newly Requested slot {index} not covered by any FetchMissing"
                    );
                }

                // F4: Received at an index that does not extend slots
                // emits no FetchMissing effects.
                if is_receive && transition.state.slots.len() == prev_len {
                    let has_fetch = transition.effects.iter()
                        .any(|e| matches!(e, Effect::FetchMissing { .. }));
                    prop_assert!(
                        !has_fetch,
                        "FetchMissing emitted by Received that did not extend slots"
                    );
                }

                state = transition.state;
            }
        }

    }
}
