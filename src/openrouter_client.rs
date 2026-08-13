//! OpenRouter analytics client using a management API key.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::CONFIG_DIR_NAME;
use crate::http::{HttpOpener, HttpRequest, UreqOpener};
use crate::logging;

pub const OPENROUTER_ANALYTICS_QUERY_API: &str = "https://openrouter.ai/api/v1/analytics/query";
pub const ENV_VAR_NAME: &str = "OPENROUTER_MANAGEMENT_API_KEY";
pub const ENV_FILE_NAME: &str = ".env";

/// Return the OpenRouter secrets `.env` path under XDG config.
pub fn env_file_path(home: Option<&Path>) -> PathBuf {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        if !base.is_empty() {
            return PathBuf::from(base)
                .join(CONFIG_DIR_NAME)
                .join(ENV_FILE_NAME);
        }
    }
    let root = home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"))
        })
        .join(".config");
    root.join(CONFIG_DIR_NAME).join(ENV_FILE_NAME)
}

fn parse_env_file(path: &Path) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return values;
    };
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') || !stripped.contains('=') {
            continue;
        }
        let (key, value) = stripped.split_once('=').unwrap();
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut value = value.trim().to_string();
        if value.len() >= 2 {
            let bytes = value.as_bytes();
            if (bytes[0] == b'"' || bytes[0] == b'\'') && bytes[0] == bytes[bytes.len() - 1] {
                value = value[1..value.len() - 1].to_string();
            }
        }
        values.insert(key.to_string(), value);
    }
    values
}

/// Return the configured management API key, if any.
pub fn management_api_key(home: Option<&Path>) -> Option<String> {
    if let Ok(override_key) = std::env::var(ENV_VAR_NAME) {
        let trimmed = override_key.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let path = env_file_path(home);
    if !path.is_file() {
        return None;
    }
    parse_env_file(&path)
        .get(ENV_VAR_NAME)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Return inclusive-exclusive UTC ISO bounds for one calendar day.
pub fn analytics_day_range(date_str: &str) -> Option<(String, String)> {
    let start = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()?;
    let start_dt = start.and_hms_opt(0, 0, 0)?.and_utc();
    let end_dt = start_dt + chrono::Duration::days(1);
    Some((
        start_dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        end_dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    ))
}

/// Fetch per-model request rows for one UTC day from the analytics API.
pub fn fetch_analytics_rows(
    api_key: &str,
    date_str: &str,
    model_slug: &str,
    timeout: Duration,
    opener: Option<&dyn HttpOpener>,
) -> Result<Vec<Value>, String> {
    let (start, end) =
        analytics_day_range(date_str).ok_or_else(|| "invalid analytics date".to_string())?;
    let mut body = json!({
        "metrics": ["request_count"],
        "dimensions": ["model", "variant"],
        "time_range": {"start": start, "end": end},
        "limit": 100,
    });
    if model_slug.trim().ends_with(":free") {
        body["filters"] = json!([{"field": "variant", "operator": "eq", "value": "free"}]);
    }
    let request = HttpRequest {
        url: OPENROUTER_ANALYTICS_QUERY_API.into(),
        method: "POST".into(),
        headers: vec![
            ("Authorization".into(), format!("Bearer {api_key}")),
            ("Accept".into(), "application/json".into()),
            ("Content-Type".into(), "application/json".into()),
        ],
        body: Some(serde_json::to_vec(&body).unwrap_or_default()),
        timeout,
    };
    logging::info(format!(
        "fetching OpenRouter analytics for {date_str} (management key)"
    ));
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    let response = opener.open(&request).map_err(|exc| {
        if let Some(status) = exc.status {
            logging::error(format!(
                "OpenRouter analytics HTTP {status}: {}",
                String::from_utf8_lossy(&exc.body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ));
            format!("OpenRouter analytics HTTP {status}")
        } else {
            format!("OpenRouter analytics request failed: {exc}")
        }
    })?;
    if response.status >= 400 {
        logging::error(format!(
            "OpenRouter analytics HTTP {}: {}",
            response.status,
            response.text().chars().take(200).collect::<String>()
        ));
        return Err(format!("OpenRouter analytics HTTP {}", response.status));
    }
    let payload: Value = serde_json::from_str(&response.text())
        .map_err(|_| "OpenRouter analytics response is not JSON".to_string())?;
    if !payload.is_object() {
        return Err("OpenRouter analytics response root is not an object".into());
    }
    let data = payload
        .get("data")
        .filter(|v| v.is_object())
        .ok_or_else(|| "OpenRouter analytics response missing data object".to_string())?;
    let rows = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "OpenRouter analytics response missing data rows".to_string())?;
    Ok(rows.iter().filter(|row| row.is_object()).cloned().collect())
}
