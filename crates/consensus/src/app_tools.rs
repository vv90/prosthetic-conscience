//! Read-only tool definitions and dispatch over app-owned state.

use serde::Serialize;
use serde_json::Value;

use crate::app;
use crate::engine::{ClaimRef, EngineError};
use crate::tools::{self, ToolDef};
use crate::types::ClaimId;

#[derive(Debug, thiserror::Error)]
pub enum AppToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("missing required argument: {0}")]
    MissingArgument(&'static str),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("query error: {0}")]
    Query(#[from] EngineError),
}

pub fn tool_definitions() -> Vec<ToolDef> {
    tools::tool_definitions()
        .into_iter()
        .filter(|tool| {
            matches!(
                tool.name,
                "overview"
                    | "claim_detail"
                    | "show_drafts"
                    | "preview_overview"
                    | "preview_claim_detail"
                    | "impact_analysis"
            )
        })
        .collect()
}

pub fn dispatch(state: &app::State, tool: &str, args: Value) -> Result<Value, AppToolError> {
    match tool {
        "overview" => Ok(to_json(&app::overview(state))?),
        "claim_detail" => {
            let id = require_str(&args, "claim_id")?;
            Ok(to_json(&app::claim_detail(state, &ClaimId(id.to_owned())))?)
        }
        "show_drafts" => Ok(to_json(&app::show_drafts(state))?),
        "preview_overview" => Ok(to_json(&app::preview_overview(state)?)?),
        "preview_claim_detail" => {
            let claim = require_claim_ref(&args, "claim")?;
            Ok(to_json(&app::preview_claim_detail(state, &claim)?)?)
        }
        "impact_analysis" => Ok(to_json(&app::impact_analysis(state)?)?),
        _ => Err(AppToolError::UnknownTool(tool.to_owned())),
    }
}

fn require_str<'a>(args: &'a Value, field: &'static str) -> Result<&'a str, AppToolError> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or(AppToolError::MissingArgument(field))
}

fn parse_claim_ref(value: &Value, field: &'static str) -> Result<ClaimRef, AppToolError> {
    ClaimRef::from_json_value(value).ok_or_else(|| {
        AppToolError::InvalidArgument(format!(
            "{field}: expected a claim reference string (claim:<id>, draft:<n>, #<n>) \
             or an object with exactly one of claim_id or draft_id"
        ))
    })
}

fn require_claim_ref(args: &Value, field: &'static str) -> Result<ClaimRef, AppToolError> {
    let value = args
        .get(field)
        .ok_or(AppToolError::MissingArgument(field))?;
    parse_claim_ref(value, field)
}

fn to_json<T: Serialize>(value: &T) -> Result<Value, AppToolError> {
    serde_json::to_value(value).map_err(AppToolError::from)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::app;
    use crate::engine::{ClaimRef, DraftContent, DraftEntry, DraftId, ImpactAnalysis};
    use crate::status::EpistemicStatus;
    use crate::types::{ClaimId, ClaimKind, Entry, RelationKind};

    fn state_with_committed_and_drafts() -> app::State {
        app::state_for_tests(
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
                DraftEntry {
                    id: DraftId(0),
                    entry: DraftContent::Claim {
                        body: "Draft proposal".into(),
                        claim_kind: ClaimKind::Proposal,
                        parent: None,
                    },
                },
                DraftEntry {
                    id: DraftId(1),
                    entry: DraftContent::Relation {
                        source: ClaimRef::Committed(ClaimId("c2".into())),
                        target: ClaimRef::Committed(ClaimId("c1".into())),
                        kind: RelationKind::Attacks,
                    },
                },
            ],
        )
    }

    #[test]
    fn tool_definitions_include_exact_read_only_set() {
        let names = tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "overview",
                "claim_detail",
                "show_drafts",
                "preview_overview",
                "preview_claim_detail",
                "impact_analysis",
            ]
        );
        assert!(!names.contains(&"draft_claim"));
        assert!(!names.contains(&"remove_draft"));
    }

    #[test]
    fn tool_definitions_match_cli_argument_shapes_for_claim_queries() {
        let defs = tool_definitions();
        let claim_detail = defs
            .iter()
            .find(|tool| tool.name == "claim_detail")
            .expect("claim_detail definition");
        let preview_claim_detail = defs
            .iter()
            .find(|tool| tool.name == "preview_claim_detail")
            .expect("preview_claim_detail definition");

        assert_eq!(
            claim_detail.parameters,
            json!({
                "type": "object",
                "properties": {
                    "claim_id": {"type": "string", "description": "The claim identifier"}
                },
                "required": ["claim_id"]
            })
        );
        assert_eq!(
            preview_claim_detail.parameters,
            json!({
                "type": "object",
                "properties": {
                    "claim": {
                        "type": "string",
                        "description": "The committed or draft-local claim to preview. Use claim:<id> or draft:<id>.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    }
                },
                "required": ["claim"]
            })
        );
    }

    #[test]
    fn dispatch_overview_and_show_drafts_match_app_queries() {
        let state = state_with_committed_and_drafts();

        assert_eq!(
            dispatch(&state, "overview", json!({})).unwrap(),
            serde_json::to_value(app::overview(&state)).unwrap()
        );
        assert_eq!(
            dispatch(&state, "show_drafts", json!({})).unwrap(),
            serde_json::to_value(app::show_drafts(&state)).unwrap()
        );
    }

    #[test]
    fn dispatch_claim_detail_handles_known_and_unknown_claims() {
        let state = state_with_committed_and_drafts();

        assert_eq!(
            dispatch(&state, "claim_detail", json!({"claim_id": "c1"})).unwrap(),
            serde_json::to_value(app::claim_detail(&state, &ClaimId("c1".into()))).unwrap()
        );
        assert_eq!(
            dispatch(&state, "claim_detail", json!({"claim_id": "missing"})).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn dispatch_preview_queries_cover_committed_and_draft_claims() {
        let state = state_with_committed_and_drafts();

        let preview = dispatch(&state, "preview_overview", json!({})).unwrap();
        assert_eq!(preview["total_claims"], 3);

        let committed =
            dispatch(&state, "preview_claim_detail", json!({"claim": "claim:c1"})).unwrap();
        assert_eq!(committed["claim"]["id"], "c1");

        let draft = dispatch(&state, "preview_claim_detail", json!({"claim": "draft:0"})).unwrap();
        assert_eq!(draft["claim"]["id"], "draft-0");
    }

    #[test]
    fn dispatch_impact_analysis_reports_new_claims_and_status_changes() {
        let state = state_with_committed_and_drafts();
        let impact = dispatch(&state, "impact_analysis", json!({})).unwrap();

        assert_eq!(
            impact,
            serde_json::to_value(ImpactAnalysis {
                new_claims: vec![crate::engine::ImpactNewClaim {
                    draft_id: DraftId(0),
                    body: "Draft proposal".into(),
                    author: "assistant".into(),
                    kind: ClaimKind::Proposal,
                    status: Some(EpistemicStatus::Unexamined),
                }],
                status_changes: vec![crate::engine::ImpactStatusChange {
                    claim_id: ClaimId("c1".into()),
                    body: "Target".into(),
                    before: Some(EpistemicStatus::Unexamined),
                    after: Some(EpistemicStatus::Defeated),
                }],
            })
            .unwrap()
        );
    }
}
