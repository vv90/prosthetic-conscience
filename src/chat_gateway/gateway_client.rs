//! HTTP client for sending chat completion requests to the gateway
//! and collecting streamed SSE responses.

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("gateway returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("SSE parse error: {0}")]
    Parse(String),
}

pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
    auth_token: Option<String>,
}

impl GatewayClient {
    pub fn new(base_url: String, auth_token: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
            auth_token,
        }
    }

    /// Send a streaming chat completion request and collect all SSE data chunks.
    ///
    /// Returns the raw JSON `Value` chunks (one per SSE `data:` line) that can
    /// be fed to `response_assembler::assemble()`.
    pub async fn chat(&self, mut payload: Value) -> Result<Vec<Value>, ClientError> {
        // Ensure stream: true in the payload.
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("stream".to_owned(), Value::Bool(true));
        }

        let url = format!("{}/v1/chat/completions", self.base_url);

        let mut request = self.http.post(&url).json(&payload);
        if let Some(token) = &self.auth_token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        let response = request.send().await?;

        let status = response.status().as_u16();
        if status != 200 {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| String::from("<unreadable>"));
            return Err(ClientError::Status { status, body });
        }

        collect_sse_chunks(response).await
    }
}

/// Read an SSE response body and collect all `data:` payloads as parsed JSON values.
///
/// Stops on `data: [DONE]` or when the stream ends. Skips comment lines and
/// empty lines.
async fn collect_sse_chunks(response: reqwest::Response) -> Result<Vec<Value>, ClientError> {
    let mut buffer = String::new();
    let mut body = response;
    let mut chunks = Vec::new();

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
                    return Ok(chunks);
                }
                match serde_json::from_str::<Value>(data) {
                    Ok(value) => chunks.push(value),
                    Err(e) => {
                        return Err(ClientError::Parse(format!("invalid JSON in SSE data: {e}")));
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
                // Stream ended. Process any remaining buffered data.
                if buffer.trim().is_empty() {
                    return Ok(chunks);
                }
                buffer.push('\n');
            }
        }
    }
}
