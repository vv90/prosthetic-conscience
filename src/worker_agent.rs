use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use prosthetic_conscience::worker::client::WorkerClient;
use prosthetic_conscience::worker::inference::InferenceClient;

#[derive(Parser)]
#[command(name = "pc-worker", about = "Prosthetic Conscience worker agent")]
struct Args {
    /// Gateway WebSocket URL.
    #[arg(long, default_value = "ws://127.0.0.1:3000/ws/worker")]
    gateway_url: String,

    /// Inference server base URL (llama-server or any OpenAI-compatible endpoint).
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    inference_url: String,

    /// Auth token for gateway connection.
    #[arg(long)]
    auth_token: Option<String>,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();

    info!(
        gateway_url = %args.gateway_url,
        inference_url = %args.inference_url,
        "starting pc-worker"
    );

    let inference = InferenceClient::new(args.inference_url);
    let client = WorkerClient::new(args.gateway_url, inference, args.auth_token);

    client.run().await;
}
