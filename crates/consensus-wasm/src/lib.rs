//! Thin wasm-bindgen wrapper for the consensus app state machine.

use consensus::{app, coordinator, types::Entry};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct DispatchResult {
    view: app::View,
    effects: Vec<app::Effect>,
}

#[wasm_bindgen]
pub struct ConsensusAppHandle {
    participant: String,
    state: Option<app::State>,
}

#[wasm_bindgen]
impl ConsensusAppHandle {
    #[wasm_bindgen(constructor)]
    pub fn new(participant: String) -> Self {
        Self {
            participant,
            state: None,
        }
    }

    pub fn bootstrap(&mut self, latest_entry_index: Option<usize>) -> Result<JsValue, JsValue> {
        let result = self.bootstrap_model(latest_entry_index).map_err(js_error)?;
        to_js_value(&result)
    }

    pub fn view(&self) -> Result<JsValue, JsValue> {
        let view = self.view_model().map_err(js_error)?;
        to_js_value(&view)
    }

    #[wasm_bindgen(js_name = receiveEntry)]
    pub fn receive_entry(&mut self, index: usize, entry: JsValue) -> Result<JsValue, JsValue> {
        let entry = from_js_entry(entry)?;
        let result = self.receive_entry_model(index, entry).map_err(js_error)?;
        to_js_value(&result)
    }
}

impl ConsensusAppHandle {
    fn bootstrap_model(
        &mut self,
        latest_entry_index: Option<usize>,
    ) -> Result<DispatchResult, String> {
        let transition = app::init(self.participant.clone(), latest_entry_index);
        let view = app::view(&transition.state);
        self.state = Some(transition.state);

        Ok(DispatchResult {
            view,
            effects: transition.effects,
        })
    }

    fn view_model(&self) -> Result<app::View, String> {
        let state = self
            .state
            .as_ref()
            .ok_or_else(|| String::from("app state unavailable; call bootstrap first"))?;
        Ok(app::view(state))
    }

    fn receive_entry_model(
        &mut self,
        index: usize,
        entry: Entry,
    ) -> Result<DispatchResult, String> {
        let state = self
            .state
            .take()
            .ok_or_else(|| String::from("app state unavailable; call bootstrap first"))?;
        let transition = app::reduce(
            state,
            app::Event::CoordinatorEvent {
                event: coordinator::Event::Received { index, entry },
            },
        );
        let view = app::view(&transition.state);
        self.state = Some(transition.state);

        Ok(DispatchResult {
            view,
            effects: transition.effects,
        })
    }
}

fn to_js_value<T>(value: &T) -> Result<JsValue, JsValue>
where
    T: Serialize,
{
    serde_wasm_bindgen::to_value(value).map_err(js_error)
}

fn from_js_entry(value: JsValue) -> Result<Entry, JsValue> {
    serde_wasm_bindgen::from_value(value).map_err(js_error)
}

fn js_error(error: impl ToString) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[cfg(test)]
mod tests {
    use consensus::types::{ClaimId, ClaimKind};

    use super::*;

    fn claim_entry(id: &str, body: &str) -> Entry {
        Entry::Claim {
            claim_id: ClaimId(id.to_owned()),
            author: String::from("alice"),
            body: body.to_owned(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        }
    }

    #[test]
    fn view_requires_bootstrap_first() {
        let handle = ConsensusAppHandle::new(String::from("alice"));
        let error = handle.view_model().unwrap_err();

        assert!(error.contains("bootstrap"));
    }

    #[test]
    fn bootstrap_without_latest_entry_index_produces_empty_view() {
        let mut handle = ConsensusAppHandle::new(String::from("alice"));
        let result = handle.bootstrap_model(None).unwrap();

        assert!(result.effects.is_empty());
        assert_eq!(result.view.overview.total_claims, 0);
        assert_eq!(result.view.overview.total_relations, 0);
        assert_eq!(result.view.overview.total_stances, 0);
        assert!(result.view.overview.attention.is_empty());
        assert!(result.view.drafts.is_empty());
        assert!(result.view.notice.is_none());
    }

    #[test]
    fn bootstrap_with_latest_entry_index_requests_missing_history() {
        let mut handle = ConsensusAppHandle::new(String::from("alice"));
        let result = handle.bootstrap_model(Some(3)).unwrap();

        assert_eq!(result.view.overview.total_claims, 0);
        assert_eq!(result.effects.len(), 1);
        assert!(matches!(
            &result.effects[0],
            app::Effect::CoordinatorEffect {
                effect: coordinator::Effect::FetchMissing { from, limit, .. }
            } if *from == 0 && *limit == 4
        ));
    }

    #[test]
    fn contiguous_receive_updates_overview_total_claims() {
        let mut handle = ConsensusAppHandle::new(String::from("alice"));
        handle.bootstrap_model(None).unwrap();
        let result = handle
            .receive_entry_model(0, claim_entry("c1", "Use JWT"))
            .unwrap();

        assert!(result.effects.is_empty());
        assert_eq!(result.view.overview.total_claims, 1);
    }

    #[test]
    fn out_of_order_receive_keeps_overview_empty_and_requests_fetch() {
        let mut handle = ConsensusAppHandle::new(String::from("alice"));
        handle.bootstrap_model(None).unwrap();
        let result = handle
            .receive_entry_model(2, claim_entry("c3", "third"))
            .unwrap();

        assert_eq!(result.view.overview.total_claims, 0);
        assert_eq!(result.effects.len(), 1);
        assert!(matches!(
            &result.effects[0],
            app::Effect::CoordinatorEffect {
                effect: coordinator::Effect::FetchMissing { from, limit, .. }
            } if *from == 0 && *limit == 2
        ));
    }

    #[test]
    fn filling_gap_materializes_all_contiguous_entries_in_overview() {
        let mut handle = ConsensusAppHandle::new(String::from("alice"));
        handle.bootstrap_model(None).unwrap();

        let result = handle
            .receive_entry_model(2, claim_entry("c3", "third"))
            .unwrap();
        assert_eq!(result.view.overview.total_claims, 0);

        let result = handle
            .receive_entry_model(0, claim_entry("c1", "first"))
            .unwrap();
        assert_eq!(result.view.overview.total_claims, 1);

        let result = handle
            .receive_entry_model(1, claim_entry("c2", "second"))
            .unwrap();
        assert_eq!(result.view.overview.total_claims, 3);
    }

    #[test]
    fn receive_entry_requires_bootstrap_first() {
        let mut handle = ConsensusAppHandle::new(String::from("alice"));
        let error = handle
            .receive_entry_model(0, claim_entry("c1", "Use JWT"))
            .unwrap_err();

        assert!(error.contains("bootstrap"));
    }
}
