use serde_json::{Value, json};

use crate::chat_gateway::gateway_client::{ClientError, GatewayClient};
use crate::chat_gateway::response_assembler::{
    self, AssemblerError, CompletedMessage, assistant_message_value, tool_result_message,
};
use crate::consensus::engine::ConsensusEngine;
use crate::consensus::format::{format_drafts, format_impact_analysis, format_overview};
use crate::consensus::tools;

const MAX_TOOL_ROUNDS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("gateway request failed: {0}")]
    Client(#[from] ClientError),
    #[error("response assembly failed: {0}")]
    Assembler(#[from] AssemblerError),
    #[error("max tool rounds ({max}) exceeded")]
    MaxRoundsExceeded { max: usize },
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
        let tool_defs = tool_definitions_json();

        for round in 0.. {
            truncate_history(history, self.max_history);
            let mut request_messages = Vec::with_capacity(history.len() + 1);
            request_messages.push(json!({
                "role": "system",
                "content": self.build_system_prompt(engine),
            }));
            request_messages.extend(history.iter().cloned());

            let mut payload = json!({
                "model": self.model,
                "messages": request_messages,
            });
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("tools".to_owned(), json!(tool_defs));
            }

            let chunks = self.client.chat(payload).await?;
            let msg = response_assembler::assemble(&chunks)?;
            history.push(assistant_message_value(&msg));

            if msg.tool_calls.is_empty() {
                return Ok(msg);
            }

            if round >= MAX_TOOL_ROUNDS {
                return Err(LlmError::MaxRoundsExceeded {
                    max: MAX_TOOL_ROUNDS,
                });
            }

            for tool_call in &msg.tool_calls {
                let content = match serde_json::from_str::<Value>(&tool_call.arguments_json) {
                    Ok(arguments) => {
                        match tools::dispatch(engine, &tool_call.function_name, arguments) {
                            Ok(value) => {
                                serde_json::to_string_pretty(&value).unwrap_or_else(|_| {
                                    String::from("{\"error\":\"failed to serialize tool result\"}")
                                })
                            }
                            Err(e) => format!("{{\"error\":\"{e}\"}}"),
                        }
                    }
                    Err(e) => format!("{{\"error\":\"invalid tool call arguments: {e}\"}}"),
                };
                history.push(tool_result_message(&tool_call.id, &content));
            }
        }

        Err(LlmError::MaxRoundsExceeded {
            max: MAX_TOOL_ROUNDS,
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
             Prefer structured entries over freeform text when the user's intent is clear.\n\
             Use comments only when the contribution does not cleanly fit claim, relation, stance, or resolve.\n\
             Explain what you are doing in natural language, but use tools whenever you need exact state.\n\n\
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
        assert!(prompt.contains("impact_analysis"));
        assert!(prompt.contains("draft_comment"));
        assert!(!prompt.contains("submit_drafts"));
        assert!(!prompt.contains("clear_drafts"));
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
