use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use prosthetic_conscience::consensus_cli::app::{AppConfig, ConsensusApp};

#[derive(Parser)]
#[command(name = "pc-consensus", about = "Consensus protocol terminal client")]
struct Args {
    /// Gateway base URL.
    #[arg(long, default_value = "http://127.0.0.1:3000")]
    gateway_url: String,

    /// Auth token for gateway requests.
    #[arg(long)]
    auth_token: Option<String>,

    /// Model name to include in requests.
    #[arg(long, default_value = "default")]
    model: String,

    /// Participant name used in drafted entries.
    #[arg(long)]
    participant: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new consensus session.
    Create,
    /// Join an existing consensus session.
    Join { session_id: String },
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();
    let config = AppConfig {
        gateway_url: args.gateway_url,
        auth_token: args.auth_token,
        model: args.model,
        participant: args.participant,
    };

    let app = match args.command {
        Command::Create => ConsensusApp::create(config).await,
        Command::Join { session_id } => ConsensusApp::join(config, session_id).await,
    };

    let mut app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("pc-consensus failed: {error}");
            std::process::exit(1);
        }
    };

    if let Err(error) = app.run().await {
        eprintln!("pc-consensus failed: {error}");
        std::process::exit(1);
    }
}
