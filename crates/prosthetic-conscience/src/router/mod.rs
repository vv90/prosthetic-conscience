mod audio_transcriptions;
mod auth;
mod chat_completions;
mod response;
mod session_entries;
mod session_ws;
pub mod state;
mod ui;
mod worker_ws_upgrade;

use axum::Json;
use axum::Router;
use axum::routing::{get, post};
use serde::Serialize;

use self::audio_transcriptions::audio_transcriptions;
use self::auth::require_auth;
use self::chat_completions::chat_completions;
use self::session_entries::get_entries;
use self::session_ws::session_ws_upgrade;
use self::ui::{consensus_ui, consensus_wasm_bg_wasm, consensus_wasm_js, transcribe_ui};
use self::worker_ws_upgrade::worker_ws_upgrade;

pub use state::AppState;

#[derive(Serialize)]
struct Model {
    id: &'static str,
    object: &'static str,
    created: u64,
    owned_by: &'static str,
}

#[derive(Serialize)]
struct ModelList {
    object: &'static str,
    data: Vec<Model>,
}

async fn list_models() -> Json<ModelList> {
    Json(ModelList {
        object: "list",
        data: vec![Model {
            id: "mystery_model",
            object: "model",
            created: 0,
            owned_by: "prosthetic-conscience",
        }],
    })
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(transcribe_ui))
        .route("/consensus", get(consensus_ui))
        .route(
            "/consensus-assets/consensus_wasm.js",
            get(consensus_wasm_js),
        )
        .route(
            "/consensus-assets/consensus_wasm_bg.wasm",
            get(consensus_wasm_bg_wasm),
        )
        .route("/ws/worker", get(worker_ws_upgrade))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/audio/transcriptions", post(audio_transcriptions))
        .route("/v1/models", get(list_models))
        .route("/v1/sessions", get(session_ws_upgrade))
        .route("/v1/sessions/{session_id}/entries", get(get_entries))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ))
        .with_state(state)
}
