use im::OrdSet;

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
pub struct State<SubId: Clone + Ord> {
    pub entries: AppendLog,
    pub subscribers: OrdSet<SubId>,
}

impl<SubId: Clone + Ord> Default for State<SubId> {
    fn default() -> Self {
        Self {
            entries: AppendLog::new(),
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
}
