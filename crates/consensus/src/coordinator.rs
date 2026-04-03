//! Minimal session coordination interface.
//!
//! This module intentionally defines only the smallest reducer-shaped boundary
//! for stream entry reception, gap fetching, and entry submission.
//!
//! Invariants for the current surface:
//! - Receiving an entry with a known index does not modify state or produce effects.
//! - Every emitted `Effect::FetchMissing` has `limit > 0`.
//! - When a transition learns a new received index, every missing index below
//!   the maximum received index is covered by at least one `FetchMissing`
//!   interval `[after, after + limit)`.
//! - `Event::EntryCreated` emits exactly one `Effect::SubmitEntry`.

use std::collections::BTreeSet;
use std::marker::PhantomData;

/// Opaque reducer state for stream coordination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorState<T> {
    next_expected: usize,
    received: BTreeSet<usize>,
    marker: PhantomData<T>,
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
    FetchMissing { after: usize, limit: usize },
    SubmitEntry(T),
}

/// Reducer result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition<T> {
    pub state: CoordinatorState<T>,
    pub effects: Vec<Effect<T>>,
}

impl<T> CoordinatorState<T> {
    /// Create an empty coordinator state.
    pub fn new() -> Self {
        Self {
            next_expected: 0,
            received: BTreeSet::new(),
            marker: PhantomData,
        }
    }

    /// The next stream index the coordinator expects to receive.
    pub fn next_expected(&self) -> usize {
        self.next_expected
    }
}

/// Reduce one event into a new state and requested effects.
pub fn reduce<T>(state: CoordinatorState<T>, event: Event<T>) -> Transition<T> {
    match event {
        Event::Received { index, entry } => {
            let _ = entry;

            if state.received.contains(&index) {
                return Transition {
                    state,
                    effects: Vec::new(),
                };
            }

            let mut state = state;
            state.received.insert(index);

            while state.received.contains(&state.next_expected) {
                state.next_expected += 1;
            }

            let effects = match state.received.last().copied() {
                Some(max_received) if state.next_expected < max_received => {
                    vec![Effect::FetchMissing {
                        after: state.next_expected,
                        limit: max_received - state.next_expected,
                    }]
                }
                _ => Vec::new(),
            };

            Transition { state, effects }
        }
        Event::EntryCreated(entry) => Transition {
            state,
            effects: vec![Effect::SubmitEntry(entry)],
        },
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Dummy(usize);

    #[test]
    fn state_new_starts_with_zero_frontier() {
        let state = CoordinatorState::<Dummy>::new();

        assert_eq!(state.next_expected(), 0);
    }

    #[test]
    fn receiving_next_expected_advances_frontier_without_effects() {
        let state = CoordinatorState::<Dummy>::new();

        let transition = reduce(
            state,
            Event::Received {
                index: 0,
                entry: Dummy(0),
            },
        );

        assert_eq!(transition.state.next_expected(), 1);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn receiving_known_index_is_noop() {
        let first = reduce(
            CoordinatorState::<Dummy>::new(),
            Event::Received {
                index: 0,
                entry: Dummy(0),
            },
        );

        let duplicate = reduce(
            first.state.clone(),
            Event::Received {
                index: 0,
                entry: Dummy(999),
            },
        );

        assert_eq!(duplicate.state, first.state);
        assert!(duplicate.effects.is_empty());
    }

    #[test]
    fn receiving_gap_emits_fetch_missing_covering_gap_prefix() {
        let transition = reduce(
            CoordinatorState::<Dummy>::new(),
            Event::Received {
                index: 3,
                entry: Dummy(3),
            },
        );

        assert_eq!(transition.state.next_expected(), 0);
        assert_eq!(
            transition.effects,
            vec![Effect::FetchMissing { after: 0, limit: 3 }]
        );
    }

    #[test]
    fn entry_created_emits_exactly_one_submit_entry() {
        let transition = reduce(
            CoordinatorState::<Dummy>::new(),
            Event::EntryCreated(Dummy(42)),
        );

        assert_eq!(transition.effects, vec![Effect::SubmitEntry(Dummy(42))]);
    }

    #[test]
    fn type_shapes_cover_all_public_variants() {
        let received = Event::Received {
            index: 3,
            entry: Dummy(3),
        };
        let entry_created = Event::EntryCreated(Dummy(4));
        let fetch_missing = Effect::<Dummy>::FetchMissing {
            after: 9,
            limit: 100,
        };
        let submit_entry = Effect::SubmitEntry(Dummy(5));

        match received {
            Event::Received { index, entry } => {
                assert_eq!(index, 3);
                assert_eq!(entry, Dummy(3));
            }
            Event::EntryCreated(_) => panic!("constructed wrong event variant"),
        }

        match entry_created {
            Event::EntryCreated(entry) => assert_eq!(entry, Dummy(4)),
            Event::Received { .. } => panic!("constructed wrong event variant"),
        }

        match fetch_missing {
            Effect::FetchMissing { after, limit } => {
                assert_eq!(after, 9);
                assert_eq!(limit, 100);
            }
            Effect::SubmitEntry(_) => panic!("constructed wrong effect variant"),
        }

        match submit_entry {
            Effect::SubmitEntry(entry) => assert_eq!(entry, Dummy(5)),
            Effect::FetchMissing { .. } => panic!("constructed wrong effect variant"),
        }
    }

    proptest! {
        #[test]
        fn every_fetch_missing_has_positive_limit(indices in prop::collection::vec(0usize..32, 0..64)) {
            let mut state = CoordinatorState::<Dummy>::new();

            for index in indices {
                let transition = reduce(state, Event::Received { index, entry: Dummy(index) });

                for effect in &transition.effects {
                    if let Effect::FetchMissing { limit, .. } = effect {
                        prop_assert!(*limit > 0);
                    }
                }

                state = transition.state;
            }
        }

        #[test]
        fn newly_received_indices_cover_all_missing_indices(indices in prop::collection::vec(0usize..32, 0..64)) {
            let mut state = CoordinatorState::<Dummy>::new();

            for index in indices {
                let was_known = state.received.contains(&index);
                let transition = reduce(state, Event::Received { index, entry: Dummy(index) });

                if !was_known {
                    if let Some(max_received) = transition.state.received.last().copied() {
                        for missing in 0..max_received {
                            if !transition.state.received.contains(&missing) {
                                prop_assert!(
                                    transition.effects.iter().any(|effect| matches!(
                                        effect,
                                        Effect::FetchMissing { after, limit }
                                            if *limit > 0
                                                && *after <= missing
                                                && missing < after.saturating_add(*limit)
                                    )),
                                    "missing index {missing} was not covered by any FetchMissing effect"
                                );
                            }
                        }
                    }
                } else {
                    prop_assert!(transition.effects.is_empty());
                }

                state = transition.state;
            }
        }

        #[test]
        fn each_entry_created_emits_exactly_one_submit_entry(values in prop::collection::vec(0usize..32, 0..64)) {
            let mut state = CoordinatorState::<Dummy>::new();

            for value in values {
                let transition = reduce(state, Event::EntryCreated(Dummy(value)));

                prop_assert_eq!(transition.effects.len(), 1);
                prop_assert_eq!(transition.effects.first(), Some(&Effect::SubmitEntry(Dummy(value))));

                state = transition.state;
            }
        }
    }
}
