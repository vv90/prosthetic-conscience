use async_stream::try_stream;
use futures_util::Stream;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("inference server request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("inference server returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("malformed SSE data: {0}")]
    Parse(String),
}

pub struct InferenceClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model_override: Option<String>,
}

/// Replace the `model` field in an outgoing request payload with the override value.
fn apply_model_override(payload: &mut Value, model: &str) {
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("model".to_owned(), Value::String(model.to_owned()));
    }
}

/// Replace the `model` field in a response chunk with `"mystery_model"` to
/// prevent leaking the real model name back to the client.
fn scrub_model_from_chunk(chunk: &mut Value) {
    if let Some(obj) = chunk.as_object_mut()
        && obj.contains_key("model")
    {
        obj.insert(
            "model".to_owned(),
            Value::String("mystery_model".to_owned()),
        );
    }
}

impl InferenceClient {
    pub fn new(base_url: String, api_key: Option<String>, model_override: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
            api_key,
            model_override,
        }
    }

    /// Stream a chat completion request to the inference server.
    ///
    /// Returns a stream of JSON chunk values. Each value is the parsed
    /// `data:` payload from the SSE response (an OpenAI-compatible chunk
    /// object). The stream ends when the server sends `data: [DONE]`.
    pub fn stream_completion(
        &self,
        mut payload: Value,
    ) -> impl Stream<Item = Result<Value, InferenceError>> + '_ {
        // Ensure streaming is enabled in the payload.
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("stream".to_owned(), Value::Bool(true));
        }

        // Override model in the outgoing request if configured.
        if let Some(model) = &self.model_override {
            apply_model_override(&mut payload, model);
        }

        let scrub = self.model_override.is_some();

        try_stream! {
            let url = format!("{}/v1/chat/completions", self.base_url);
            let mut request = self.http.post(&url).json(&payload);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            let response = request.send().await?;

            let status = response.status().as_u16();
            let mut body = if status != 200 {
                let error_body = response.text().await.unwrap_or_default();
                // Truncate to avoid flooding logs with huge error pages.
                let error_body = if error_body.len() > 512 {
                    format!("{}…", &error_body[..error_body.floor_char_boundary(512)])
                } else {
                    error_body
                };
                Err(InferenceError::Status { status, body: error_body })?
            } else {
                response
            };

            // Buffer-based SSE parsing, same approach as tests/support/client.rs.
            let mut buffer = String::new();

            loop {
                // Try to extract a complete line from the buffer.
                if let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim_end_matches('\r').to_owned();
                    buffer = buffer[newline_pos + 1..].to_owned();

                    // Skip empty lines (SSE event separators) and comments.
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }

                    // Parse data: lines.
                    if let Some(data) = line
                        .strip_prefix("data: ")
                        .or_else(|| line.strip_prefix("data:"))
                    {
                        let data = data.trim();
                        if data == "[DONE]" {
                            return;
                        }
                        match serde_json::from_str::<Value>(data) {
                            Ok(mut value) => {
                                if scrub {
                                    scrub_model_from_chunk(&mut value);
                                }
                                yield value;
                            }
                            Err(e) => {
                                Err(InferenceError::Parse(format!(
                                    "invalid JSON in SSE data: {e}"
                                )))?;
                            }
                        }
                    }

                    // Skip unknown line prefixes (event:, id:, retry:, etc.).
                    continue;
                }

                // Buffer doesn't contain a complete line — read more.
                let chunk = body.chunk().await?;
                match chunk {
                    Some(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        buffer.push_str(&text);
                    }
                    None => {
                        // Stream ended. If there's remaining buffered data with
                        // a data: prefix, try to process it.
                        if !buffer.trim().is_empty() {
                            buffer.push('\n');
                        } else {
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- apply_model_override ------------------------------------------------

    #[test]
    fn model_override_replaces_existing_model() {
        let mut payload = json!({"model": "client-choice", "messages": []});
        apply_model_override(&mut payload, "Meta-Llama-3.3-70B-Instruct");
        assert_eq!(payload["model"], "Meta-Llama-3.3-70B-Instruct");
        assert_eq!(payload["messages"], json!([]));
    }

    #[test]
    fn model_override_inserts_when_absent() {
        let mut payload = json!({"messages": []});
        apply_model_override(&mut payload, "Meta-Llama-3.3-70B-Instruct");
        assert_eq!(payload["model"], "Meta-Llama-3.3-70B-Instruct");
    }

    #[test]
    fn model_override_is_noop_for_non_object() {
        let mut payload = json!("not an object");
        apply_model_override(&mut payload, "anything");
        assert_eq!(payload, json!("not an object"));
    }

    // -- scrub_model_from_chunk ----------------------------------------------

    #[test]
    fn scrub_replaces_model_with_mystery() {
        let mut chunk = json!({"model": "Meta-Llama-3.3-70B-Instruct", "choices": []});
        scrub_model_from_chunk(&mut chunk);
        assert_eq!(chunk["model"], "mystery_model");
        assert_eq!(chunk["choices"], json!([]));
    }

    #[test]
    fn scrub_leaves_chunk_without_model_untouched() {
        let mut chunk = json!({"choices": [{"delta": {"content": "hi"}}]});
        let original = chunk.clone();
        scrub_model_from_chunk(&mut chunk);
        assert_eq!(chunk, original);
    }

    #[test]
    fn scrub_is_noop_for_non_object() {
        let mut chunk = json!(42);
        scrub_model_from_chunk(&mut chunk);
        assert_eq!(chunk, json!(42));
    }
}
