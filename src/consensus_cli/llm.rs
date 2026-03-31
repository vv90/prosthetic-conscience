use serde::Serialize;
use serde_json::{Value, json};

use crate::chat_gateway::gateway_client::{ClientError, GatewayClient};
use crate::chat_gateway::response_assembler::{
    self, AssemblerError, CompletedMessage, assistant_message_value, tool_result_message,
};
use crate::consensus::engine::{ClaimRef, ConsensusEngine};
use crate::consensus::format::{format_drafts, format_impact_analysis, format_overview};
use crate::consensus::tools;

const MAX_TOOL_ROUNDS: usize = 8;
const MAX_COMPLETION_TOKENS: u64 = 512;
const CLARIFICATION_MARKER_PREFIX: &str = "[internal clarification pending]";

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("gateway request failed: {0}")]
    Client(#[from] ClientError),
    #[error("response assembly failed: {0}")]
    Assembler(#[from] AssemblerError),
    #[error("max tool rounds ({max}) exceeded")]
    MaxRoundsExceeded { max: usize },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LlmTurnTrace {
    pub rounds: Vec<LlmRoundTrace>,
    pub final_message: Option<CompletedMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmRoundTrace {
    pub round: usize,
    pub request_history_messages: usize,
    pub request_messages: usize,
    pub response_chunks: usize,
    pub assistant_message: Option<CompletedMessage>,
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

#[derive(Debug)]
pub struct LlmTurnTraceError {
    pub error: LlmError,
    pub trace: LlmTurnTrace,
}

pub struct ConsensusLlm {
    client: GatewayClient,
    model: String,
    participant: String,
    max_history: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolPolicyPhase {
    ClarifyOrInspect,
    MutationAllowed,
}

impl ConsensusLlm {
    pub fn new(
        gateway_url: String,
        auth_token: Option<String>,
        model: String,
        participant: String,
        max_history: usize,
    ) -> Self {
        Self {
            client: GatewayClient::new(gateway_url, auth_token),
            model,
            participant,
            max_history,
        }
    }

    pub async fn run_turn(
        &self,
        engine: &mut ConsensusEngine,
        history: &mut Vec<Value>,
    ) -> Result<CompletedMessage, LlmError> {
        self.run_turn_with_trace(engine, history)
            .await
            .map(|trace| {
                trace
                    .final_message
                    .expect("successful trace has final message")
            })
            .map_err(|error| error.error)
    }

    pub async fn run_turn_with_trace(
        &self,
        engine: &mut ConsensusEngine,
        history: &mut Vec<Value>,
    ) -> Result<LlmTurnTrace, LlmTurnTraceError> {
        let phase = phase_for_turn(engine, history);
        let mut trace = LlmTurnTrace::default();

        for round in 0.. {
            truncate_history(history, self.max_history);
            let tool_defs = tool_definitions_json(phase);
            let payload = self.build_request_payload(engine, history, &tool_defs, phase);

            let mut round_trace = LlmRoundTrace {
                round,
                request_history_messages: history.len(),
                request_messages: history.len() + 1,
                response_chunks: 0,
                assistant_message: None,
                tool_results: Vec::new(),
                error: None,
            };

            let chunks = match self.client.chat(payload).await {
                Ok(chunks) => chunks,
                Err(error) => {
                    round_trace.error = Some(error.to_string());
                    trace.rounds.push(round_trace);
                    return Err(LlmTurnTraceError {
                        error: error.into(),
                        trace,
                    });
                }
            };
            round_trace.response_chunks = chunks.len();

            let msg = match response_assembler::assemble(&chunks) {
                Ok(msg) => msg,
                Err(error) => {
                    round_trace.error = Some(error.to_string());
                    trace.rounds.push(round_trace);
                    return Err(LlmTurnTraceError {
                        error: error.into(),
                        trace,
                    });
                }
            };
            round_trace.assistant_message = Some(msg.clone());
            history.push(assistant_message_value(&msg));

            if msg.tool_calls.is_empty() {
                trace.final_message = Some(msg);
                trace.rounds.push(round_trace);
                return Ok(trace);
            }

            if round >= MAX_TOOL_ROUNDS {
                round_trace.error = Some(format!("max tool rounds ({MAX_TOOL_ROUNDS}) exceeded"));
                trace.rounds.push(round_trace);
                return Err(LlmTurnTraceError {
                    error: LlmError::MaxRoundsExceeded {
                        max: MAX_TOOL_ROUNDS,
                    },
                    trace,
                });
            }

            let mut round_mutated_drafts = false;
            for tool_call in &msg.tool_calls {
                let (parsed_arguments, argument_parse_error) =
                    match serde_json::from_str::<Value>(&tool_call.arguments_json) {
                        Ok(arguments) => (Some(arguments), None),
                        Err(error) => (None, Some(error.to_string())),
                    };

                if tool_call.function_name == "no_structured_action" {
                    let text = parsed_arguments
                        .as_ref()
                        .and_then(|value| {
                            value
                                .get("raw_text_fallback")
                                .and_then(Value::as_str)
                                .map(String::from)
                        })
                        .unwrap_or_default();
                    let needs_clarification_marker = parsed_arguments
                        .as_ref()
                        .and_then(|value| value.get("reason"))
                        .and_then(Value::as_str)
                        .is_some_and(|reason| reason == "need_clarification");
                    round_trace.tool_results.push(ToolExecutionTrace {
                        call_id: tool_call.id.clone(),
                        function_name: tool_call.function_name.clone(),
                        arguments_json: tool_call.arguments_json.clone(),
                        parsed_arguments,
                        argument_parse_error,
                        tool_result_content: text.clone(),
                        dispatch_error: None,
                    });
                    // Replace the assistant tool-call message in history with the
                    // plain assistant reply the human actually saw so the next
                    // turn continues from natural conversation, not a dangling
                    // tool-call stub.
                    history.pop();
                    let final_message = CompletedMessage {
                        role: "assistant".into(),
                        content: Some(text),
                        tool_calls: vec![],
                        finish_reason: msg.finish_reason.clone(),
                    };
                    history.push(assistant_message_value(&final_message));
                    if needs_clarification_marker {
                        history.push(clarification_marker_message());
                    }
                    trace.final_message = Some(final_message);
                    trace.rounds.push(round_trace);
                    return Ok(trace);
                }

                let (content, dispatch_error) = match parsed_arguments.clone() {
                    Some(arguments) => {
                        match tools::dispatch(engine, &tool_call.function_name, arguments) {
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
                        }
                    }
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

            if round_mutated_drafts {
                let final_message =
                    synthesize_mutation_follow_up(engine, &round_trace.tool_results);
                history.push(assistant_message_value(&final_message));
                trace.final_message = Some(final_message);
                trace.rounds.push(round_trace);
                return Ok(trace);
            }
            trace.rounds.push(round_trace);
        }

        Err(LlmTurnTraceError {
            error: LlmError::MaxRoundsExceeded {
                max: MAX_TOOL_ROUNDS,
            },
            trace,
        })
    }

    fn build_request_payload(
        &self,
        engine: &ConsensusEngine,
        history: &[Value],
        tool_defs: &[Value],
        phase: ToolPolicyPhase,
    ) -> Value {
        let mut request_messages = Vec::with_capacity(history.len() + 1);
        request_messages.push(json!({
            "role": "system",
            "content": self.build_system_prompt(engine, phase),
        }));
        request_messages.extend(history.iter().cloned());

        json!({
            "model": self.model,
            "messages": request_messages,
            "tools": tool_defs,
            "tool_choice": "required",
            "max_tokens": MAX_COMPLETION_TOKENS,
        })
    }

    fn build_system_prompt(&self, engine: &ConsensusEngine, phase: ToolPolicyPhase) -> String {
        let overview = format_overview(&engine.overview());
        let drafts = format_drafts(engine.show_drafts());
        let impact = format_impact_analysis(&engine.impact_analysis());
        let tool_list = llm_tool_definitions_for_phase(phase)
            .into_iter()
            .map(|tool| format!("- {}: {}", tool.name, tool.description))
            .collect::<Vec<_>>()
            .join("\n");
        let phase_policy = match phase {
            ToolPolicyPhase::ClarifyOrInspect => {
                "This is a clarify-or-inspect turn. Do not create, revise, or remove drafts on this turn. Use read-only tools and no_structured_action to answer, inspect, or ask one focused clarification in natural language."
            }
            ToolPolicyPhase::MutationAllowed => {
                "This turn may create or revise at most one concrete draft because either there is already a pending draft buffer or the participant is responding to a previous clarification. If intent is still ambiguous, ask one more clarification instead of drafting."
            }
        };

        format!(
            "You are an AI drafting assistant helping a human participant contribute to a shared consensus log.\n\
             You are participating as \"{participant}\".\n\
             The shared log is authoritative. You may inspect committed state and manipulate only local drafts.\n\
             Never claim a draft is committed. Only the human can commit drafts by typing /submit.\n\
             You must use a tool on every turn.\n\
             All drafts are on behalf of the current participant, \"{participant}\". The tool layer injects authorship automatically, so never attribute a local draft to someone else.\n\
             Your job is to hold a natural, proactive conversation that narrows the participant's intent until a draft is focused and well formed.\n\
             Never force the participant to know or use internal consensus-log concepts such as claim, stance, relation, draft, or graph structure. Infer those privately.\n\
             In user-facing text, speak naturally. Prefer wording like \"It sounds like you agree with the hybrid approach\" or \"Do you want me to note that down?\" over internal jargon like \"I drafted a stance.\"\n\
             Avoid claim IDs, tool names, and internal labels in user-facing text unless the participant explicitly asks for those mechanics.\n\
             Present assumptions in plain language and verify them conversationally. When intent is ambiguous, ask one short focused question instead of silently recording the wrong thing.\n\
             When you ask the participant to confirm whether something should be recorded, or to choose between plausible interpretations before recording, use no_structured_action with reason=need_clarification.\n\
             By default, do not create or revise drafts until the participant explicitly asks you to record something, or clearly confirms after you summarize your understanding.\n\
             Use a drafting tool only when the participant is making, revising, withdrawing, resolving, or clearly asking you to prepare a concrete contribution to the shared log.\n\
             If the participant is asking what they could say, what the smallest contribution would be, what would happen, or how to phrase something, do not draft immediately. Use no_structured_action to discuss options and, if needed, ask one focused follow-up.\n\
             If the participant asks for a summary, explanation, comparison, process guidance, or strategy, use no_structured_action unless they also ask you to record something.\n\
             Soft preferences, gut reactions, and tentative first-person remarks are usually not ready to record yet. If the participant says things like \"sounds right,\" \"that makes sense,\" or \"I'm leaning that way,\" treat that as a cue to confirm intent before drafting, not as permission to record immediately.\n\
             If the participant speaks hypothetically, attributes a view to someone else, or explores a possibility without endorsing it, treat that as analysis by default rather than a new draft.\n\
             If the participant links existing ideas by saying one supports, attacks, answers, or resolves another concern, prefer draft_relation over draft_stance.\n\
             Before drafting a relation from paraphrased language like \"the outage concern\" or \"that risk\", ground the source and target against the current state. Inspect first if needed. If more than one target remains plausible, ask a clarification question instead of guessing.\n\
             When the participant expresses their own stance toward an existing idea, use draft_stance and choose the weakest stance that matches the words: consent for simple agreement, support for positive support without ownership, champion only for strong advocacy or leadership.\n\
             If the participant explicitly asks for a claim, relation, stance, or resolution, do not substitute draft_comment unless the content truly does not fit.\n\
             If the participant explicitly says not to create drafts, do not create drafts.\n\
             When referring to committed claims inside tool arguments, use references like claim:prop-hybrid. When referring to locally drafted claims, use draft:3.\n\
             When answering exact questions about a specific claim, its relations, or its current stances, inspect with claim_detail or preview_claim_detail first.\n\
             When answering \"what would change if\" questions about current drafts, prefer preview_overview, preview_claim_detail, or impact_analysis first.\n\
             When you use no_structured_action, do not merely echo the participant's words. Add a concrete next step, clarification, or grounded explanation.\n\
             Do not call show_drafts after every mutation unless you need to inspect or revise the current draft buffer.\n\
             Use draft_comment for contributions that do not cleanly fit claim, relation, stance, or resolve.\n\
             {phase_policy}\n\
             Use no_structured_action whenever no draft is appropriate, and put the user-facing reply in raw_text_fallback.\n\n\
             ## Current deliberation state\n\
             {overview}\n\
             ## Pending drafts\n\
             {drafts}\n\n\
             ## Current draft impact\n\
             {impact}\n\n\
             ## Available tools\n\
             {tool_list}\n",
            participant = self.participant,
            phase_policy = phase_policy,
        )
    }
}

/// Truncate history to at most `max` messages, preserving tool call integrity.
///
/// Never splits an assistant tool-call message from its subsequent tool result
/// messages. The cut point is always a `user` or bare `assistant` message
/// (no tool calls), so the remaining history starts at a clean conversation
/// boundary.
fn truncate_history(history: &mut Vec<Value>, max: usize) {
    if history.len() <= max {
        return;
    }

    let excess = history.len() - max;

    // Scan forward from `excess` to find a safe cut point: a message that is
    // a `user` role or a bare `assistant` (no tool_calls). This avoids
    // orphaning tool result messages or leaving an assistant tool-call message
    // without its results.
    let mut cut = excess;
    while cut < history.len() {
        let role = history[cut]
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("");
        let has_tool_calls = history[cut]
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty());

        // A user message right after a clarification marker is not a safe
        // cut point — cutting here would orphan the marker.  Skip it so
        // the marker + confirmation pair stays together.
        let follows_clarification_marker = cut > 0
            && history[cut - 1]
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|c| c.starts_with(CLARIFICATION_MARKER_PREFIX));

        if (role == "user" && !follows_clarification_marker)
            || (role == "assistant" && !has_tool_calls)
        {
            break;
        }
        cut += 1;
    }

    if cut > 0 && cut < history.len() {
        history.drain(..cut);
    }
}

fn llm_tool_definitions_for_phase(phase: ToolPolicyPhase) -> Vec<tools::ToolDef> {
    match phase {
        ToolPolicyPhase::ClarifyOrInspect => tools::llm_tool_definitions()
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
                        | "no_structured_action"
                )
            })
            .collect(),
        ToolPolicyPhase::MutationAllowed => tools::llm_tool_definitions(),
    }
}

fn tool_definitions_json(phase: ToolPolicyPhase) -> Vec<Value> {
    llm_tool_definitions_for_phase(phase)
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

fn is_draft_mutation_tool(function_name: &str) -> bool {
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

fn clarification_marker_message() -> Value {
    json!({
        "role": "system",
        "content": format!(
            "{CLARIFICATION_MARKER_PREFIX} The assistant asked a focused clarification on the previous turn. If the user's latest reply clearly confirms that interpretation, you may now prepare one matching draft. If the user corrects, declines, or stays ambiguous, do not draft yet."
        ),
    })
}

fn has_pending_clarification(history: &[Value]) -> bool {
    if history
        .last()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        != Some("user")
    {
        return false;
    }

    history
        .iter()
        .rev()
        .nth(1)
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|role| role == "system")
        && history
            .iter()
            .rev()
            .nth(1)
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .is_some_and(|content| content.starts_with(CLARIFICATION_MARKER_PREFIX))
}

fn phase_for_turn(engine: &ConsensusEngine, history: &[Value]) -> ToolPolicyPhase {
    if has_pending_clarification(history) || !engine.show_drafts().is_empty() {
        ToolPolicyPhase::MutationAllowed
    } else {
        ToolPolicyPhase::ClarifyOrInspect
    }
}

fn synthesize_mutation_follow_up(
    engine: &ConsensusEngine,
    tool_results: &[ToolExecutionTrace],
) -> CompletedMessage {
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

    CompletedMessage {
        role: "assistant".into(),
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
        .map(|detail| format!("\"{}\"", detail.claim.body))
        .unwrap_or_else(|| String::from("that idea"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::types::{ClaimId, ClaimKind, Entry};

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

        let llm = ConsensusLlm::new(
            String::from("http://127.0.0.1:3000"),
            None,
            String::from("default"),
            String::from("assistant"),
            100,
        );

        let prompt = llm.build_system_prompt(&engine, ToolPolicyPhase::ClarifyOrInspect);
        assert!(prompt.contains("Only the human can commit drafts"));
        assert!(prompt.contains("You must use a tool on every turn"));
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
        assert!(prompt.contains("no_structured_action"));
        assert!(prompt.contains("impact_analysis"));
        assert!(prompt.contains("draft_comment"));
        assert!(prompt.contains("This is a clarify-or-inspect turn"));
        assert!(!prompt.contains("submit_drafts"));
        assert!(!prompt.contains("clear_drafts"));
    }

    #[test]
    fn request_payload_uses_required_tool_choice_and_completion_cap() {
        let llm = ConsensusLlm::new(
            String::from("http://127.0.0.1:3000"),
            None,
            String::from("default"),
            String::from("assistant"),
            100,
        );

        let engine = ConsensusEngine::new(String::from("assistant"));
        let history = vec![json!({"role": "user", "content": "Summarize the current state."})];
        let payload = llm.build_request_payload(
            &engine,
            &history,
            &tool_definitions_json(ToolPolicyPhase::ClarifyOrInspect),
            ToolPolicyPhase::ClarifyOrInspect,
        );

        assert_eq!(payload["tool_choice"], "required");
        assert_eq!(payload["max_tokens"], MAX_COMPLETION_TOKENS);
        assert_eq!(payload["messages"][0]["role"], "system");
        assert_eq!(
            payload["messages"][1]["content"],
            "Summarize the current state."
        );
    }

    #[test]
    fn clarify_phase_exposes_only_read_and_conversation_tools() {
        let defs = llm_tool_definitions_for_phase(ToolPolicyPhase::ClarifyOrInspect);
        let names = defs.into_iter().map(|def| def.name).collect::<Vec<_>>();
        assert!(names.contains(&"overview"));
        assert!(names.contains(&"claim_detail"));
        assert!(names.contains(&"show_drafts"));
        assert!(names.contains(&"preview_overview"));
        assert!(names.contains(&"preview_claim_detail"));
        assert!(names.contains(&"impact_analysis"));
        assert!(names.contains(&"no_structured_action"));
        assert!(!names.contains(&"draft_stance"));
        assert!(!names.contains(&"draft_relation"));
        assert!(!names.contains(&"draft_claim"));
    }

    #[test]
    fn phase_for_turn_allows_mutation_after_clarification_marker() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let history = vec![
            json!({"role": "assistant", "content": "It sounds like you agree. Want me to note that down?"}),
            clarification_marker_message(),
            json!({"role": "user", "content": "Yes, please."}),
        ];

        assert_eq!(
            phase_for_turn(&engine, &history),
            ToolPolicyPhase::MutationAllowed
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
            // tool call group — must not be split
            json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "overview", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "{}"}),
            // safe boundary
            json!({"role": "user", "content": "ok"}),
            json!({"role": "assistant", "content": "done"}),
        ];

        // max=4 means excess=2, naive cut at index 2 would land on the
        // assistant tool_calls message — truncation should skip forward to
        // the user message at index 4.
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

        // max=3, excess=2, naive cut at index 2 is a tool message — should
        // advance to index 3 (bare assistant). Drains [0..3], leaving 2.
        truncate_history(&mut history, 3);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["role"], "assistant");
        assert_eq!(history[0]["content"], "b");
    }

    #[test]
    fn truncate_history_noop_when_no_safe_cut_point() {
        // Pathological: all messages are tool results
        let mut history = vec![
            json!({"role": "tool", "tool_call_id": "c1", "content": "r1"}),
            json!({"role": "tool", "tool_call_id": "c2", "content": "r2"}),
            json!({"role": "tool", "tool_call_id": "c3", "content": "r3"}),
        ];

        truncate_history(&mut history, 1);
        // No safe cut point found — history unchanged
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn truncation_does_not_lose_clarification_marker() {
        // The clarification tail is: assistant, system(marker), user.
        // truncate_history considers "user" a safe cut point.  If the
        // excess index lands on the system marker (not a safe cut point),
        // the scanner advances to the final user message and drains
        // everything before it — including the marker.
        //
        // To hit that path we need:  excess = len - max  to land exactly
        // on the system marker (index len-2).  With the trio at the tail
        // that means max = 2 triggers: excess = len-2, scanner starts at
        // the system marker, skips to the user message, and drains the
        // marker away.
        let engine = ConsensusEngine::new(String::from("assistant"));
        let mut history: Vec<Value> = (0..4)
            .flat_map(|i| {
                vec![
                    json!({"role": "user", "content": format!("old msg {i}")}),
                    json!({"role": "assistant", "content": format!("old reply {i}")}),
                ]
            })
            .collect();
        // 8 old messages (indices 0..7), then the clarification trio
        history.push(json!({"role": "assistant", "content": "Want me to note that down?"}));
        history.push(clarification_marker_message());
        history.push(json!({"role": "user", "content": "Yes, go ahead."}));
        // total = 11, marker at index 9, user at index 10

        // max=2 → excess=9, scanner starts at index 9 (the system marker).
        // System is not user/bare-assistant, so scanner advances to
        // index 10 (user) and drains [0..10], leaving only the final user
        // message.  The marker is gone.
        truncate_history(&mut history, 2);

        assert_eq!(
            phase_for_turn(&engine, &history),
            ToolPolicyPhase::MutationAllowed,
            "clarification marker must survive truncation so the user's \
             confirmation unlocks mutation tools"
        );
    }
}
