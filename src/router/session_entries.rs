use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::gateway::kernel::SessionId;
use crate::router::response::error_response;
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

#[derive(Serialize)]
struct SessionEntriesResponse {
    entries: Vec<Value>,
    total: usize,
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
        Ok(Some(query)) => axum::Json(SessionEntriesResponse {
            entries: query.entries,
            total: query.total,
        })
        .into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(_) => {
            error_response(StatusCode::SERVICE_UNAVAILABLE, "gateway unavailable").into_response()
        }
    }
}
