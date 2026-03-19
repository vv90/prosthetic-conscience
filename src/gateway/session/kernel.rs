use im::OrdSet;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State<SubId: Clone + Ord> {
    pub entries: Vec<Value>,
    pub subscribers: OrdSet<SubId>,
}

impl<SubId: Clone + Ord> Default for State<SubId> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            subscribers: OrdSet::new(),
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

pub struct Transition<SubId: Clone + Ord> {
    pub state: State<SubId>,
    pub effects: Vec<Effect<SubId>>,
}

pub fn reduce<SubId: Clone + Ord>(state: State<SubId>, _event: Event<SubId>) -> Transition<SubId> {
    Transition {
        state,
        effects: Vec::new(),
    }
}

#[cfg(test)]
mod tests {}
