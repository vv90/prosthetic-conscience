//! Pure local conversation state machine.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::response::{CompletedAssistantMessage, RawToolCall, assemble};

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
        tool_calls: Vec<RawToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
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

pub fn history(state: &State) -> &[Message] {
    &state.history
}

pub fn message_to_json(message: &Message) -> Value {
    match message {
        Message::User { content } => json!({
            "role": "user",
            "content": content,
        }),
        Message::Assistant {
            content,
            tool_calls,
        } if tool_calls.is_empty() => json!({
            "role": "assistant",
            "content": content.as_deref().unwrap_or(""),
        }),
        Message::Assistant {
            content,
            tool_calls,
        } => json!({
            "role": "assistant",
            "content": content.as_deref(),
            "tool_calls": tool_calls,
        }),
        Message::Tool {
            tool_call_id,
            content,
        } => json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    }
}

pub fn history_to_json(history: &[Message]) -> Vec<Value> {
    history.iter().map(message_to_json).collect()
}

pub fn truncate_history(history: &[Value], max: usize) -> Vec<Value> {
    if history.len() <= max {
        return history.to_vec();
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
            .is_some_and(|array| !array.is_empty());

        if role == "user" || (role == "assistant" && !has_tool_calls) {
            break;
        }
        cut += 1;
    }

    if cut > 0 && cut < history.len() {
        history[cut..].to_vec()
    } else {
        history.to_vec()
    }
}

#[cfg(test)]
pub(crate) fn state_with_history(history: Vec<Message>) -> State {
    State { history }
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
        tool_calls: message.tool_calls,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
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
                    tool_calls: vec![RawToolCall::new(
                        String::from("call_1"),
                        String::from("draft_claim"),
                        String::from("{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"),
                    )],
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
            tool_calls: vec![RawToolCall::new(
                String::from("call_1"),
                String::from("draft_claim"),
                String::from("{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"),
            )],
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
                tool_calls: vec![RawToolCall::new(
                    String::from("call_1"),
                    String::from("overview"),
                    String::from("{\"claim\":\"draft:0\"}"),
                )],
            }]
        );
    }

    #[test]
    fn chat_completion_received_keeps_assistant_message_when_tool_args_fail_to_decode() {
        let state = init();

        let transition = reduce(
            state,
            Event::ChatCompletionReceived {
                chunks: vec![
                    role_chunk(),
                    tool_call_start_chunk(0, "call_1", "draft_claim", "{\"body\""),
                    finish_chunk("tool_calls"),
                ],
            },
        );

        assert!(transition.effects.is_empty());
        assert_eq!(
            transition.state.history,
            vec![Message::Assistant {
                content: None,
                tool_calls: vec![RawToolCall::new(
                    String::from("call_1"),
                    String::from("draft_claim"),
                    String::from("{\"body\""),
                )],
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

    #[test]
    fn history_accessor_returns_stored_messages() {
        let state = state_with_history(vec![Message::User {
            content: String::from("hello"),
        }]);

        assert_eq!(
            history(&state),
            vec![Message::User {
                content: String::from("hello"),
            }]
            .as_slice()
        );
    }

    #[test]
    fn message_to_json_serializes_user_message_with_exact_shape() {
        let message = Message::User {
            content: String::from("hello"),
        };

        assert_eq!(
            message_to_json(&message),
            json!({
                "role": "user",
                "content": "hello"
            })
        );
    }

    #[test]
    fn message_to_json_serializes_plain_assistant_message_with_exact_shape() {
        let message = Message::Assistant {
            content: Some(String::from("hi")),
            tool_calls: Vec::new(),
        };

        assert_eq!(
            message_to_json(&message),
            json!({
                "role": "assistant",
                "content": "hi"
            })
        );
    }

    #[test]
    fn message_to_json_serializes_assistant_tool_call_message_with_exact_shape() {
        let message = Message::Assistant {
            content: None,
            tool_calls: vec![RawToolCall::new(
                String::from("call_1"),
                String::from("draft_claim"),
                String::from("{\"body\":\"Use JWT\",\"kind\":\"proposal\"}"),
            )],
        };

        assert_eq!(
            message_to_json(&message),
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
    }

    #[test]
    fn message_to_json_serializes_tool_message_with_exact_shape() {
        let message = Message::Tool {
            tool_call_id: String::from("call_1"),
            content: String::from("{\"draft_id\":0}"),
        };

        assert_eq!(
            message_to_json(&message),
            json!({
                "role": "tool",
                "tool_call_id": "call_1",
                "content": "{\"draft_id\":0}"
            })
        );
    }

    #[test]
    fn history_to_json_preserves_order() {
        let history = vec![
            Message::User {
                content: String::from("hello"),
            },
            Message::Assistant {
                content: Some(String::from("hi")),
                tool_calls: Vec::new(),
            },
            Message::Tool {
                tool_call_id: String::from("call_1"),
                content: String::from("{}"),
            },
        ];

        assert_eq!(
            history_to_json(&history),
            vec![
                json!({"role": "user", "content": "hello"}),
                json!({"role": "assistant", "content": "hi"}),
                json!({"role": "tool", "tool_call_id": "call_1", "content": "{}"}),
            ]
        );
    }

    #[test]
    fn truncate_history_noop_when_under_limit() {
        let history = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
        ];

        assert_eq!(truncate_history(&history, 5), history);
    }

    #[test]
    fn truncate_history_drops_oldest_messages() {
        let history: Vec<Value> = (0..10)
            .flat_map(|i| {
                vec![
                    json!({"role": "user", "content": format!("msg {i}")}),
                    json!({"role": "assistant", "content": format!("reply {i}")}),
                ]
            })
            .collect();

        let truncated = truncate_history(&history, 6);

        assert!(truncated.len() <= 6);
        assert_eq!(truncated[0]["role"], "user");
    }

    #[test]
    fn truncate_history_preserves_tool_call_pairs() {
        let history = vec![
            json!({"role": "user", "content": "start"}),
            json!({"role": "assistant", "content": "noted"}),
            json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "overview", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "{}"}),
            json!({"role": "user", "content": "ok"}),
            json!({"role": "assistant", "content": "done"}),
        ];

        assert_eq!(
            truncate_history(&history, 4),
            vec![
                json!({"role": "user", "content": "ok"}),
                json!({"role": "assistant", "content": "done"}),
            ]
        );
    }

    #[test]
    fn truncate_history_skips_tool_result_at_cut_point() {
        let history = vec![
            json!({"role": "user", "content": "a"}),
            json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "r"}),
            json!({"role": "assistant", "content": "b"}),
            json!({"role": "user", "content": "c"}),
        ];

        assert_eq!(
            truncate_history(&history, 3),
            vec![
                json!({"role": "assistant", "content": "b"}),
                json!({"role": "user", "content": "c"}),
            ]
        );
    }

    #[test]
    fn truncate_history_noop_when_no_safe_cut_point() {
        let history = vec![
            json!({"role": "tool", "tool_call_id": "c1", "content": "r1"}),
            json!({"role": "tool", "tool_call_id": "c2", "content": "r2"}),
            json!({"role": "tool", "tool_call_id": "c3", "content": "r3"}),
        ];

        assert_eq!(truncate_history(&history, 1), history);
    }

    fn history_message_json_strategy() -> impl Strategy<Value = Value> {
        prop_oneof![
            Just(json!({"role": "user", "content": "u"})),
            Just(json!({"role": "assistant", "content": "a"})),
            Just(json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "overview",
                        "arguments": "{}"
                    }
                }]
            })),
            Just(json!({"role": "tool", "tool_call_id": "call_1", "content": "{}"})),
        ]
    }

    proptest! {
        #[test]
        fn truncate_history_returns_a_suffix_without_reordering(
            history in prop::collection::vec(history_message_json_strategy(), 0..20),
            max in 0usize..20,
        ) {
            let truncated = truncate_history(&history, max);

            prop_assert!(truncated.len() <= history.len());
            let start = history.len() - truncated.len();
            prop_assert_eq!(truncated.as_slice(), &history[start..]);

            if history.len() <= max {
                prop_assert_eq!(truncated.as_slice(), history.as_slice());
            }

            if truncated.len() < history.len() && !truncated.is_empty() {
                let first = &truncated[0];
                let role = first.get("role").and_then(Value::as_str).unwrap_or("");
                let has_tool_calls = first
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|array| !array.is_empty());
                prop_assert!(role == "user" || (role == "assistant" && !has_tool_calls));
            }
        }
    }
}
