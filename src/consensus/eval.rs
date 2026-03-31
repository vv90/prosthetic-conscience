use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::chat_gateway::response_assembler::{
    CompletedMessage, CompletedToolCall, assistant_message_value, tool_result_message,
};
use crate::consensus::engine::{ClaimRef, ConsensusEngine, DraftContent, DraftEntry};
use crate::consensus::fixtures::{FixtureScenario, scenario_log};
use crate::consensus::render::OverviewData;
use crate::consensus::types::{ClaimKind, Entry, Outcome, Position, RelationKind};
use crate::consensus_cli::llm::{ConsensusLlm, LlmTurnTrace};

fn default_history_turns() -> Vec<usize> {
    vec![0, 4, 12]
}

fn default_max_history_values() -> Vec<usize> {
    vec![12, 100]
}

fn default_request_model() -> String {
    String::from("default")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallExperimentSuite {
    pub suite_id: String,
    pub description: String,
    pub scenario_id: String,
    #[serde(default = "default_request_model")]
    pub request_model: String,
    #[serde(default = "default_history_turns")]
    pub history_turns: Vec<usize>,
    #[serde(default = "default_max_history_values")]
    pub max_history_values: Vec<usize>,
    pub cases: Vec<ToolCallCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallCase {
    pub id: String,
    pub description: String,
    pub checkpoint_entries: usize,
    pub participant: String,
    pub user_message: String,
    pub expected: Vec<ExpectedToolUse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ExpectedToolUse {
    DraftClaim {
        author: Option<String>,
        kind: Option<ClaimKind>,
        parent_id: Option<String>,
        #[serde(default)]
        body_contains: Vec<String>,
    },
    DraftRelation {
        author: Option<String>,
        source_id: String,
        target_id: String,
        kind: RelationKind,
    },
    DraftStance {
        author: Option<String>,
        target_id: String,
        position: Position,
    },
    DraftResolve {
        author: Option<String>,
        claim_id: String,
        outcome: Outcome,
    },
    DraftComment {
        author: Option<String>,
        claim_id: Option<String>,
        #[serde(default)]
        body_contains: Vec<String>,
    },
    PlainTextResponse {
        #[serde(default)]
        text_contains: Vec<String>,
    },
    AnyStructuredTool,
    AnyTool {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ExperimentRunConfig {
    pub run_name: String,
    pub gateway_url: String,
    pub auth_token: Option<String>,
    pub repeats: usize,
    pub history_turns: Vec<usize>,
    pub max_history_values: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExperimentReport {
    pub run_name: String,
    pub suite_id: String,
    pub description: String,
    pub scenario_id: String,
    pub request_model: String,
    pub repeats: usize,
    pub history_turns: Vec<usize>,
    pub max_history_values: Vec<usize>,
    pub runs: Vec<ExperimentRun>,
    pub aggregates: Vec<ExperimentAggregate>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExperimentRun {
    pub case_id: String,
    pub case_description: String,
    pub participant: String,
    pub checkpoint_entries: usize,
    pub history_turns: usize,
    pub max_history: usize,
    pub repeat_index: usize,
    pub user_message: String,
    pub duration_ms: u128,
    pub trace: LlmTurnTrace,
    pub error: Option<String>,
    pub final_drafts: Vec<DraftEntry>,
    pub evaluation: RunEvaluation,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunEvaluation {
    pub tool_call_made: bool,
    pub structured_tool_call_made: bool,
    pub expected_tool_match: bool,
    pub expected_argument_match: bool,
    pub expected_outcome_match: bool,
    pub turn_success: bool,
    pub matched_variant_index: Option<usize>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExperimentAggregate {
    pub case_id: String,
    pub history_turns: usize,
    pub max_history: usize,
    pub repeats: usize,
    pub tool_call_made: MetricSummary,
    pub structured_tool_call_made: MetricSummary,
    pub expected_tool_match: MetricSummary,
    pub expected_argument_match: MetricSummary,
    pub expected_outcome_match: MetricSummary,
    pub turn_success: MetricSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricSummary {
    pub success: usize,
    pub total: usize,
    pub rate: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum ExperimentError {
    #[error("failed to read experiment suite {path}: {source}")]
    ReadSuite {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid experiment suite JSON: {0}")]
    ParseSuite(String),
    #[error("unknown fixture scenario in suite: {0}")]
    UnknownScenario(String),
    #[error(
        "case {case_id} requests checkpoint {checkpoint_entries}, but scenario only has {available} entries"
    )]
    CheckpointOutOfBounds {
        case_id: String,
        checkpoint_entries: usize,
        available: usize,
    },
}

pub fn load_suite_from_path(path: &Path) -> Result<ToolCallExperimentSuite, ExperimentError> {
    let text = fs::read_to_string(path).map_err(|source| ExperimentError::ReadSuite {
        path: path.display().to_string(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|error| ExperimentError::ParseSuite(error.to_string()))
}

pub async fn run_suite(
    suite: &ToolCallExperimentSuite,
    config: &ExperimentRunConfig,
) -> Result<ExperimentReport, ExperimentError> {
    let scenario = FixtureScenario::from_str(&suite.scenario_id)
        .map_err(|_| ExperimentError::UnknownScenario(suite.scenario_id.clone()))?;
    let scenario_log = scenario_log(scenario);

    let history_turns = if config.history_turns.is_empty() {
        suite.history_turns.clone()
    } else {
        config.history_turns.clone()
    };
    let max_history_values = if config.max_history_values.is_empty() {
        suite.max_history_values.clone()
    } else {
        config.max_history_values.clone()
    };

    let mut runs = Vec::new();

    for case in &suite.cases {
        if case.checkpoint_entries > scenario_log.entries.len() {
            return Err(ExperimentError::CheckpointOutOfBounds {
                case_id: case.id.clone(),
                checkpoint_entries: case.checkpoint_entries,
                available: scenario_log.entries.len(),
            });
        }

        for &history_turn_count in &history_turns {
            for &max_history in &max_history_values {
                for repeat_index in 0..config.repeats {
                    let mut engine = replay_checkpoint(
                        &scenario_log.entries,
                        case.checkpoint_entries,
                        &case.participant,
                    );
                    let mut history = build_synthetic_history(&engine, history_turn_count);
                    history.push(json!({"role": "user", "content": case.user_message}));

                    let llm = ConsensusLlm::new(
                        config.gateway_url.clone(),
                        config.auth_token.clone(),
                        suite.request_model.clone(),
                        case.participant.clone(),
                        max_history,
                    );

                    let started = Instant::now();
                    let (trace, error) =
                        match llm.run_turn_with_trace(&mut engine, &mut history).await {
                            Ok(trace) => (trace, None),
                            Err(error) => (error.trace, Some(error.error.to_string())),
                        };
                    let duration_ms = started.elapsed().as_millis();
                    let final_drafts = engine.show_drafts().to_vec();
                    let evaluation = evaluate_case(case, &trace, &final_drafts, error.as_deref());

                    runs.push(ExperimentRun {
                        case_id: case.id.clone(),
                        case_description: case.description.clone(),
                        participant: case.participant.clone(),
                        checkpoint_entries: case.checkpoint_entries,
                        history_turns: history_turn_count,
                        max_history,
                        repeat_index,
                        user_message: case.user_message.clone(),
                        duration_ms,
                        trace,
                        error,
                        final_drafts,
                        evaluation,
                    });
                }
            }
        }
    }

    let aggregates = aggregate_runs(&runs);
    Ok(ExperimentReport {
        run_name: config.run_name.clone(),
        suite_id: suite.suite_id.clone(),
        description: suite.description.clone(),
        scenario_id: suite.scenario_id.clone(),
        request_model: suite.request_model.clone(),
        repeats: config.repeats,
        history_turns,
        max_history_values,
        runs,
        aggregates,
    })
}

pub fn render_markdown_summary(report: &ExperimentReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Tool-Calling Eval Summary\n\nRun: `{}`\n\nSuite: `{}`\n\nRequest model: `{}`\n\n",
        report.run_name, report.suite_id, report.request_model
    ));
    out.push_str(
        "| Case | History Turns | Max History | Tool Call | Expected Tool | Arg Match | Outcome Match | Success |\n",
    );
    out.push_str("| --- | ---: | ---: | --- | --- | --- | --- | --- |\n");

    for aggregate in &report.aggregates {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            aggregate.case_id,
            aggregate.history_turns,
            aggregate.max_history,
            format_metric(&aggregate.tool_call_made),
            format_metric(&aggregate.expected_tool_match),
            format_metric(&aggregate.expected_argument_match),
            format_metric(&aggregate.expected_outcome_match),
            format_metric(&aggregate.turn_success),
        ));
    }

    out
}

fn format_metric(metric: &MetricSummary) -> String {
    format!(
        "{}/{} ({:.1}%)",
        metric.success,
        metric.total,
        metric.rate * 100.0
    )
}

fn replay_checkpoint(
    entries: &[Entry],
    checkpoint_entries: usize,
    draft_author: &str,
) -> ConsensusEngine {
    let mut engine = ConsensusEngine::new(draft_author.to_owned());
    for entry in entries.iter().take(checkpoint_entries).cloned() {
        engine.append(entry);
    }
    engine
}

fn build_synthetic_history(engine: &ConsensusEngine, turns: usize) -> Vec<Value> {
    if turns == 0 {
        return Vec::new();
    }

    let overview = engine.overview();
    let tool_content =
        serde_json::to_string_pretty(&overview).unwrap_or_else(|_| String::from("{}"));
    let summary = synthetic_overview_summary(&overview);

    let mut history = Vec::with_capacity(turns * 4);
    for turn in 0..turns {
        history.push(json!({
            "role": "user",
            "content": format!(
                "Before we continue, give me another quick recap of the deliberation state. Context seed {}.",
                turn + 1
            ),
        }));

        let call_id = format!("seed_overview_{}", turn + 1);
        history.push(assistant_message_value(&CompletedMessage {
            role: String::from("assistant"),
            content: None,
            tool_calls: vec![CompletedToolCall {
                id: call_id.clone(),
                call_type: String::from("function"),
                function_name: String::from("overview"),
                arguments_json: String::from("{}"),
            }],
            finish_reason: Some(String::from("tool_calls")),
        }));
        history.push(tool_result_message(&call_id, &tool_content));
        history.push(json!({
            "role": "assistant",
            "content": summary,
        }));
    }

    history
}

fn synthetic_overview_summary(overview: &OverviewData) -> String {
    let lead_item = overview
        .items
        .first()
        .map(|item| item.body.as_str())
        .unwrap_or("no active item yet");
    format!(
        "There are {} claims, {} relations, and {} stances. Current focus: {}.",
        overview.total_claims, overview.total_relations, overview.total_stances, lead_item
    )
}

fn evaluate_case(
    case: &ToolCallCase,
    trace: &LlmTurnTrace,
    final_drafts: &[DraftEntry],
    error: Option<&str>,
) -> RunEvaluation {
    let tool_results: Vec<_> = trace
        .rounds
        .iter()
        .flat_map(|round| round.tool_results.iter())
        .collect();

    let tool_call_made = trace.rounds.iter().any(|round| {
        round
            .assistant_message
            .as_ref()
            .is_some_and(|message| !message.tool_calls.is_empty())
    });
    let structured_tool_call_made = !tool_results.is_empty();

    let mut expected_tool_match = false;
    let mut expected_argument_match = false;
    let mut expected_outcome_match = false;
    let mut matched_variant_index = None;

    let final_content = trace
        .final_message
        .as_ref()
        .and_then(|msg| msg.content.as_deref())
        .unwrap_or("");

    for (index, expected) in case.expected.iter().enumerate() {
        // PlainTextResponse is special: it matches when the model replied
        // with plain text (no tool calls) and the content passes filters.
        if let ExpectedToolUse::PlainTextResponse { text_contains } = expected {
            let text_match = !tool_call_made
                && (text_contains.is_empty()
                    || text_contains
                        .iter()
                        .all(|needle| final_content.contains(needle.as_str())));
            if text_match && final_drafts.is_empty() {
                expected_tool_match = true;
                expected_argument_match = true;
                expected_outcome_match = true;
                matched_variant_index = Some(index);
                break;
            }
            continue;
        }

        let tool_name_match = tool_results
            .iter()
            .any(|execution| matches_expected_tool_name(expected, &execution.function_name));
        expected_tool_match |= tool_name_match;

        let argument_match = tool_results
            .iter()
            .any(|execution| matches_expected_tool_call(expected, execution));
        expected_argument_match |= argument_match;

        let outcome_match = match expected {
            ExpectedToolUse::PlainTextResponse { .. } => unreachable!(),
            ExpectedToolUse::AnyStructuredTool => !final_drafts.is_empty(),
            ExpectedToolUse::AnyTool { .. } => argument_match,
            _ => final_drafts
                .iter()
                .any(|draft| matches_expected_draft(expected, draft)),
        };

        if outcome_match {
            expected_outcome_match = true;
            matched_variant_index = Some(index);
            break;
        }
    }

    let mut notes = Vec::new();
    if let Some(error) = error {
        notes.push(error.to_owned());
    }
    if !tool_call_made {
        notes.push(String::from(
            "assistant ended the turn without any tool calls",
        ));
    }
    if tool_call_made && !expected_tool_match {
        notes.push(String::from(
            "tool calls were emitted, but none matched the expected tool family",
        ));
    }
    if expected_tool_match && !expected_argument_match {
        notes.push(String::from(
            "the expected tool was attempted, but its arguments did not match the rubric",
        ));
    }
    if expected_argument_match && !expected_outcome_match {
        notes.push(String::from(
            "a matching tool call appeared, but the final draft buffer did not match the expected outcome",
        ));
    }

    RunEvaluation {
        tool_call_made,
        structured_tool_call_made,
        expected_tool_match,
        expected_argument_match,
        expected_outcome_match,
        turn_success: error.is_none() && expected_outcome_match,
        matched_variant_index,
        notes,
    }
}

fn matches_expected_tool_name(expected: &ExpectedToolUse, actual_name: &str) -> bool {
    match expected {
        ExpectedToolUse::DraftClaim { .. } => actual_name == "draft_claim",
        ExpectedToolUse::DraftRelation { .. } => actual_name == "draft_relation",
        ExpectedToolUse::DraftStance { .. } => actual_name == "draft_stance",
        ExpectedToolUse::DraftResolve { .. } => actual_name == "draft_resolve",
        ExpectedToolUse::DraftComment { .. } => actual_name == "draft_comment",
        ExpectedToolUse::PlainTextResponse { .. } => false,
        ExpectedToolUse::AnyStructuredTool => true,
        ExpectedToolUse::AnyTool { names } => names.iter().any(|name| name == actual_name),
    }
}

fn matches_expected_tool_call(
    expected: &ExpectedToolUse,
    execution: &crate::consensus_cli::llm::ToolExecutionTrace,
) -> bool {
    if !matches_expected_tool_name(expected, &execution.function_name) {
        return false;
    }

    match expected {
        ExpectedToolUse::DraftClaim {
            author: _,
            kind,
            parent_id,
            body_contains,
        } => execution
            .parsed_arguments
            .as_ref()
            .is_some_and(|arguments| {
                optional_enum_matches(arguments, "kind", kind.map(claim_kind_name))
                    && optional_claim_ref_matches(arguments, "parent", parent_id.as_deref())
                    && required_body_matches(arguments, "body", body_contains)
            }),
        ExpectedToolUse::DraftRelation {
            source_id,
            target_id,
            kind,
            author: _,
        } => execution
            .parsed_arguments
            .as_ref()
            .is_some_and(|arguments| {
                claim_ref_field_matches(arguments, "source", source_id)
                    && claim_ref_field_matches(arguments, "target", target_id)
                    && str_field(arguments, "kind") == Some(relation_kind_name(*kind))
            }),
        ExpectedToolUse::DraftStance {
            target_id,
            position,
            author: _,
        } => execution
            .parsed_arguments
            .as_ref()
            .is_some_and(|arguments| {
                claim_ref_field_matches(arguments, "target", target_id)
                    && str_field(arguments, "position") == Some(position_name(*position))
            }),
        ExpectedToolUse::DraftResolve {
            claim_id,
            outcome,
            author: _,
        } => execution
            .parsed_arguments
            .as_ref()
            .is_some_and(|arguments| {
                claim_ref_field_matches(arguments, "claim", claim_id)
                    && str_field(arguments, "outcome") == Some(outcome_name(*outcome))
            }),
        ExpectedToolUse::DraftComment {
            claim_id,
            body_contains,
            author: _,
        } => execution
            .parsed_arguments
            .as_ref()
            .is_some_and(|arguments| {
                optional_claim_ref_matches(arguments, "claim", claim_id.as_deref())
                    && required_body_matches(arguments, "body", body_contains)
            }),
        ExpectedToolUse::PlainTextResponse { .. } => false,
        ExpectedToolUse::AnyStructuredTool => true,
        ExpectedToolUse::AnyTool { names } => {
            names.iter().any(|name| name == &execution.function_name)
        }
    }
}

fn matches_expected_draft(expected: &ExpectedToolUse, draft: &DraftEntry) -> bool {
    match (expected, &draft.entry) {
        (
            ExpectedToolUse::DraftClaim {
                author: _,
                kind,
                parent_id,
                body_contains,
            },
            DraftContent::Claim {
                body,
                claim_kind,
                parent,
            },
        ) => {
            kind.is_none_or(|kind| kind == *claim_kind)
                && claim_ref_option_matches(parent.as_ref(), parent_id.as_deref())
                && text_matches(body, body_contains)
        }
        (
            ExpectedToolUse::DraftRelation {
                source_id,
                target_id,
                kind,
                author: _,
            },
            DraftContent::Relation {
                source,
                target,
                kind: actual_kind,
            },
        ) => {
            claim_ref_matches(source, source_id)
                && claim_ref_matches(target, target_id)
                && *actual_kind == *kind
        }
        (
            ExpectedToolUse::DraftStance {
                target_id,
                position,
                author: _,
            },
            DraftContent::Stance {
                target: actual_target,
                position: actual_position,
            },
        ) => claim_ref_matches(actual_target, target_id) && *actual_position == *position,
        (
            ExpectedToolUse::DraftResolve {
                claim_id,
                outcome,
                author: _,
            },
            DraftContent::Resolve {
                claim: actual_claim,
                outcome: actual_outcome,
            },
        ) => claim_ref_matches(actual_claim, claim_id) && *actual_outcome == *outcome,
        (
            ExpectedToolUse::DraftComment {
                claim_id,
                body_contains,
                author: _,
            },
            DraftContent::Comment { claim, body },
        ) => {
            claim_ref_option_matches(claim.as_ref(), claim_id.as_deref())
                && text_matches(body, body_contains)
        }
        (ExpectedToolUse::AnyStructuredTool, _) => true,
        _ => false,
    }
}

fn aggregate_runs(runs: &[ExperimentRun]) -> Vec<ExperimentAggregate> {
    let mut grouped: BTreeMap<(String, usize, usize), Vec<&ExperimentRun>> = BTreeMap::new();
    for run in runs {
        grouped
            .entry((run.case_id.clone(), run.history_turns, run.max_history))
            .or_default()
            .push(run);
    }

    grouped
        .into_iter()
        .map(
            |((case_id, history_turns, max_history), group)| ExperimentAggregate {
                case_id,
                history_turns,
                max_history,
                repeats: group.len(),
                tool_call_made: summarize_metric(&group, |run| run.evaluation.tool_call_made),
                structured_tool_call_made: summarize_metric(&group, |run| {
                    run.evaluation.structured_tool_call_made
                }),
                expected_tool_match: summarize_metric(&group, |run| {
                    run.evaluation.expected_tool_match
                }),
                expected_argument_match: summarize_metric(&group, |run| {
                    run.evaluation.expected_argument_match
                }),
                expected_outcome_match: summarize_metric(&group, |run| {
                    run.evaluation.expected_outcome_match
                }),
                turn_success: summarize_metric(&group, |run| run.evaluation.turn_success),
            },
        )
        .collect()
}

fn summarize_metric(
    runs: &[&ExperimentRun],
    predicate: impl Fn(&ExperimentRun) -> bool,
) -> MetricSummary {
    let success = runs.iter().filter(|run| predicate(run)).count();
    MetricSummary {
        success,
        total: runs.len(),
        rate: if runs.is_empty() {
            0.0
        } else {
            success as f64 / runs.len() as f64
        },
    }
}

fn str_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn optional_enum_matches(value: &Value, field: &str, expected: Option<&str>) -> bool {
    expected.is_none_or(|expected| str_field(value, field) == Some(expected))
}

fn claim_ref_matches(actual: &ClaimRef, expected: &str) -> bool {
    match actual {
        ClaimRef::Committed(claim_id) => {
            claim_id.0 == expected || format!("claim:{}", claim_id.0) == expected
        }
        ClaimRef::Draft(draft_id) => {
            format!("draft:{}", draft_id.0) == expected || format!("#{}", draft_id.0) == expected
        }
    }
}

fn claim_ref_option_matches(actual: Option<&ClaimRef>, expected: Option<&str>) -> bool {
    expected.is_none_or(|expected| actual.is_some_and(|actual| claim_ref_matches(actual, expected)))
}

fn claim_ref_field_matches(value: &Value, field: &str, expected: &str) -> bool {
    value
        .get(field)
        .is_some_and(|claim_ref| claim_ref_value_matches(claim_ref, expected))
}

fn optional_claim_ref_matches(value: &Value, field: &str, expected: Option<&str>) -> bool {
    expected.is_none_or(|expected| {
        value
            .get(field)
            .is_some_and(|claim_ref| claim_ref_value_matches(claim_ref, expected))
    })
}

fn claim_ref_value_matches(value: &Value, expected: &str) -> bool {
    if let Some(raw) = value.as_str() {
        return raw == expected
            || raw == format!("claim:{expected}")
            || raw == format!("draft:{expected}")
            || raw == format!("#{expected}");
    }

    value
        .get("claim_id")
        .and_then(Value::as_str)
        .is_some_and(|claim_id| claim_id == expected || format!("claim:{claim_id}") == expected)
        || value
            .get("draft_id")
            .and_then(Value::as_u64)
            .is_some_and(|draft_id| {
                format!("draft:{draft_id}") == expected || format!("#{draft_id}") == expected
            })
}

fn required_body_matches(value: &Value, field: &str, contains: &[String]) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|body| text_matches(body, contains))
}

fn text_matches(actual: &str, contains: &[String]) -> bool {
    let normalized_actual = normalize_text(actual);
    if normalized_actual.is_empty() {
        return false;
    }
    contains
        .iter()
        .all(|needle| normalized_actual.contains(&normalize_text(needle)))
}

fn normalize_text(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn claim_kind_name(kind: ClaimKind) -> &'static str {
    match kind {
        ClaimKind::Item => "item",
        ClaimKind::Proposal => "proposal",
        ClaimKind::Fact => "fact",
        ClaimKind::Conditional => "conditional",
        ClaimKind::Value => "value",
        ClaimKind::Reference => "reference",
    }
}

fn relation_kind_name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Attacks => "attacks",
        RelationKind::Supports => "supports",
    }
}

fn position_name(position: Position) -> &'static str {
    match position {
        Position::Block => "block",
        Position::Object => "object",
        Position::StandAside => "stand_aside",
        Position::Abstain => "abstain",
        Position::Consent => "consent",
        Position::Support => "support",
        Position::Champion => "champion",
    }
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Accepted => "accepted",
        Outcome::Rejected => "rejected",
        Outcome::Tabled => "tabled",
        Outcome::Withdrawn => "withdrawn",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::types::ClaimId;
    use crate::consensus_cli::llm::ToolExecutionTrace;

    #[test]
    fn draft_stance_matches_argument_and_draft() {
        let expected = ExpectedToolUse::DraftStance {
            author: Some(String::from("alice")),
            target_id: String::from("prop-jwt"),
            position: Position::Block,
        };
        let execution = ToolExecutionTrace {
            call_id: String::from("call_1"),
            function_name: String::from("draft_stance"),
            arguments_json: String::from(
                "{\"target\":{\"claim_id\":\"prop-jwt\"},\"position\":\"block\"}",
            ),
            parsed_arguments: Some(json!({
                "target": { "claim_id": "prop-jwt" },
                "position": "block",
            })),
            argument_parse_error: None,
            tool_result_content: String::from("{\"draft_id\":1}"),
            dispatch_error: None,
        };
        let draft = DraftEntry {
            id: crate::consensus::engine::DraftId(1),
            entry: DraftContent::Stance {
                target: ClaimRef::Committed(ClaimId(String::from("prop-jwt"))),
                position: Position::Block,
            },
        };

        assert!(matches_expected_tool_call(&expected, &execution));
        assert!(matches_expected_draft(&expected, &draft));
    }

    #[test]
    fn plain_text_response_requires_empty_draft_buffer() {
        let case = ToolCallCase {
            id: String::from("process"),
            description: String::from("Process question"),
            checkpoint_entries: 0,
            participant: String::from("alice"),
            user_message: String::from("What does /submit do?"),
            expected: vec![ExpectedToolUse::PlainTextResponse {
                text_contains: vec![String::from("/submit")],
            }],
        };
        let trace = LlmTurnTrace {
            rounds: vec![crate::consensus_cli::llm::LlmRoundTrace {
                round: 0,
                request_history_messages: 1,
                request_messages: 2,
                response_chunks: 1,
                assistant_message: Some(CompletedMessage {
                    role: String::from("assistant"),
                    content: Some(String::from("Use /submit to commit pending drafts.")),
                    tool_calls: vec![],
                    finish_reason: Some(String::from("stop")),
                }),
                tool_results: vec![],
                error: None,
            }],
            final_message: Some(CompletedMessage {
                role: String::from("assistant"),
                content: Some(String::from("Use /submit to commit pending drafts.")),
                tool_calls: vec![],
                finish_reason: Some(String::from("stop")),
            }),
        };

        let evaluation = evaluate_case(&case, &trace, &[], None);
        assert!(evaluation.expected_outcome_match);
        assert!(evaluation.turn_success);
    }

    #[test]
    fn synthetic_history_contains_tool_pairs() {
        let history = build_synthetic_history(&ConsensusEngine::new(String::new()), 2);
        assert_eq!(history.len(), 8);
        assert_eq!(history[0]["role"], "user");
        assert_eq!(history[1]["role"], "assistant");
        assert_eq!(history[2]["role"], "tool");
        assert_eq!(history[3]["role"], "assistant");
    }
}
