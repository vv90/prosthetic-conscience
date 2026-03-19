use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum WhisperError {
    #[error("whisper server request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("whisper server returned status {0}")]
    Status(u16),
    #[error("failed to parse whisper response: {0}")]
    Parse(String),
    #[error("failed to decode audio: {0}")]
    Decode(String),
}

pub struct WhisperClient {
    http: reqwest::Client,
    base_url: String,
}

impl WhisperClient {
    pub fn new(base_url: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url,
        }
    }

    /// Send an audio transcription request to the whisper backend.
    ///
    /// Expects a JSON payload with `audio_base64` (base64-encoded audio bytes),
    /// `model`, and optional fields (`language`, `prompt`, `response_format`,
    /// `temperature`). Decodes audio, sends as multipart form data to the
    /// whisper server, and returns the JSON response.
    pub async fn transcribe(&self, payload: Value) -> Result<Value, WhisperError> {
        let audio_b64 = payload["audio_base64"]
            .as_str()
            .ok_or_else(|| WhisperError::Decode("missing audio_base64 field".into()))?;

        let audio_bytes = BASE64
            .decode(audio_b64)
            .map_err(|e| WhisperError::Decode(format!("invalid base64: {e}")))?;

        let model = payload["model"].as_str().unwrap_or("whisper-1").to_owned();

        let file_name = payload["file_name"]
            .as_str()
            .unwrap_or("audio.wav")
            .to_owned();

        let file_part = reqwest::multipart::Part::bytes(audio_bytes)
            .file_name(file_name)
            .mime_str("application/octet-stream")
            .map_err(|e| WhisperError::Parse(format!("invalid mime type: {e}")))?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", model);

        if let Some(language) = payload["language"].as_str() {
            form = form.text("language", language.to_owned());
        }
        if let Some(prompt) = payload["prompt"].as_str() {
            form = form.text("prompt", prompt.to_owned());
        }
        if let Some(format) = payload["response_format"].as_str() {
            form = form.text("response_format", format.to_owned());
        }
        if let Some(temp) = payload["temperature"].as_f64() {
            form = form.text("temperature", temp.to_string());
        }

        let url = format!("{}/v1/audio/transcriptions", self.base_url);
        let response = self.http.post(&url).multipart(form).send().await?;

        let status = response.status().as_u16();
        if status != 200 {
            return Err(WhisperError::Status(status));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| WhisperError::Parse(format!("invalid JSON response: {e}")))?;

        Ok(result)
    }
}
