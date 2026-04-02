use std::net::SocketAddr;

use reqwest::multipart;
use serde_json::{Value, json};

#[derive(Debug, PartialEq)]
pub enum SseEvent {
    /// A `data:` line parsed as JSON.
    Data(Value),
    /// The `data: [DONE]` sentinel.
    Done,
}

pub struct SseClient {
    /// Buffered text remaining from the last chunk read.
    buffer: String,
    /// The streaming response body.
    body: reqwest::Response,
}

impl SseClient {
    /// Send a streaming chat completion request and return the SSE stream handle.
    pub async fn chat(addr: SocketAddr, payload: Value) -> Self {
        let url = format!("http://{}/v1/chat/completions", addr);

        // Merge {"stream": true} into the payload object.
        let mut body = payload;
        body.as_object_mut()
            .expect("payload must be a JSON object")
            .insert("stream".to_owned(), json!(true));

        let response = reqwest::Client::new()
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("failed to send chat request");

        assert_eq!(
            response.status(),
            200,
            "expected 200, got {}",
            response.status()
        );

        Self {
            buffer: String::new(),
            body: response,
        }
    }

    /// Read the next SSE event from the stream.
    ///
    /// Returns `None` when the stream ends (connection closed).
    /// Skips SSE comment lines (starting with `:`) and empty lines.
    pub async fn next_event(&mut self) -> Option<SseEvent> {
        loop {
            // Try to extract a complete line from the buffer.
            if let Some(newline_pos) = self.buffer.find('\n') {
                let line = self.buffer[..newline_pos].trim_end_matches('\r').to_owned();
                self.buffer = self.buffer[newline_pos + 1..].to_owned();

                // Skip empty lines (SSE event separators).
                if line.is_empty() {
                    continue;
                }

                // Skip SSE comment lines (keep-alive, etc.).
                if line.starts_with(':') {
                    continue;
                }

                // Skip non-data fields (e.g., "event:", "id:", "retry:").
                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return Some(SseEvent::Done);
                    }
                    match serde_json::from_str::<Value>(data) {
                        Ok(value) => return Some(SseEvent::Data(value)),
                        Err(_) => {
                            // Non-JSON data line — return as string in a Value.
                            return Some(SseEvent::Data(Value::String(data.to_owned())));
                        }
                    }
                }

                // Unknown line prefix — skip.
                continue;
            }

            // Buffer doesn't contain a complete line. Read more from the body.
            let chunk = self
                .body
                .chunk()
                .await
                .expect("error reading response body");
            match chunk {
                Some(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    self.buffer.push_str(&text);
                }
                None => {
                    // Stream ended. Process any remaining data in buffer.
                    if self.buffer.is_empty() {
                        return None;
                    }
                    // Add a newline so the line-extraction logic above picks it up.
                    self.buffer.push('\n');
                }
            }
        }
    }

    /// Collect all events until `[DONE]` or stream end.
    pub async fn collect_all(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.next_event().await {
            let is_done = event == SseEvent::Done;
            events.push(event);
            if is_done {
                break;
            }
        }
        events
    }
}

/// Send a multipart transcription request and return the raw response.
pub async fn transcribe(addr: SocketAddr, file_bytes: &[u8], model: &str) -> reqwest::Response {
    let url = format!("http://{}/v1/audio/transcriptions", addr);

    let file_part = multipart::Part::bytes(file_bytes.to_vec())
        .file_name("test.wav")
        .mime_str("audio/wav")
        .expect("valid mime type");

    let form = multipart::Form::new()
        .part("file", file_part)
        .text("model", model.to_owned());

    reqwest::Client::new()
        .post(&url)
        .multipart(form)
        .send()
        .await
        .expect("failed to send transcription request")
}
