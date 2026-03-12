//! Wire protocol types shared between gateway, worker agent, and client sidecar.
//!
//! These types define the JSON schema for messages exchanged over WebSocket
//! (worker ↔ gateway) and HTTP (client → gateway). They are the canonical
//! source of truth for the wire format.
//!
//! Internal channel types (`StreamFrame`, `RelayOutcome`, `WorkerJob`) live
//! in their respective gateway modules — they are not wire types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Worker → Gateway (WebSocket)
// ---------------------------------------------------------------------------

fn default_error_message() -> String {
    String::from("unknown worker error")
}

/// Messages sent by a worker to the gateway during job processing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerMessage {
    Chunk {
        data: Value,
    },
    End,
    Error {
        #[serde(default = "default_error_message")]
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Gateway → Worker (WebSocket)
// ---------------------------------------------------------------------------

/// Messages sent by the gateway to a connected worker.
///
/// An enum rather than a struct to allow future extension (cancel, ping,
/// capability query) without breaking the schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GatewayToWorker {
    Job {
        client_stream_id: String,
        payload: Value,
    },
}

// ---------------------------------------------------------------------------
// Client → Gateway (HTTP POST)
// ---------------------------------------------------------------------------

/// OpenAI-compatible chat completion request.
///
/// The gateway extracts the `stream` flag and passes the rest of the body
/// through as an opaque `payload: Value`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(flatten)]
    pub payload: Value,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // -- WorkerMessage round-trip tests --

    #[test]
    fn worker_message_chunk_round_trip() {
        let msg = WorkerMessage::Chunk {
            data: json!({"content": "hello"}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: WorkerMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn worker_message_chunk_from_json() {
        let input = r#"{"type":"chunk","data":{"content":"hello"}}"#;
        let parsed: WorkerMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            WorkerMessage::Chunk {
                data: json!({"content": "hello"})
            }
        );
    }

    #[test]
    fn worker_message_end_round_trip() {
        let msg = WorkerMessage::End;
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: WorkerMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn worker_message_end_from_json() {
        let input = r#"{"type":"end"}"#;
        let parsed: WorkerMessage = serde_json::from_str(input).unwrap();
        assert_eq!(parsed, WorkerMessage::End);
    }

    #[test]
    fn worker_message_error_round_trip() {
        let msg = WorkerMessage::Error {
            message: String::from("OOM"),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: WorkerMessage = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn worker_message_error_from_json() {
        let input = r#"{"type":"error","message":"OOM"}"#;
        let parsed: WorkerMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            WorkerMessage::Error {
                message: String::from("OOM")
            }
        );
    }

    #[test]
    fn worker_message_error_missing_message_uses_default() {
        let input = r#"{"type":"error"}"#;
        let parsed: WorkerMessage = serde_json::from_str(input).unwrap();
        assert_eq!(
            parsed,
            WorkerMessage::Error {
                message: String::from("unknown worker error")
            }
        );
    }

    #[test]
    fn worker_message_unknown_type_is_error() {
        let input = r#"{"type":"unknown_thing"}"#;
        let result = serde_json::from_str::<WorkerMessage>(input);
        assert!(result.is_err());
    }

    #[test]
    fn worker_message_missing_type_is_error() {
        let input = r#"{"data":"hello"}"#;
        let result = serde_json::from_str::<WorkerMessage>(input);
        assert!(result.is_err());
    }

    #[test]
    fn worker_message_invalid_json_is_error() {
        let input = r#"not json at all"#;
        let result = serde_json::from_str::<WorkerMessage>(input);
        assert!(result.is_err());
    }

    // -- GatewayToWorker tests --

    #[test]
    fn gateway_to_worker_job_round_trip() {
        let msg = GatewayToWorker::Job {
            client_stream_id: String::from("abc-123"),
            payload: json!({"model": "llama", "messages": []}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayToWorker = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn gateway_to_worker_job_matches_current_wire_format() {
        let msg = GatewayToWorker::Job {
            client_stream_id: String::from("abc-123"),
            payload: json!({"model": "demo"}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        // Must match the shape previously produced by json!() in worker_ws_upgrade.rs
        assert_eq!(parsed["type"], "job");
        assert_eq!(parsed["client_stream_id"], "abc-123");
        assert_eq!(parsed["payload"], json!({"model": "demo"}));
    }

    // -- ChatRequest tests --

    #[test]
    fn chat_request_with_stream_true() {
        let input = r#"{"stream": true, "model": "llama", "messages": []}"#;
        let req: ChatRequest = serde_json::from_str(input).unwrap();
        assert_eq!(req.stream, Some(true));
        assert_eq!(req.payload["model"], "llama");
    }

    #[test]
    fn chat_request_with_stream_false() {
        let input = r#"{"stream": false, "model": "llama"}"#;
        let req: ChatRequest = serde_json::from_str(input).unwrap();
        assert_eq!(req.stream, Some(false));
    }

    #[test]
    fn chat_request_without_stream_field() {
        let input = r#"{"model": "llama"}"#;
        let req: ChatRequest = serde_json::from_str(input).unwrap();
        assert_eq!(req.stream, None);
    }

    #[test]
    fn chat_request_preserves_all_fields_in_payload() {
        let input =
            r#"{"stream": true, "model": "llama", "messages": [{"role":"user","content":"hi"}]}"#;
        let req: ChatRequest = serde_json::from_str(input).unwrap();
        assert_eq!(req.payload["model"], "llama");
        assert_eq!(req.payload["messages"][0]["role"], "user");
    }
}
