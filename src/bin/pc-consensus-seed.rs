use std::path::PathBuf;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use prosthetic_conscience::consensus::fixtures::{FixtureScenario, scenario_log};
use prosthetic_conscience::consensus_cli::seed::{join_and_seed_session, load_entries_from_path};

#[derive(Parser)]
#[command(
    name = "pc-consensus-seed",
    about = "Create a consensus session and seed it with a fixture log"
)]
struct Args {
    /// Gateway base URL.
    #[arg(long, default_value = "http://127.0.0.1:3000")]
    gateway_url: String,

    /// Auth token for gateway requests.
    #[arg(long)]
    auth_token: Option<String>,

    /// Built-in fixture scenario to seed when --input is not provided.
    #[arg(long, value_enum, conflicts_with = "input")]
    scenario: Option<FixtureScenario>,

    /// JSON fixture file to seed. Accepts session-response, bundle, or raw entry array shapes.
    #[arg(long, conflicts_with = "scenario")]
    input: Option<PathBuf>,

    /// Existing session id to seed.
    session_id: String,
}

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let args = Args::parse();
    let default_scenario = FixtureScenario::AuthenticationDeliberation;

    let (entries, source_description) = match &args.input {
        Some(path) => match load_entries_from_path(path) {
            Ok(entries) => (entries, format!("file {}", path.display())),
            Err(error) => {
                eprintln!("pc-consensus-seed failed: {error}");
                std::process::exit(1);
            }
        },
        None => {
            let scenario = args.scenario.unwrap_or(default_scenario);
            let log = scenario_log(scenario);
            (log.entries, format!("scenario {}", scenario.slug()))
        }
    };

    let result = match join_and_seed_session(
        args.gateway_url.clone(),
        args.auth_token.clone(),
        args.session_id.clone(),
        &entries,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            eprintln!("pc-consensus-seed failed: {error}");
            std::process::exit(1);
        }
    };

    let base_url = args.gateway_url.trim_end_matches('/');
    let entries_url = format!(
        "{base_url}/v1/sessions/{}/entries?after=0&limit={}",
        result.session_id, result.total_entries
    );

    println!("Session ID: {}", result.session_id);
    println!("Seeded entries: {}", result.total_entries);
    println!("Source: {source_description}");
    println!("Entries URL: {entries_url}");
}
