//! Session wire protocol types shared between I/O shells.
//!
//! These enums define the JSON schema for session messages exchanged over
//! WebSocket between a session client and the gateway. They are pure wire
//! types so they can be shared by the native CLI and future wasm/browser
//! adapters.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Messages sent by a session client to the gateway over WebSocket.
///
/// The first message must be a handshake: `Create` (new session) or
/// `Subscribe` (join existing). Subsequent messages are `Append` or
/// `Heartbeat`. Closing the WS connection is the unsubscribe signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionClientMessage {
    Create,
    Subscribe { session_id: String },
    Append { payload: Value },
    Heartbeat,
}

/// Messages sent by the gateway to a session client over WebSocket.
///
/// `Subscribed` confirms the client is now subscribed (whether via create or
/// join). `Entry` pushes new entries. `SubscriberRemoved` signals the gateway
/// removed the subscription (e.g. timeout). `Error` reports protocol issues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionGatewayMessage {
    Subscribed {
        session_id: String,
        latest_entry_index: Option<usize>,
    },
    Entry {
        index: usize,
        payload: Value,
    },
    SubscriberRemoved,
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn session_client_create_round_trip() {
        let msg = SessionClientMessage::Create;
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionClientMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_client_create_from_json() {
        let input = r#"{"type":"create"}"#;
        let parsed: SessionClientMessage = serde_json::from_str(input).unwrap();
        assert_eq!(parsed, SessionClientMessage::Create);
    }

    #[test]
    fn session_client_subscribe_round_trip() {
        let msg = SessionClientMessage::Subscribe {
            session_id: String::from("sess-abc"),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionClientMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_client_subscribe_from_json() {
        let input = r#"{"type":"subscribe","session_id":"sess-abc"}"#;
        let parsed: SessionClientMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            SessionClientMessage::Subscribe {
                session_id: String::from("sess-abc"),
            }
        );
    }

    #[test]
    fn session_client_append_round_trip() {
        let msg = SessionClientMessage::Append {
            payload: json!({"text": "hello"}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionClientMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_client_append_from_json() {
        let input = r#"{"type":"append","payload":{"text":"hello"}}"#;
        let parsed: SessionClientMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            SessionClientMessage::Append {
                payload: json!({"text": "hello"}),
            }
        );
    }

    #[test]
    fn session_client_heartbeat_round_trip() {
        let msg = SessionClientMessage::Heartbeat;
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionClientMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_client_heartbeat_from_json() {
        let input = r#"{"type":"heartbeat"}"#;
        let parsed: SessionClientMessage = serde_json::from_str(input).unwrap();
        assert_eq!(parsed, SessionClientMessage::Heartbeat);
    }

    #[test]
    fn session_client_unknown_type_is_error() {
        let input = r#"{"type":"unknown"}"#;
        let result = serde_json::from_str::<SessionClientMessage>(input);
        assert!(result.is_err());
    }

    #[test]
    fn session_client_missing_type_is_error() {
        let input = r#"{"payload":"hello"}"#;
        let result = serde_json::from_str::<SessionClientMessage>(input);
        assert!(result.is_err());
    }

    #[test]
    fn session_gateway_subscribed_round_trip() {
        let msg = SessionGatewayMessage::Subscribed {
            session_id: String::from("sess-abc"),
            latest_entry_index: Some(7),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionGatewayMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_gateway_subscribed_from_json() {
        let input = r#"{"type":"subscribed","session_id":"sess-abc","latest_entry_index":7}"#;
        let parsed: SessionGatewayMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            SessionGatewayMessage::Subscribed {
                session_id: String::from("sess-abc"),
                latest_entry_index: Some(7),
            }
        );
    }

    #[test]
    fn session_gateway_subscribed_with_null_latest_entry_index_from_json() {
        let input = r#"{"type":"subscribed","session_id":"sess-abc","latest_entry_index":null}"#;
        let parsed: SessionGatewayMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            SessionGatewayMessage::Subscribed {
                session_id: String::from("sess-abc"),
                latest_entry_index: None,
            }
        );
    }

    #[test]
    fn session_gateway_entry_round_trip() {
        let msg = SessionGatewayMessage::Entry {
            index: 42,
            payload: json!({"data": "value"}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionGatewayMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_gateway_entry_from_json() {
        let input = r#"{"type":"entry","index":0,"payload":{"data":"value"}}"#;
        let parsed: SessionGatewayMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            SessionGatewayMessage::Entry {
                index: 0,
                payload: json!({"data": "value"}),
            }
        );
    }

    #[test]
    fn session_gateway_subscriber_removed_round_trip() {
        let msg = SessionGatewayMessage::SubscriberRemoved;
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionGatewayMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_gateway_subscriber_removed_from_json() {
        let input = r#"{"type":"subscriber_removed"}"#;
        let parsed: SessionGatewayMessage = serde_json::from_str(input).unwrap();
        assert_eq!(parsed, SessionGatewayMessage::SubscriberRemoved);
    }

    #[test]
    fn session_gateway_error_round_trip() {
        let msg = SessionGatewayMessage::Error {
            message: String::from("session not found"),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: SessionGatewayMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn session_gateway_error_from_json() {
        let input = r#"{"type":"error","message":"session not found"}"#;
        let parsed: SessionGatewayMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            SessionGatewayMessage::Error {
                message: String::from("session not found"),
            }
        );
    }

    #[test]
    fn session_gateway_unknown_type_is_error() {
        let input = r#"{"type":"unknown"}"#;
        let result = serde_json::from_str::<SessionGatewayMessage>(input);
        assert!(result.is_err());
    }
}
