//! System prompt construction for the consensus LLM participant.
//!
//! Builds a system prompt from engine state and caller-supplied configuration.
//! Pure function — reads engine state, returns a string.

use std::fmt::Write;

use super::engine::ConsensusEngine;
use super::format::format_overview;

/// Caller-supplied configuration for the system prompt.
pub struct PromptConfig {
    /// The LLM's participant name in the deliberation (used as the local draft author).
    pub participant_name: String,
    /// High-level role description, e.g. "You are an AI facilitator helping a team reach consensus."
    pub role_description: String,
    /// Session-specific instructions appended at the end. May be empty.
    pub custom_instructions: String,
}

/// Build the system prompt for an LLM consensus participant.
pub fn build_system_prompt(engine: &ConsensusEngine, config: &PromptConfig) -> String {
    let overview = engine.overview();
    let state_text = format_overview(&overview);

    let mut prompt = String::new();

    // Role
    let _ = writeln!(prompt, "{}", config.role_description);
    let _ = writeln!(prompt);
    let _ = writeln!(
        prompt,
        "You are participating as \"{}\".",
        config.participant_name
    );

    // Deliberation state
    let _ = writeln!(prompt);
    let _ = writeln!(prompt, "## Current deliberation state");
    let _ = writeln!(prompt);
    let _ = write!(prompt, "{state_text}");

    // Tool reference
    let _ = writeln!(prompt);
    let _ = writeln!(prompt, "## Available tools");
    let _ = writeln!(prompt);
    let _ = writeln!(prompt, "- overview: Get the current deliberation overview");
    let _ = writeln!(
        prompt,
        "- claim_detail(claim_id): Examine a specific claim in detail"
    );
    let _ = writeln!(
        prompt,
        "- draft_claim(body, kind, parent?): Propose a new claim"
    );
    let _ = writeln!(
        prompt,
        "- draft_relation(source, target, kind): Add attack/support relation"
    );
    let _ = writeln!(
        prompt,
        "- draft_stance(target, position): Take a position on a claim"
    );
    let _ = writeln!(
        prompt,
        "- draft_resolve(claim, outcome): Propose resolution"
    );
    let _ = writeln!(
        prompt,
        "- draft_comment(body, claim?): Draft a freeform comment"
    );
    let _ = writeln!(prompt, "- show_drafts: See your pending drafts");
    let _ = writeln!(prompt, "- remove_draft(draft_id): Remove a draft");
    let _ = writeln!(
        prompt,
        "- submit_drafts: Submit all drafts to the deliberation"
    );
    let _ = writeln!(prompt, "- clear_drafts: Discard all drafts");
    let _ = writeln!(
        prompt,
        "- preview_overview: Preview state with your drafts applied"
    );
    let _ = writeln!(
        prompt,
        "- preview_claim_detail(claim): Preview a claim with drafts applied"
    );
    let _ = writeln!(
        prompt,
        "- impact_analysis: Compare committed state with your current drafts"
    );

    // Guidelines
    let _ = writeln!(prompt);
    let _ = writeln!(prompt, "## Guidelines");
    let _ = writeln!(prompt);
    let _ = writeln!(
        prompt,
        "- Examine claims before taking stances (use claim_detail)"
    );
    let _ = writeln!(
        prompt,
        "- Address attention signals: unexamined claims need stances, contested claims need arguments"
    );
    let _ = writeln!(
        prompt,
        "- Draft entries before submitting — review with show_drafts and preview_overview"
    );
    let _ = writeln!(
        prompt,
        "- The active participant is \"{}\"; draft authorship is injected automatically",
        config.participant_name
    );
    let _ = writeln!(
        prompt,
        "- Draft-local claim references use draft IDs; committed claim references use claim IDs"
    );
    let _ = writeln!(
        prompt,
        "- Explain your reasoning to the user before and after using tools"
    );

    // Custom instructions
    if !config.custom_instructions.is_empty() {
        let _ = writeln!(prompt);
        let _ = write!(prompt, "{}", config.custom_instructions);
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::types::*;

    fn test_config() -> PromptConfig {
        PromptConfig {
            participant_name: "assistant".into(),
            role_description: "You are an AI facilitator helping a team reach consensus.".into(),
            custom_instructions: String::new(),
        }
    }

    #[test]
    fn includes_role_and_participant() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let prompt = build_system_prompt(&engine, &test_config());
        assert!(prompt.contains("AI facilitator"));
        assert!(prompt.contains("\"assistant\""));
    }

    #[test]
    fn includes_deliberation_state() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let prompt = build_system_prompt(&engine, &test_config());
        assert!(prompt.contains("Current deliberation state"));
        assert!(prompt.contains("0 claims"));
    }

    #[test]
    fn includes_tool_listing() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let prompt = build_system_prompt(&engine, &test_config());
        assert!(prompt.contains("Available tools"));
        assert!(prompt.contains("draft_claim"));
        assert!(prompt.contains("submit_drafts"));
        assert!(prompt.contains("preview_overview"));
    }

    #[test]
    fn includes_custom_instructions() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let config = PromptConfig {
            custom_instructions: "Focus on security concerns.".into(),
            ..test_config()
        };
        let prompt = build_system_prompt(&engine, &config);
        assert!(prompt.contains("Focus on security concerns."));
    }

    #[test]
    fn omits_custom_section_when_empty() {
        let engine = ConsensusEngine::new(String::from("assistant"));
        let prompt = build_system_prompt(&engine, &test_config());
        // The prompt should end with the guidelines, no trailing custom section
        assert!(!prompt.contains("Focus on"));
    }

    #[test]
    fn populated_engine_shows_counts() {
        let mut engine = ConsensusEngine::new(String::from("assistant"));
        engine.append(Entry::Claim {
            claim_id: ClaimId("p1".into()),
            author: "alice".into(),
            body: "Use JWT".into(),
            claim_kind: ClaimKind::Proposal,
            parent_id: None,
        });
        engine.append(Entry::Stance {
            target_id: ClaimId("p1".into()),
            author: "bob".into(),
            position: Position::Consent,
        });

        let prompt = build_system_prompt(&engine, &test_config());
        assert!(prompt.contains("1 claims"));
        assert!(prompt.contains("1 stances"));
        assert!(prompt.contains("Active proposals: 1"));
    }
}
