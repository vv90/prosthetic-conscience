//! Pure local conversation state machine.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::response::{CompletedAssistantMessage, CompletedToolCall, assemble};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    history: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    ChatCompletionReceived { chunks: Vec<Value> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    ChatCompletionDecodeFailed { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        content: String,
    },
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct View {
    pub history: Vec<Message>,
}

pub fn init() -> State {
    State {
        history: Vec::new(),
    }
}

pub fn reduce(_state: State, event: Event) -> Transition {
    reduce_event(_state, event)
}

pub fn view(state: &State) -> View {
    View {
        history: state.history.clone(),
    }
}

fn reduce_event(mut state: State, event: Event) -> Transition {
    match event {
        Event::ChatCompletionReceived { chunks } => match assemble(&chunks) {
            Ok(message) => {
                state.history.push(map_assistant_message(message));
                Transition {
                    state,
                    effects: Vec::new(),
                }
            }
            Err(error) => Transition {
                state,
                effects: vec![Effect::ChatCompletionDecodeFailed {
                    error: error.to_string(),
                }],
            },
        },
    }
}

fn map_assistant_message(message: CompletedAssistantMessage) -> Message {
    Message::Assistant {
        content: message.content,
        tool_calls: message.tool_calls.into_iter().map(map_tool_call).collect(),
    }
}

fn map_tool_call(tool_call: CompletedToolCall) -> ToolCall {
    ToolCall {
        id: tool_call.id,
        call_type: tool_call.call_type,
        function: ToolFunction {
            name: tool_call.function_name,
            arguments: tool_call.arguments_json,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn init_starts_with_empty_history() {
        let state = init();

        assert!(state.history.is_empty());
    }

    #[test]
    fn view_of_initial_state_is_empty() {
        let state = init();

        assert_eq!(view(&state).history, Vec::<Message>::new());
    }

    #[test]
    fn view_clones_arbitrary_typed_history() {
        let state = State {
            history: vec![
                Message::User {
                    content: String::from("hello"),
                },
                Message::Assistant {
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: String::from("call_1"),
                        call_type: String::from("function"),
                        function: ToolFunction {
                            name: String::from("draft_claim"),
                            arguments: String::from("{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"),
                        },
                    }],
                },
                Message::Tool {
                    tool_call_id: String::from("call_1"),
                    content: String::from("{\"draft_id\":0}"),
                },
            ],
        };

        let conversation_view = view(&state);

        assert_eq!(conversation_view.history, state.history);
    }

    #[test]
    fn user_message_serde_round_trip() {
        let message = Message::User {
            content: String::from("hello"),
        };

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value,
            json!({
                "role": "user",
                "content": "hello"
            })
        );

        let round_trip: Message = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, message);
    }

    #[test]
    fn assistant_text_message_serde_round_trip() {
        let message = Message::Assistant {
            content: Some(String::from("hi")),
            tool_calls: Vec::new(),
        };

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value,
            json!({
                "role": "assistant",
                "content": "hi"
            })
        );

        let round_trip: Message = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, message);
    }

    #[test]
    fn assistant_tool_call_message_serde_round_trip() {
        let message = Message::Assistant {
            content: None,
            tool_calls: vec![ToolCall {
                id: String::from("call_1"),
                call_type: String::from("function"),
                function: ToolFunction {
                    name: String::from("draft_claim"),
                    arguments: String::from("{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"),
                },
            }],
        };

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value,
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "draft_claim",
                        "arguments": "{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"
                    }
                }]
            })
        );

        let round_trip: Message = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, message);
    }

    #[test]
    fn tool_message_serde_round_trip() {
        let message = Message::Tool {
            tool_call_id: String::from("call_1"),
            content: String::from("{\"draft_id\":0}"),
        };

        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(
            value,
            json!({
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"draft_id\":0}"
            })
        );

        let round_trip: Message = serde_json::from_value(value).unwrap();
        assert_eq!(round_trip, message);
    }

    fn role_chunk() -> Value {
        json!({"choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]})
    }

    fn content_chunk(content: &str) -> Value {
        json!({"choices": [{"index": 0, "delta": {"content": content}, "finish_reason": null}]})
    }

    fn finish_chunk(reason: &str) -> Value {
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]})
    }

    fn tool_call_start_chunk(index: usize, id: &str, name: &str, arguments: &str) -> Value {
        json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": index,
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments
                        }
                    }]
                },
                "finish_reason": null
            }]
        })
    }

    fn tool_call_args_chunk(index: usize, arguments: &str) -> Value {
        json!({
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": index,
                        "function": {
                            "arguments": arguments
                        }
                    }]
                },
                "finish_reason": null
            }]
        })
    }

    #[test]
    fn chat_completion_received_appends_content_only_assistant_message() {
        let state = init();

        let transition = reduce(
            state,
            Event::ChatCompletionReceived {
                chunks: vec![
                    role_chunk(),
                    content_chunk("hello"),
                    content_chunk(" world"),
                    finish_chunk("stop"),
                ],
            },
        );

        assert!(transition.effects.is_empty());
        assert_eq!(
            transition.state.history,
            vec![Message::Assistant {
                content: Some(String::from("hello world")),
                tool_calls: Vec::new(),
            }]
        );
    }

    #[test]
    fn chat_completion_received_appends_assembled_tool_call_message() {
        let state = init();

        let transition = reduce(
            state,
            Event::ChatCompletionReceived {
                chunks: vec![
                    role_chunk(),
                    tool_call_start_chunk(0, "call_1", "overview", "{"),
                    tool_call_args_chunk(0, "\"claim\":\"draft:0\"}"),
                    finish_chunk("tool_calls"),
                ],
            },
        );

        assert!(transition.effects.is_empty());
        assert_eq!(
            transition.state.history,
            vec![Message::Assistant {
                content: None,
                tool_calls: vec![ToolCall {
                    id: String::from("call_1"),
                    call_type: String::from("function"),
                    function: ToolFunction {
                        name: String::from("overview"),
                        arguments: String::from("{\"claim\":\"draft:0\"}"),
                    },
                }],
            }]
        );
    }

    #[test]
    fn chat_completion_received_preserves_existing_history_prefix() {
        let state = State {
            history: vec![Message::User {
                content: String::from("hello"),
            }],
        };

        let transition = reduce(
            state,
            Event::ChatCompletionReceived {
                chunks: vec![role_chunk(), content_chunk("hi"), finish_chunk("stop")],
            },
        );

        assert!(transition.effects.is_empty());
        assert_eq!(
            transition.state.history,
            vec![
                Message::User {
                    content: String::from("hello"),
                },
                Message::Assistant {
                    content: Some(String::from("hi")),
                    tool_calls: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn chat_completion_received_decode_failure_keeps_history_unchanged() {
        let state = State {
            history: vec![Message::User {
                content: String::from("hello"),
            }],
        };

        let transition = reduce(
            state.clone(),
            Event::ChatCompletionReceived { chunks: vec![] },
        );

        assert_eq!(transition.state, state);
        assert_eq!(
            transition.effects,
            vec![Effect::ChatCompletionDecodeFailed {
                error: String::from("no chunks to assemble"),
            }]
        );
    }

    #[test]
    fn chat_completion_received_rejects_non_assistant_role() {
        let state = State {
            history: vec![Message::User {
                content: String::from("hello"),
            }],
        };

        let transition = reduce(
            state.clone(),
            Event::ChatCompletionReceived {
                chunks: vec![json!({
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "user",
                            "content": "hello"
                        },
                        "finish_reason": "stop"
                    }]
                })],
            },
        );

        assert_eq!(transition.state, state);
        assert_eq!(
            transition.effects,
            vec![Effect::ChatCompletionDecodeFailed {
                error: String::from("unexpected role in chat completion response: user"),
            }]
        );
    }

    #[test]
    fn chat_completion_received_event_serde_shape() {
        let value = serde_json::to_value(Event::ChatCompletionReceived {
            chunks: vec![role_chunk(), content_chunk("hello")],
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "chat_completion_received",
                "chunks": [
                    {"choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]},
                    {"choices": [{"index": 0, "delta": {"content": "hello"}, "finish_reason": null}]}
                ]
            })
        );
    }

    #[test]
    fn chat_completion_decode_failed_effect_serde_shape() {
        let value = serde_json::to_value(Effect::ChatCompletionDecodeFailed {
            error: String::from("no chunks to assemble"),
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "type": "chat_completion_decode_failed",
                "error": "no chunks to assemble"
            })
        );
    }
}
