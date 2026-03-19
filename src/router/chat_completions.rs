use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::gateway::relay::StreamFrame;
use crate::protocol::Capability;
use crate::protocol::ChatRequest;
use crate::router::state::AppState;

pub(crate) async fn chat_completions(
    State(state): State<AppState>,
    axum::Json(request): axum::Json<ChatRequest>,
) -> impl IntoResponse {
    let stream = request.stream.unwrap_or(false);
    if !stream {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": {"message": "stream=true is required"}})),
        )
            .into_response();
    }

    let (tx, rx) = mpsc::channel::<StreamFrame>(32);

    let stream_id = match state.runtime.register_stream(tx).await {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({"error": {"message": "gateway unavailable"}})),
            )
                .into_response();
        }
    };

    if state
        .runtime
        .http_chat_requested(stream_id, request.payload, true, Capability::Chat)
        .await
        .is_err()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"error": {"message": "gateway unavailable"}})),
        )
            .into_response();
    }

    let event_stream = ReceiverStream::new(rx).map(|frame| {
        Ok::<_, std::convert::Infallible>(match frame {
            StreamFrame::Chunk { data } => Event::default().data(data.to_string()),
            StreamFrame::Done => Event::default().data("[DONE]"),
            StreamFrame::Error { message } => {
                Event::default().data(json!({"error": {"message": message}}).to_string())
            }
        })
    });

    Sse::new(event_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
