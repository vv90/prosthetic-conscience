//! Pure entry buffering and submission tracking for consensus sessions.
//!
//! `EntryBuffer` encapsulates the state management for receiving session
//! entries (possibly out of order), applying them to the consensus engine,
//! and tracking pending submissions. It contains no I/O — all side effects
//! (printing, network) are handled by the caller based on return values.

use std::collections::BTreeMap;

use serde_json::Value;
use uuid::Uuid;

use crate::consensus::engine::{ConsensusEngine, DraftId, EngineError};
use crate::consensus::llm_turn::LlmTurnTrace;
use crate::consensus::types::{ClaimId, Entry};

// ---------------------------------------------------------------------------
// Pending submission tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PendingSubmission {
    pub draft_ids: Vec<DraftId>,
    pub payloads: Vec<Value>,
    pub next_entry: usize,
}

// ---------------------------------------------------------------------------
// Entry application result
// ---------------------------------------------------------------------------

/// What happened when a payload was applied to the engine.
#[derive(Debug)]
pub enum ApplyResult {
    /// The entry was parsed and applied to the engine.
    Applied(Entry),
    /// The payload could not be parsed as a consensus entry (skipped).
    Skipped { error: String },
}

// ---------------------------------------------------------------------------
// EntryBuffer
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum EntryBufferError {
    #[error("engine error: {0}")]
    Engine(#[from] EngineError),
    #[error("failed to serialize submission payload: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub struct EntryBuffer {
    engine: ConsensusEngine,
    next_index: usize,
    buffered_entries: BTreeMap<usize, Value>,
    pending_submission: Option<PendingSubmission>,
}

impl EntryBuffer {
    pub fn new(participant: String) -> Self {
        Self {
            engine: ConsensusEngine::new(participant),
            next_index: 0,
            buffered_entries: BTreeMap::new(),
            pending_submission: None,
        }
    }

    /// Access the underlying consensus engine.
    pub fn engine(&self) -> &ConsensusEngine {
        &self.engine
    }

    /// Mutable access to the underlying consensus engine.
    pub fn engine_mut(&mut self) -> &mut ConsensusEngine {
        &mut self.engine
    }

    /// The next expected entry index.
    pub fn next_index(&self) -> usize {
        self.next_index
    }

    /// Access the pending submission state, if any.
    pub fn pending_submission(&self) -> Option<&PendingSubmission> {
        self.pending_submission.as_ref()
    }

    /// Returns true if a submission is in progress and all entries have been
    /// echoed back.
    pub fn is_submission_complete(&self) -> bool {
        self.pending_submission
            .as_ref()
            .is_some_and(|pending| pending.next_entry >= pending.payloads.len())
    }

    /// Apply an entry at the given index, buffering it if it arrives out of order.
    ///
    /// Returns the list of entries that were actually applied (may be more than
    /// one if buffered entries became contiguous).
    pub fn apply_or_buffer_entry(
        &mut self,
        index: usize,
        payload: Value,
    ) -> Result<Vec<(usize, ApplyResult)>, EntryBufferError> {
        if index < self.next_index {
            return Ok(vec![]);
        }

        if index > self.next_index {
            self.buffered_entries.insert(index, payload);
            return Ok(vec![]);
        }

        let mut results = Vec::new();
        results.push((self.next_index, self.apply_payload(payload)?));
        self.next_index += 1;

        // Drain any buffered entries that are now contiguous.
        while let Some(buffered) = self.buffered_entries.remove(&self.next_index) {
            results.push((self.next_index, self.apply_payload(buffered)?));
            self.next_index += 1;
        }

        Ok(results)
    }

    /// Apply a single payload to the engine.
    fn apply_payload(&mut self, payload: Value) -> Result<ApplyResult, EntryBufferError> {
        self.note_submission_payload(&payload);

        match serde_json::from_value::<Entry>(payload) {
            Ok(entry) => {
                self.engine.append(entry.clone());
                Ok(ApplyResult::Applied(entry))
            }
            Err(error) => Ok(ApplyResult::Skipped {
                error: error.to_string(),
            }),
        }
    }

    /// Advance the pending submission counter when we see our own entry echoed
    /// back from the session log.
    fn note_submission_payload(&mut self, payload: &Value) {
        if let Some(pending) = &mut self.pending_submission
            && let Some(expected) = pending.payloads.get(pending.next_entry)
            && expected == payload
        {
            pending.next_entry += 1;
        }
    }

    /// Prepare a submission: serialize the current drafts into payloads.
    pub fn begin_submission(&mut self) -> Result<Option<PendingSubmission>, EntryBufferError> {
        let bundle = self
            .engine
            .submission_bundle(|| ClaimId(Uuid::new_v4().to_string()));

        if bundle.entries.is_empty() {
            return Ok(None);
        }

        let payloads: Vec<Value> = bundle
            .entries
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;

        let pending = PendingSubmission {
            draft_ids: bundle.draft_ids,
            payloads,
            next_entry: 0,
        };
        self.pending_submission = Some(pending.clone());
        Ok(Some(pending))
    }

    /// Finalize a completed submission: remove the submitted drafts.
    pub fn finish_submission(&mut self) -> Result<bool, EntryBufferError> {
        let Some(pending) = self.pending_submission.take() else {
            return Ok(false);
        };

        for draft_id in pending.draft_ids {
            self.engine.remove_draft(draft_id)?;
        }

        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Trace formatting (pure)
// ---------------------------------------------------------------------------

/// Format an LLM turn trace into human-readable lines.
pub fn format_tool_trace(trace: &LlmTurnTrace) -> Vec<String> {
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

/// Compact text for debug display, collapsing whitespace and truncating.
pub fn compact_debug_text(text: &str, max_chars: usize) -> String {
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    compact = compact.chars().take(max_chars).collect();
    compact.push_str("...");
    compact
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::llm_turn::{LlmRoundTrace, ToolExecutionTrace};
    use serde_json::json;

    #[test]
    fn note_submission_payload_advances_only_matching_entry() {
        let mut buffer = EntryBuffer::new(String::from("assistant"));
        buffer.pending_submission = Some(PendingSubmission {
            draft_ids: vec![DraftId(0)],
            payloads: vec![json!({"type":"comment","author":"alice","body":"hello"})],
            next_entry: 0,
        });

        // Simulate applying the matching payload.
        buffer
            .apply_or_buffer_entry(0, json!({"type":"comment","author":"alice","body":"hello"}))
            .unwrap();
        assert_eq!(buffer.pending_submission.as_ref().unwrap().next_entry, 1);
    }

    #[test]
    fn apply_or_buffer_entry_holds_future_entries_until_gap_is_closed() {
        let mut buffer = EntryBuffer::new(String::from("assistant"));

        let results = buffer
            .apply_or_buffer_entry(1, json!({"type":"comment","author":"alice","body":"later"}))
            .unwrap();
        assert!(results.is_empty());
        assert_eq!(buffer.next_index, 0);

        let results = buffer
            .apply_or_buffer_entry(0, json!({"type":"comment","author":"alice","body":"now"}))
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(buffer.next_index, 2);
        assert!(buffer.buffered_entries.is_empty());
    }

    #[test]
    fn format_tool_trace_lists_each_tool_round() {
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
