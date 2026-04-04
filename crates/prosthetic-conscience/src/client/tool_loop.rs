//! Tool use loop: sends chat requests, detects tool calls in the response,
//! executes tools locally, appends results, and re-requests until the model
//! produces a final answer.

use serde_json::{Value, json};

use super::gateway_client::{ClientError, GatewayClient};
use super::tools::{ToolError, ToolRegistry};
use consensus::response::{
    self as response_assembler, AssemblerError, CompletedMessage, assistant_message_value,
    tool_result_message,
};

#[derive(Debug, thiserror::Error)]
pub enum ToolLoopError {
    #[error("gateway request failed: {0}")]
    Client(#[from] ClientError),
    #[error("response assembly failed: {0}")]
    Assembler(#[from] AssemblerError),
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolError),
    #[error("max tool rounds ({max}) exceeded")]
    MaxRoundsExceeded { max: usize },
}

/// Run the tool use loop: send a chat request, detect tool calls, execute
/// tools, re-request, repeat until a final answer or max rounds exceeded.
///
/// On success, returns the final `CompletedMessage` (with no tool calls).
/// The `messages` vec is updated in place with all intermediate messages
/// (assistant tool call messages + tool result messages).
pub async fn run(
    client: &GatewayClient,
    registry: &ToolRegistry,
    messages: &mut Vec<Value>,
    model: &str,
    max_rounds: usize,
) -> Result<CompletedMessage, ToolLoopError> {
    let tool_defs = registry.definitions();

    // Print available tools.
    if !tool_defs.is_empty() {
        let names: Vec<&str> = tool_defs
            .iter()
            .filter_map(|d| {
                d.get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
            })
            .collect();
        eprintln!("\x1b[2m--- tools: {} ---\x1b[0m", names.join(", "));
    }

    for round in 0.. {
        let mut payload = json!({
            "model": model,
            "messages": messages,
        });

        if !tool_defs.is_empty()
            && let Some(obj) = payload.as_object_mut()
        {
            obj.insert("tools".to_owned(), json!(tool_defs));
        }

        let chunks = client.chat(payload).await?;
        let msg = response_assembler::assemble(&chunks)?;

        // Always append the assistant message to history.
        messages.push(assistant_message_value(&msg));

        // No tool calls — this is the final answer.
        if msg.tool_calls.is_empty() {
            return Ok(msg);
        }

        // Check round limit before executing tools.
        if round >= max_rounds {
            return Err(ToolLoopError::MaxRoundsExceeded { max: max_rounds });
        }

        // Execute each tool call and append results.
        for tc in &msg.tool_calls {
            eprintln!(
                "\x1b[36m┌─ tool call: {}({})\x1b[0m",
                tc.function_name, tc.arguments_json
            );

            let arguments: Value = serde_json::from_str(&tc.arguments_json).unwrap_or_default();
            let result = registry.execute(&tc.function_name, arguments)?;

            // Print result, indented and dimmed.
            for line in result.lines() {
                eprintln!("\x1b[2m│ {line}\x1b[0m");
            }
            eprintln!("\x1b[36m└─\x1b[0m");

            messages.push(tool_result_message(&tc.id, &result));
        }
    }

    // Unreachable due to the loop structure, but satisfies the compiler.
    Err(ToolLoopError::MaxRoundsExceeded { max: max_rounds })
}
