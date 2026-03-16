use std::io::{self, BufRead, Write};

use clap::Parser;
use serde_json::json;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use prosthetic_conscience::client::gateway_client::GatewayClient;
use prosthetic_conscience::client::response_assembler;

#[derive(Parser)]
#[command(name = "pc-client", about = "Prosthetic Conscience client")]
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

    /// Optional system prompt.
    #[arg(long)]
    system: Option<String>,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();

    info!(
        gateway_url = %args.gateway_url,
        model = %args.model,
        "starting pc-client"
    );

    let client = GatewayClient::new(args.gateway_url, args.auth_token);
    let mut messages: Vec<serde_json::Value> = Vec::new();

    if let Some(system) = &args.system {
        messages.push(json!({"role": "system", "content": system}));
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("> ");
        if stdout.flush().is_err() {
            break;
        }

        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                error!(error = %e, "failed to read stdin");
                break;
            }
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        messages.push(json!({"role": "user", "content": line}));

        let payload = json!({
            "model": args.model,
            "messages": messages,
        });

        match client.chat(payload).await {
            Ok(chunks) => {
                match response_assembler::assemble(&chunks) {
                    Ok(msg) => {
                        if let Some(content) = &msg.content {
                            println!("{content}");
                        }
                        // Append assistant message to conversation history.
                        messages.push(json!({"role": "assistant", "content": msg.content.unwrap_or_default()}));
                    }
                    Err(e) => {
                        error!(error = %e, "failed to assemble response");
                        // Remove the failed user message so conversation stays consistent.
                        messages.pop();
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "request failed");
                messages.pop();
            }
        }
    }
}
