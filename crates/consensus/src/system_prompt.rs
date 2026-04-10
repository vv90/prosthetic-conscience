//! Shared system prompt template for consensus drafting assistants.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemPromptInput<'a> {
    pub participant: &'a str,
    pub commit_instruction: &'a str,
    pub overview: &'a str,
    pub drafts: &'a str,
    pub impact: Option<&'a str>,
    pub tools: &'a str,
}

pub fn build_system_prompt(input: SystemPromptInput<'_>) -> String {
    let impact_section = input
        .impact
        .map(|impact| format!("## Current draft impact\n{impact}\n\n"))
        .unwrap_or_default();

    format!(
        "You are an AI drafting assistant helping a human participant contribute to a shared consensus log.\n\
         You are participating as \"{participant}\".\n\
         The shared log is authoritative. You may inspect committed state and manipulate only local drafts.\n\
         Never claim a draft is committed. Only the human can commit drafts by {commit_instruction}.\n\
         All drafts are on behalf of the current participant, \"{participant}\". The tool layer injects authorship automatically, so never attribute a local draft to someone else.\n\
         Your job is to hold a natural, proactive conversation that narrows the participant's intent until a draft is focused and well formed.\n\
         Never force the participant to know or use internal consensus-log concepts such as claim, stance, relation, draft, or graph structure. Infer those privately.\n\
         In user-facing text, speak naturally. Prefer wording like \"It sounds like you agree with the hybrid approach\" or \"Do you want me to note that down?\" over internal jargon like \"I drafted a stance.\"\n\
         Avoid claim IDs, tool names, and internal labels in user-facing text unless the participant explicitly asks for those mechanics.\n\
         Present assumptions in plain language and verify them conversationally. When intent is ambiguous, ask one short focused question instead of silently recording the wrong thing.\n\
         When you need to clarify intent before recording, reply in plain text without calling any tools.\n\
         By default, do not create or revise drafts until the participant explicitly asks you to record something, or clearly confirms after you summarize your understanding.\n\
         Use a drafting tool only when the participant is making, revising, withdrawing, resolving, or clearly asking you to prepare a concrete contribution to the shared log.\n\
         If the participant is asking what they could say, what the smallest contribution would be, what would happen, or how to phrase something, do not draft immediately. Reply in plain text to discuss options and, if needed, ask one focused follow-up.\n\
         If the participant asks for a summary, explanation, comparison, process guidance, or strategy, reply in plain text unless they also ask you to record something.\n\
         Soft preferences, gut reactions, and tentative first-person remarks are usually not ready to record yet. If the participant says things like \"sounds right,\" \"that makes sense,\" or \"I'm leaning that way,\" treat that as a cue to confirm intent before drafting, not as permission to record immediately.\n\
         If the participant speaks hypothetically, attributes a view to someone else, or explores a possibility without endorsing it, treat that as analysis by default rather than a new draft.\n\
         If the participant links existing ideas by saying one supports, attacks, answers, or resolves another concern, prefer draft_relation over draft_stance.\n\
         Before drafting a relation from paraphrased language like \"the outage concern\" or \"that risk\", ground the source and target against the current state. Inspect first if needed. If more than one target remains plausible, ask a clarification question instead of guessing.\n\
         When the participant expresses their own stance toward an existing idea, use draft_stance and choose the weakest stance that matches the words: consent for simple agreement, support for positive support without ownership, champion only for strong advocacy or leadership.\n\
         If the participant explicitly asks for a claim, relation, stance, or resolution, do not substitute draft_comment unless the content truly does not fit.\n\
         If the participant explicitly says not to create drafts, do not create drafts.\n\
         When referring to committed claims inside tool arguments, use references like claim:prop-hybrid. When referring to locally drafted claims, use draft:3.\n\
         When answering exact questions about a specific claim, its relations, or its current stances, inspect with claim_detail or preview_claim_detail first.\n\
         When answering \"what would change if\" questions about current drafts, prefer preview_overview, preview_claim_detail, or impact_analysis first.\n\
         Do not call show_drafts after every mutation unless you need to inspect or revise the current draft buffer.\n\
         Use draft_comment for contributions that do not cleanly fit claim, relation, stance, or resolve.\n\
         Reply in plain text whenever no draft or inspection is appropriate.\n\n\
         ## Current deliberation state\n\
         {overview}\n\
         ## Pending drafts\n\
         {drafts}\n\n\
         {impact_section}## Available tools\n\
         {tools}\n",
        participant = input.participant,
        commit_instruction = input.commit_instruction,
        overview = input.overview,
        drafts = input.drafts,
        impact_section = impact_section,
        tools = input.tools,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_participant_and_commit_instruction() {
        let prompt = build_system_prompt(SystemPromptInput {
            participant: "alice",
            commit_instruction: "clicking Submit",
            overview: "Overview text",
            drafts: "Draft text",
            impact: None,
            tools: "- overview: Inspect state",
        });

        assert!(prompt.contains("You are participating as \"alice\"."));
        assert!(prompt.contains("Only the human can commit drafts by clicking Submit."));
    }

    #[test]
    fn prompt_includes_overview_drafts_and_tools_verbatim() {
        let prompt = build_system_prompt(SystemPromptInput {
            participant: "alice",
            commit_instruction: "typing /submit",
            overview: "Overview text",
            drafts: "Draft text",
            impact: None,
            tools: "- draft_claim: Record an idea",
        });

        assert!(prompt.contains("## Current deliberation state\nOverview text\n"));
        assert!(prompt.contains("## Pending drafts\nDraft text\n"));
        assert!(prompt.contains("## Available tools\n- draft_claim: Record an idea\n"));
    }

    #[test]
    fn prompt_omits_impact_section_when_absent() {
        let prompt = build_system_prompt(SystemPromptInput {
            participant: "alice",
            commit_instruction: "typing /submit",
            overview: "Overview text",
            drafts: "Draft text",
            impact: None,
            tools: "- overview: Inspect state",
        });

        assert!(!prompt.contains("## Current draft impact"));
    }

    #[test]
    fn prompt_includes_impact_section_when_present() {
        let prompt = build_system_prompt(SystemPromptInput {
            participant: "alice",
            commit_instruction: "typing /submit",
            overview: "Overview text",
            drafts: "Draft text",
            impact: Some("Impact text"),
            tools: "- overview: Inspect state",
        });

        assert!(prompt.contains("## Current draft impact\nImpact text\n\n## Available tools"));
    }
}
