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

/// Return windows from a snapshot, or an empty slice.
pub fn windows_of(snapshot: &Value) -> &[Value] {
    snapshot
        .get("windows")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[])
}
