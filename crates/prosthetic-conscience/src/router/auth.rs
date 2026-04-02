use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;

use crate::router::response::error_response;
use crate::router::state::AppState;

pub(crate) async fn require_auth(
    State(state): State<AppState>,
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> impl IntoResponse {
    let expected = match &state.auth_token {
        Some(token) => token,
        None => return next.run(request).await.into_response(),
    };

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.strip_prefix("Bearer ").is_some_and(|t| t == expected) => {
            next.run(request).await.into_response()
        }
        _ => error_response(StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
    }
}
