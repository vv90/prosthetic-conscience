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
}

impl ConsensusLlm {
    pub fn new(
        gateway_url: String,
        auth_token: Option<String>,
        model: String,
        participant: String,
    ) -> Self {
        Self {
            client: GatewayClient::new(gateway_url, auth_token),
            model,
            participant,
        }
    }

    pub async fn run_turn(
        &self,
        engine: &mut ConsensusEngine,
        history: &mut Vec<Value>,
    ) -> Result<CompletedMessage, LlmError> {
        let tool_defs = tool_definitions_json();

        for round in 0.. {
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
        );

        let prompt = llm.build_system_prompt(&engine);
        assert!(prompt.contains("Only the human can commit drafts"));
        assert!(prompt.contains("impact_analysis"));
        assert!(prompt.contains("draft_comment"));
        assert!(!prompt.contains("submit_drafts"));
        assert!(!prompt.contains("clear_drafts"));
    }
}
