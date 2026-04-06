use axum::body::Bytes;
use axum::http::header::CONTENT_TYPE;
use axum::response::{Html, IntoResponse};

const TRANSCRIBE_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../static/transcribe.html"
));
const CONSENSUS_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../static/consensus_ui/index.html"
));
const CONSENSUS_WASM_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../static/consensus_ui/consensus_wasm.js"
));
const CONSENSUS_WASM_BG_WASM: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../static/consensus_ui/consensus_wasm_bg.wasm"
));

pub(crate) async fn transcribe_ui() -> Html<&'static str> {
    Html(TRANSCRIBE_HTML)
}

pub(crate) async fn consensus_ui() -> Html<&'static str> {
    Html(CONSENSUS_HTML)
}

pub(crate) async fn consensus_wasm_js() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/javascript; charset=utf-8")],
        CONSENSUS_WASM_JS,
    )
}

pub(crate) async fn consensus_wasm_bg_wasm() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "application/wasm")],
        Bytes::from_static(CONSENSUS_WASM_BG_WASM),
    )
}
