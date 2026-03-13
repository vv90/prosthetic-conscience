mod auth;
mod chat_completions;
pub mod state;
mod worker_ws_upgrade;

use axum::Router;
use axum::routing::{get, post};

use self::auth::require_auth;
use self::chat_completions::chat_completions;
use self::worker_ws_upgrade::worker_ws_upgrade;

pub use state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/ws/worker", get(worker_ws_upgrade))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ))
        .with_state(state)
}
