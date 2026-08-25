//! OpenRouter: `/api/v1/analytics/query` (management API key from installer setup).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::util::{next_utc_midnight_epoch, now_epoch, to_percent, utc_date_string};
use crate::config::{
    OPENROUTER_DAILY_LIMIT_DEFAULT, OPENROUTER_FETCH_MIN_INTERVAL_SECONDS,
    OPENROUTER_MODEL_SLUG_DEFAULT,
};
use crate::logging;
use crate::openrouter_client::{fetch_analytics_rows, management_api_key};
use crate::snapshot::{carry_last_reading, snapshot, Window};

/// Return whether an activity row's model matches the configured slug.
pub fn model_matches_slug(item_model: &str, model_slug: &str) -> bool {
    if item_model.is_empty() || model_slug.is_empty() {
        return false;
    }
    if item_model == model_slug {
        return true;
    }
    let slug = model_slug.trim();
    let model = item_model.trim();
    if model.to_lowercase().contains(&slug.to_lowercase()) {
        return true;
    }
    let base = slug.split(':').next().unwrap_or(slug);
    model.starts_with(base) || model.contains(base)
}

fn activity_row_date(item: &Value) -> String {
    item.get("date")
        .and_then(Value::as_str)
        .filter(|raw| raw.len() >= 10)
        .map(|raw| raw[..10].to_string())
        .unwrap_or_default()
}

/// Sum `request_count` analytics rows for `model_slug`.
pub fn openrouter_requests_from_analytics(rows: &[Value], model_slug: &str) -> i64 {
    let mut total = 0;
    for item in rows {
        let model = item.get("model").and_then(Value::as_str).unwrap_or("");
        if !model_matches_slug(model, model_slug) {
            continue;
        }
        if let Some(count) = item.get("request_count").and_then(Value::as_i64) {
            if count >= 0 {
                total += count;
            }
        } else if let Some(count) = item.get("request_count").and_then(Value::as_str) {
            if count.chars().all(|c| c.is_ascii_digit()) {
                total += count.parse::<i64>().unwrap_or(0);
            }
        }
    }
    total
}

/// Build the daily usage window from a request count.
pub fn openrouter_window_from_requests(
    requests_today: i64,
    now: f64,
    daily_limit: i64,
) -> Vec<Window> {
    let used = if daily_limit <= 0 {
        100.0
    } else {
        to_percent(&json!(requests_today as f64 / daily_limit as f64 * 100.0)).unwrap_or(0.0)
    };
    vec![Window {
        label: "day".into(),
        used_percent: used,
        resets_at: Some(next_utc_midnight_epoch(now)),
    }]
}

/// Sum `requests` for `model_slug` on one UTC activity day (legacy helper).
pub fn openrouter_requests_today(payload: &Value, model_slug: &str, date_str: &str) -> i64 {
    let mut total = 0;
    if let Some(items) = payload.get("data").and_then(Value::as_array) {
        for item in items {
            if !item.is_object() {
                continue;
            }
            if activity_row_date(item) != date_str {
                continue;
            }
            let model = item
                .get("model")
                .and_then(Value::as_str)
                .or_else(|| item.get("model_permaslug").and_then(Value::as_str))
                .unwrap_or("");
            if !model_matches_slug(model, model_slug) {
                continue;
            }
            if let Some(requests) = item.get("requests").and_then(Value::as_i64) {
                if requests >= 0 {
                    total += requests;
                }
            }
        }
    }
    total
}

pub type OpenRouterFetch =
    Arc<dyn Fn(&str, &str, &str) -> Result<Vec<Value>, String> + Send + Sync>;

/// Collect OpenRouter free-model daily usage from the analytics API.
pub fn collect_openrouter(
    home: &Path,
    last_result: Option<&Value>,
    now: Option<f64>,
    min_interval: Option<f64>,
    model_slug: Option<&str>,
    daily_limit: Option<i64>,
    fetch_fn: Option<OpenRouterFetch>,
) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let min_interval = min_interval.unwrap_or(OPENROUTER_FETCH_MIN_INTERVAL_SECONDS as f64);
    if let Some(last) = last_result {
        if let Some(checked) = last.get("checked_at").and_then(Value::as_f64) {
            if now - checked < min_interval {
                return last.clone();
            }
        }
    }
    let slug = model_slug
        .unwrap_or(OPENROUTER_MODEL_SLUG_DEFAULT)
        .trim()
        .to_string();
    let limit = daily_limit.unwrap_or(OPENROUTER_DAILY_LIMIT_DEFAULT);
    let Some(api_key) = management_api_key(Some(home)) else {
        let mut snap = snapshot(
            "openrouter_activity",
            None,
            None,
            Vec::new(),
            Some(
                "OpenRouter management API key unavailable; run openrouter_monitor configure"
                    .into(),
            ),
        );
        snap["checked_at"] = json!(now);
        return carry_last_reading(snap, last_result);
    };
    let date_str = utc_date_string(now);
    let rows = if let Some(fetch) = fetch_fn {
        fetch(&api_key, &date_str, &slug)
    } else {
        fetch_analytics_rows(&api_key, &date_str, &slug, Duration::from_secs(60), None)
    };
    match rows {
        Ok(rows) => {
            let requests_today = openrouter_requests_from_analytics(&rows, &slug);
            let windows = openrouter_window_from_requests(requests_today, now, limit);
            if windows.is_empty() {
                let mut snap = snapshot(
                    "openrouter_activity",
                    None,
                    None,
                    Vec::new(),
                    Some("no daily usage window from activity response".into()),
                );
                snap["checked_at"] = json!(now);
                carry_last_reading(snap, last_result)
            } else {
                let mut snap = snapshot(
                    "openrouter_activity",
                    Some(now),
                    Some(json!(slug)),
                    windows,
                    None,
                );
                snap["checked_at"] = json!(now);
                snap
            }
        }
        Err(exc) => {
            logging::error(format!("openrouter analytics fetch failed: {exc}"));
            let mut snap = snapshot("openrouter_activity", None, None, Vec::new(), Some(exc));
            snap["checked_at"] = json!(now);
            carry_last_reading(snap, last_result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{openrouter_analytics_fixture, write_openrouter_env, Sandbox};

    #[test]
    fn requests_today_honors_model_slug() {
        let payload = json!({"data": [
            {"date": "2026-07-08", "model": "nvidia/nemotron-3-ultra-550b-a55b:free", "requests": 420},
            {"date": "2026-07-08", "model": "openai/gpt-4.1", "requests": 3},
        ]});
        assert_eq!(
            openrouter_requests_today(
                &payload,
                "nvidia/nemotron-3-ultra-550b-a55b:free",
                "2026-07-08"
            ),
            420
        );
    }

    #[test]
    fn short_slug_matches_ultra_free() {
        assert!(model_matches_slug(
            "nvidia/nemotron-3-ultra-550b-a55b-20260604",
            "nvidia/nemotron-3-ultra-550b-a55b:free"
        ));
    }

    #[test]
    fn used_percent_and_resets_at() {
        let windows = openrouter_window_from_requests(420, 1_783_502_400.0, 1000);
        assert_eq!(windows[0].used_percent, 42.0);
        assert_eq!(windows[0].resets_at, Some(1_783_555_200));
        assert_eq!(windows[0].label, "day");
    }

    #[test]
    fn utc_midnight_boundary_before_and_after() {
        assert_eq!(next_utc_midnight_epoch(1_783_555_199.0), 1_783_555_200);
        assert_eq!(next_utc_midnight_epoch(1_783_555_200.0), 1_783_641_600);
    }

    #[test]
    fn requests_from_analytics_rows() {
        assert_eq!(
            openrouter_requests_from_analytics(
                &openrouter_analytics_fixture(),
                "stealth/ox-alpha"
            ),
            420
        );
    }

    #[test]
    fn collect_openrouter_with_fixture_fetch() {
        let sandbox = Sandbox::new();
        write_openrouter_env(&sandbox.home);
        let fetch: OpenRouterFetch = Arc::new(|_, _, _| Ok(openrouter_analytics_fixture()));
        let result = collect_openrouter(
            &sandbox.home,
            None,
            Some(1_783_502_400.0),
            None,
            None,
            None,
            Some(fetch),
        );
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(result["windows"][0]["used_percent"], json!(42.0));
        assert_eq!(result["windows"][0]["resets_at"], json!(1_783_555_200_i64));
        assert_eq!(result["plan"], json!("stealth/ox-alpha"));
    }

    #[test]
    fn throttles_within_fetch_interval() {
        let sandbox = Sandbox::new();
        write_openrouter_env(&sandbox.home);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let calls_ref = Arc::new(calls);
        let c1 = calls_ref.clone();
        let fetch: OpenRouterFetch = Arc::new(move |_, _, _| {
            c1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(openrouter_analytics_fixture())
        });
        let first = collect_openrouter(
            &sandbox.home,
            None,
            Some(1_783_502_400.0),
            None,
            None,
            None,
            Some(fetch.clone()),
        );
        let second = collect_openrouter(
            &sandbox.home,
            Some(&first),
            Some(1_783_502_459.0),
            None,
            None,
            None,
            Some(fetch),
        );
        assert_eq!(second["windows"][0]["used_percent"], json!(42.0));
        assert_eq!(calls_ref.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn missing_key_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_openrouter(&sandbox.home, None, Some(1.0), None, None, None, None);
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("management API key"));
    }

    #[test]
    fn fetch_failure_does_not_raise() {
        let sandbox = Sandbox::new();
        write_openrouter_env(&sandbox.home);
        let fetch: OpenRouterFetch = Arc::new(|_, _, _| Err("boom".into()));
        let result = collect_openrouter(
            &sandbox.home,
            None,
            Some(1.0),
            None,
            None,
            None,
            Some(fetch),
        );
        assert_eq!(result["error"], json!("boom"));
    }

    #[test]
    fn activity_row_date_handles_timestamp() {
        assert_eq!(
            activity_row_date(&json!({"date": "2026-07-08T12:00:00Z"})),
            "2026-07-08"
        );
    }
}
