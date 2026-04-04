//! Pure domain logic for assembling streamed OpenAI-compatible chat completion
//! delta chunks into a complete assistant message.
//!
//! Handles both fragmented deltas (OpenAI style, where tool call arguments
//! arrive across many chunks) and single-chunk tool calls (llama-server style).

use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompletedMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Vec<CompletedToolCall>,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompletedToolCall {
    pub id: String,
    pub call_type: String,
    pub function_name: String,
    pub arguments_json: String,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AssemblerError {
    #[error("no chunks to assemble")]
    Empty,
    #[error("chunk {index} missing 'choices' array")]
    MissingChoices { index: usize },
    #[error("chunk {index} has empty 'choices' array")]
    EmptyChoices { index: usize },
}

/// Assemble a sequence of streamed chat completion chunk values into a
/// complete assistant message.
///
/// Each `Value` in `chunks` is a parsed `ChatCompletionChunk` object
/// (the JSON payload from an SSE `data:` line).
///
/// This is a pure function with no side effects.
pub fn assemble(chunks: &[Value]) -> Result<CompletedMessage, AssemblerError> {
    if chunks.is_empty() {
        return Err(AssemblerError::Empty);
    }

    let mut role = String::new();
    let mut content: Option<String> = None;
    let mut finish_reason: Option<String> = None;

    // Tool calls accumulate by index. We use a BTreeMap so the final
    // Vec is sorted by index.
    let mut tool_calls: std::collections::BTreeMap<usize, ToolCallAccumulator> =
        std::collections::BTreeMap::new();

    for (chunk_idx, chunk) in chunks.iter().enumerate() {
        let choices = chunk
            .get("choices")
            .and_then(Value::as_array)
            .ok_or(AssemblerError::MissingChoices { index: chunk_idx })?;

        if choices.is_empty() {
            return Err(AssemblerError::EmptyChoices { index: chunk_idx });
        }

        let Some(choice) = choices.first() else {
            return Err(AssemblerError::EmptyChoices { index: chunk_idx });
        };
        let delta = match choice.get("delta") {
            Some(d) => d,
            None => continue,
        };

        // Role — take first non-empty.
        if role.is_empty()
            && let Some(r) = delta.get("role").and_then(Value::as_str)
            && !r.is_empty()
        {
            role.push_str(r);
        }

        // Content — concatenate fragments.
        if let Some(c) = delta.get("content").and_then(Value::as_str) {
            content.get_or_insert_with(String::new).push_str(c);
        }

        // Tool calls — accumulate by index.
        if let Some(tc_array) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tc_array {
                let tc_index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;

                let acc = tool_calls
                    .entry(tc_index)
                    .or_insert_with(ToolCallAccumulator::new);

                if let Some(id) = tc.get("id").and_then(Value::as_str)
                    && !id.is_empty()
                    && acc.id.is_empty()
                {
                    acc.id.push_str(id);
                }

                if let Some(t) = tc.get("type").and_then(Value::as_str)
                    && !t.is_empty()
                    && acc.call_type.is_empty()
                {
                    acc.call_type.push_str(t);
                }

                if let Some(func) = tc.get("function") {
                    if let Some(name) = func.get("name").and_then(Value::as_str)
                        && !name.is_empty()
                        && acc.function_name.is_empty()
                    {
                        acc.function_name.push_str(name);
                    }
                    if let Some(args) = func.get("arguments").and_then(Value::as_str) {
                        acc.arguments_json.push_str(args);
                    }
                }
            }
        }

        // Finish reason — take last non-null.
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            finish_reason = Some(fr.to_owned());
        }
    }

    if role.is_empty() {
        role.push_str("assistant");
    }

    let completed_tool_calls: Vec<CompletedToolCall> = tool_calls
        .into_values()
        .map(|acc| CompletedToolCall {
            id: acc.id,
            call_type: acc.call_type,
            function_name: acc.function_name,
            arguments_json: acc.arguments_json,
        })
        .collect();

    Ok(CompletedMessage {
        role,
        content,
        tool_calls: completed_tool_calls,
        finish_reason,
    })
}

/// Build the JSON representation of an assistant message for conversation history.
///
/// If the message has tool calls, they are included so the model sees its own
/// prior tool calls when processing the follow-up request.
pub fn assistant_message_value(msg: &CompletedMessage) -> Value {
    if msg.tool_calls.is_empty() {
        json!({
            "role": "assistant",
            "content": msg.content.as_deref().unwrap_or(""),
        })
    } else {
        let tool_calls: Vec<Value> = msg
            .tool_calls
            .iter()
            .map(|tc| {
                json!({
                    "id": tc.id,
                    "type": tc.call_type,
                    "function": {
                        "name": tc.function_name,
                        "arguments": tc.arguments_json,
                    }
                })
            })
            .collect();

        json!({
            "role": "assistant",
            "content": msg.content.as_deref(),
            "tool_calls": tool_calls,
        })
    }
}

/// Build a tool result message for conversation history.
pub fn tool_result_message(tool_call_id: &str, content: &str) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content,
    })
}

/// Internal accumulator for building a tool call from fragmented deltas.
struct ToolCallAccumulator {
    id: String,
    call_type: String,
    function_name: String,
    arguments_json: String,
}

impl ToolCallAccumulator {
    fn new() -> Self {
        Self {
            id: String::new(),
            call_type: String::new(),
            function_name: String::new(),
            arguments_json: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn content_chunk(content: &str) -> Value {
        json!({"choices": [{"index": 0, "delta": {"content": content}, "finish_reason": null}]})
    }

    fn role_chunk() -> Value {
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}]})
    }

    fn finish_chunk(reason: &str) -> Value {
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]})
    }

    fn tool_call_start_chunk(index: usize, id: &str, name: &str, arguments: &str) -> Value {
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": index, "id": id, "type": "function", "function": {"name": name, "arguments": arguments}}]}, "finish_reason": null}]})
    }

    fn tool_call_args_chunk(index: usize, arguments: &str) -> Value {
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": index, "function": {"arguments": arguments}}]}, "finish_reason": null}]})
    }

    #[test]
    fn test_content_only_single_chunk() {
        let chunks = vec![
            json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": "hello"}, "finish_reason": "stop"}]}),
        ];
        let msg = assemble(&chunks).expect("should assemble");
        assert_eq!(msg.role, "assistant");
        assert_eq!(msg.content, Some("hello".to_owned()));
        assert!(msg.tool_calls.is_empty());
        assert_eq!(msg.finish_reason, Some("stop".to_owned()));
    }

    #[test]
    fn test_content_only_multiple_chunks() {
        let chunks = vec![
            role_chunk(),
            content_chunk("He"),
            content_chunk("llo"),
            content_chunk(" world"),
            finish_chunk("stop"),
        ];
        let msg = assemble(&chunks).expect("should assemble");
        assert_eq!(msg.content, Some("Hello world".to_owned()));
        assert_eq!(msg.finish_reason, Some("stop".to_owned()));
        assert!(msg.tool_calls.is_empty());
    }

    #[test]
    fn test_tool_call_single_chunk_llama_server_style() {
        let chunks = vec![
            json!({"choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": [{"index": 0, "id": "call_abc", "type": "function", "function": {"name": "exec_code", "arguments": "{\"lang\":\"python\"}"}}]}, "finish_reason": null}]}),
            finish_chunk("tool_calls"),
        ];
        let msg = assemble(&chunks).expect("should assemble");
        assert_eq!(msg.finish_reason, Some("tool_calls".to_owned()));
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call_abc");
        assert_eq!(msg.tool_calls[0].call_type, "function");
        assert_eq!(msg.tool_calls[0].function_name, "exec_code");
        assert_eq!(msg.tool_calls[0].arguments_json, "{\"lang\":\"python\"}");
    }

    #[test]
    fn test_tool_call_fragmented_arguments_openai_style() {
        let chunks = vec![
            role_chunk(),
            tool_call_start_chunk(0, "call_abc", "exec_code", ""),
            tool_call_args_chunk(0, "{\"la"),
            tool_call_args_chunk(0, "ng\":"),
            tool_call_args_chunk(0, " \"python"),
            tool_call_args_chunk(0, "\"}"),
            finish_chunk("tool_calls"),
        ];
        let msg = assemble(&chunks).expect("should assemble");
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call_abc");
        assert_eq!(msg.tool_calls[0].function_name, "exec_code");
        assert_eq!(msg.tool_calls[0].arguments_json, "{\"lang\": \"python\"}");
        assert_eq!(msg.finish_reason, Some("tool_calls".to_owned()));
    }
}
