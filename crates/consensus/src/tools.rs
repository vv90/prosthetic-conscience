//! LLM tool dispatch for the consensus engine.
//!
//! Maps tool name + JSON arguments to engine methods. Returns JSON results.
//! WASM-compatible: no async, no network, no traits — just functions.

use serde::Serialize;
use serde_json::{Value, json};

use super::engine::{ClaimRef, ConsensusEngine, DraftId, EngineError};
use super::types::{ClaimId, ClaimKind, Outcome, Position, RelationKind};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Tool definition for OpenAI function calling format.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

/// Errors from tool dispatch.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("missing required argument: {0}")]
    MissingArgument(&'static str),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
}

// ---------------------------------------------------------------------------
// Arg extraction helpers
// ---------------------------------------------------------------------------

fn require_str<'a>(args: &'a Value, field: &'static str) -> Result<&'a str, ToolError> {
    args.get(field)
        .and_then(Value::as_str)
        .ok_or(ToolError::MissingArgument(field))
}

fn require_u64(args: &Value, field: &'static str) -> Result<u64, ToolError> {
    args.get(field)
        .and_then(Value::as_u64)
        .ok_or(ToolError::MissingArgument(field))
}

fn parse_claim_ref(value: &Value, field: &'static str) -> Result<ClaimRef, ToolError> {
    ClaimRef::from_json_value(value).ok_or_else(|| {
        ToolError::InvalidArgument(format!(
            "{field}: expected a claim reference string (claim:<id>, draft:<n>, #<n>) \
             or an object with exactly one of claim_id or draft_id"
        ))
    })
}

fn require_claim_ref(args: &Value, field: &'static str) -> Result<ClaimRef, ToolError> {
    let value = args.get(field).ok_or(ToolError::MissingArgument(field))?;
    parse_claim_ref(value, field)
}

fn optional_claim_ref(args: &Value, field: &'static str) -> Result<Option<ClaimRef>, ToolError> {
    args.get(field)
        .map(|value| parse_claim_ref(value, field))
        .transpose()
}

fn require_claim_kind(args: &Value, field: &'static str) -> Result<ClaimKind, ToolError> {
    let s = require_str(args, field)?;
    match s {
        "item" => Ok(ClaimKind::Item),
        "proposal" => Ok(ClaimKind::Proposal),
        "fact" => Ok(ClaimKind::Fact),
        "conditional" => Ok(ClaimKind::Conditional),
        "value" => Ok(ClaimKind::Value),
        "reference" => Ok(ClaimKind::Reference),
        other => Err(ToolError::InvalidArgument(format!(
            "{field}: unknown claim kind '{other}'"
        ))),
    }
}

fn require_relation_kind(args: &Value, field: &'static str) -> Result<RelationKind, ToolError> {
    let s = require_str(args, field)?;
    match s {
        "attacks" => Ok(RelationKind::Attacks),
        "supports" => Ok(RelationKind::Supports),
        other => Err(ToolError::InvalidArgument(format!(
            "{field}: unknown relation kind '{other}'"
        ))),
    }
}

fn require_position(args: &Value, field: &'static str) -> Result<Position, ToolError> {
    let s = require_str(args, field)?;
    match s {
        "block" => Ok(Position::Block),
        "object" => Ok(Position::Object),
        "stand_aside" => Ok(Position::StandAside),
        "abstain" => Ok(Position::Abstain),
        "consent" => Ok(Position::Consent),
        "support" => Ok(Position::Support),
        "champion" => Ok(Position::Champion),
        other => Err(ToolError::InvalidArgument(format!(
            "{field}: unknown position '{other}'"
        ))),
    }
}

fn require_outcome(args: &Value, field: &'static str) -> Result<Outcome, ToolError> {
    let s = require_str(args, field)?;
    match s {
        "accepted" => Ok(Outcome::Accepted),
        "rejected" => Ok(Outcome::Rejected),
        "tabled" => Ok(Outcome::Tabled),
        "withdrawn" => Ok(Outcome::Withdrawn),
        other => Err(ToolError::InvalidArgument(format!(
            "{field}: unknown outcome '{other}'"
        ))),
    }
}

/// Serialize a value to JSON without panicking.
fn to_json<T: Serialize>(value: &T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(ToolError::from)
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a tool call to the appropriate engine method.
pub fn dispatch(engine: &mut ConsensusEngine, tool: &str, args: Value) -> Result<Value, ToolError> {
    match tool {
        // -- Query committed state --
        "overview" => Ok(to_json(&engine.overview())?),

        "claim_detail" => {
            let id = require_str(&args, "claim_id")?;
            Ok(to_json(&engine.claim_detail(&ClaimId(id.to_owned())))?)
        }

        // -- Draft creation --
        "draft_claim" => {
            let body = require_str(&args, "body")?;
            let kind = require_claim_kind(&args, "kind")?;
            let parent = optional_claim_ref(&args, "parent")?;
            let draft_id = engine.draft_claim(body.to_owned(), kind, parent)?;
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_relation" => {
            let source = require_claim_ref(&args, "source")?;
            let target = require_claim_ref(&args, "target")?;
            let kind = require_relation_kind(&args, "kind")?;
            let draft_id = engine.draft_relation(source, target, kind)?;
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_stance" => {
            let target = require_claim_ref(&args, "target")?;
            let position = require_position(&args, "position")?;
            let draft_id = engine.draft_stance(target, position)?;
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_resolve" => {
            let claim = require_claim_ref(&args, "claim")?;
            let outcome = require_outcome(&args, "outcome")?;
            let draft_id = engine.draft_resolve(claim, outcome)?;
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_comment" => {
            let body = require_str(&args, "body")?;
            let claim = optional_claim_ref(&args, "claim")?;
            let draft_id = engine.draft_comment(body.to_owned(), claim)?;
            Ok(json!({ "draft_id": draft_id }))
        }

        // -- Draft management --
        "show_drafts" => Ok(to_json(&engine.show_drafts())?),

        "remove_draft" => {
            let id = require_u64(&args, "draft_id")?;
            engine.remove_draft(DraftId(id))?;
            Ok(json!({ "removed": id }))
        }

        "submit_drafts" => {
            let entries = engine.submit_drafts()?;
            let count = entries.len();
            Ok(json!({ "submitted": count, "entries": to_json(&entries)? }))
        }

        "clear_drafts" => {
            let count = engine.show_drafts().len();
            engine.clear_drafts();
            Ok(json!({ "cleared": count }))
        }

        // -- Preview --
        "preview_overview" => Ok(to_json(&engine.preview_overview()?)?),

        "preview_claim_detail" => {
            let claim = require_claim_ref(&args, "claim")?;
            Ok(to_json(&engine.preview_claim_detail(&claim)?)?)
        }

        "impact_analysis" => Ok(to_json(&engine.impact_analysis()?)?),

        _ => Err(ToolError::UnknownTool(tool.to_owned())),
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

fn empty_params() -> Value {
    json!({"type": "object", "properties": {}, "required": []})
}

fn claim_id_param() -> Value {
    json!({
        "type": "object",
        "properties": {
            "claim_id": {"type": "string", "description": "The claim identifier"}
        },
        "required": ["claim_id"]
    })
}

fn claim_ref_param(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description,
        "examples": ["claim:prop-hybrid", "draft:7"],
        "default": "claim:example-claim"
    })
}

/// Returns OpenAI-format tool definitions for all consensus tools.
pub fn tool_definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "overview",
            description: "Get a high-level overview of the current deliberation state including claims, proposals, stances, and attention signals.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "claim_detail",
            description: "Get detailed information about a specific committed claim including its relations, stances, and epistemic status. Prefer this for exact factual questions, then answer in plain language rather than log jargon.",
            parameters: claim_id_param(),
        },
        ToolDef {
            name: "draft_claim",
            description: "Record a new idea for later human review and submission on behalf of the current participant. The author is derived from the active participant automatically. In your follow-up, describe the idea naturally rather than naming internal log types unless the participant already used them.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "body": {"type": "string", "description": "The claim text"},
                    "kind": {"type": "string", "enum": ["item", "proposal", "fact", "conditional", "value", "reference"], "description": "Type of claim"},
                    "parent": claim_ref_param("Optional parent claim reference. Use claim:<id> for committed claims or draft:<id> for a locally drafted claim.")
                },
                "required": ["body", "kind"]
            }),
        },
        ToolDef {
            name: "draft_relation",
            description: "Record that one idea supports or attacks another on behalf of the current participant. References may target either committed claims or locally drafted claims. In conversation, explain the connection naturally rather than talking about graph edges or relation objects. If the participant refers to a concern or risk indirectly, inspect or clarify before choosing source and target.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": claim_ref_param("The claim making the attack/support. Use claim:<id> or draft:<id>."),
                    "target": claim_ref_param("The claim being attacked/supported. Use claim:<id> or draft:<id>."),
                    "kind": {"type": "string", "enum": ["attacks", "supports"], "description": "Relation type"}
                },
                "required": ["source", "target", "kind"]
            }),
        },
        ToolDef {
            name: "draft_stance",
            description: "Record the participant's own degree of agreement or disagreement with an idea, but only when they clearly want that view noted down now rather than merely thinking out loud. Use the weakest matching stance: consent for simple agreement, support for positive support without ownership, champion only for strong advocacy or leadership. In conversation, phrase this naturally, for example as noting agreement or objection.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "target": claim_ref_param("The claim to take a stance on. Use claim:<id> or draft:<id>."),
                    "position": {"type": "string", "enum": ["block", "object", "stand_aside", "abstain", "consent", "support", "champion"], "description": "Position on the claim: consent=simple agreement, support=positive support, champion=strong advocacy/leadership, object/block=disagreement"}
                },
                "required": ["target", "position"]
            }),
        },
        ToolDef {
            name: "draft_resolve",
            description: "Record a proposed resolution for an idea on behalf of the current participant. In conversation, describe the decision plainly rather than using internal workflow jargon unless the participant asks for it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "claim": claim_ref_param("The proposal to resolve. Use claim:<id> or draft:<id>."),
                    "outcome": {"type": "string", "enum": ["accepted", "rejected", "tabled", "withdrawn"], "description": "Resolution outcome"}
                },
                "required": ["claim", "outcome"]
            }),
        },
        ToolDef {
            name: "draft_comment",
            description: "Record a concrete freeform note for the shared log on behalf of the current participant, optionally attached to a specific claim. Do not use this for advice, hypotheticals, or as a placeholder when the user is still exploring what they mean.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "body": {"type": "string", "description": "Comment text"},
                    "claim": claim_ref_param("Optional related claim reference. Use claim:<id> or draft:<id>.")
                },
                "required": ["body"]
            }),
        },
        ToolDef {
            name: "show_drafts",
            description: "Show all pending draft entries that have not yet been submitted. Use when you need to inspect or revise the current draft buffer, not after every mutation.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "remove_draft",
            description: "Remove a specific draft by its ID.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "draft_id": {"type": "number", "description": "The draft ID to remove"}
                },
                "required": ["draft_id"]
            }),
        },
        ToolDef {
            name: "submit_drafts",
            description: "Submit all pending drafts to the deliberation. Returns the submitted entries and clears the draft buffer.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "clear_drafts",
            description: "Discard all pending drafts without submitting them.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "preview_overview",
            description: "Preview the deliberation overview as it would look if all current drafts were submitted.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "preview_claim_detail",
            description: "Preview a claim's detail as it would look if all current drafts were submitted. Prefer this for exact questions about a claim when pending drafts may matter.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "claim": claim_ref_param("The committed or draft-local claim to preview. Use claim:<id> or draft:<id>.")
                },
                "required": ["claim"]
            }),
        },
        ToolDef {
            name: "impact_analysis",
            description: "Compare the current committed state with the state produced by applying all current drafts. Prefer this before answering \"what would change if\" questions about current drafts.",
            parameters: empty_params(),
        },
    ]
}

/// Tool definitions safe to advertise to the model in the terminal client.
pub fn llm_tool_definitions() -> Vec<ToolDef> {
    tool_definitions()
        .into_iter()
        .filter(|def| !matches!(def.name, "submit_drafts" | "clear_drafts"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine() -> ConsensusEngine {
        ConsensusEngine::new(String::from("assistant"))
    }

    fn empty() -> Value {
        json!({})
    }

    fn committed(claim_id: &str) -> Value {
        json!({ "claim_id": claim_id })
    }

    fn draft(draft_id: u64) -> Value {
        json!({ "draft_id": draft_id })
    }

    #[test]
    fn unknown_tool() {
        let mut engine = engine();
        let result = dispatch(&mut engine, "nonexistent", empty());
        assert!(matches!(result, Err(ToolError::UnknownTool(_))));
    }

    #[test]
    fn overview_empty() {
        let mut engine = engine();
        let result = dispatch(&mut engine, "overview", empty()).unwrap();
        assert_eq!(result["total_claims"], 0);
        assert_eq!(result["total_relations"], 0);
        assert_eq!(result["total_stances"], 0);
    }

    #[test]
    fn claim_detail_missing_arg() {
        let mut engine = engine();
        let result = dispatch(&mut engine, "claim_detail", empty());
        assert!(matches!(
            result,
            Err(ToolError::MissingArgument("claim_id"))
        ));
    }

    #[test]
    fn claim_detail_unknown_claim() {
        let mut engine = engine();
        let result = dispatch(&mut engine, "claim_detail", json!({"claim_id": "nope"})).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn draft_claim_returns_only_draft_id() {
        let mut engine = engine();
        let result = dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "Use JWT", "kind": "proposal"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
        assert!(result.get("claim_id").is_none());
    }

    #[test]
    fn draft_claim_missing_arg() {
        let mut engine = engine();
        let result = dispatch(&mut engine, "draft_claim", json!({}));
        assert!(matches!(result, Err(ToolError::MissingArgument("body"))));
    }

    #[test]
    fn draft_relation() {
        let mut engine = engine();
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "Use JWT", "kind": "proposal"}),
        )
        .unwrap();
        let draft_id = created["draft_id"].as_u64().unwrap();
        let result = dispatch(
            &mut engine,
            "draft_relation",
            json!({"source": draft(draft_id), "target": committed("c2"), "kind": "attacks"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn draft_stance() {
        let mut engine = engine();
        let result = dispatch(
            &mut engine,
            "draft_stance",
            json!({"target": committed("c1"), "position": "block"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn draft_resolve() {
        let mut engine = engine();
        let result = dispatch(
            &mut engine,
            "draft_resolve",
            json!({"claim": committed("p1"), "outcome": "accepted"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn draft_comment() {
        let mut engine = engine();
        let result = dispatch(
            &mut engine,
            "draft_comment",
            json!({"body": "Needs more evidence", "claim": committed("c1")}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn show_drafts_returns_array() {
        let mut engine = engine();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "A", "kind": "fact"}),
        )
        .unwrap();
        let result = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 1);
    }

    #[test]
    fn remove_draft_valid() {
        let mut engine = engine();
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "A", "kind": "fact"}),
        )
        .unwrap();
        let draft_id = created["draft_id"].as_u64().unwrap();
        let result = dispatch(&mut engine, "remove_draft", json!({"draft_id": draft_id})).unwrap();
        assert_eq!(result["removed"], draft_id);
    }

    #[test]
    fn remove_draft_invalid() {
        let mut engine = engine();
        let result = dispatch(&mut engine, "remove_draft", json!({"draft_id": 999}));
        assert!(matches!(result, Err(ToolError::Engine(_))));
    }

    #[test]
    fn submit_drafts() {
        let mut engine = engine();
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "A", "kind": "fact"}),
        )
        .unwrap();
        let draft_id = created["draft_id"].as_u64().unwrap();
        dispatch(
            &mut engine,
            "draft_stance",
            json!({"target": draft(draft_id), "position": "consent"}),
        )
        .unwrap();
        let result = dispatch(&mut engine, "submit_drafts", empty()).unwrap();
        assert_eq!(result["submitted"], 2);
        assert!(result["entries"].is_array());
        // Buffer should be empty now
        let show = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert_eq!(show.as_array().unwrap().len(), 0);
    }

    #[test]
    fn clear_drafts() {
        let mut engine = engine();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "A", "kind": "fact"}),
        )
        .unwrap();
        let result = dispatch(&mut engine, "clear_drafts", empty()).unwrap();
        assert_eq!(result["cleared"], 1);
        let show = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert_eq!(show.as_array().unwrap().len(), 0);
    }

    #[test]
    fn preview_overview_includes_drafts() {
        let mut engine = engine();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "A proposal", "kind": "proposal"}),
        )
        .unwrap();

        let committed = dispatch(&mut engine, "overview", empty()).unwrap();
        let preview = dispatch(&mut engine, "preview_overview", empty()).unwrap();
        assert_eq!(committed["total_claims"], 0);
        assert_eq!(preview["total_claims"], 1);
    }

    #[test]
    fn impact_analysis_reports_changes() {
        let mut engine = engine();
        engine.append(super::super::types::Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "Target".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        engine.append(super::super::types::Entry::Claim {
            claim_id: ClaimId("c2".into()),
            author: "bob".into(),
            body: "Attacker".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });
        dispatch(
            &mut engine,
            "draft_relation",
            json!({"source": committed("c2"), "target": committed("c1"), "kind": "attacks"}),
        )
        .unwrap();

        let result = dispatch(&mut engine, "impact_analysis", empty()).unwrap();
        assert_eq!(result["status_changes"].as_array().unwrap().len(), 1);
        assert_eq!(result["status_changes"][0]["claim_id"], "c1");
    }

    #[test]
    fn tool_definitions_count() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 14);
        // All have non-empty names and descriptions
        for def in &defs {
            assert!(!def.name.is_empty());
            assert!(!def.description.is_empty());
            assert!(def.parameters.is_object());
        }
    }

    #[test]
    fn llm_tool_definitions_exclude_submit_and_clear() {
        let defs = llm_tool_definitions();
        let names: Vec<&str> = defs.iter().map(|def| def.name).collect();
        assert!(!names.contains(&"submit_drafts"));
        assert!(!names.contains(&"clear_drafts"));
        assert!(names.contains(&"draft_comment"));
        assert!(names.contains(&"remove_draft"));
        assert!(!names.contains(&"no_structured_action"));
    }

    #[test]
    fn round_trip_draft_show_submit() {
        let mut engine = engine();

        // Draft a claim
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "Use JWT for auth", "kind": "proposal"}),
        )
        .unwrap();
        let draft_id = created["draft_id"].as_u64().unwrap();

        // Draft a stance on it
        dispatch(
            &mut engine,
            "draft_stance",
            json!({"target": draft(draft_id), "position": "consent"}),
        )
        .unwrap();

        // Show drafts
        let drafts = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert_eq!(drafts.as_array().unwrap().len(), 2);

        // Submit
        let submitted = dispatch(&mut engine, "submit_drafts", empty()).unwrap();
        assert_eq!(submitted["submitted"], 2);

        // Buffer empty
        let after = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert_eq!(after.as_array().unwrap().len(), 0);
    }

    #[test]
    fn claim_ref_validation_requires_exactly_one_field() {
        let mut engine = engine();
        let result = dispatch(
            &mut engine,
            "draft_stance",
            json!({"target": {"claim_id": "c1", "draft_id": 7}, "position": "consent"}),
        );
        assert!(
            matches!(result, Err(ToolError::InvalidArgument(message)) if message.contains("exactly one"))
        );
    }

    #[test]
    fn preview_claim_detail_accepts_draft_reference() {
        let mut engine = engine();
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"body": "Draft proposal", "kind": "proposal"}),
        )
        .unwrap();
        let draft_id = created["draft_id"].as_u64().unwrap();
        let result = dispatch(
            &mut engine,
            "preview_claim_detail",
            json!({"claim": draft(draft_id)}),
        )
        .unwrap();
        assert_eq!(result["claim"]["id"], "draft-0");
        assert_eq!(result["claim"]["body"], "Draft proposal");
    }

    #[test]
    fn draft_tool_schemas_do_not_expose_author() {
        let defs = tool_definitions();
        let draft_claim = defs.iter().find(|def| def.name == "draft_claim").unwrap();
        let draft_relation = defs
            .iter()
            .find(|def| def.name == "draft_relation")
            .unwrap();
        let draft_stance = defs.iter().find(|def| def.name == "draft_stance").unwrap();
        let draft_resolve = defs.iter().find(|def| def.name == "draft_resolve").unwrap();
        let draft_comment = defs.iter().find(|def| def.name == "draft_comment").unwrap();
        for def in [
            draft_claim,
            draft_relation,
            draft_stance,
            draft_resolve,
            draft_comment,
        ] {
            assert!(def.parameters["properties"].get("author").is_none());
        }
    }
}
