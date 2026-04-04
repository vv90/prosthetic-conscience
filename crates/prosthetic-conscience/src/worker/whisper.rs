use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Validated, typed representation of a transcription request.
/// Produced by the pure `parse_request` function from an opaque JSON payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptionRequest {
    pub audio_bytes: Vec<u8>,
    pub file_name: String,
    pub mime_type: String,
    pub model: String,
    pub language: Option<String>,
    pub prompt: Option<String>,
    pub response_format: Option<String>,
    pub temperature: Option<f64>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TranscriptionError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid base64 in audio_base64: {0}")]
    InvalidBase64(String),
    #[error("temperature must be a number")]
    InvalidTemperature,
    #[error("audio data is empty")]
    EmptyAudio,
    #[error("whisper server request failed: {0}")]
    HttpError(String),
    #[error("whisper server returned status {status}: {body}")]
    ServerError { status: u16, body: String },
    #[error("invalid response from whisper server: {0}")]
    InvalidResponse(String),
}

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Derive MIME type from a file name extension.
pub fn mime_type_for_filename(file_name: &str) -> &'static str {
    match file_name.rsplit('.').next() {
        Some("wav") => "audio/wav",
        Some("webm") => "audio/webm",
        Some("mp4") | Some("m4a") => "audio/mp4",
        Some("ogg") | Some("oga") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// Parse and validate an opaque JSON payload into a typed `TranscriptionRequest`.
///
/// This is a pure function: no I/O, no async, deterministic.
pub fn parse_request(payload: &Value) -> Result<TranscriptionRequest, TranscriptionError> {
    let audio_b64 = payload
        .get("audio_base64")
        .and_then(Value::as_str)
        .ok_or(TranscriptionError::MissingField("audio_base64"))?;

    let audio_bytes = BASE64
        .decode(audio_b64)
        .map_err(|e| TranscriptionError::InvalidBase64(e.to_string()))?;

    if audio_bytes.is_empty() {
        return Err(TranscriptionError::EmptyAudio);
    }

    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .ok_or(TranscriptionError::MissingField("model"))?
        .to_owned();

    let file_name = payload
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or("audio.wav")
        .to_owned();

    let mime_type = mime_type_for_filename(&file_name).to_owned();

    let language = payload
        .get("language")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let prompt = payload
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let response_format = payload
        .get("response_format")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let temperature = match payload.get("temperature") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_f64().ok_or(TranscriptionError::InvalidTemperature)?),
    };

    Ok(TranscriptionRequest {
        audio_bytes,
        file_name,
        mime_type,
        model,
        language,
        prompt,
        response_format,
        temperature,
    })
}

const MAX_ERROR_BODY_LEN: usize = 512;
const MAX_PREVIEW_LEN: usize = 200;

/// Truncate a string at a char boundary, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_owned();
    }
    let end = s.floor_char_boundary(max_len);
    format!("{}...", &s[..end])
}

/// Interpret an HTTP response (status + body bytes) into a result.
///
/// Pure function: no I/O, no async, deterministic.
pub fn parse_response(status: u16, body: &[u8]) -> Result<Value, TranscriptionError> {
    let body_text = String::from_utf8_lossy(body);

    if status != 200 {
        return Err(TranscriptionError::ServerError {
            status,
            body: truncate_str(&body_text, MAX_ERROR_BODY_LEN),
        });
    }

    serde_json::from_slice(body).map_err(|e| {
        let preview = truncate_str(&body_text, MAX_PREVIEW_LEN);
        TranscriptionError::InvalidResponse(format!("{e} (body: {preview})"))
    })
}

// ---------------------------------------------------------------------------
// Impure transport
// ---------------------------------------------------------------------------

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

    pub async fn transcribe(&self, payload: Value) -> Result<Value, TranscriptionError> {
        let req = parse_request(&payload)?;
        let header_len = req.audio_bytes.len().min(16);
        let header = req
            .audio_bytes
            .get(..header_len)
            .unwrap_or(&req.audio_bytes);

        tracing::debug!(
            file_name = %req.file_name,
            mime_type = %req.mime_type,
            audio_size = req.audio_bytes.len(),
            header = ?header,
            "transcription request"
        );

        let file_part = reqwest::multipart::Part::bytes(req.audio_bytes)
            .file_name(req.file_name)
            .mime_str(&req.mime_type)
            .map_err(|e| TranscriptionError::HttpError(format!("invalid mime type: {e}")))?;

        let mut form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", req.model);

        if let Some(language) = req.language {
            form = form.text("language", language);
        }
        if let Some(prompt) = req.prompt {
            form = form.text("prompt", prompt);
        }
        if let Some(format) = req.response_format {
            form = form.text("response_format", format);
        }
        if let Some(temp) = req.temperature {
            form = form.text("temperature", temp.to_string());
        }

        let url = format!("{}/v1/audio/transcriptions", self.base_url);
        let response = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| TranscriptionError::HttpError(e.to_string()))?;

        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|e| TranscriptionError::HttpError(format!("failed to read body: {e}")))?;

        parse_response(status, &body)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- mime_type_for_filename --

    #[test]
    fn mime_wav() {
        assert_eq!(mime_type_for_filename("recording.wav"), "audio/wav");
    }

    #[test]
    fn mime_webm() {
        assert_eq!(mime_type_for_filename("recording.webm"), "audio/webm");
    }

    #[test]
    fn mime_mp4() {
        assert_eq!(mime_type_for_filename("recording.mp4"), "audio/mp4");
    }

    #[test]
    fn mime_m4a() {
        assert_eq!(mime_type_for_filename("recording.m4a"), "audio/mp4");
    }

    #[test]
    fn mime_ogg() {
        assert_eq!(mime_type_for_filename("recording.ogg"), "audio/ogg");
    }

    #[test]
    fn mime_oga() {
        assert_eq!(mime_type_for_filename("file.oga"), "audio/ogg");
    }

    #[test]
    fn mime_flac() {
        assert_eq!(mime_type_for_filename("recording.flac"), "audio/flac");
    }

    #[test]
    fn mime_mp3() {
        assert_eq!(mime_type_for_filename("recording.mp3"), "audio/mpeg");
    }

    #[test]
    fn mime_unknown_extension() {
        assert_eq!(
            mime_type_for_filename("recording.xyz"),
            "application/octet-stream"
        );
    }

    #[test]
    fn mime_no_extension() {
        assert_eq!(
            mime_type_for_filename("recording"),
            "application/octet-stream"
        );
    }

    // -- parse_request: happy path --

    fn valid_payload() -> Value {
        json!({
            "audio_base64": BASE64.encode(b"RIFF fake wav data"),
            "model": "whisper-1",
            "file_name": "test.wav",
        })
    }

    #[test]
    fn parse_request_happy_path() {
        let req = parse_request(&valid_payload()).unwrap();
        assert_eq!(req.audio_bytes, b"RIFF fake wav data");
        assert_eq!(req.file_name, "test.wav");
        assert_eq!(req.mime_type, "audio/wav");
        assert_eq!(req.model, "whisper-1");
        assert!(req.language.is_none());
        assert!(req.prompt.is_none());
        assert!(req.response_format.is_none());
        assert!(req.temperature.is_none());
    }

    #[test]
    fn parse_request_with_all_optional_fields() {
        let payload = json!({
            "audio_base64": BASE64.encode(b"audio"),
            "model": "whisper-1",
            "file_name": "clip.webm",
            "language": "en",
            "prompt": "meeting notes",
            "response_format": "verbose_json",
            "temperature": 0.5,
        });
        let req = parse_request(&payload).unwrap();
        assert_eq!(req.mime_type, "audio/webm");
        assert_eq!(req.language.as_deref(), Some("en"));
        assert_eq!(req.prompt.as_deref(), Some("meeting notes"));
        assert_eq!(req.response_format.as_deref(), Some("verbose_json"));
        assert_eq!(req.temperature, Some(0.5));
    }

    #[test]
    fn parse_request_default_file_name() {
        let payload = json!({
            "audio_base64": BASE64.encode(b"audio"),
            "model": "whisper-1",
        });
        let req = parse_request(&payload).unwrap();
        assert_eq!(req.file_name, "audio.wav");
        assert_eq!(req.mime_type, "audio/wav");
    }

    // -- parse_request: error paths --

    #[test]
    fn parse_request_missing_audio_base64() {
        let payload = json!({"model": "whisper-1"});
        assert_eq!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::MissingField("audio_base64"),
        );
    }

    #[test]
    fn parse_request_invalid_base64() {
        let payload = json!({
            "audio_base64": "not valid base64!!!",
            "model": "whisper-1",
        });
        assert!(matches!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::InvalidBase64(_),
        ));
    }

    #[test]
    fn parse_request_empty_audio() {
        let payload = json!({
            "audio_base64": BASE64.encode(b""),
            "model": "whisper-1",
        });
        assert_eq!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::EmptyAudio,
        );
    }

    #[test]
    fn parse_request_missing_model() {
        let payload = json!({
            "audio_base64": BASE64.encode(b"audio"),
        });
        assert_eq!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::MissingField("model"),
        );
    }

    #[test]
    fn parse_request_temperature_as_string() {
        let payload = json!({
            "audio_base64": BASE64.encode(b"audio"),
            "model": "whisper-1",
            "temperature": "hot",
        });
        assert_eq!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::InvalidTemperature,
        );
    }

    #[test]
    fn parse_request_temperature_null_is_none() {
        let payload = json!({
            "audio_base64": BASE64.encode(b"audio"),
            "model": "whisper-1",
            "temperature": null,
        });
        let req = parse_request(&payload).unwrap();
        assert!(req.temperature.is_none());
    }

    #[test]
    fn parse_request_audio_base64_not_string() {
        let payload = json!({
            "audio_base64": 12345,
            "model": "whisper-1",
        });
        assert_eq!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::MissingField("audio_base64"),
        );
    }

    #[test]
    fn parse_request_model_not_string() {
        let payload = json!({
            "audio_base64": BASE64.encode(b"audio"),
            "model": 42,
        });
        assert_eq!(
            parse_request(&payload).unwrap_err(),
            TranscriptionError::MissingField("model"),
        );
    }

    // -- parse_response --

    #[test]
    fn parse_response_success() {
        let body = br#"{"text": "hello world"}"#;
        let result = parse_response(200, body).unwrap();
        assert_eq!(result, json!({"text": "hello world"}));
    }

    #[test]
    fn parse_response_non_200_includes_body() {
        let body = b"failed to read audio data as wav (Unknown error)";
        let err = parse_response(400, body).unwrap_err();
        match err {
            TranscriptionError::ServerError { status, body } => {
                assert_eq!(status, 400);
                assert!(body.contains("failed to read audio data as wav"));
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_non_200_empty_body() {
        let err = parse_response(500, b"").unwrap_err();
        assert!(matches!(
            err,
            TranscriptionError::ServerError { status: 500, .. }
        ));
    }

    #[test]
    fn parse_response_non_200_truncates_long_body() {
        let body = "x".repeat(1000);
        let err = parse_response(502, body.as_bytes()).unwrap_err();
        match err {
            TranscriptionError::ServerError { body: b, .. } => {
                assert!(b.len() <= MAX_ERROR_BODY_LEN + 3); // +3 for "..."
                assert!(b.ends_with("..."));
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_non_200_truncates_at_char_boundary() {
        // Multi-byte chars: each is 4 bytes. 128 chars = 512 bytes exactly at limit.
        // 129 chars = 516 bytes, exceeds limit, must truncate at a char boundary.
        let body = "🔥".repeat(129);
        let err = parse_response(500, body.as_bytes()).unwrap_err();
        match err {
            TranscriptionError::ServerError { body: b, .. } => {
                assert!(b.ends_with("..."));
                // Truncated portion must be valid UTF-8 (no panic, no replacement chars)
                let without_dots = &b[..b.len() - 3];
                assert!(without_dots.chars().all(|c| c == '🔥'));
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_200_invalid_json() {
        let err = parse_response(200, b"not json").unwrap_err();
        assert!(matches!(err, TranscriptionError::InvalidResponse(_)));
    }

    #[test]
    fn parse_response_200_empty_body() {
        let err = parse_response(200, b"").unwrap_err();
        assert!(matches!(err, TranscriptionError::InvalidResponse(_)));
    }

    // -- property tests --

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        prop_compose! {
            fn arb_audio_bytes()(bytes in prop::collection::vec(any::<u8>(), 1..256)) -> Vec<u8> {
                bytes
            }
        }

        prop_compose! {
            fn arb_model()(model in "[a-z][a-z0-9_-]{0,20}") -> String {
                model
            }
        }

        prop_compose! {
            fn arb_extension()(ext in prop::sample::select(vec![
                "wav", "webm", "mp4", "m4a", "ogg", "oga", "flac", "mp3", "xyz",
            ])) -> &'static str {
                ext
            }
        }

        proptest! {
            #[test]
            fn valid_payload_always_parses(
                audio in arb_audio_bytes(),
                model in arb_model(),
                ext in arb_extension(),
            ) {
                let file_name = format!("recording.{ext}");
                let payload = json!({
                    "audio_base64": BASE64.encode(&audio),
                    "model": model,
                    "file_name": file_name,
                });
                let req = parse_request(&payload).unwrap();
                prop_assert_eq!(&req.audio_bytes, &audio);
                prop_assert_eq!(&req.model, &model);
                prop_assert_eq!(&req.file_name, &file_name);
            }

            #[test]
            fn audio_bytes_round_trip(audio in arb_audio_bytes()) {
                let payload = json!({
                    "audio_base64": BASE64.encode(&audio),
                    "model": "test",
                });
                let req = parse_request(&payload).unwrap();
                prop_assert_eq!(req.audio_bytes, audio);
            }

            #[test]
            fn mime_type_consistent_with_extension(ext in arb_extension()) {
                let file_name = format!("file.{ext}");
                let expected = mime_type_for_filename(&file_name);
                let payload = json!({
                    "audio_base64": BASE64.encode(b"audio"),
                    "model": "test",
                    "file_name": file_name,
                });
                let req = parse_request(&payload).unwrap();
                prop_assert_eq!(req.mime_type.as_str(), expected);
            }

            #[test]
            fn non_200_always_produces_server_error(
                status in (100u16..200).prop_union(201u16..600),
                body in prop::collection::vec(any::<u8>(), 0..1024),
            ) {
                let err = parse_response(status, &body).unwrap_err();
                match err {
                    TranscriptionError::ServerError { status: s, .. } => {
                        prop_assert_eq!(s, status);
                    }
                    other => prop_assert!(false, "expected ServerError, got {:?}", other),
                }
            }

            #[test]
            fn valid_json_200_always_parses(value in prop::sample::select(vec![
                json!({"text": "hello"}),
                json!({"text": ""}),
                json!({"text": "hello", "extra": 42}),
                json!({}),
                json!(null),
                json!("string"),
                json!(42),
            ])) {
                let body = serde_json::to_vec(&value).unwrap();
                let result = parse_response(200, &body).unwrap();
                prop_assert_eq!(result, value);
            }
        }
    }
}
