//! Trivial tool that returns the current UTC time.
//! Used primarily for testing the tool use loop mechanics.

use serde_json::{Value, json};

use super::{Tool, ToolDefinition};

pub struct GetCurrentTime;

impl Tool for GetCurrentTime {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "get_current_time".to_owned(),
            description: "Get the current date and time in UTC".to_owned(),
            parameters: json!({"type": "object", "properties": {}}),
        }
    }

    fn execute(&self, _arguments: Value) -> Result<String, super::ToolError> {
        let now = std::time::SystemTime::now();
        let duration = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();

        // Basic UTC formatting without chrono dependency.
        let days = secs / 86400;
        let time_secs = secs % 86400;
        let hours = time_secs / 3600;
        let minutes = (time_secs % 3600) / 60;
        let seconds = time_secs % 60;

        // Days since Unix epoch to year/month/day (simplified civil calendar).
        let (year, month, day) = days_to_date(days);

        Ok(format!(
            "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z"
        ))
    }
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
fn days_to_date(days_since_epoch: u64) -> (u64, u64, u64) {
    // Algorithm from Howard Hinnant's civil_from_days.
    let z = days_since_epoch + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_iso8601_utc_string() {
        let tool = GetCurrentTime;
        let result = tool.execute(serde_json::json!({})).expect("should succeed");
        // Should match pattern like "2026-03-16T12:34:56Z"
        assert!(result.ends_with('Z'), "should end with Z: {result}");
        assert_eq!(result.len(), 20, "ISO 8601 length: {result}");
        assert_eq!(&result[4..5], "-");
        assert_eq!(&result[7..8], "-");
        assert_eq!(&result[10..11], "T");
        assert_eq!(&result[13..14], ":");
        assert_eq!(&result[16..17], ":");
    }

    #[test]
    fn definition_has_correct_name() {
        let tool = GetCurrentTime;
        let def = tool.definition();
        assert_eq!(def.name, "get_current_time");
    }

    #[test]
    fn days_to_date_epoch() {
        assert_eq!(days_to_date(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_date_known_date() {
        // 2026-03-16 is day 20528 since epoch
        assert_eq!(days_to_date(20528), (2026, 3, 16));
    }
}
