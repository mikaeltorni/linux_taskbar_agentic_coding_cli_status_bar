//! Shared collector snapshot shape written into `status.json`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// One rate-limit window inside a CLI snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    pub resets_at: Option<i64>,
}

/// Build the shared collector snapshot dictionary.
pub fn snapshot(
    source: &str,
    observed_at: Option<f64>,
    plan: Option<Value>,
    windows: Vec<Window>,
    error: Option<String>,
) -> Value {
    json!({
        "source": source,
        "observed_at": observed_at,
        "plan": plan,
        "windows": windows,
        "error": error,
    })
}

/// Fill an empty error snapshot with the previous good reading.
pub fn carry_last_reading(mut snapshot: Value, last_result: Option<&Value>) -> Value {
    let empty_windows = snapshot
        .get("windows")
        .and_then(Value::as_array)
        .map(|a| a.is_empty())
        .unwrap_or(true);
    if let Some(last) = last_result {
        if last
            .get("windows")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
            && empty_windows
        {
            snapshot["windows"] = last["windows"].clone();
            snapshot["observed_at"] = last.get("observed_at").cloned().unwrap_or(Value::Null);
            if let Some(credits) = last.get("credits") {
                if !credits.is_null() {
                    snapshot["credits"] = credits.clone();
                }
            }
            if snapshot.get("plan").map(|p| p.is_null()).unwrap_or(true) {
                snapshot["plan"] = last.get("plan").cloned().unwrap_or(Value::Null);
            }
        }
    }
    snapshot
}

/// Copy Codex usage-limit reset counts from a previous snapshot.
///
/// `/usage daily` heatmaps and `/status` panes do not include the count, so a
/// later capture must not erase a count this account already reported.
pub fn carry_reset_metadata(snapshot: Value, last: Option<&Value>) -> Value {
    let Some(last) = last else {
        return snapshot;
    };
    if snapshot.get("resets_available").is_some() {
        return snapshot;
    }
    let mut carried = snapshot;
    for key in ["resets_available", "resets_checked_at", "reset_probe_at"] {
        if let Some(value) = last.get(key) {
            carried[key] = value.clone();
        }
    }
    carried
}

/// Return windows from a snapshot, or an empty slice.
pub fn windows_of(snapshot: &Value) -> &[Value] {
    snapshot
        .get("windows")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carry_reset_metadata_keeps_a_positive_count_when_the_new_reading_has_none() {
        let snapshot = json!({"windows": [{"label": "5h", "used_percent": 1.0}]});
        let last = json!({"resets_available": 2, "resets_checked_at": 40.0});
        let carried = carry_reset_metadata(snapshot, Some(&last));
        assert_eq!(carried["resets_available"], json!(2));
        assert_eq!(carried["resets_checked_at"], json!(40.0));
    }

    #[test]
    fn carry_reset_metadata_does_not_overwrite_a_fresh_count() {
        let snapshot = json!({"resets_available": 1});
        let last = json!({"resets_available": 2});
        let carried = carry_reset_metadata(snapshot, Some(&last));
        assert_eq!(carried["resets_available"], json!(1));
    }
}
