use prosthetic_conscience::router::{AppState, router};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let state = AppState::new();
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:3000").await?;
    info!("listening on ws://127.0.0.1:3000/ws/worker and POST /v1/chat/completions");

    axum::serve(listener, app).await?;
    Ok(())
}
