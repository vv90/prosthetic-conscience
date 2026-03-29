//! LLM tool dispatch for the consensus engine.
//!
//! Maps tool name + JSON arguments to engine methods. Returns JSON results.
//! WASM-compatible: no async, no network, no traits — just functions.

use serde::Serialize;
use serde_json::{Value, json};

use super::engine::{ConsensusEngine, DraftId, EngineError};
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

fn optional_str<'a>(args: &'a Value, field: &str) -> Option<&'a str> {
    args.get(field).and_then(Value::as_str)
}

fn require_u64(args: &Value, field: &'static str) -> Result<u64, ToolError> {
    args.get(field)
        .and_then(Value::as_u64)
        .ok_or(ToolError::MissingArgument(field))
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

/// Serialize a value that is known to be serializable.
/// All render/engine types derive Serialize, so this cannot fail.
fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("type derives Serialize")
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a tool call to the appropriate engine method.
pub fn dispatch(engine: &mut ConsensusEngine, tool: &str, args: Value) -> Result<Value, ToolError> {
    match tool {
        // -- Query committed state --
        "overview" => Ok(to_json(&engine.overview())),

        "claim_detail" => {
            let id = require_str(&args, "claim_id")?;
            Ok(to_json(&engine.claim_detail(&ClaimId(id.to_owned()))))
        }

        // -- Draft creation --
        "draft_claim" => {
            let author = require_str(&args, "author")?;
            let body = require_str(&args, "body")?;
            let kind = require_claim_kind(&args, "kind")?;
            let parent_id = optional_str(&args, "parent_id").map(|s| ClaimId(s.to_owned()));
            let draft_id = engine.draft_claim(author.to_owned(), body.to_owned(), kind, parent_id);
            // Extract the provisional claim_id from the draft we just created
            let claim_id = engine
                .show_drafts()
                .iter()
                .rev()
                .find(|d| d.id == draft_id)
                .and_then(|d| {
                    if let super::types::Entry::Claim { ref claim_id, .. } = d.entry {
                        Some(claim_id.0.clone())
                    } else {
                        None
                    }
                });
            Ok(json!({ "draft_id": draft_id, "claim_id": claim_id }))
        }

        "draft_relation" => {
            let source_id = require_str(&args, "source_id")?;
            let target_id = require_str(&args, "target_id")?;
            let kind = require_relation_kind(&args, "kind")?;
            let author = require_str(&args, "author")?;
            let draft_id = engine.draft_relation(
                ClaimId(source_id.to_owned()),
                ClaimId(target_id.to_owned()),
                kind,
                author.to_owned(),
            );
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_stance" => {
            let target_id = require_str(&args, "target_id")?;
            let author = require_str(&args, "author")?;
            let position = require_position(&args, "position")?;
            let draft_id =
                engine.draft_stance(ClaimId(target_id.to_owned()), author.to_owned(), position);
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_resolve" => {
            let claim_id = require_str(&args, "claim_id")?;
            let author = require_str(&args, "author")?;
            let outcome = require_outcome(&args, "outcome")?;
            let draft_id =
                engine.draft_resolve(ClaimId(claim_id.to_owned()), author.to_owned(), outcome);
            Ok(json!({ "draft_id": draft_id }))
        }

        "draft_comment" => {
            let author = require_str(&args, "author")?;
            let body = require_str(&args, "body")?;
            let claim_id = optional_str(&args, "claim_id").map(|s| ClaimId(s.to_owned()));
            let draft_id = engine.draft_comment(author.to_owned(), body.to_owned(), claim_id);
            Ok(json!({ "draft_id": draft_id }))
        }

        // -- Draft management --
        "show_drafts" => Ok(to_json(&engine.show_drafts())),

        "remove_draft" => {
            let id = require_u64(&args, "draft_id")?;
            engine.remove_draft(DraftId(id))?;
            Ok(json!({ "removed": id }))
        }

        "submit_drafts" => {
            let entries = engine.submit_drafts();
            let count = entries.len();
            Ok(json!({ "submitted": count, "entries": to_json(&entries) }))
        }

        "clear_drafts" => {
            let count = engine.show_drafts().len();
            engine.clear_drafts();
            Ok(json!({ "cleared": count }))
        }

        // -- Preview --
        "preview_overview" => Ok(to_json(&engine.preview_overview())),

        "preview_claim_detail" => {
            let id = require_str(&args, "claim_id")?;
            Ok(to_json(
                &engine.preview_claim_detail(&ClaimId(id.to_owned())),
            ))
        }

        "impact_analysis" => Ok(to_json(&engine.impact_analysis())),

        "no_structured_action" => {
            let reason = require_str(&args, "reason")?;
            let text = require_str(&args, "raw_text_fallback")?;
            Ok(json!({ "reason": reason, "response": text }))
        }

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
            description: "Get detailed information about a specific claim including its relations, stances, and epistemic status.",
            parameters: claim_id_param(),
        },
        ToolDef {
            name: "draft_claim",
            description: "Draft a new claim (item, proposal, fact, etc.) for later submission.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "author": {"type": "string", "description": "Author of the claim"},
                    "body": {"type": "string", "description": "The claim text"},
                    "kind": {"type": "string", "enum": ["item", "proposal", "fact", "conditional", "value", "reference"], "description": "Type of claim"},
                    "parent_id": {"type": "string", "description": "Optional parent claim ID (e.g. item that a proposal addresses)"}
                },
                "required": ["author", "body", "kind"]
            }),
        },
        ToolDef {
            name: "draft_relation",
            description: "Draft a relation (attack or support) between two claims.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "source_id": {"type": "string", "description": "The claim making the attack/support"},
                    "target_id": {"type": "string", "description": "The claim being attacked/supported"},
                    "kind": {"type": "string", "enum": ["attacks", "supports"], "description": "Relation type"},
                    "author": {"type": "string", "description": "Author of the relation"}
                },
                "required": ["source_id", "target_id", "kind", "author"]
            }),
        },
        ToolDef {
            name: "draft_stance",
            description: "Draft a stance (position) on a claim. Positions range from block (strongest disagreement) to champion (strongest agreement).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "string", "description": "The claim to take a stance on"},
                    "author": {"type": "string", "description": "Author of the stance"},
                    "position": {"type": "string", "enum": ["block", "object", "stand_aside", "abstain", "consent", "support", "champion"], "description": "Position on the claim"}
                },
                "required": ["target_id", "author", "position"]
            }),
        },
        ToolDef {
            name: "draft_resolve",
            description: "Draft a resolution for a proposal (accepted, rejected, tabled, or withdrawn).",
            parameters: json!({
                "type": "object",
                "properties": {
                    "claim_id": {"type": "string", "description": "The proposal to resolve"},
                    "author": {"type": "string", "description": "Author of the resolution"},
                    "outcome": {"type": "string", "enum": ["accepted", "rejected", "tabled", "withdrawn"], "description": "Resolution outcome"}
                },
                "required": ["claim_id", "author", "outcome"]
            }),
        },
        ToolDef {
            name: "draft_comment",
            description: "Draft a freeform comment, optionally attached to a specific claim.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "author": {"type": "string", "description": "Author of the comment"},
                    "body": {"type": "string", "description": "Comment text"},
                    "claim_id": {"type": "string", "description": "Optional related claim identifier"}
                },
                "required": ["author", "body"]
            }),
        },
        ToolDef {
            name: "show_drafts",
            description: "Show all pending draft entries that have not yet been submitted.",
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
            description: "Preview a claim's detail as it would look if all current drafts were submitted.",
            parameters: claim_id_param(),
        },
        ToolDef {
            name: "impact_analysis",
            description: "Compare the current committed state with the state produced by applying all current drafts.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "no_structured_action",
            description: "Use when the participant is asking for a summary, explanation, analysis, \
                process help, hypothetical discussion, or otherwise is not making a concrete \
                contribution to the shared log. Only draft when the participant is clearly \
                expressing or requesting a contribution.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "reason": {
                        "type": "string",
                        "enum": [
                            "user_asked_question",
                            "need_clarification",
                            "summary_or_analysis",
                            "hypothetical_or_meta",
                            "no_actionable_content",
                            "off_topic"
                        ],
                        "description": "Why no structured entry is appropriate"
                    },
                    "raw_text_fallback": {
                        "type": "string",
                        "description": "Plain text response when no structured action applies"
                    }
                },
                "required": ["reason", "raw_text_fallback"]
            }),
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

    fn empty() -> Value {
        json!({})
    }

    #[test]
    fn unknown_tool() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(&mut engine, "nonexistent", empty());
        assert!(matches!(result, Err(ToolError::UnknownTool(_))));
    }

    #[test]
    fn overview_empty() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(&mut engine, "overview", empty()).unwrap();
        assert_eq!(result["total_claims"], 0);
        assert_eq!(result["total_relations"], 0);
        assert_eq!(result["total_stances"], 0);
    }

    #[test]
    fn claim_detail_missing_arg() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(&mut engine, "claim_detail", empty());
        assert!(matches!(
            result,
            Err(ToolError::MissingArgument("claim_id"))
        ));
    }

    #[test]
    fn claim_detail_unknown_claim() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(&mut engine, "claim_detail", json!({"claim_id": "nope"})).unwrap();
        assert!(result.is_null());
    }

    #[test]
    fn draft_claim_returns_ids() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "Use JWT", "kind": "proposal"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
        assert!(result["claim_id"].is_string());
        assert!(result["claim_id"].as_str().unwrap().starts_with("draft-"));
    }

    #[test]
    fn draft_claim_missing_arg() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(&mut engine, "draft_claim", json!({"author": "alice"}));
        assert!(matches!(result, Err(ToolError::MissingArgument("body"))));
    }

    #[test]
    fn draft_relation() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "draft_relation",
            json!({"source_id": "c1", "target_id": "c2", "kind": "attacks", "author": "bob"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn draft_stance() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "draft_stance",
            json!({"target_id": "c1", "author": "carol", "position": "block"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn draft_resolve() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "draft_resolve",
            json!({"claim_id": "p1", "author": "alice", "outcome": "accepted"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn draft_comment() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "draft_comment",
            json!({"author": "alice", "body": "Needs more evidence", "claim_id": "c1"}),
        )
        .unwrap();
        assert!(result["draft_id"].is_number());
    }

    #[test]
    fn show_drafts_returns_array() {
        let mut engine = ConsensusEngine::new();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "A", "kind": "fact"}),
        )
        .unwrap();
        let result = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert!(result.is_array());
        assert_eq!(result.as_array().unwrap().len(), 1);
    }

    #[test]
    fn remove_draft_valid() {
        let mut engine = ConsensusEngine::new();
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "A", "kind": "fact"}),
        )
        .unwrap();
        let draft_id = created["draft_id"].as_u64().unwrap();
        let result = dispatch(&mut engine, "remove_draft", json!({"draft_id": draft_id})).unwrap();
        assert_eq!(result["removed"], draft_id);
    }

    #[test]
    fn remove_draft_invalid() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(&mut engine, "remove_draft", json!({"draft_id": 999}));
        assert!(matches!(result, Err(ToolError::Engine(_))));
    }

    #[test]
    fn submit_drafts() {
        let mut engine = ConsensusEngine::new();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "A", "kind": "fact"}),
        )
        .unwrap();
        dispatch(
            &mut engine,
            "draft_stance",
            json!({"target_id": "c1", "author": "bob", "position": "consent"}),
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
        let mut engine = ConsensusEngine::new();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "A", "kind": "fact"}),
        )
        .unwrap();
        let result = dispatch(&mut engine, "clear_drafts", empty()).unwrap();
        assert_eq!(result["cleared"], 1);
        let show = dispatch(&mut engine, "show_drafts", empty()).unwrap();
        assert_eq!(show.as_array().unwrap().len(), 0);
    }

    #[test]
    fn preview_overview_includes_drafts() {
        let mut engine = ConsensusEngine::new();
        dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "A proposal", "kind": "proposal"}),
        )
        .unwrap();

        let committed = dispatch(&mut engine, "overview", empty()).unwrap();
        let preview = dispatch(&mut engine, "preview_overview", empty()).unwrap();
        assert_eq!(committed["total_claims"], 0);
        assert_eq!(preview["total_claims"], 1);
    }

    #[test]
    fn impact_analysis_reports_changes() {
        let mut engine = ConsensusEngine::new();
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
            json!({"source_id": "c2", "target_id": "c1", "kind": "attacks", "author": "bob"}),
        )
        .unwrap();

        let result = dispatch(&mut engine, "impact_analysis", empty()).unwrap();
        assert_eq!(result["status_changes"].as_array().unwrap().len(), 1);
        assert_eq!(result["status_changes"][0]["claim_id"], "c1");
    }

    #[test]
    fn tool_definitions_count() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 15);
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
        assert!(names.contains(&"no_structured_action"));
        assert_eq!(
            names.last().copied(),
            Some("no_structured_action"),
            "no_structured_action must be listed last"
        );
    }

    #[test]
    fn dispatch_no_structured_action_valid() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "no_structured_action",
            json!({"reason": "user_asked_question", "raw_text_fallback": "The session is about X."}),
        )
        .unwrap();
        assert_eq!(result["reason"], "user_asked_question");
        assert_eq!(result["response"], "The session is about X.");
    }

    #[test]
    fn dispatch_no_structured_action_missing_reason() {
        let mut engine = ConsensusEngine::new();
        let result = dispatch(
            &mut engine,
            "no_structured_action",
            json!({"raw_text_fallback": "hello"}),
        );
        assert!(matches!(result, Err(ToolError::MissingArgument("reason"))));
    }

    #[test]
    fn round_trip_draft_show_submit() {
        let mut engine = ConsensusEngine::new();

        // Draft a claim
        let created = dispatch(
            &mut engine,
            "draft_claim",
            json!({"author": "alice", "body": "Use JWT for auth", "kind": "proposal"}),
        )
        .unwrap();
        let claim_id = created["claim_id"].as_str().unwrap().to_owned();

        // Draft a stance on it
        dispatch(
            &mut engine,
            "draft_stance",
            json!({"target_id": claim_id, "author": "bob", "position": "consent"}),
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
}
