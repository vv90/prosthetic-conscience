//! Pure LLM turn logic for the consensus drafting assistant.
//!
//! This module contains all the WASM-compatible parts of the LLM tool loop:
//! request construction, response processing, tool dispatch, history management,
//! mutation confirmation synthesis, and system prompt generation.
//!
//! The only thing *not* here is the actual HTTP call to the inference endpoint.
//! An I/O wrapper (CLI or browser) provides the SSE chunks; this module does
//! everything else.

use serde::Serialize;
use serde_json::{Value, json};

use crate::engine::{ClaimRef, ConsensusEngine};
use crate::format::{format_drafts, format_impact_analysis, format_overview};
use crate::response::{
    AssemblerError, CompletedAssistantMessage, assemble, assistant_message_value,
    tool_result_message,
};
use crate::system_prompt::{self, SystemPromptInput};
use crate::tools;

pub const MAX_TOOL_ROUNDS: usize = 8;
pub const MAX_COMPLETION_TOKENS: u64 = 512;

// ---------------------------------------------------------------------------
// Trace types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct LlmTurnTrace {
    pub rounds: Vec<LlmRoundTrace>,
    pub final_message: Option<CompletedAssistantMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmRoundTrace {
    pub round: usize,
    pub request_history_messages: usize,
    pub request_messages: usize,
    pub response_chunks: usize,
    pub assistant_message: Option<CompletedAssistantMessage>,
    pub tool_results: Vec<ToolExecutionTrace>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolExecutionTrace {
    pub call_id: String,
    pub function_name: String,
    pub arguments_json: String,
    pub parsed_arguments: Option<Value>,
    pub argument_parse_error: Option<String>,
    pub tool_result_content: String,
    pub dispatch_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Turn step result
// ---------------------------------------------------------------------------

/// The result of processing one LLM response (one round of the tool loop).
pub enum TurnStep {
    /// The model produced a final assistant message (no tool calls, or a
    /// draft mutation was detected and a deterministic confirmation was
    /// synthesized). The turn is complete.
    Final {
        message: CompletedAssistantMessage,
        round_trace: LlmRoundTrace,
    },
    /// The model made tool calls that did not mutate drafts (e.g. inspection
    /// tools). Another inference request is needed. `payload` is the
    /// ready-to-send request body.
    NeedsRequest {
        payload: Value,
        round_trace: LlmRoundTrace,
    },
    /// The maximum number of tool rounds has been exceeded.
    MaxRoundsExceeded { round_trace: LlmRoundTrace },
}

/// Error from `process_llm_response`: the SSE chunks could not be assembled.
#[derive(Debug, thiserror::Error)]
#[error("response assembly failed: {0}")]
pub struct ProcessResponseError(#[from] pub AssemblerError);

/// Configuration for a turn's processing rounds.
pub struct TurnConfig<'a> {
    pub round: usize,
    pub max_rounds: usize,
    pub max_history: usize,
    pub model: &'a str,
    pub participant: &'a str,
}

// ---------------------------------------------------------------------------
// Core pure function: process one round of LLM response
// ---------------------------------------------------------------------------

/// Process the SSE chunks from one inference call and advance the turn.
///
/// This is the pure core of the tool loop. It:
/// 1. Assembles chunks into a `CompletedAssistantMessage`
/// 2. Appends the assistant message to history
/// 3. If no tool calls → returns `TurnStep::Final`
/// 4. If max rounds exceeded → returns `TurnStep::MaxRoundsExceeded`
/// 5. Dispatches each tool call against the engine
/// 6. If any draft mutation succeeded → synthesizes a confirmation and returns `Final`
/// 7. Otherwise → builds the next request payload and returns `NeedsRequest`
pub fn process_llm_response(
    chunks: &[Value],
    engine: &mut ConsensusEngine,
    history: &mut Vec<Value>,
    config: &TurnConfig<'_>,
) -> Result<TurnStep, ProcessResponseError> {
    let mut round_trace = LlmRoundTrace {
        round: config.round,
        request_history_messages: history.len(),
        request_messages: history.len() + 1,
        response_chunks: chunks.len(),
        assistant_message: None,
        tool_results: Vec::new(),
        error: None,
    };

    let msg = match assemble(chunks) {
        Ok(msg) => msg,
        Err(error) => {
            round_trace.error = Some(error.to_string());
            return Err(ProcessResponseError(error));
        }
    };
    round_trace.assistant_message = Some(msg.clone());
    history.push(assistant_message_value(&msg));

    // No tool calls → final message.
    if msg.tool_calls.is_empty() {
        return Ok(TurnStep::Final {
            message: msg,
            round_trace,
        });
    }

    // Max rounds check.
    if config.round >= config.max_rounds {
        round_trace.error = Some(format!("max tool rounds ({}) exceeded", config.max_rounds));
        return Ok(TurnStep::MaxRoundsExceeded { round_trace });
    }

    // Dispatch tool calls.
    let mut round_mutated_drafts = false;
    for tool_call in &msg.tool_calls {
        let (parsed_arguments, argument_parse_error) =
            match serde_json::from_str::<Value>(&tool_call.arguments_json) {
                Ok(arguments) => (Some(arguments), None),
                Err(error) => (None, Some(error.to_string())),
            };

        let (content, dispatch_error) = match parsed_arguments.clone() {
            Some(arguments) => match tools::dispatch(engine, &tool_call.function_name, arguments) {
                Ok(value) => (
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| {
                        String::from("{\"error\":\"failed to serialize tool result\"}")
                    }),
                    None,
                ),
                Err(error) => (
                    format!("{{\"error\":\"{error}\"}}"),
                    Some(error.to_string()),
                ),
            },
            None => {
                let error = argument_parse_error
                    .clone()
                    .unwrap_or_else(|| String::from("unknown parse error"));
                (
                    format!("{{\"error\":\"invalid tool call arguments: {error}\"}}"),
                    Some(format!("invalid tool call arguments: {error}")),
                )
            }
        };

        let dispatch_succeeded = dispatch_error.is_none();
        round_trace.tool_results.push(ToolExecutionTrace {
            call_id: tool_call.id.clone(),
            function_name: tool_call.function_name.clone(),
            arguments_json: tool_call.arguments_json.clone(),
            parsed_arguments,
            argument_parse_error,
            tool_result_content: content.clone(),
            dispatch_error,
        });
        history.push(tool_result_message(&tool_call.id, &content));

        if dispatch_succeeded && is_draft_mutation_tool(&tool_call.function_name) {
            round_mutated_drafts = true;
        }
    }

    // Draft mutation → deterministic confirmation, turn is done.
    if round_mutated_drafts {
        let final_message = synthesize_mutation_follow_up(engine, &round_trace.tool_results);
        history.push(assistant_message_value(&final_message));
        return Ok(TurnStep::Final {
            message: final_message,
            round_trace,
        });
    }

    // Non-mutation tool calls → need another inference round.
    truncate_history(history, config.max_history);
    let tool_defs = tool_definitions_json();
    let payload = build_request_payload(
        config.model,
        config.participant,
        engine,
        history,
        &tool_defs,
    );
    Ok(TurnStep::NeedsRequest {
        payload,
        round_trace,
    })
}

// ---------------------------------------------------------------------------
// Request construction
// ---------------------------------------------------------------------------

/// Build the initial request payload for the first round of a turn.
///
/// Call this before the first inference request. For subsequent rounds,
/// `process_llm_response` returns the next payload via `TurnStep::NeedsRequest`.
pub fn build_initial_request(
    model: &str,
    participant: &str,
    engine: &ConsensusEngine,
    history: &mut Vec<Value>,
    max_history: usize,
) -> Value {
    truncate_history(history, max_history);
    let tool_defs = tool_definitions_json();
    build_request_payload(model, participant, engine, history, &tool_defs)
}

/// Build a chat completion request payload.
pub fn build_request_payload(
    model: &str,
    participant: &str,
    engine: &ConsensusEngine,
    history: &[Value],
    tool_defs: &[Value],
) -> Value {
    let mut request_messages = Vec::with_capacity(history.len() + 1);
    request_messages.push(json!({
        "role": "system",
        "content": build_system_prompt(participant, engine),
    }));
    request_messages.extend(history.iter().cloned());

    json!({
        "model": model,
        "messages": request_messages,
        "tools": tool_defs,
        "tool_choice": "auto",
        "max_tokens": MAX_COMPLETION_TOKENS,
    })
}

/// Build the system prompt for the consensus drafting assistant.
pub fn build_system_prompt(participant: &str, engine: &ConsensusEngine) -> String {
    let overview = format_overview(&engine.overview());
    let drafts = format_drafts(engine.show_drafts());
    let impact = match engine.impact_analysis() {
        Ok(impact) => format_impact_analysis(&impact),
        Err(error) => format!("Impact analysis unavailable: {error}"),
    };
    let tool_list = tools::llm_tool_definitions()
        .into_iter()
        .map(|tool| format!("- {}: {}", tool.name, tool.description))
        .collect::<Vec<_>>()
        .join("\n");

    system_prompt::build_system_prompt(SystemPromptInput {
        participant,
        commit_instruction: "typing /submit",
        overview: &overview,
        drafts: &drafts,
        impact: Some(&impact),
        tools: &tool_list,
    })
}

// ---------------------------------------------------------------------------
// History management
// ---------------------------------------------------------------------------

/// Truncate history to at most `max` messages, preserving tool call integrity.
///
/// Never splits an assistant tool-call message from its subsequent tool result
/// messages. The cut point is always a `user` or bare `assistant` message
/// (no tool calls), so the remaining history starts at a clean conversation
/// boundary.
pub fn truncate_history(history: &mut Vec<Value>, max: usize) {
    if history.len() <= max {
        return;
    }

    let excess = history.len() - max;

    let mut cut = excess;
    while cut < history.len() {
        let role = history
            .get(cut)
            .and_then(|value| value.get("role"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let has_tool_calls = history
            .get(cut)
            .and_then(|value| value.get("tool_calls"))
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());

        if role == "user" || (role == "assistant" && !has_tool_calls) {
            break;
        }
        cut += 1;
    }

    if cut > 0 && cut < history.len() {
        history.drain(..cut);
    }
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

/// Build OpenAI-compatible tool definition JSON for the LLM request.
pub fn tool_definitions_json() -> Vec<Value> {
    tools::llm_tool_definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Draft mutation detection and confirmation synthesis
// ---------------------------------------------------------------------------

/// Returns true if the named tool mutates the draft buffer.
pub fn is_draft_mutation_tool(function_name: &str) -> bool {
    matches!(
        function_name,
        "draft_claim"
            | "draft_relation"
            | "draft_stance"
            | "draft_resolve"
            | "draft_comment"
            | "remove_draft"
    )
}

/// Synthesize a deterministic confirmation message after a draft mutation.
pub fn synthesize_mutation_follow_up(
    engine: &ConsensusEngine,
    tool_results: &[ToolExecutionTrace],
) -> CompletedAssistantMessage {
    let content = tool_results
        .iter()
        .rev()
        .find(|execution| {
            execution.dispatch_error.is_none() && is_draft_mutation_tool(&execution.function_name)
        })
        .map(|execution| render_mutation_confirmation(engine, execution))
        .unwrap_or_else(|| {
            String::from(
                "I've updated the pending draft. It's still only local for now, so we can adjust it before you submit it.",
            )
        });

    CompletedAssistantMessage {
        content: Some(content),
        tool_calls: vec![],
        finish_reason: Some("stop".into()),
    }
}

fn render_mutation_confirmation(
    engine: &ConsensusEngine,
    execution: &ToolExecutionTrace,
) -> String {
    let Some(arguments) = execution.parsed_arguments.as_ref() else {
        return String::from(
            "I've updated the pending draft. It's still only local for now, so we can adjust it before you submit it.",
        );
    };

    match execution.function_name.as_str() {
        "draft_claim" => {
            let body = arguments
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("that idea");
            format!(
                "I've prepared a draft for \"{body}\". It's still only a local draft, so we can adjust the wording before you submit it."
            )
        }
        "draft_relation" => {
            let source = arguments
                .get("source")
                .and_then(ClaimRef::from_json_value)
                .map(|claim| describe_claim_ref(engine, &claim))
                .unwrap_or_else(|| String::from("the first idea"));
            let target = arguments
                .get("target")
                .and_then(ClaimRef::from_json_value)
                .map(|claim| describe_claim_ref(engine, &claim))
                .unwrap_or_else(|| String::from("the second idea"));
            let verb = match arguments.get("kind").and_then(Value::as_str) {
                Some("attacks") => "challenges",
                _ => "supports",
            };
            format!(
                "I've prepared a draft saying that {source} {verb} {target}. It's still only a local draft, so we can refine that connection before you submit it."
            )
        }
        "draft_stance" => {
            let target = arguments
                .get("target")
                .and_then(ClaimRef::from_json_value)
                .map(|claim| describe_claim_ref(engine, &claim))
                .unwrap_or_else(|| String::from("that idea"));
            let stance = match arguments.get("position").and_then(Value::as_str) {
                Some("consent") => "agree with",
                Some("support") => "support",
                Some("champion") => "strongly back",
                Some("object") => "object to",
                Some("block") => "block",
                Some("abstain") => "abstain on",
                Some("stand_aside") => "stand aside on",
                _ => "take a position on",
            };
            format!(
                "I've noted that you {stance} {target}. It's still only a local draft, so we can adjust the strength or wording before you submit it."
            )
        }
        "draft_resolve" => {
            let target = arguments
                .get("claim")
                .and_then(ClaimRef::from_json_value)
                .map(|claim| describe_claim_ref(engine, &claim))
                .unwrap_or_else(|| String::from("that proposal"));
            let outcome = match arguments.get("outcome").and_then(Value::as_str) {
                Some("accepted") => "accepted",
                Some("rejected") => "rejected",
                Some("tabled") => "tabled",
                Some("withdrawn") => "withdrawn",
                _ => "resolved",
            };
            format!(
                "I've prepared a draft to mark {target} as {outcome}. It's still only a local draft, so we can adjust it before you submit it."
            )
        }
        "draft_comment" => {
            let body = arguments
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("that note");
            format!(
                "I've prepared a draft note: \"{body}\". It's still only a local draft, so we can revise it before you submit it."
            )
        }
        "remove_draft" => String::from(
            "I've removed that pending draft. If you want, we can prepare a revised version instead.",
        ),
        _ => String::from(
            "I've updated the pending draft. It's still only local for now, so we can adjust it before you submit it.",
        ),
    }
}

fn describe_claim_ref(engine: &ConsensusEngine, claim: &ClaimRef) -> String {
    engine
        .preview_claim_detail(claim)
        .ok()
        .flatten()
        .map(|detail| format!("\"{}\"", detail.claim.body))
        .unwrap_or_else(|| String::from("that idea"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{format_drafts, format_impact_analysis, format_overview};
    use crate::system_prompt::SystemPromptInput;
    use crate::types::{ClaimId, ClaimKind, Entry};

    #[test]
    fn prompt_includes_review_boundary_and_safe_tools() {
        let mut engine = ConsensusEngine::new(String::from("assistant"));
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "A fact".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });

        let prompt = build_system_prompt("assistant", &engine);
        assert!(prompt.contains("Only the human can commit drafts"));
        assert!(prompt.contains("The tool layer injects authorship automatically"));
        assert!(prompt.contains(
            "Never force the participant to know or use internal consensus-log concepts"
        ));
        assert!(prompt.contains("Present assumptions in plain language"));
        assert!(prompt.contains("By default, do not create or revise drafts"));
        assert!(prompt.contains("smallest contribution would be"));
        assert!(prompt.contains("choose the weakest stance"));
        assert!(prompt.contains("claim:prop-hybrid"));
        assert!(prompt.contains("draft:3"));
        assert!(prompt.contains("inspect with claim_detail or preview_claim_detail first"));
        assert!(prompt.contains("If the participant explicitly says not to create drafts"));
        assert!(prompt.contains("prefer draft_relation over draft_stance"));
        assert!(prompt.contains("impact_analysis"));
        assert!(prompt.contains("draft_comment"));
        assert!(prompt.contains("Reply in plain text whenever no draft or inspection"));
        assert!(!prompt.contains("submit_drafts"));
        assert!(!prompt.contains("clear_drafts"));
        assert!(!prompt.contains("no_structured_action"));
    }

    #[test]
    fn build_system_prompt_matches_shared_template_adapter() {
        let mut engine = ConsensusEngine::new(String::from("assistant"));
        engine.append(Entry::Claim {
            claim_id: ClaimId("c1".into()),
            author: "alice".into(),
            body: "A fact".into(),
            claim_kind: ClaimKind::Fact,
            parent_id: None,
        });

        let overview = format_overview(&engine.overview());
        let drafts = format_drafts(engine.show_drafts());
        let impact = format_impact_analysis(&engine.impact_analysis().expect("impact available"));
        let tools = tools::llm_tool_definitions()
            .into_iter()
            .map(|tool| format!("- {}: {}", tool.name, tool.description))
            .collect::<Vec<_>>()
            .join("\n");

        let direct = system_prompt::build_system_prompt(SystemPromptInput {
            participant: "assistant",
            commit_instruction: "typing /submit",
            overview: &overview,
            drafts: &drafts,
            impact: Some(&impact),
            tools: &tools,
        });

        assert_eq!(build_system_prompt("assistant", &engine), direct);
    }

    #[test]
    fn request_payload_uses_auto_tool_choice_and_completion_cap() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let history = vec![json!({"role": "user", "content": "Summarize the current state."})];
        let tool_defs = tool_definitions_json();
        let payload = build_request_payload("default", "assistant", &engine, &history, &tool_defs);

        assert_eq!(payload["tool_choice"], "auto");
        assert_eq!(payload["max_tokens"], MAX_COMPLETION_TOKENS);
        assert_eq!(payload["messages"][0]["role"], "system");
        assert_eq!(
            payload["messages"][1]["content"],
            "Summarize the current state."
        );
    }

    #[test]
    fn truncate_history_noop_when_under_limit() {
        let mut history = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        truncate_history(&mut history, 5);
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn truncate_history_drops_oldest_messages() {
        let mut history: Vec<Value> = (0..10)
            .flat_map(|i| {
                vec![
                    json!({"role": "user", "content": format!("msg {i}")}),
                    json!({"role": "assistant", "content": format!("reply {i}")}),
                ]
            })
            .collect();
        assert_eq!(history.len(), 20);

        truncate_history(&mut history, 6);
        assert!(history.len() <= 6);
        assert_eq!(history[0]["role"], "user");
    }

    #[test]
    fn truncate_history_preserves_tool_call_pairs() {
        let mut history = vec![
            json!({"role": "user", "content": "start"}),
            json!({"role": "assistant", "content": "noted"}),
            json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "overview", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "{}"}),
            json!({"role": "user", "content": "ok"}),
            json!({"role": "assistant", "content": "done"}),
        ];

        truncate_history(&mut history, 4);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["role"], "user");
        assert_eq!(history[0]["content"], "ok");
    }

    #[test]
    fn truncate_history_skips_tool_result_at_cut_point() {
        let mut history = vec![
            json!({"role": "user", "content": "a"}),
            json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "r"}),
            json!({"role": "assistant", "content": "b"}),
            json!({"role": "user", "content": "c"}),
        ];

        truncate_history(&mut history, 3);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["role"], "assistant");
        assert_eq!(history[0]["content"], "b");
    }

    #[test]
    fn truncate_history_noop_when_no_safe_cut_point() {
        let mut history = vec![
            json!({"role": "tool", "tool_call_id": "c1", "content": "r1"}),
            json!({"role": "tool", "tool_call_id": "c2", "content": "r2"}),
            json!({"role": "tool", "tool_call_id": "c3", "content": "r3"}),
        ];

        truncate_history(&mut history, 1);
        assert_eq!(history.len(), 3);
    }
}
