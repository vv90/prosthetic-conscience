use serde::Serialize;
use serde_json::{Value, json};

use crate::chat_gateway::gateway_client::{ClientError, GatewayClient};
use crate::chat_gateway::response_assembler::{
    self, AssemblerError, CompletedMessage, assistant_message_value, tool_result_message,
};
use crate::consensus::engine::ConsensusEngine;
use crate::consensus::format::{format_drafts, format_impact_analysis, format_overview};
use crate::consensus::tools;

const MAX_TOOL_ROUNDS: usize = 8;
const MAX_COMPLETION_TOKENS: u64 = 512;

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
        let tool_defs = tool_definitions_json();
        let mut trace = LlmTurnTrace::default();

        for round in 0.. {
            truncate_history(history, self.max_history);
            let payload = self.build_request_payload(engine, history, &tool_defs);

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
                    round_trace.tool_results.push(ToolExecutionTrace {
                        call_id: tool_call.id.clone(),
                        function_name: tool_call.function_name.clone(),
                        arguments_json: tool_call.arguments_json.clone(),
                        parsed_arguments,
                        argument_parse_error,
                        tool_result_content: text.clone(),
                        dispatch_error: None,
                    });
                    trace.final_message = Some(CompletedMessage {
                        role: "assistant".into(),
                        content: Some(text),
                        tool_calls: vec![],
                        finish_reason: msg.finish_reason.clone(),
                    });
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
    ) -> Value {
        let mut request_messages = Vec::with_capacity(history.len() + 1);
        request_messages.push(json!({
            "role": "system",
            "content": self.build_system_prompt(engine),
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

    fn build_system_prompt(&self, engine: &ConsensusEngine) -> String {
        let overview = format_overview(&engine.overview());
        let drafts = format_drafts(engine.show_drafts());
        let impact = format_impact_analysis(&engine.impact_analysis());
        let tool_list = tools::llm_tool_definitions()
            .into_iter()
            .map(|tool| format!("- {}: {}", tool.name, tool.description))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "You are an AI drafting assistant helping a human participant contribute to a shared consensus log.\n\
             You are participating as \"{participant}\".\n\
             The shared log is authoritative. You may inspect committed state and manipulate only local drafts.\n\
             Never claim a draft is committed. Only the human can commit drafts by typing /submit.\n\
             You must use a tool on every turn.\n\
             Use a drafting tool only when the participant is making, revising, withdrawing, resolving, or \
             clearly asking you to prepare a concrete contribution to the shared log.\n\
             When the participant clearly expresses their own position, preference, objection, proposal, \
             relation, or resolution — even informally — draft it using the appropriate tool.\n\
             If the participant asks for a summary, explanation, comparison, process guidance, or strategy, \
             use no_structured_action unless they also ask you to draft something.\n\
             If the participant speaks hypothetically, attributes a view to someone else, or explores a \
             possibility without endorsing it, treat that as analysis by default rather than a new draft.\n\
             If the participant links existing claims by saying one supports, attacks, answers, or \
             resolves another concern, prefer draft_relation over draft_stance.\n\
             When the participant expresses their own stance toward an existing claim, use draft_stance.\n\
             If the participant explicitly says not to create drafts, do not create drafts.\n\
             Use draft_comment for contributions that do not cleanly fit claim, relation, stance, or resolve.\n\
             Use no_structured_action whenever no draft is appropriate, and put the user-facing reply in raw_text_fallback.\n\n\
             ## Current deliberation state\n\
             {overview}\n\
             ## Pending drafts\n\
             {drafts}\n\n\
             ## Current draft impact\n\
             {impact}\n\n\
             ## Available tools\n\
             {tool_list}\n",
            participant = self.participant
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

        if role == "user" || (role == "assistant" && !has_tool_calls) {
            break;
        }
        cut += 1;
    }

    if cut > 0 && cut < history.len() {
        history.drain(..cut);
    }
}

fn tool_definitions_json() -> Vec<Value> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::types::{ClaimId, ClaimKind, Entry};

    #[test]
    fn prompt_includes_review_boundary_and_safe_tools() {
        let mut engine = ConsensusEngine::new();
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

        let prompt = llm.build_system_prompt(&engine);
        assert!(prompt.contains("Only the human can commit drafts"));
        assert!(prompt.contains("You must use a tool on every turn"));
        assert!(prompt.contains("If the participant explicitly says not to create drafts"));
        assert!(prompt.contains("prefer draft_relation over draft_stance"));
        assert!(prompt.contains("no_structured_action"));
        assert!(prompt.contains("impact_analysis"));
        assert!(prompt.contains("draft_comment"));
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

        let engine = ConsensusEngine::new();
        let history = vec![json!({"role": "user", "content": "Summarize the current state."})];
        let payload = llm.build_request_payload(&engine, &history, &tool_definitions_json());

        assert_eq!(payload["tool_choice"], "required");
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
}
