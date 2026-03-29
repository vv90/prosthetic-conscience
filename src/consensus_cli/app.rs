use std::collections::BTreeMap;
use std::io::{self, Write};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use uuid::Uuid;

use crate::consensus::engine::{ConsensusEngine, DraftId, EngineError};
use crate::consensus::format::{
    format_claim_detail, format_drafts, format_impact_analysis, format_overview,
};
use crate::consensus::render::OverviewData;
use crate::consensus::types::{ClaimId, Entry};

use super::llm::{ConsensusLlm, LlmError, LlmTurnTrace};
use super::session::{SessionClient, SessionError, SessionEvent};

const ENTRY_PAGE_LIMIT: usize = 1000;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub gateway_url: String,
    pub auth_token: Option<String>,
    pub model: String,
    pub participant: String,
    pub max_history: usize,
    pub debug_tool_trace: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("session error: {0}")]
    Session(#[from] SessionError),
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("failed to serialize submission payload: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("stdin read failed: {0}")]
    Stdin(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
enum ConfirmationAction {
    Submit,
    ClearDrafts,
}

#[derive(Debug, Clone)]
struct PendingSubmission {
    draft_ids: Vec<DraftId>,
    payloads: Vec<Value>,
    next_entry: usize,
}

pub struct ConsensusApp {
    llm: ConsensusLlm,
    session: SessionClient,
    engine: ConsensusEngine,
    history: Vec<Value>,
    next_index: usize,
    buffered_entries: BTreeMap<usize, Value>,
    connected: bool,
    confirmation: Option<ConfirmationAction>,
    pending_submission: Option<PendingSubmission>,
    debug_tool_trace: bool,
}

impl ConsensusApp {
    pub async fn create(config: AppConfig) -> Result<Self, AppError> {
        let llm = ConsensusLlm::new(
            config.gateway_url.clone(),
            config.auth_token.clone(),
            config.model,
            config.participant.clone(),
            config.max_history,
        );
        let session = SessionClient::create(config.gateway_url, config.auth_token).await?;
        let mut app = Self {
            llm,
            session,
            engine: ConsensusEngine::new(config.participant),
            history: Vec::new(),
            next_index: 0,
            buffered_entries: BTreeMap::new(),
            connected: true,
            confirmation: None,
            pending_submission: None,
            debug_tool_trace: config.debug_tool_trace,
        };
        app.catch_up().await?;
        Ok(app)
    }

    pub async fn join(config: AppConfig, session_id: String) -> Result<Self, AppError> {
        let llm = ConsensusLlm::new(
            config.gateway_url.clone(),
            config.auth_token.clone(),
            config.model,
            config.participant.clone(),
            config.max_history,
        );
        let session =
            SessionClient::join(config.gateway_url, config.auth_token, session_id).await?;
        let mut app = Self {
            llm,
            session,
            engine: ConsensusEngine::new(config.participant),
            history: Vec::new(),
            next_index: 0,
            buffered_entries: BTreeMap::new(),
            connected: true,
            confirmation: None,
            pending_submission: None,
            debug_tool_trace: config.debug_tool_trace,
        };
        app.catch_up().await?;
        Ok(app)
    }

    pub fn session_id(&self) -> &str {
        self.session.session_id()
    }

    pub fn overview(&self) -> OverviewData {
        self.engine.overview()
    }

    pub async fn run(&mut self) -> Result<(), AppError> {
        println!("Session: {}", self.session.session_id());
        println!("{}", format_overview(&self.engine.overview()));

        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();

        loop {
            print!("> ");
            io::stdout().flush()?;

            let next_line = lines.next_line();
            tokio::pin!(next_line);

            tokio::select! {
                line = &mut next_line => {
                    let Some(line) = line? else {
                        break;
                    };
                    if !self.handle_input(line).await? {
                        break;
                    }
                }
                event = self.session.next_event() => {
                    let event = event?;
                    self.handle_session_event(event).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_input(&mut self, line: String) -> Result<bool, AppError> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(true);
        }

        if let Some(action) = self.confirmation.take() {
            self.handle_confirmation(action, line).await?;
            return Ok(true);
        }

        if let Some(pending) = &self.pending_submission
            && !matches!(line, "/overview" | "/drafts" | "/help" | "/quit")
            && !line.starts_with("/claim ")
        {
            println!(
                "Submission recovery is in progress ({} of {} entries confirmed).",
                pending.next_entry,
                pending.payloads.len()
            );
            return Ok(true);
        }

        if let Some(command) = line.strip_prefix('/') {
            return self.handle_command(command).await;
        }

        if !self.connected {
            println!("Session is disconnected. Waiting to reconnect before sending to the LLM.");
            return Ok(true);
        }

        self.history.push(json!({"role": "user", "content": line}));
        if self.debug_tool_trace {
            match self
                .llm
                .run_turn_with_trace(&mut self.engine, &mut self.history)
                .await
            {
                Ok(trace) => {
                    self.print_tool_trace(&trace);
                    let message = trace
                        .final_message
                        .expect("successful trace has final message");
                    if let Some(content) = &message.content
                        && !content.trim().is_empty()
                    {
                        println!("{content}");
                    }
                    self.print_draft_review();
                }
                Err(error) => {
                    self.print_tool_trace(&error.trace);
                    eprintln!("LLM turn failed: {}", error.error);
                    self.history.pop();
                }
            }
        } else {
            match self.llm.run_turn(&mut self.engine, &mut self.history).await {
                Ok(message) => {
                    if let Some(content) = &message.content
                        && !content.trim().is_empty()
                    {
                        println!("{content}");
                    }
                    self.print_draft_review();
                }
                Err(error) => {
                    eprintln!("LLM turn failed: {error}");
                    self.history.pop();
                }
            }
        }

        Ok(true)
    }

    async fn handle_command(&mut self, command: &str) -> Result<bool, AppError> {
        let (name, arg) = command
            .split_once(' ')
            .map_or((command, ""), |(n, a)| (n, a.trim()));

        match name {
            "overview" => {
                println!("{}", format_overview(&self.engine.overview()));
            }
            "claim" => {
                if arg.is_empty() {
                    println!("Usage: /claim <claim_id>");
                } else if let Some(detail) = self.engine.claim_detail(&ClaimId(arg.to_owned())) {
                    println!("{}", format_claim_detail(&detail));
                } else {
                    println!("Claim not found: {arg}");
                }
            }
            "drafts" => {
                self.print_draft_review();
            }
            "submit" => {
                if self.engine.show_drafts().is_empty() {
                    println!("No pending drafts to submit.");
                } else if !self.connected {
                    println!("Session is disconnected. Wait for reconnect before submitting.");
                } else {
                    self.print_draft_review();
                    println!("Submit these drafts? [y/N]");
                    self.confirmation = Some(ConfirmationAction::Submit);
                }
            }
            "clear" => {
                if self.engine.show_drafts().is_empty() {
                    println!("No pending drafts to clear.");
                } else {
                    println!("{}", format_drafts(self.engine.show_drafts()));
                    println!("Discard all pending drafts? [y/N]");
                    self.confirmation = Some(ConfirmationAction::ClearDrafts);
                }
            }
            "help" => {
                println!("/overview, /claim <id>, /drafts, /submit, /clear, /help, /quit");
            }
            "quit" => return Ok(false),
            _ => println!("Unknown command: /{name}"),
        }

        Ok(true)
    }

    async fn handle_confirmation(
        &mut self,
        action: ConfirmationAction,
        response: &str,
    ) -> Result<(), AppError> {
        let confirmed = matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes");
        if !confirmed {
            println!("Cancelled.");
            return Ok(());
        }

        match action {
            ConfirmationAction::Submit => self.begin_submission().await?,
            ConfirmationAction::ClearDrafts => {
                let count = self.engine.show_drafts().len();
                self.engine.clear_drafts();
                println!("Cleared {count} drafts.");
            }
        }

        Ok(())
    }

    async fn begin_submission(&mut self) -> Result<(), AppError> {
        let bundle = self
            .engine
            .submission_bundle(|| ClaimId(Uuid::new_v4().to_string()));

        if bundle.entries.is_empty() {
            println!("No pending drafts to submit.");
            return Ok(());
        }

        let payloads: Vec<Value> = bundle
            .entries
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;

        println!("Submitting {} entries...", payloads.len());
        self.pending_submission = Some(PendingSubmission {
            draft_ids: bundle.draft_ids,
            payloads,
            next_entry: 0,
        });

        self.resume_pending_submission().await?;
        Ok(())
    }

    async fn resume_pending_submission(&mut self) -> Result<(), AppError> {
        loop {
            let Some(pending) = &self.pending_submission else {
                return Ok(());
            };

            if pending.next_entry >= pending.payloads.len() {
                self.finish_submission()?;
                return Ok(());
            }

            if !self.connected {
                println!("Submission paused until the session reconnects.");
                return Ok(());
            }

            let waiting_for = pending.next_entry;
            let payload = pending.payloads[waiting_for].clone();
            match self.session.append_json(payload.clone()).await {
                Ok(()) => {}
                Err(SessionError::Disconnected(reason)) => {
                    self.connected = false;
                    eprintln!("Session disconnected during submit: {reason}");
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            }

            loop {
                if self
                    .pending_submission
                    .as_ref()
                    .map(|pending| pending.next_entry > waiting_for)
                    .unwrap_or(false)
                {
                    break;
                }

                let event = self.session.next_event().await?;
                match event {
                    SessionEvent::Entry { index, payload } => {
                        self.apply_or_buffer_entry(index, payload)?;
                        println!("\n[sync] applied session entry #{index}");
                        if self
                            .pending_submission
                            .as_ref()
                            .is_some_and(|pending| pending.next_entry >= pending.payloads.len())
                        {
                            self.finish_submission()?;
                        }
                    }
                    SessionEvent::Disconnected { reason } => {
                        self.connected = false;
                        eprintln!("\n[sync] disconnected: {reason}");
                    }
                    SessionEvent::Reconnected => {
                        self.connected = true;
                        println!("\n[sync] reconnected, resyncing session state...");
                        self.catch_up().await?;
                    }
                    SessionEvent::Warning(message) => {
                        eprintln!("\n[sync] warning: {message}");
                    }
                }

                if !self.connected {
                    return Ok(());
                }
            }
        }
    }

    async fn catch_up(&mut self) -> Result<(), AppError> {
        loop {
            let page = self
                .session
                .fetch_entries(self.next_index, ENTRY_PAGE_LIMIT)
                .await?;
            if page.entries.is_empty() {
                break;
            }

            for (offset, payload) in page.entries.into_iter().enumerate() {
                self.apply_or_buffer_entry(page.start_index + offset, payload)?;
            }
        }

        self.drain_queued_session_events()?;
        self.drain_buffered_entries()?;

        if self
            .pending_submission
            .as_ref()
            .is_some_and(|pending| pending.next_entry >= pending.payloads.len())
        {
            self.finish_submission()?;
        }

        Ok(())
    }

    fn drain_queued_session_events(&mut self) -> Result<(), AppError> {
        while let Some(event) = self.session.try_next_event() {
            match event {
                SessionEvent::Entry { index, payload } => {
                    self.apply_or_buffer_entry(index, payload)?;
                }
                SessionEvent::Disconnected { reason } => {
                    self.connected = false;
                    eprintln!("Session disconnected: {reason}");
                }
                SessionEvent::Reconnected => {
                    self.connected = true;
                }
                SessionEvent::Warning(message) => {
                    eprintln!("Session warning: {message}");
                }
            }
        }
        Ok(())
    }

    async fn handle_session_event(&mut self, event: SessionEvent) -> Result<(), AppError> {
        match event {
            SessionEvent::Entry { index, payload } => {
                self.apply_or_buffer_entry(index, payload)?;
                println!("\n[sync] applied session entry #{index}");
                if self
                    .pending_submission
                    .as_ref()
                    .is_some_and(|pending| pending.next_entry >= pending.payloads.len())
                {
                    self.finish_submission()?;
                }
            }
            SessionEvent::Disconnected { reason } => {
                self.connected = false;
                eprintln!("\n[sync] disconnected: {reason}");
            }
            SessionEvent::Reconnected => {
                self.connected = true;
                println!("\n[sync] reconnected, resyncing session state...");
                self.catch_up().await?;
                self.resume_pending_submission().await?;
            }
            SessionEvent::Warning(message) => {
                eprintln!("\n[sync] warning: {message}");
            }
        }

        Ok(())
    }

    fn apply_or_buffer_entry(&mut self, index: usize, payload: Value) -> Result<(), AppError> {
        if index < self.next_index {
            return Ok(());
        }

        if index > self.next_index {
            self.buffered_entries.insert(index, payload);
            return Ok(());
        }

        self.apply_payload(payload)?;
        self.next_index += 1;
        self.drain_buffered_entries()?;
        Ok(())
    }

    fn drain_buffered_entries(&mut self) -> Result<(), AppError> {
        while let Some(payload) = self.buffered_entries.remove(&self.next_index) {
            self.apply_payload(payload)?;
            self.next_index += 1;
        }
        Ok(())
    }

    fn apply_payload(&mut self, payload: Value) -> Result<(), AppError> {
        self.note_submission_payload(&payload);

        match serde_json::from_value::<Entry>(payload.clone()) {
            Ok(entry) => {
                self.engine.append(entry);
            }
            Err(error) => {
                eprintln!("Skipping non-consensus session payload: {error}");
            }
        }

        Ok(())
    }

    /// Advance the pending submission counter when we see our own entry echoed
    /// back from the session log. Uses `Value` equality, which is safe here
    /// because the gateway stores payloads as opaque JSON blobs without
    /// transformation, and `serde_json::Value` compares structurally (key
    /// order does not affect equality).
    fn note_submission_payload(&mut self, payload: &Value) {
        if let Some(pending) = &mut self.pending_submission
            && let Some(expected) = pending.payloads.get(pending.next_entry)
            && expected == payload
        {
            pending.next_entry += 1;
        }
    }

    fn finish_submission(&mut self) -> Result<(), AppError> {
        let Some(pending) = self.pending_submission.take() else {
            return Ok(());
        };

        for draft_id in pending.draft_ids {
            self.engine.remove_draft(draft_id)?;
        }

        println!("Submission complete.");
        Ok(())
    }

    fn print_draft_review(&self) {
        println!("{}", format_drafts(self.engine.show_drafts()));
        println!("{}", format_impact_analysis(&self.engine.impact_analysis()));
    }

    fn print_tool_trace(&self, trace: &LlmTurnTrace) {
        for line in format_tool_trace(trace) {
            println!("{line}");
        }
    }
}

fn format_tool_trace(trace: &LlmTurnTrace) -> Vec<String> {
    let mut lines = Vec::new();

    for round in &trace.rounds {
        if round.tool_results.is_empty() {
            lines.push(format!("[trace] round {} no tool calls", round.round));
            continue;
        }

        for execution in &round.tool_results {
            lines.push(format!(
                "[trace] round {} tool {} {}",
                round.round,
                execution.function_name,
                compact_debug_text(&execution.arguments_json, 160)
            ));

            if let Some(error) = &execution.dispatch_error {
                lines.push(format!(
                    "[trace] round {} error {}",
                    round.round,
                    compact_debug_text(error, 160)
                ));
            }
        }
    }

    lines
}

fn compact_debug_text(text: &str, max_chars: usize) -> String {
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    compact = compact.chars().take(max_chars).collect();
    compact.push_str("...");
    compact
}

#[cfg(test)]
mod tests {
    use super::super::session::SessionClient;
    use super::*;

    #[test]
    fn note_submission_payload_advances_only_matching_entry() {
        let mut app = ConsensusApp {
            llm: ConsensusLlm::new(
                String::from("http://127.0.0.1:3000"),
                None,
                String::from("default"),
                String::from("assistant"),
                100,
            ),
            session: SessionClient::stub("session"),
            engine: ConsensusEngine::new(String::from("assistant")),
            history: Vec::new(),
            next_index: 0,
            buffered_entries: BTreeMap::new(),
            connected: true,
            confirmation: None,
            pending_submission: Some(PendingSubmission {
                draft_ids: vec![DraftId(0)],
                payloads: vec![json!({"type":"comment","author":"alice","body":"hello"})],
                next_entry: 0,
            }),
            debug_tool_trace: false,
        };

        app.note_submission_payload(&json!({"type":"comment","author":"alice","body":"hello"}));
        assert_eq!(app.pending_submission.as_ref().unwrap().next_entry, 1);
    }

    #[test]
    fn apply_or_buffer_entry_holds_future_entries_until_gap_is_closed() {
        let llm = ConsensusLlm::new(
            String::from("http://127.0.0.1:3000"),
            None,
            String::from("default"),
            String::from("assistant"),
            100,
        );
        let session = SessionClient::stub("session");
        let mut app = ConsensusApp {
            llm,
            session,
            engine: ConsensusEngine::new(String::from("assistant")),
            history: Vec::new(),
            next_index: 0,
            buffered_entries: BTreeMap::new(),
            connected: true,
            confirmation: None,
            pending_submission: None,
            debug_tool_trace: false,
        };

        app.apply_or_buffer_entry(1, json!({"type":"comment","author":"alice","body":"later"}))
            .unwrap();
        assert_eq!(app.next_index, 0);
        assert!(app.buffered_entries.contains_key(&1));

        app.apply_or_buffer_entry(0, json!({"type":"comment","author":"alice","body":"now"}))
            .unwrap();
        assert_eq!(app.next_index, 2);
        assert!(app.buffered_entries.is_empty());
    }

    #[test]
    fn format_tool_trace_lists_each_tool_round() {
        let trace = LlmTurnTrace {
            rounds: vec![crate::consensus_cli::llm::LlmRoundTrace {
                round: 0,
                request_history_messages: 0,
                request_messages: 1,
                response_chunks: 2,
                assistant_message: None,
                tool_results: vec![crate::consensus_cli::llm::ToolExecutionTrace {
                    call_id: String::from("call_1"),
                    function_name: String::from("draft_stance"),
                    arguments_json: String::from(
                        "{\"target_id\":\"prop-hybrid\",\"position\":\"consent\"}",
                    ),
                    parsed_arguments: None,
                    argument_parse_error: None,
                    tool_result_content: String::new(),
                    dispatch_error: None,
                }],
                error: None,
            }],
            final_message: None,
        };

        let lines = format_tool_trace(&trace);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("round 0"));
        assert!(lines[0].contains("draft_stance"));
        assert!(lines[0].contains("prop-hybrid"));
    }
}
