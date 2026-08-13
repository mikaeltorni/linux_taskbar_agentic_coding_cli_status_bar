//! Shared collector helpers: epoch/percent normalization and bounded tail reads.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

/// How much of the end of a large session/log file to read per tick.
pub const TAIL_READ_BYTES: u64 = 262_144;

/// Normalize an epoch integer or ISO-8601 string to an epoch integer.
pub fn to_epoch(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(raw) => parse_iso_epoch(raw),
        _ => None,
    }
}

/// Parse an ISO-8601 / RFC3339 timestamp to a UTC epoch second.
pub fn parse_iso_epoch(raw: &str) -> Option<i64> {
    let normalized = raw.replace('Z', "+00:00");
    DateTime::parse_from_rfc3339(&normalized)
        .ok()
        .map(|dt| dt.timestamp())
        .or_else(|| {
            // Python fromisoformat accepts space separators and missing offsets.
            let candidate = if normalized.contains(' ') {
                normalized.replace(' ', "T")
            } else {
                normalized
            };
            DateTime::parse_from_rfc3339(&candidate)
                .ok()
                .map(|dt| dt.timestamp())
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(&candidate, "%Y-%m-%dT%H:%M:%S%.f")
                        .ok()
                        .map(|naive| naive.and_utc().timestamp())
                })
        })
}

/// Normalize a numeric percentage, clamping to the 0-100 range.
pub fn to_percent(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(n) => n.as_f64(),
        _ => None,
    }?;
    Some(number.clamp(0.0, 100.0))
}

/// Return the last complete lines of a file, reading at most `max_bytes`.
pub fn tail_lines(path: &Path, max_bytes: u64) -> std::io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size > max_bytes {
        file.seek(SeekFrom::Start(size - max_bytes))?;
        let mut skip = [0u8; 1];
        // Drop the (likely) partial first line, matching Python's readline().
        loop {
            let n = file.read(&mut skip)?;
            if n == 0 || skip[0] == b'\n' {
                break;
            }
        }
    }
    let mut data = String::new();
    file.read_to_string(&mut data)?;
    Ok(data.split('\n').map(str::to_string).collect())
}

/// Current UTC wall time as an epoch float (Python `time.time()`).
pub fn now_epoch() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// UTC calendar date for `now` as `YYYY-MM-DD`.
pub fn utc_date_string(now: f64) -> String {
    DateTime::<Utc>::from_timestamp(now as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap())
        .format("%Y-%m-%d")
        .to_string()
}

/// Epoch of the next 00:00 UTC boundary after `now`.
pub fn next_utc_midnight_epoch(now: f64) -> i64 {
    let current = DateTime::<Utc>::from_timestamp(now as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap());
    let next = current
        .date_naive()
        .succ_opt()
        .unwrap_or(current.date_naive());
    next.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()
}
