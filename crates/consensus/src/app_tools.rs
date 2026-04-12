//! Typed app-owned tool definitions and decoding.

use serde_json::{Value, json};

use crate::engine::{ClaimRef, DraftId};
use crate::response::RawToolCall;
use crate::tools::ToolDef;
use crate::types::{ClaimId, ClaimKind};

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
    }
}
