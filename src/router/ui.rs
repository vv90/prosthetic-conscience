use axum::response::Html;

const TRANSCRIBE_HTML: &str = include_str!("../../static/transcribe.html");

pub(crate) async fn transcribe_ui() -> Html<&'static str> {
    Html(TRANSCRIBE_HTML)
}
