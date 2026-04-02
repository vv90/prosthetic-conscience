use axum::http::StatusCode;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct ErrorDetail {
    pub message: String,
}

#[derive(Serialize)]
pub(crate) struct ErrorResponse {
    pub error: ErrorDetail,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                message: message.into(),
            },
        }
    }
}

pub(crate) fn error_response(
    status: StatusCode,
    message: impl Into<String>,
) -> (StatusCode, axum::Json<ErrorResponse>) {
    (status, axum::Json(ErrorResponse::new(message)))
}
