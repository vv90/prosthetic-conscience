//! Deterministic consensus log fixtures for offline experiments.
//!
//! These fixtures are designed to look like realistic deliberation histories
//! while remaining valid against the implemented MVP `Entry` schema.

use std::str::FromStr;

use clap::ValueEnum;
use serde::Serialize;

use super::engine::ConsensusEngine;
use super::format::format_overview;
use super::render::OverviewData;
use super::types::{ClaimId, ClaimKind, Entry, Outcome, Position, RelationKind};

/// Named built-in fixture scenarios.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FixtureScenario {
    AuthenticationDeliberation,
}

impl FixtureScenario {
    pub fn slug(self) -> &'static str {
        match self {
            Self::AuthenticationDeliberation => "authentication-deliberation",
        }
    }
}

impl std::fmt::Display for FixtureScenario {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.slug())
    }
}

impl FromStr for FixtureScenario {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "authentication-deliberation" => Ok(Self::AuthenticationDeliberation),
            other => Err(format!("unknown fixture scenario: {other}")),
        }
    }
}

/// A portable deliberation log fixture suitable for LLM experiments.
#[derive(Debug, Clone, Serialize)]
pub struct TrialLog {
    pub scenario_id: String,
    pub title: String,
    pub description: String,
    pub participants: Vec<String>,
    pub entries: Vec<Entry>,
}

impl TrialLog {
    /// Materialized overview of the final committed state.
    pub fn final_overview(&self) -> OverviewData {
        let mut engine = ConsensusEngine::new(String::new());
        for entry in &self.entries {
            engine.append(entry.clone());
        }
        engine.overview()
    }

    /// Human-readable overview of the final committed state.
    pub fn final_overview_text(&self) -> String {
        format_overview(&self.final_overview())
    }
}

/// A non-trivial authentication deliberation adapted from the consensus docs.
pub fn authentication_deliberation_log() -> TrialLog {
    TrialLog {
        scenario_id: String::from("authentication-deliberation"),
        title: String::from("Authentication Approach Deliberation"),
        description: String::from(
            "Four participants work through competing authentication proposals, \
             exchange factual and value claims, revise direction after objections, \
             and converge on a hybrid proposal with explicit resolution entries.",
        ),
        participants: vec![
            String::from("alice"),
            String::from("bob"),
            String::from("carol"),
            String::from("dave"),
        ],
        entries: vec![
            claim(
                "item-auth",
                "carol",
                "Decide the authentication approach for the public API and the admin dashboard before beta.",
                ClaimKind::Item,
            ),
            comment(
                Some("item-auth"),
                "carol",
                "We need one decision that operations can actually support in the first release.",
            ),
            claim(
                "ref-oidc-practice",
                "carol",
                "External OIDC providers give us mature MFA, recovery, and federation flows that are expensive to reproduce internally.",
                ClaimKind::Reference,
            ),
            claim(
                "fact-public-api-clients",
                "bob",
                "The public API already has a browser SPA client and the mobile app will need token-based access soon.",
                ClaimKind::Fact,
            ),
            claim(
                "fact-no-shared-session-store",
                "dave",
                "We do not have a low-latency shared session store across regions in the current platform.",
                ClaimKind::Fact,
            ),
            claim(
                "value-fast-revocation",
                "alice",
                "We should prefer an approach that lets us revoke compromised access quickly during incident response.",
                ClaimKind::Value,
            ),
            proposal(
                "prop-jwt",
                "bob",
                "Use self-issued JWT access tokens for both the public API and the admin dashboard.",
                "item-auth",
            ),
            proposal(
                "prop-session",
                "alice",
                "Use HTTP-only session cookies backed by our application for both the admin dashboard and browser clients.",
                "item-auth",
            ),
            proposal(
                "prop-oidc",
                "carol",
                "Use a third-party OIDC provider for all authentication and rely on its hosted flows.",
                "item-auth",
            ),
            supports("fact-public-api-clients", "prop-jwt", "bob"),
            supports("ref-oidc-practice", "prop-oidc", "carol"),
            attacks("fact-no-shared-session-store", "prop-session", "dave"),
            claim(
                "fact-jwt-revocation-hard",
                "alice",
                "Self-issued JWTs are hard to revoke before expiry unless we also run a denylist or token introspection service.",
                ClaimKind::Fact,
            ),
            attacks("fact-jwt-revocation-hard", "prop-jwt", "alice"),
            claim(
                "cond-revocation-gap",
                "alice",
                "If disabled users keep valid tokens for too long, the security team loses a practical way to contain account compromise.",
                ClaimKind::Conditional,
            ),
            supports("fact-jwt-revocation-hard", "cond-revocation-gap", "alice"),
            supports("value-fast-revocation", "cond-revocation-gap", "alice"),
            attacks("cond-revocation-gap", "prop-jwt", "alice"),
            stance("prop-jwt", "bob", Position::Support),
            stance("prop-jwt", "alice", Position::Block),
            comment(
                Some("prop-jwt"),
                "alice",
                "I could only support self-issued JWTs if we also commit to near-immediate revocation for disabled or compromised accounts.",
            ),
            claim(
                "fact-admin-browser-only",
                "dave",
                "The admin dashboard is used only by employees in browsers, so cookie-based UX is acceptable there.",
                ClaimKind::Fact,
            ),
            supports("fact-admin-browser-only", "prop-session", "dave"),
            stance("prop-session", "carol", Position::Object),
            comment(
                Some("prop-session"),
                "carol",
                "Regional failover becomes risky if the session store turns into an implicit control plane dependency.",
            ),
            claim(
                "fact-external-idp-cost",
                "bob",
                "A third-party identity provider adds vendor cost, tenant setup overhead, and a migration burden for the team.",
                ClaimKind::Fact,
            ),
            attacks("fact-external-idp-cost", "prop-oidc", "bob"),
            stance("prop-oidc", "bob", Position::Object),
            stance("prop-oidc", "alice", Position::Support),
            comment(
                Some("item-auth"),
                "carol",
                "Splitting workforce auth from public API auth may resolve the apparent tradeoff instead of forcing one mechanism everywhere.",
            ),
            resolve("prop-oidc", "carol", Outcome::Withdrawn),
            proposal(
                "prop-hybrid",
                "carol",
                "Use an external OIDC provider for workforce and admin auth, issue provider-backed JWTs for the public API, and keep a thin adapter layer so we can swap vendors later.",
                "item-auth",
            ),
            supports("ref-oidc-practice", "prop-hybrid", "carol"),
            supports("fact-public-api-clients", "prop-hybrid", "bob"),
            supports("fact-no-shared-session-store", "prop-hybrid", "dave"),
            claim(
                "fact-adapter-abstraction",
                "carol",
                "A narrow identity adapter keeps provider-specific code out of the application so a later migration stays feasible.",
                ClaimKind::Fact,
            ),
            supports("fact-adapter-abstraction", "prop-hybrid", "carol"),
            stance("prop-hybrid", "bob", Position::Consent),
            stance("prop-hybrid", "alice", Position::Champion),
            claim(
                "fact-provider-outage-risk",
                "dave",
                "A provider outage could lock administrators out during an incident unless we preserve a documented break-glass path.",
                ClaimKind::Fact,
            ),
            attacks("fact-provider-outage-risk", "prop-hybrid", "dave"),
            stance("prop-hybrid", "dave", Position::Object),
            comment(
                Some("prop-hybrid"),
                "dave",
                "I like the direction, but I need a concrete outage story before I can consent.",
            ),
            claim(
                "cond-break-glass",
                "alice",
                "If we keep two audited local break-glass admin accounts outside the provider, the outage risk becomes operationally acceptable.",
                ClaimKind::Conditional,
            ),
            attacks("cond-break-glass", "fact-provider-outage-risk", "alice"),
            supports("cond-break-glass", "prop-hybrid", "alice"),
            comment(
                Some("prop-hybrid"),
                "bob",
                "This preserves mobile-friendly tokens without asking us to invent our own authentication stack from scratch.",
            ),
            stance("prop-hybrid", "dave", Position::Consent),
            stance("prop-hybrid", "carol", Position::Support),
            resolve("prop-jwt", "carol", Outcome::Rejected),
            resolve("prop-session", "carol", Outcome::Tabled),
            resolve("prop-hybrid", "carol", Outcome::Accepted),
            comment(
                Some("item-auth"),
                "carol",
                "Decision: move forward with the hybrid plan, document vendor exit constraints, and track break-glass controls in the rollout checklist.",
            ),
        ],
    }
}

pub fn scenario_log(scenario: FixtureScenario) -> TrialLog {
    match scenario {
        FixtureScenario::AuthenticationDeliberation => authentication_deliberation_log(),
    }
}

fn claim(id: &str, author: &str, body: &str, kind: ClaimKind) -> Entry {
    Entry::Claim {
        claim_id: ClaimId(String::from(id)),
        author: String::from(author),
        body: String::from(body),
        claim_kind: kind,
        parent_id: None,
    }
}

fn proposal(id: &str, author: &str, body: &str, parent_id: &str) -> Entry {
    Entry::Claim {
        claim_id: ClaimId(String::from(id)),
        author: String::from(author),
        body: String::from(body),
        claim_kind: ClaimKind::Proposal,
        parent_id: Some(ClaimId(String::from(parent_id))),
    }
}

fn supports(source_id: &str, target_id: &str, author: &str) -> Entry {
    relation(source_id, target_id, RelationKind::Supports, author)
}

fn attacks(source_id: &str, target_id: &str, author: &str) -> Entry {
    relation(source_id, target_id, RelationKind::Attacks, author)
}

fn relation(source_id: &str, target_id: &str, kind: RelationKind, author: &str) -> Entry {
    Entry::Relation {
        source_id: ClaimId(String::from(source_id)),
        target_id: ClaimId(String::from(target_id)),
        kind,
        author: String::from(author),
    }
}

fn stance(target_id: &str, author: &str, position: Position) -> Entry {
    Entry::Stance {
        target_id: ClaimId(String::from(target_id)),
        author: String::from(author),
        position,
    }
}

fn resolve(claim_id: &str, author: &str, outcome: Outcome) -> Entry {
    Entry::Resolve {
        claim_id: ClaimId(String::from(claim_id)),
        author: String::from(author),
        outcome,
    }
}

fn comment(claim_id: Option<&str>, author: &str, body: &str) -> Entry {
    Entry::Comment {
        claim_id: claim_id.map(|id| ClaimId(String::from(id))),
        author: String::from(author),
        body: String::from(body),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::consensus::reducer::replay;

    #[test]
    fn fixture_scenario_slug_and_lookup_are_stable() {
        let scenario = FixtureScenario::AuthenticationDeliberation;
        assert_eq!(scenario.slug(), "authentication-deliberation");
        assert_eq!(
            <FixtureScenario as std::str::FromStr>::from_str("authentication-deliberation")
                .unwrap(),
            scenario
        );
        assert_eq!(
            scenario_log(scenario).scenario_id,
            "authentication-deliberation"
        );
    }

    #[test]
    fn authentication_fixture_is_non_trivial_and_replayable() {
        let log = authentication_deliberation_log();

        assert!(log.entries.len() >= 45);

        let state = replay(&log.entries);
        assert_eq!(state.claims.len(), 16);
        assert_eq!(state.relations.len(), 16);
        assert_eq!(state.stances.len(), 9);

        let overview = log.final_overview();
        assert_eq!(overview.participants.len(), 4);
        assert_eq!(overview.items.len(), 1);
        assert_eq!(overview.resolved.len(), 4);
        assert!(
            overview
                .resolved
                .iter()
                .any(|claim| claim.id.0 == "prop-hybrid"
                    && matches!(
                        claim
                            .resolution
                            .as_ref()
                            .map(|resolution| resolution.outcome),
                        Some(Outcome::Accepted)
                    ))
        );
    }

    #[test]
    fn authentication_fixture_covers_major_entry_shapes() {
        let log = authentication_deliberation_log();

        let mut kinds = BTreeSet::new();
        let mut has_attack = false;
        let mut has_support = false;
        let mut comment_count = 0;

        for entry in &log.entries {
            match entry {
                Entry::Claim { claim_kind, .. } => {
                    kinds.insert(*claim_kind as u8);
                }
                Entry::Relation { kind, .. } => match kind {
                    RelationKind::Attacks => has_attack = true,
                    RelationKind::Supports => has_support = true,
                },
                Entry::Comment { .. } => comment_count += 1,
                Entry::Stance { .. } | Entry::Resolve { .. } => {}
            }
        }

        assert_eq!(kinds.len(), 6);
        assert!(has_attack);
        assert!(has_support);
        assert!(comment_count >= 5);
    }

    #[test]
    fn authentication_fixture_round_trips_through_json() {
        let log = authentication_deliberation_log();
        let json = serde_json::to_string(&log.entries).unwrap();
        let parsed: Vec<Entry> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, log.entries);
    }
}
