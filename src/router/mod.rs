mod auth;
mod chat_completions;
pub mod state;
mod worker_ws_upgrade;

use axum::Json;
use axum::Router;
use axum::routing::{get, post};
use serde_json::{Value, json};

use self::auth::require_auth;
use self::chat_completions::chat_completions;
use self::worker_ws_upgrade::worker_ws_upgrade;

pub use state::AppState;

async fn list_models() -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [
            {
                "id": "mystery_model",
                "object": "model",
                "created": 0,
                "owned_by": "prosthetic-conscience"
            }
        ]
    }))
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws/worker", get(worker_ws_upgrade))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ))
        .with_state(state)
}
