use async_stream::try_stream;
use futures_util::Stream;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum InferenceError {
    #[error("inference server request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("inference server returned status {0}")]
    Status(u16),
    #[error("malformed SSE data: {0}")]
    Parse(String),
}

pub struct InferenceClient {
    http: reqwest::Client,
    base_url: String,
}

impl InferenceClient {
    pub fn new(base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
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

        try_stream! {
            let url = format!("{}/v1/chat/completions", self.base_url);
            let response = self.http.post(&url).json(&payload).send().await?;

            let status = response.status().as_u16();
            if status != 200 {
                Err(InferenceError::Status(status))?;
            }

            // Buffer-based SSE parsing, same approach as tests/support/client.rs.
            let mut buffer = String::new();
            let mut body = response;

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
                            Ok(value) => yield value,
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
