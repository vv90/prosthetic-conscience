//! Pure local conversation state machine.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    history: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Event {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Effect {}

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
    match event {}
}

pub fn view(state: &State) -> View {
    View {
        history: state.history.clone(),
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
}
