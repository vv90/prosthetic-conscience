use axum::response::Html;

const TRANSCRIBE_HTML: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../static/transcribe.html"));

pub(crate) async fn transcribe_ui() -> Html<&'static str> {
    Html(TRANSCRIBE_HTML)
}
