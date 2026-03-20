use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::gateway::kernel::SessionId;
use crate::router::state::AppState;

#[derive(Deserialize)]
pub(crate) struct Params {
    #[serde(default)]
    after: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

const MAX_LIMIT: usize = 1000;

pub(crate) async fn get_entries(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<Params>,
) -> impl IntoResponse {
    let limit = params.limit.min(MAX_LIMIT);

    let result = state
        .runtime
        .query_session_entries(SessionId(session_id), params.after, limit)
        .await;

    match result {
        Ok(Some(query)) => axum::Json(json!({
            "entries": query.entries,
            "total": query.total,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": {"message": "session not found"}})),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"error": {"message": "gateway unavailable"}})),
        )
            .into_response(),
    }
}
