use std::io::{self, BufRead, Write};

use clap::Parser;
use serde_json::json;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use prosthetic_conscience::client::gateway_client::GatewayClient;
use prosthetic_conscience::client::tool_loop;
use prosthetic_conscience::client::tools::ToolRegistry;
use prosthetic_conscience::client::tools::current_time::GetCurrentTime;
use prosthetic_conscience::client::tools::shell::ShellTool;

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

    /// Maximum tool call rounds per user message.
    #[arg(long, default_value = "10")]
    max_rounds: usize,

    /// Docker container name for shell tool. If omitted, shell tool is not registered.
    #[arg(long)]
    container: Option<String>,

    /// Timeout in seconds for shell commands.
    #[arg(long, default_value = "30")]
    shell_timeout: u64,

    /// Maximum output bytes per shell command.
    #[arg(long, default_value = "51200")]
    max_output: usize,
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

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(GetCurrentTime));

    if let Some(container) = &args.container {
        let timeout = std::time::Duration::from_secs(args.shell_timeout);
        registry.register(Box::new(ShellTool::new(
            container.clone(),
            timeout,
            args.max_output,
        )));
        info!(container = %container, "shell tool registered");
    }

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

        match tool_loop::run(
            &client,
            &registry,
            &mut messages,
            &args.model,
            args.max_rounds,
        )
        .await
        {
            Ok(msg) => {
                if let Some(content) = &msg.content {
                    println!("{content}");
                }
                // The tool loop already appends assistant messages to history.
            }
            Err(e) => {
                error!(error = %e, "tool loop failed");
                // Remove the failed user message so conversation stays consistent.
                messages.pop();
            }
        }
    }
}
