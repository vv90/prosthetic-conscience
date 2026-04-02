use std::fs;
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use serde::Serialize;

use prosthetic_conscience::consensus_support::fixtures::{FixtureScenario, TrialLog, scenario_log};

#[derive(Parser)]
#[command(
    name = "pc-consensus-sim",
    about = "Generate deterministic consensus deliberation logs for offline LLM experiments"
)]
struct Args {
    /// Which built-in scenario to generate.
    #[arg(long, value_enum, default_value_t = FixtureScenario::AuthenticationDeliberation)]
    scenario: FixtureScenario,

    /// Output shape for the generated log.
    #[arg(long, value_enum, default_value_t = OutputFormat::SessionResponse)]
    format: OutputFormat,

    /// Write the generated output to this path instead of stdout.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Emit compact JSON instead of pretty-printed JSON.
    #[arg(long)]
    compact: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Entries,
    SessionResponse,
    Bundle,
    Jsonl,
}

#[derive(Serialize)]
struct SessionEntriesResponse<'a> {
    entries: &'a [consensus::types::Entry],
    total: usize,
}

#[derive(Serialize)]
struct ExperimentBundle<'a> {
    scenario_id: &'a str,
    title: &'a str,
    description: &'a str,
    participants: &'a [String],
    entries: &'a [consensus::types::Entry],
    total: usize,
    final_overview: consensus::render::OverviewData,
    final_overview_text: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let log = scenario_log(args.scenario);

    let output = render_output(&log, args.format, args.compact)?;

    if let Some(path) = args.output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, output)?;
    } else {
        print!("{output}");
    }

    Ok(())
}

fn render_output(
    log: &TrialLog,
    format: OutputFormat,
    compact: bool,
) -> Result<String, serde_json::Error> {
    match format {
        OutputFormat::Entries => render_json(&log.entries, compact),
        OutputFormat::SessionResponse => render_json(
            &SessionEntriesResponse {
                entries: &log.entries,
                total: log.entries.len(),
            },
            compact,
        ),
        OutputFormat::Bundle => render_json(
            &ExperimentBundle {
                scenario_id: &log.scenario_id,
                title: &log.title,
                description: &log.description,
                participants: &log.participants,
                entries: &log.entries,
                total: log.entries.len(),
                final_overview: log.final_overview(),
                final_overview_text: log.final_overview_text(),
            },
            compact,
        ),
        OutputFormat::Jsonl => render_jsonl(&log.entries),
    }
}

fn render_json<T: Serialize>(value: &T, compact: bool) -> Result<String, serde_json::Error> {
    if compact {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
}

fn render_jsonl(entries: &[consensus::types::Entry]) -> Result<String, serde_json::Error> {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&serde_json::to_string(entry)?);
        out.push('\n');
    }
    Ok(out)
}
