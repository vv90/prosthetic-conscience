use std::time::Duration;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::json;
use tokio::sync::mpsc;

use crate::gateway::relay::StreamFrame;
use crate::protocol::Capability;
use crate::router::state::AppState;

/// Maximum audio file size: 25 MB (matches OpenAI's limit).
const MAX_FILE_SIZE: usize = 25 * 1024 * 1024;

/// Timeout for collecting the worker response.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) async fn audio_transcriptions(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Response {
    let mut file_bytes: Option<(Vec<u8>, String)> = None;
    let mut model: Option<String> = None;
    let mut language: Option<String> = None;
    let mut prompt: Option<String> = None;
    let mut response_format: Option<String> = None;
    let mut temperature: Option<f64> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = match field.name() {
            Some(n) => n.to_owned(),
            None => continue,
        };

        match name.as_str() {
            "file" => {
                let file_name = field.file_name().unwrap_or("audio.wav").to_owned();
                let bytes = match field.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            axum::Json(
                                json!({"error": {"message": format!("failed to read file: {e}")}}),
                            ),
                        )
                            .into_response();
                    }
                };
                if bytes.len() > MAX_FILE_SIZE {
                    return (
                        StatusCode::BAD_REQUEST,
                        axum::Json(json!({"error": {"message": "file exceeds 25 MB limit"}})),
                    )
                        .into_response();
                }
                file_bytes = Some((bytes.to_vec(), file_name));
            }
            "model" => {
                if let Ok(text) = field.text().await {
                    model = Some(text);
                }
            }
            "language" => {
                if let Ok(text) = field.text().await {
                    language = Some(text);
                }
            }
            "prompt" => {
                if let Ok(text) = field.text().await {
                    prompt = Some(text);
                }
            }
            "response_format" => {
                if let Ok(text) = field.text().await {
                    response_format = Some(text);
                }
            }
            "temperature" => {
                if let Ok(text) = field.text().await {
                    temperature = text.parse().ok();
                }
            }
            _ => {} // ignore unknown fields
        }
    }

    let (bytes, file_name) = match file_bytes {
        Some(b) => b,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": {"message": "missing required field: file"}})),
            )
                .into_response();
        }
    };

    let model = match model {
        Some(m) => m,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(json!({"error": {"message": "missing required field: model"}})),
            )
                .into_response();
        }
    };

    // Build JSON payload with base64-encoded audio for the worker protocol.
    let audio_base64 = BASE64.encode(&bytes);
    let mut payload = json!({
        "audio_base64": audio_base64,
        "model": model,
        "file_name": file_name,
    });
    if let Some(lang) = language {
        payload["language"] = json!(lang);
    }
    if let Some(p) = prompt {
        payload["prompt"] = json!(p);
    }
    if let Some(fmt) = response_format {
        payload["response_format"] = json!(fmt);
    }
    if let Some(temp) = temperature {
        payload["temperature"] = json!(temp);
    }

    // Register a stream channel to receive the worker's response.
    let (tx, mut rx) = mpsc::channel::<StreamFrame>(4);

    let stream_id = match state.runtime.register_stream(tx).await {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({"error": {"message": "gateway unavailable"}})),
            )
                .into_response();
        }
    };

    if state
        .runtime
        .http_chat_requested(stream_id, payload, true, Capability::Transcription)
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"error": {"message": "gateway unavailable"}})),
        )
            .into_response();
    }

    // Collect the worker response (expect a single Chunk + Done, or an Error).
    match tokio::time::timeout(RESPONSE_TIMEOUT, collect_response(&mut rx)).await {
        Ok(Ok(data)) => axum::Json(data).into_response(),
        Ok(Err(message)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": {"message": message}})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            axum::Json(json!({"error": {"message": "transcription timed out"}})),
        )
            .into_response(),
    }
}

/// Collect frames from the stream channel into a single response value.
/// Expects one `Chunk` followed by `Done`. Returns the chunk data on success.
async fn collect_response(
    rx: &mut mpsc::Receiver<StreamFrame>,
) -> Result<serde_json::Value, String> {
    let mut result: Option<serde_json::Value> = None;

    while let Some(frame) = rx.recv().await {
        match frame {
            StreamFrame::Chunk { data } => {
                result = Some(data);
            }
            StreamFrame::Done => {
                return result.ok_or_else(|| String::from("worker sent done without data"));
            }
            StreamFrame::Error { message } => {
                return Err(message);
            }
        }
    }

    // Channel closed without Done frame.
    result.ok_or_else(|| String::from("worker disconnected without response"))
}
