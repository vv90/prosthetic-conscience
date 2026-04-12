use serde_json::Value;

use crate::chat_gateway::gateway_client::{ClientError, GatewayClient};
use consensus::engine::ConsensusEngine;
use consensus::llm_turn::{
    self, LlmRoundTrace, LlmTurnTrace, ProcessResponseError, TurnConfig, TurnStep,
};
use consensus::response::CompletedAssistantMessage;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("gateway request failed: {0}")]
    Client(#[from] ClientError),
    #[error("{0}")]
    ProcessResponse(#[from] ProcessResponseError),
    #[error("max tool rounds ({max}) exceeded")]
    MaxRoundsExceeded { max: usize },
    #[error("turn completed without a final message")]
    MissingFinalMessage,
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
    ) -> Result<CompletedAssistantMessage, LlmError> {
        let trace = self
            .run_turn_with_trace(engine, history)
            .await
            .map_err(|error| error.error)?;
        trace.final_message.ok_or(LlmError::MissingFinalMessage)
    }

    pub async fn run_turn_with_trace(
        &self,
        engine: &mut ConsensusEngine,
        history: &mut Vec<Value>,
    ) -> Result<LlmTurnTrace, LlmTurnTraceError> {
        let mut trace = LlmTurnTrace::default();
        let mut payload = llm_turn::build_initial_request(
            &self.model,
            &self.participant,
            engine,
            history,
            self.max_history,
        );

        for round in 0.. {
            let chunks = match self.client.chat(payload).await {
                Ok(chunks) => chunks,
                Err(error) => {
                    let round_trace = LlmRoundTrace {
                        round,
                        request_history_messages: history.len(),
                        request_messages: history.len() + 1,
                        response_chunks: 0,
                        assistant_message: None,
                        tool_results: Vec::new(),
                        error: Some(error.to_string()),
                    };
                    trace.rounds.push(round_trace);
                    return Err(LlmTurnTraceError {
                        error: error.into(),
                        trace,
                    });
                }
            };

            let config = TurnConfig {
                round,
                max_rounds: llm_turn::MAX_TOOL_ROUNDS,
                max_history: self.max_history,
                model: &self.model,
                participant: &self.participant,
            };
            match llm_turn::process_llm_response(&chunks, engine, history, &config) {
                Ok(TurnStep::Final {
                    message,
                    round_trace,
                }) => {
                    trace.rounds.push(round_trace);
                    trace.final_message = Some(message);
                    return Ok(trace);
                }
                Ok(TurnStep::NeedsRequest {
                    payload: next_payload,
                    round_trace,
                }) => {
                    trace.rounds.push(round_trace);
                    payload = next_payload;
                }
                Ok(TurnStep::MaxRoundsExceeded { round_trace }) => {
                    trace.rounds.push(round_trace);
                    return Err(LlmTurnTraceError {
                        error: LlmError::MaxRoundsExceeded {
                            max: llm_turn::MAX_TOOL_ROUNDS,
                        },
                        trace,
                    });
                }
                Err(error) => {
                    return Err(LlmTurnTraceError {
                        error: error.into(),
                        trace,
                    });
                }
            }
        }

        Err(LlmTurnTraceError {
            error: LlmError::MaxRoundsExceeded {
                max: llm_turn::MAX_TOOL_ROUNDS,
            },
            trace,
        })
    }
}
