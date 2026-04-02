use std::io::{self, Write};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::consensus::engine::EngineError;
use crate::consensus::entry_buffer::{
    ApplyResult, EntryBuffer, EntryBufferError, format_tool_trace,
};
use crate::consensus::format::{
    format_claim_detail, format_drafts, format_impact_analysis, format_overview,
};
use crate::consensus::llm_turn::LlmTurnTrace;
use crate::consensus::render::OverviewData;
use crate::consensus::types::ClaimId;

use super::llm::{ConsensusLlm, LlmError};
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
    #[error("entry buffer error: {0}")]
    EntryBuffer(#[from] EntryBufferError),
    #[error("stdin read failed: {0}")]
    Stdin(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
enum ConfirmationAction {
    Submit,
    ClearDrafts,
}

pub struct ConsensusApp {
    llm: ConsensusLlm,
    session: SessionClient,
    buffer: EntryBuffer,
    history: Vec<Value>,
    connected: bool,
    confirmation: Option<ConfirmationAction>,
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
            buffer: EntryBuffer::new(config.participant),
            history: Vec::new(),
            connected: true,
            confirmation: None,
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
            buffer: EntryBuffer::new(config.participant),
            history: Vec::new(),
            connected: true,
            confirmation: None,
            debug_tool_trace: config.debug_tool_trace,
        };
        app.catch_up().await?;
        Ok(app)
    }

    pub fn session_id(&self) -> &str {
        self.session.session_id()
    }

    pub fn overview(&self) -> OverviewData {
        self.buffer.engine().overview()
    }

    pub async fn run(&mut self) -> Result<(), AppError> {
        println!("Session: {}", self.session.session_id());
        println!("{}", format_overview(&self.buffer.engine().overview()));

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

        if let Some(pending) = self.buffer.pending_submission()
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
                .run_turn_with_trace(self.buffer.engine_mut(), &mut self.history)
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
            match self
                .llm
                .run_turn(self.buffer.engine_mut(), &mut self.history)
                .await
            {
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
                println!("{}", format_overview(&self.buffer.engine().overview()));
            }
            "claim" => {
                if arg.is_empty() {
                    println!("Usage: /claim <claim_id>");
                } else if let Some(detail) =
                    self.buffer.engine().claim_detail(&ClaimId(arg.to_owned()))
                {
                    println!("{}", format_claim_detail(&detail));
                } else {
                    println!("Claim not found: {arg}");
                }
            }
            "drafts" => {
                self.print_draft_review();
            }
            "submit" => {
                if self.buffer.engine().show_drafts().is_empty() {
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
                if self.buffer.engine().show_drafts().is_empty() {
                    println!("No pending drafts to clear.");
                } else {
                    println!("{}", format_drafts(self.buffer.engine().show_drafts()));
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
                let count = self.buffer.engine().show_drafts().len();
                self.buffer.engine_mut().clear_drafts();
                println!("Cleared {count} drafts.");
            }
        }

        Ok(())
    }

    async fn begin_submission(&mut self) -> Result<(), AppError> {
        let Some(pending) = self.buffer.begin_submission()? else {
            println!("No pending drafts to submit.");
            return Ok(());
        };

        println!("Submitting {} entries...", pending.payloads.len());
        self.resume_pending_submission().await?;
        Ok(())
    }

    async fn resume_pending_submission(&mut self) -> Result<(), AppError> {
        loop {
            let Some(pending) = self.buffer.pending_submission() else {
                return Ok(());
            };

            if pending.next_entry >= pending.payloads.len() {
                if self.buffer.finish_submission()? {
                    println!("Submission complete.");
                }
                return Ok(());
            }

            if !self.connected {
                println!("Submission paused until the session reconnects.");
                return Ok(());
            }

            let waiting_for = pending.next_entry;
            let payload = pending.payloads[waiting_for].clone();
            match self.session.append_json(payload).await {
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
                    .buffer
                    .pending_submission()
                    .map(|p| p.next_entry > waiting_for)
                    .unwrap_or(false)
                {
                    break;
                }

                let event = self.session.next_event().await?;
                match event {
                    SessionEvent::Entry { index, payload } => {
                        self.apply_and_log_entry(index, payload)?;
                        if self.buffer.is_submission_complete()
                            && self.buffer.finish_submission()?
                        {
                            println!("Submission complete.");
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
                .fetch_entries(self.buffer.next_index(), ENTRY_PAGE_LIMIT)
                .await?;
            if page.entries.is_empty() {
                break;
            }

            for (offset, payload) in page.entries.into_iter().enumerate() {
                self.apply_entry(page.start_index + offset, payload)?;
            }
        }

        self.drain_queued_session_events()?;

        if self.buffer.is_submission_complete() && self.buffer.finish_submission()? {
            println!("Submission complete.");
        }

        Ok(())
    }

    fn drain_queued_session_events(&mut self) -> Result<(), AppError> {
        while let Some(event) = self.session.try_next_event() {
            match event {
                SessionEvent::Entry { index, payload } => {
                    self.apply_entry(index, payload)?;
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
                self.apply_and_log_entry(index, payload)?;
                if self.buffer.is_submission_complete() && self.buffer.finish_submission()? {
                    println!("Submission complete.");
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

    /// Apply an entry and print a sync notification for each applied entry.
    fn apply_and_log_entry(&mut self, index: usize, payload: Value) -> Result<(), AppError> {
        let results = self.buffer.apply_or_buffer_entry(index, payload)?;
        for (idx, result) in &results {
            match result {
                ApplyResult::Applied(_) => {
                    println!("\n[sync] applied session entry #{idx}");
                }
                ApplyResult::Skipped { error } => {
                    eprintln!("Skipping non-consensus session payload: {error}");
                }
            }
        }
        Ok(())
    }

    /// Apply an entry silently (for catch-up).
    fn apply_entry(&mut self, index: usize, payload: Value) -> Result<(), AppError> {
        let results = self.buffer.apply_or_buffer_entry(index, payload)?;
        for (_, result) in &results {
            if let ApplyResult::Skipped { error } = result {
                eprintln!("Skipping non-consensus session payload: {error}");
            }
        }
        Ok(())
    }

    fn print_draft_review(&self) {
        println!("{}", format_drafts(self.buffer.engine().show_drafts()));
        println!(
            "{}",
            format_impact_analysis(&self.buffer.engine().impact_analysis())
        );
    }

    fn print_tool_trace(&self, trace: &LlmTurnTrace) {
        for line in format_tool_trace(trace) {
            println!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tool_trace_lists_each_tool_round() {
        use crate::consensus::llm_turn::{LlmRoundTrace, ToolExecutionTrace};

        let trace = LlmTurnTrace {
            rounds: vec![LlmRoundTrace {
                round: 0,
                request_history_messages: 0,
                request_messages: 1,
                response_chunks: 2,
                assistant_message: None,
                tool_results: vec![ToolExecutionTrace {
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
