use std::fs;
use std::path::PathBuf;

use clap::Parser;

use prosthetic_conscience::consensus::eval::{
    ExperimentRunConfig, load_suite_from_path, render_markdown_summary, run_suite,
};

#[derive(Parser)]
#[command(
    name = "pc-consensus-eval",
    about = "Evaluate consensus tool-calling reliability for a named test run against a known worker/model"
)]
struct Args {
    /// Gateway base URL.
    #[arg(long, default_value = "http://127.0.0.1:3000")]
    gateway_url: String,

    /// Auth token for gateway requests.
    #[arg(long)]
    auth_token: Option<String>,

    /// JSON suite definition describing checkpoints, prompts, and expected tool outcomes.
    #[arg(
        long,
        default_value = "fixtures/tool-call-eval/authentication-tool-reliability.json"
    )]
    suite: PathBuf,

    /// Human-readable label for this test run.
    #[arg(long, default_value = "tool-call-eval")]
    run_name: String,

    /// Number of repeats per case/history/max_history cell.
    #[arg(long, default_value_t = 5)]
    repeats: usize,

    /// Synthetic prior context turns to seed before the measured user turn.
    #[arg(long)]
    history_turns: Vec<usize>,

    /// `pc-consensus` max_history values to test. Repeat to compare truncation budgets.
    #[arg(long)]
    max_history: Vec<usize>,

    /// Write the full JSON report to this path.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Write the markdown summary table to this path.
    #[arg(long)]
    markdown_output: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let suite = load_suite_from_path(&args.suite)?;
    let report = run_suite(
        &suite,
        &ExperimentRunConfig {
            run_name: args.run_name,
            gateway_url: args.gateway_url,
            auth_token: args.auth_token,
            repeats: args.repeats,
            history_turns: args.history_turns,
            max_history_values: args.max_history,
        },
    )
    .await?;

    let markdown = render_markdown_summary(&report);
    println!("{markdown}");

    if let Some(path) = args.output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }

    if let Some(path) = args.markdown_output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, markdown)?;
    }

    Ok(())
}
