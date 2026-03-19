//! Wire protocol types shared between gateway, worker agent, and client sidecar.
//!
//! These types define the JSON schema for messages exchanged over WebSocket
//! (worker ↔ gateway) and HTTP (client → gateway). They are the canonical
//! source of truth for the wire format.
//!
//! Internal channel types (`StreamFrame`, `RelayOutcome`, `WorkerJob`) live
//! in their respective gateway modules — they are not wire types.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Capability — shared protocol type
// ---------------------------------------------------------------------------

/// A capability that a worker can declare and a job can require.
///
/// Used across the entire stack: routers, kernel, effects, wire protocol,
/// and worker agent. Serializes to/from lowercase strings (`"chat"`,
/// `"transcription"`) for wire compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Chat,
    Transcription,
}

impl Capability {
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Chat => "chat",
            Capability::Transcription => "transcription",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseCapabilityError(String);

impl fmt::Display for ParseCapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown capability: {:?}", self.0)
    }
}

impl std::error::Error for ParseCapabilityError {}

impl FromStr for Capability {
    type Err = ParseCapabilityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "chat" => Ok(Capability::Chat),
            "transcription" => Ok(Capability::Transcription),
            other => Err(ParseCapabilityError(other.to_owned())),
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parse a comma-separated capability string (e.g., "chat,transcription")
/// into a `BTreeSet<Capability>`. Returns an error if any token is unknown
/// or if the result is empty.
pub fn parse_capabilities(s: &str) -> Result<BTreeSet<Capability>, ParseCapabilityError> {
    let caps: Result<BTreeSet<Capability>, _> = s
        .split(',')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(Capability::from_str)
        .collect();
    let caps = caps?;
    if caps.is_empty() {
        return Err(ParseCapabilityError(String::new()));
    }
    Ok(caps)
}

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
        capability: Capability,
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
            capability: Capability::Chat,
            payload: json!({"model": "llama", "messages": []}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayToWorker = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn gateway_to_worker_transcription_job_round_trip() {
        let msg = GatewayToWorker::Job {
            client_stream_id: String::from("xyz-789"),
            capability: Capability::Transcription,
            payload: json!({"audio_base64": "AAAA", "model": "whisper-1"}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: GatewayToWorker = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn gateway_to_worker_job_matches_wire_format() {
        let msg = GatewayToWorker::Job {
            client_stream_id: String::from("abc-123"),
            capability: Capability::Chat,
            payload: json!({"model": "demo"}),
        };
        let json_str = serde_json::to_string(&msg).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["type"], "job");
        assert_eq!(parsed["client_stream_id"], "abc-123");
        assert_eq!(parsed["capability"], "chat");
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

    // -- Capability tests --

    #[test]
    fn capability_as_str_round_trip() {
        assert_eq!("chat".parse::<Capability>().unwrap(), Capability::Chat);
        assert_eq!(
            "transcription".parse::<Capability>().unwrap(),
            Capability::Transcription
        );
        assert_eq!(Capability::Chat.as_str(), "chat");
        assert_eq!(Capability::Transcription.as_str(), "transcription");
    }

    #[test]
    fn capability_display_matches_as_str() {
        assert_eq!(format!("{}", Capability::Chat), "chat");
        assert_eq!(format!("{}", Capability::Transcription), "transcription");
    }

    #[test]
    fn capability_parse_rejects_unknown() {
        assert!("unknown".parse::<Capability>().is_err());
        assert!("Chat".parse::<Capability>().is_err()); // case-sensitive
        assert!("CHAT".parse::<Capability>().is_err());
        assert!("".parse::<Capability>().is_err());
    }

    #[test]
    fn capability_serde_round_trip() {
        let chat_json = serde_json::to_string(&Capability::Chat).unwrap();
        assert_eq!(chat_json, r#""chat""#);
        let parsed: Capability = serde_json::from_str(&chat_json).unwrap();
        assert_eq!(parsed, Capability::Chat);

        let trans_json = serde_json::to_string(&Capability::Transcription).unwrap();
        assert_eq!(trans_json, r#""transcription""#);
        let parsed: Capability = serde_json::from_str(&trans_json).unwrap();
        assert_eq!(parsed, Capability::Transcription);
    }

    #[test]
    fn parse_capabilities_single() {
        let caps = parse_capabilities("chat").unwrap();
        assert_eq!(caps, std::collections::BTreeSet::from([Capability::Chat]));
    }

    #[test]
    fn parse_capabilities_multiple() {
        let caps = parse_capabilities("chat,transcription").unwrap();
        assert_eq!(caps.len(), 2);
    }

    #[test]
    fn parse_capabilities_trims_whitespace() {
        let caps = parse_capabilities(" chat , transcription ").unwrap();
        assert_eq!(caps.len(), 2);
    }

    #[test]
    fn parse_capabilities_deduplicates() {
        let caps = parse_capabilities("chat,chat").unwrap();
        assert_eq!(caps.len(), 1);
    }

    #[test]
    fn parse_capabilities_rejects_empty() {
        assert!(parse_capabilities("").is_err());
        assert!(parse_capabilities(",,,").is_err());
    }

    #[test]
    fn parse_capabilities_rejects_unknown_token() {
        assert!(parse_capabilities("chat,bogus").is_err());
    }
}
