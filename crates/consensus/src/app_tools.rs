//! Typed app-owned tool definitions and decoding.

use serde_json::{Value, json};

use crate::engine::{ClaimRef, DraftId};
use crate::response::RawToolCall;
use crate::tools::ToolDef;
use crate::types::{ClaimId, ClaimKind, Outcome, Position, RelationKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppTool {
    Overview,
    ClaimDetail {
        claim_id: ClaimId,
    },
    ShowDrafts,
    PreviewOverview,
    PreviewClaimDetail {
        claim: ClaimRef,
    },
    ImpactAnalysis,
    DraftClaim {
        body: String,
        kind: ClaimKind,
        parent: Option<ClaimRef>,
    },
    DraftRelation {
        source: ClaimRef,
        target: ClaimRef,
        kind: RelationKind,
    },
    DraftStance {
        target: ClaimRef,
        position: Position,
    },
    DraftResolve {
        claim: ClaimRef,
        outcome: Outcome,
    },
    DraftComment {
        body: String,
        claim: Option<ClaimRef>,
    },
    RemoveDraft {
        draft_id: DraftId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDecodeErrorKind {
    UnknownTool,
    InvalidJsonArguments,
    MissingRequiredArgument,
    InvalidArgumentValue,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ToolDecodeError {
    pub kind: ToolDecodeErrorKind,
    pub message: String,
}

pub fn decode_tool_call(raw: &RawToolCall) -> Result<AppTool, ToolDecodeError> {
    if raw.call_type != "function" {
        return Err(ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("unsupported tool call type: {}", raw.call_type),
        });
    }

    let args = serde_json::from_str::<Value>(&raw.function.arguments).map_err(|error| {
        ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidJsonArguments,
            message: format!("invalid tool call arguments: {error}"),
        }
    })?;

    match raw.function.name.as_str() {
        "overview" => Ok(AppTool::Overview),
        "claim_detail" => Ok(AppTool::ClaimDetail {
            claim_id: ClaimId(require_string(&args, "claim_id")?.to_owned()),
        }),
        "show_drafts" => Ok(AppTool::ShowDrafts),
        "preview_overview" => Ok(AppTool::PreviewOverview),
        "preview_claim_detail" => Ok(AppTool::PreviewClaimDetail {
            claim: require_claim_ref(&args, "claim")?,
        }),
        "impact_analysis" => Ok(AppTool::ImpactAnalysis),
        "draft_claim" => Ok(AppTool::DraftClaim {
            body: require_string(&args, "body")?.to_owned(),
            kind: require_claim_kind(&args, "kind")?,
            parent: optional_claim_ref(&args, "parent")?,
        }),
        "draft_relation" => Ok(AppTool::DraftRelation {
            source: require_claim_ref(&args, "source")?,
            target: require_claim_ref(&args, "target")?,
            kind: require_relation_kind(&args, "kind")?,
        }),
        "draft_stance" => Ok(AppTool::DraftStance {
            target: require_claim_ref(&args, "target")?,
            position: require_position(&args, "position")?,
        }),
        "draft_resolve" => Ok(AppTool::DraftResolve {
            claim: require_claim_ref(&args, "claim")?,
            outcome: require_outcome(&args, "outcome")?,
        }),
        "draft_comment" => Ok(AppTool::DraftComment {
            body: require_string(&args, "body")?.to_owned(),
            claim: optional_claim_ref(&args, "claim")?,
        }),
        "remove_draft" => Ok(AppTool::RemoveDraft {
            draft_id: DraftId(require_u64(&args, "draft_id")?),
        }),
        _ => Err(ToolDecodeError {
            kind: ToolDecodeErrorKind::UnknownTool,
            message: format!("unknown tool: {}", raw.function.name),
        }),
    }
}

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
            name: "preview_overview",
            description: "Show the overview that would result if the current local drafts were committed right now.",
            parameters: empty_params(),
        },
        ToolDef {
            name: "preview_claim_detail",
            description: "Show the detailed view that would result for a claim if the current local drafts were committed right now.",
            parameters: json!({
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
            }),
        },
        ToolDef {
            name: "impact_analysis",
            description: "Show what structural changes the current local drafts would cause if committed now, including new claims and status changes.",
            parameters: empty_params(),
        },
    ]
}

fn empty_params() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "required": []
    })
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

fn require_value<'a>(args: &'a Value, field: &'static str) -> Result<&'a Value, ToolDecodeError> {
    args.get(field).ok_or_else(|| ToolDecodeError {
        kind: ToolDecodeErrorKind::MissingRequiredArgument,
        message: format!("missing required argument: {field}"),
    })
}

fn require_string<'a>(args: &'a Value, field: &'static str) -> Result<&'a str, ToolDecodeError> {
    require_value(args, field)?
        .as_str()
        .ok_or_else(|| ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("{field}: expected a string"),
        })
}

fn require_u64(args: &Value, field: &'static str) -> Result<u64, ToolDecodeError> {
    require_value(args, field)?
        .as_u64()
        .ok_or_else(|| ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("{field}: expected a non-negative integer"),
        })
}

fn parse_claim_ref(value: &Value, field: &'static str) -> Result<ClaimRef, ToolDecodeError> {
    ClaimRef::from_json_value(value).ok_or_else(|| ToolDecodeError {
        kind: ToolDecodeErrorKind::InvalidArgumentValue,
        message: format!(
            "{field}: expected a claim reference string (claim:<id>, draft:<n>, #<n>) \
             or an object with exactly one of claim_id or draft_id"
        ),
    })
}

fn require_claim_ref(args: &Value, field: &'static str) -> Result<ClaimRef, ToolDecodeError> {
    parse_claim_ref(require_value(args, field)?, field)
}

fn optional_claim_ref(
    args: &Value,
    field: &'static str,
) -> Result<Option<ClaimRef>, ToolDecodeError> {
    args.get(field)
        .map(|value| parse_claim_ref(value, field))
        .transpose()
}

fn require_claim_kind(args: &Value, field: &'static str) -> Result<ClaimKind, ToolDecodeError> {
    match require_string(args, field)? {
        "item" => Ok(ClaimKind::Item),
        "proposal" => Ok(ClaimKind::Proposal),
        "fact" => Ok(ClaimKind::Fact),
        "conditional" => Ok(ClaimKind::Conditional),
        "value" => Ok(ClaimKind::Value),
        "reference" => Ok(ClaimKind::Reference),
        other => Err(ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("{field}: unknown claim kind '{other}'"),
        }),
    }
}

fn require_relation_kind(
    args: &Value,
    field: &'static str,
) -> Result<RelationKind, ToolDecodeError> {
    match require_string(args, field)? {
        "attacks" => Ok(RelationKind::Attacks),
        "supports" => Ok(RelationKind::Supports),
        other => Err(ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("{field}: unknown relation kind '{other}'"),
        }),
    }
}

fn require_position(args: &Value, field: &'static str) -> Result<Position, ToolDecodeError> {
    match require_string(args, field)? {
        "block" => Ok(Position::Block),
        "object" => Ok(Position::Object),
        "stand_aside" => Ok(Position::StandAside),
        "abstain" => Ok(Position::Abstain),
        "consent" => Ok(Position::Consent),
        "support" => Ok(Position::Support),
        "champion" => Ok(Position::Champion),
        other => Err(ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("{field}: unknown position '{other}'"),
        }),
    }
}

fn require_outcome(args: &Value, field: &'static str) -> Result<Outcome, ToolDecodeError> {
    match require_string(args, field)? {
        "accepted" => Ok(Outcome::Accepted),
        "rejected" => Ok(Outcome::Rejected),
        "tabled" => Ok(Outcome::Tabled),
        "withdrawn" => Ok(Outcome::Withdrawn),
        other => Err(ToolDecodeError {
            kind: ToolDecodeErrorKind::InvalidArgumentValue,
            message: format!("{field}: unknown outcome '{other}'"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tool_definitions_include_exact_current_app_tool_set() {
        let names = tool_definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "overview",
                "claim_detail",
                "draft_claim",
                "draft_relation",
                "draft_stance",
                "draft_resolve",
                "draft_comment",
                "show_drafts",
                "remove_draft",
                "preview_overview",
                "preview_claim_detail",
                "impact_analysis",
            ]
        );
    }

    #[test]
    fn tool_definitions_match_cli_argument_shapes_for_supported_mutations() {
        let defs = tool_definitions();
        let draft_claim = defs
            .iter()
            .find(|tool| tool.name == "draft_claim")
            .expect("draft_claim definition");
        let draft_relation = defs
            .iter()
            .find(|tool| tool.name == "draft_relation")
            .expect("draft_relation definition");
        let draft_stance = defs
            .iter()
            .find(|tool| tool.name == "draft_stance")
            .expect("draft_stance definition");
        let draft_resolve = defs
            .iter()
            .find(|tool| tool.name == "draft_resolve")
            .expect("draft_resolve definition");
        let draft_comment = defs
            .iter()
            .find(|tool| tool.name == "draft_comment")
            .expect("draft_comment definition");
        let remove_draft = defs
            .iter()
            .find(|tool| tool.name == "remove_draft")
            .expect("remove_draft definition");

        assert_eq!(
            draft_claim.parameters,
            json!({
                "type": "object",
                "properties": {
                    "body": {"type": "string", "description": "The claim text"},
                    "kind": {"type": "string", "enum": ["item", "proposal", "fact", "conditional", "value", "reference"], "description": "Type of claim"},
                    "parent": {
                        "type": "string",
                        "description": "Optional parent claim reference. Use claim:<id> for committed claims or draft:<id> for a locally drafted claim.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    }
                },
                "required": ["body", "kind"]
            })
        );
        assert_eq!(
            draft_relation.parameters,
            json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "The claim making the attack/support. Use claim:<id> or draft:<id>.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    },
                    "target": {
                        "type": "string",
                        "description": "The claim being attacked/supported. Use claim:<id> or draft:<id>.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    },
                    "kind": {"type": "string", "enum": ["attacks", "supports"], "description": "Relation type"}
                },
                "required": ["source", "target", "kind"]
            })
        );
        assert_eq!(
            draft_stance.parameters,
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "The claim to take a stance on. Use claim:<id> or draft:<id>.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    },
                    "position": {"type": "string", "enum": ["block", "object", "stand_aside", "abstain", "consent", "support", "champion"], "description": "Position on the claim: consent=simple agreement, support=positive support, champion=strong advocacy/leadership, object/block=disagreement"}
                },
                "required": ["target", "position"]
            })
        );
        assert_eq!(
            draft_resolve.parameters,
            json!({
                "type": "object",
                "properties": {
                    "claim": {
                        "type": "string",
                        "description": "The proposal to resolve. Use claim:<id> or draft:<id>.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    },
                    "outcome": {"type": "string", "enum": ["accepted", "rejected", "tabled", "withdrawn"], "description": "Resolution outcome"}
                },
                "required": ["claim", "outcome"]
            })
        );
        assert_eq!(
            draft_comment.parameters,
            json!({
                "type": "object",
                "properties": {
                    "body": {"type": "string", "description": "Comment text"},
                    "claim": {
                        "type": "string",
                        "description": "Optional related claim reference. Use claim:<id> or draft:<id>.",
                        "examples": ["claim:prop-hybrid", "draft:7"],
                        "default": "claim:example-claim"
                    }
                },
                "required": ["body"]
            })
        );
        assert_eq!(
            remove_draft.parameters,
            json!({
                "type": "object",
                "properties": {
                    "draft_id": {"type": "number", "description": "The draft ID to remove"}
                },
                "required": ["draft_id"]
            })
        );
    }

    #[test]
    fn valid_raw_calls_decode_into_typed_tools() {
        let calls = vec![
            RawToolCall::new(
                String::from("call_1"),
                String::from("overview"),
                String::from("{}"),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("claim_detail"),
                String::from("{\"claim_id\":\"c1\"}"),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("draft_claim"),
                String::from(
                    "{\"body\":\"Use JWT\",\"kind\":\"proposal\",\"parent\":\"claim:root\"}",
                ),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("remove_draft"),
                String::from("{\"draft_id\":7}"),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("draft_relation"),
                String::from(
                    "{\"source\":\"claim:c1\",\"target\":\"draft:7\",\"kind\":\"supports\"}",
                ),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("draft_stance"),
                String::from("{\"target\":\"claim:c1\",\"position\":\"support\"}"),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("draft_resolve"),
                String::from("{\"claim\":\"claim:c1\",\"outcome\":\"accepted\"}"),
            ),
            RawToolCall::new(
                String::from("call_1"),
                String::from("draft_comment"),
                String::from("{\"body\":\"Looks good\",\"claim\":\"draft:7\"}"),
            ),
        ];

        assert_eq!(decode_tool_call(&calls[0]), Ok(AppTool::Overview));
        assert_eq!(
            decode_tool_call(&calls[1]),
            Ok(AppTool::ClaimDetail {
                claim_id: ClaimId(String::from("c1")),
            })
        );
        assert_eq!(
            decode_tool_call(&calls[2]),
            Ok(AppTool::DraftClaim {
                body: String::from("Use JWT"),
                kind: ClaimKind::Proposal,
                parent: Some(ClaimRef::Committed(ClaimId(String::from("root")))),
            })
        );
        assert_eq!(
            decode_tool_call(&calls[3]),
            Ok(AppTool::RemoveDraft {
                draft_id: DraftId(7),
            })
        );
        assert_eq!(
            decode_tool_call(&calls[4]),
            Ok(AppTool::DraftRelation {
                source: ClaimRef::Committed(ClaimId(String::from("c1"))),
                target: ClaimRef::Draft(DraftId(7)),
                kind: RelationKind::Supports,
            })
        );
        assert_eq!(
            decode_tool_call(&calls[5]),
            Ok(AppTool::DraftStance {
                target: ClaimRef::Committed(ClaimId(String::from("c1"))),
                position: Position::Support,
            })
        );
        assert_eq!(
            decode_tool_call(&calls[6]),
            Ok(AppTool::DraftResolve {
                claim: ClaimRef::Committed(ClaimId(String::from("c1"))),
                outcome: Outcome::Accepted,
            })
        );
        assert_eq!(
            decode_tool_call(&calls[7]),
            Ok(AppTool::DraftComment {
                body: String::from("Looks good"),
                claim: Some(ClaimRef::Draft(DraftId(7))),
            })
        );
    }

    #[test]
    fn unknown_tool_invalid_json_missing_args_and_invalid_values_return_decode_errors() {
        let invalid_json = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_claim"),
            String::from("{\"body\":\"oops\""),
        );
        let invalid_value = RawToolCall::new(
            String::from("call_1"),
            String::from("remove_draft"),
            String::from("{\"draft_id\":\"bad\"}"),
        );
        let unknown = RawToolCall::new(
            String::from("call_1"),
            String::from("missing_tool"),
            String::from("{}"),
        );
        let missing = RawToolCall::new(
            String::from("call_1"),
            String::from("claim_detail"),
            String::from("{}"),
        );
        let invalid_ref = RawToolCall::new(
            String::from("call_1"),
            String::from("preview_claim_detail"),
            String::from("{\"claim\":{\"claim_id\":\"c1\",\"draft_id\":1}}"),
        );
        let invalid_relation_kind = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_relation"),
            String::from("{\"source\":\"claim:c1\",\"target\":\"claim:c2\",\"kind\":\"depends\"}"),
        );
        let invalid_position = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_stance"),
            String::from("{\"target\":\"claim:c1\",\"position\":\"agree\"}"),
        );
        let invalid_outcome = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_resolve"),
            String::from("{\"claim\":\"claim:c1\",\"outcome\":\"merged\"}"),
        );
        let invalid_source = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_relation"),
            String::from("{\"source\":{},\"target\":\"claim:c2\",\"kind\":\"supports\"}"),
        );
        let invalid_target = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_stance"),
            String::from("{\"target\":{},\"position\":\"support\"}"),
        );
        let invalid_claim = RawToolCall::new(
            String::from("call_1"),
            String::from("draft_comment"),
            String::from("{\"body\":\"note\",\"claim\":{}}"),
        );

        assert_eq!(
            decode_tool_call(&invalid_json),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidJsonArguments,
                message: String::from(
                    "invalid tool call arguments: EOF while parsing an object at line 1 column 14"
                ),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_value),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from("draft_id: expected a non-negative integer"),
            })
        );
        assert_eq!(
            decode_tool_call(&unknown),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::UnknownTool,
                message: String::from("unknown tool: missing_tool"),
            })
        );
        assert_eq!(
            decode_tool_call(&missing),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::MissingRequiredArgument,
                message: String::from("missing required argument: claim_id"),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_ref),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from(
                    "claim: expected a claim reference string (claim:<id>, draft:<n>, #<n>) or an object with exactly one of claim_id or draft_id"
                ),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_relation_kind),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from("kind: unknown relation kind 'depends'"),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_position),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from("position: unknown position 'agree'"),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_outcome),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from("outcome: unknown outcome 'merged'"),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_source),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from(
                    "source: expected a claim reference string (claim:<id>, draft:<n>, #<n>) or an object with exactly one of claim_id or draft_id"
                ),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_target),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from(
                    "target: expected a claim reference string (claim:<id>, draft:<n>, #<n>) or an object with exactly one of claim_id or draft_id"
                ),
            })
        );
        assert_eq!(
            decode_tool_call(&invalid_claim),
            Err(ToolDecodeError {
                kind: ToolDecodeErrorKind::InvalidArgumentValue,
                message: String::from(
                    "claim: expected a claim reference string (claim:<id>, draft:<n>, #<n>) or an object with exactly one of claim_id or draft_id"
                ),
            })
        );
    }
}
