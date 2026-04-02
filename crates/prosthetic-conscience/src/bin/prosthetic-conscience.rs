use clap::Parser;
use prosthetic_conscience::router::{AppState, router};
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "prosthetic-conscience",
    about = "Prosthetic Conscience gateway"
)]
struct Args {
    /// Host address to bind to.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 3000)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();
    let bind_addr = format!("{}:{}", args.host, args.port);

    let auth_token = std::env::var("PC_AUTH_TOKEN").ok();
    if auth_token.is_some() {
        info!("auth enabled (PC_AUTH_TOKEN set)");
    } else {
        warn!("auth disabled (PC_AUTH_TOKEN not set)");
    }

    let state = AppState::new().with_auth_token(auth_token);
    let app = router(state);

    let listener = TcpListener::bind(&bind_addr).await?;
    info!(addr = %bind_addr, "gateway listening");

    axum::serve(listener, app).await?;
    Ok(())
}
