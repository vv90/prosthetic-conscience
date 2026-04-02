use std::collections::BTreeSet;

use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use prosthetic_conscience::protocol::Capability;
use prosthetic_conscience::worker::client::WorkerClient;
use prosthetic_conscience::worker::inference::InferenceClient;
use prosthetic_conscience::worker::whisper::WhisperClient;

#[derive(Parser)]
#[command(name = "pc-worker", about = "Prosthetic Conscience worker agent")]
struct Args {
    /// Gateway WebSocket URL.
    #[arg(long, default_value = "ws://127.0.0.1:3000/ws/worker")]
    gateway_url: String,

    /// Inference server base URL (llama-server or any OpenAI-compatible endpoint).
    /// Enables the `chat` capability.
    #[arg(long)]
    inference_url: Option<String>,

    /// Whisper-compatible transcription server base URL.
    /// Enables the `transcription` capability.
    #[arg(long)]
    whisper_url: Option<String>,

    /// Auth token for gateway connection.
    #[arg(long)]
    auth_token: Option<String>,

    /// API key (Bearer token) for authenticated inference backends (e.g. SambaNova).
    #[arg(long)]
    inference_api_key: Option<String>,

    /// Model name to send to the inference backend, overriding whatever the
    /// client requested.  When set, the real model name is also scrubbed from
    /// response chunks (replaced with "mystery_model").
    #[arg(long)]
    inference_model: Option<String>,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();

    if args.inference_url.is_none() && args.whisper_url.is_none() {
        eprintln!("error: at least one of --inference-url or --whisper-url must be provided");
        std::process::exit(1);
    }

    let mut capabilities = BTreeSet::new();
    if args.inference_url.is_some() {
        capabilities.insert(Capability::Chat);
    }
    if args.whisper_url.is_some() {
        capabilities.insert(Capability::Transcription);
    }

    let inference_url = args
        .inference_url
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    info!(
        gateway_url = %args.gateway_url,
        inference_url = %inference_url,
        whisper_url = args.whisper_url.as_deref().unwrap_or("none"),
        inference_api_key_set = args.inference_api_key.is_some(),
        inference_model = args.inference_model.as_deref().unwrap_or("(passthrough)"),
        ?capabilities,
        "starting pc-worker"
    );

    let inference =
        InferenceClient::new(inference_url, args.inference_api_key, args.inference_model);
    let whisper = args.whisper_url.map(WhisperClient::new);
    let client = WorkerClient::new(
        args.gateway_url,
        inference,
        whisper,
        args.auth_token,
        capabilities,
    );

    client.run().await;
}
