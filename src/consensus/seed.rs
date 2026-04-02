//! Pure entry parsing for seed/fixture data.
//!
//! Accepts JSON in several shapes (raw array, `{ "entries": [...] }`, or bundle
//! objects with extra metadata fields) and produces a `Vec<Entry>`.

use serde_json::Value;

use crate::consensus::types::Entry;

#[derive(Debug, thiserror::Error)]
pub enum SeedParseError {
    #[error("invalid fixture format: {0}")]
    InvalidFixture(String),
}

/// Parse a JSON string containing seed entries.
pub fn load_entries_from_str(input: &str) -> Result<Vec<Entry>, SeedParseError> {
    let value: Value =
        serde_json::from_str(input).map_err(|e| SeedParseError::InvalidFixture(e.to_string()))?;
    load_entries_from_value(value)
}

/// Parse a JSON value containing seed entries.
///
/// Accepts three shapes:
/// - A raw JSON array of entry objects
/// - An object with a top-level `"entries"` array (e.g. session response or bundle)
/// - Any object containing an `"entries"` key alongside other metadata fields
pub fn load_entries_from_value(value: Value) -> Result<Vec<Entry>, SeedParseError> {
    let raw_entries = match value {
        Value::Array(entries) => entries,
        Value::Object(mut object) => match object.remove("entries") {
            Some(Value::Array(entries)) => entries,
            Some(_) => {
                return Err(SeedParseError::InvalidFixture(String::from(
                    "top-level `entries` must be an array",
                )));
            }
            None => {
                return Err(SeedParseError::InvalidFixture(String::from(
                    "expected a raw entry array or an object with top-level `entries`",
                )));
            }
        },
        _ => {
            return Err(SeedParseError::InvalidFixture(String::from(
                "expected a JSON array or object",
            )));
        }
    };

    raw_entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            serde_json::from_value(entry).map_err(|e| {
                SeedParseError::InvalidFixture(format!("entry #{index} failed to parse: {e}"))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::fixtures::{
        FixtureScenario, authentication_deliberation_log, scenario_log,
    };

    #[test]
    fn load_entries_accepts_session_response_shape() {
        let fixture = serde_json::json!({
            "entries": authentication_deliberation_log().entries,
            "total": 53,
        });
        let parsed = load_entries_from_value(fixture).unwrap();
        assert_eq!(parsed.len(), 53);
    }

    #[test]
    fn load_entries_accepts_bundle_shape() {
        let fixture = serde_json::json!({
            "scenario_id": "authentication-deliberation",
            "entries": authentication_deliberation_log().entries,
            "final_overview_text": "ignored",
        });
        let parsed = load_entries_from_value(fixture).unwrap();
        assert_eq!(parsed.len(), 53);
    }

    #[test]
    fn load_entries_accepts_raw_array_shape() {
        let entries = authentication_deliberation_log().entries;
        let parsed = load_entries_from_value(serde_json::to_value(&entries).unwrap()).unwrap();
        assert_eq!(parsed, entries);
    }

    #[test]
    fn load_entries_rejects_invalid_shape() {
        let error = load_entries_from_value(serde_json::json!({"nope": []})).unwrap_err();
        assert!(matches!(error, SeedParseError::InvalidFixture(_)));
    }

    #[test]
    fn fixture_scenarios_and_seed_loader_share_same_entry_shape() {
        let log = scenario_log(FixtureScenario::AuthenticationDeliberation);
        let parsed = load_entries_from_value(serde_json::json!({
            "entries": log.entries.clone(),
        }))
        .unwrap();
        assert_eq!(parsed, log.entries);
    }
}
