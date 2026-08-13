//! Grok collector: billing records in `~/.grok/logs/unified.jsonl`.

use std::path::Path;

use serde_json::{json, Value};

use super::util::{tail_lines, to_epoch, to_percent, TAIL_READ_BYTES};
use crate::logging;
use crate::snapshot::{snapshot, Window};

const GROK_BALANCE_EXHAUSTED: &str = "balance exhausted";

/// Collect Grok credit usage from the newest CLI billing log record.
pub fn collect_grok(home: &Path) -> Value {
    let logs_dir = home.join(".grok").join("logs");
    if !logs_dir.is_dir() {
        return snapshot(
            "grok_logs",
            None,
            None,
            Vec::new(),
            Some("no ~/.grok/logs directory".into()),
        );
    }
    let mut log_files: Vec<_> = match std::fs::read_dir(&logs_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect(),
        Err(_) => Vec::new(),
    };
    log_files.sort_by_key(|p| {
        std::cmp::Reverse(
            p.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
                .to_bits(),
        )
    });
    for log_file in log_files.into_iter().take(3) {
        if let Some(found) = grok_last_billing(&log_file) {
            return found;
        }
    }
    snapshot(
        "grok_logs",
        None,
        None,
        Vec::new(),
        Some("no billing records found".into()),
    )
}

fn grok_last_billing(log_file: &Path) -> Option<Value> {
    let mut last_ctx: Option<Value> = None;
    let mut last_billing_ts: Option<i64> = None;
    let mut last_exhaust_ts: Option<i64> = None;
    let lines = match tail_lines(log_file, TAIL_READ_BYTES) {
        Ok(lines) => lines,
        Err(exc) => {
            logging::error(format!(
                "cannot read grok log {}: {exc}",
                log_file.display()
            ));
            return None;
        }
    };
    for line in lines {
        let record: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ctx = record.get("ctx").cloned().unwrap_or(json!({}));
        if ctx.get("config").map(Value::is_object).unwrap_or(false)
            && line.contains("fetched credits config")
        {
            last_ctx = Some(ctx);
            last_billing_ts = record.get("ts").and_then(to_epoch);
            continue;
        }
        if let Some(error) = ctx.get("error").and_then(Value::as_str) {
            if error.to_lowercase().contains(GROK_BALANCE_EXHAUSTED) {
                if let Some(exhaust_ts) = record.get("ts").and_then(to_epoch) {
                    if last_exhaust_ts
                        .map(|prev| exhaust_ts > prev)
                        .unwrap_or(true)
                    {
                        last_exhaust_ts = Some(exhaust_ts);
                    }
                }
            }
        }
    }
    let last_ctx = last_ctx?;
    let billing = last_ctx.get("config")?;
    let mut used = if billing.get("creditUsagePercent").is_some() {
        to_percent(billing.get("creditUsagePercent").unwrap_or(&Value::Null))?
    } else {
        0.0
    };
    let period = billing.get("currentPeriod");
    let mut resets_at = period.and_then(|p| p.get("end")).and_then(to_epoch);
    if resets_at.is_none() {
        resets_at = billing.get("billingPeriodEnd").and_then(to_epoch);
    }
    let mtime = log_file
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut observed = last_billing_ts.map(|t| t as f64).unwrap_or(mtime);
    let exhausted = last_exhaust_ts
        .map(|ex| last_billing_ts.map(|bill| ex > bill).unwrap_or(true))
        .unwrap_or(false);
    if exhausted {
        used = 100.0;
        observed = last_exhaust_ts.unwrap() as f64;
        logging::info(format!(
            "grok credit balance exhausted (402 at {:?} newer than last billing fetch at {:?}); reporting 0% left until the next billing fetch",
            last_exhaust_ts, last_billing_ts
        ));
    }
    Some(snapshot(
        "grok_logs",
        Some(observed),
        last_ctx.get("subscriptionTier").cloned(),
        vec![Window {
            label: "week".into(),
            used_percent: used,
            resets_at,
        }],
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        grok_balance_exhausted_record, grok_billing_record, grok_billing_record_no_percent,
        write_grok_log, Sandbox, GROK_BILLING_PERIOD_END,
    };
    use serde_json::json;

    #[test]
    fn reads_last_billing_record() {
        let sandbox = Sandbox::new();
        write_grok_log(
            &sandbox.home,
            "grok.log",
            &[grok_billing_record(10.0), grok_billing_record(88.0)],
        );
        let result = collect_grok(&sandbox.home);
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(result["plan"], json!("SuperGrok"));
        assert_eq!(result["windows"][0]["used_percent"], json!(88.0));
        assert_eq!(result["windows"][0]["label"], json!("week"));
    }

    #[test]
    fn billing_period_end_from_current_period() {
        let sandbox = Sandbox::new();
        write_grok_log(&sandbox.home, "grok.log", &[grok_billing_record(50.0)]);
        let result = collect_grok(&sandbox.home);
        assert_eq!(
            result["windows"][0]["resets_at"],
            json!(GROK_BILLING_PERIOD_END)
        );
    }

    #[test]
    fn post_reset_record_without_percent_is_0_used() {
        let sandbox = Sandbox::new();
        write_grok_log(
            &sandbox.home,
            "grok.log",
            &[grok_billing_record_no_percent()],
        );
        let result = collect_grok(&sandbox.home);
        assert_eq!(result["windows"][0]["used_percent"], json!(0.0));
        assert!(result.get("error").unwrap().is_null());
    }

    #[test]
    fn missing_logs_dir_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_grok(&sandbox.home);
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("no ~/.grok/logs"));
    }

    #[test]
    fn no_billing_records_reports_error() {
        let sandbox = Sandbox::new();
        write_grok_log(&sandbox.home, "grok.log", &[json!({"msg": "hello"})]);
        let result = collect_grok(&sandbox.home);
        assert_eq!(result["error"], json!("no billing records found"));
    }

    #[test]
    fn balance_exhausted_after_billing_reports_zero_left() {
        let sandbox = Sandbox::new();
        write_grok_log(
            &sandbox.home,
            "grok.log",
            &[
                grok_billing_record(98.0),
                grok_balance_exhausted_record("2026-07-23T05:14:23.977Z"),
            ],
        );
        let result = collect_grok(&sandbox.home);
        assert_eq!(result["windows"][0]["used_percent"], json!(100.0));
    }

    #[test]
    fn billing_after_exhaust_supersedes_it() {
        let sandbox = Sandbox::new();
        write_grok_log(
            &sandbox.home,
            "grok.log",
            &[
                grok_billing_record(98.0),
                grok_balance_exhausted_record("2026-07-03T11:00:00Z"),
                grok_billing_record_at_ts(5.0, "2026-07-03T12:00:00Z"),
            ],
        );
        let result = collect_grok(&sandbox.home);
        assert_eq!(result["windows"][0]["used_percent"], json!(5.0));
    }

    fn grok_billing_record_at_ts(percent: f64, ts: &str) -> Value {
        crate::test_support::grok_billing_record_at(percent, ts, "SuperGrok")
    }

    #[test]
    fn exhausted_without_billing_anchor_reports_no_records() {
        let sandbox = Sandbox::new();
        write_grok_log(
            &sandbox.home,
            "grok.log",
            &[grok_balance_exhausted_record("2026-07-23T05:14:23.977Z")],
        );
        let result = collect_grok(&sandbox.home);
        assert_eq!(result["error"], json!("no billing records found"));
    }

    #[test]
    fn only_post_reset_records_still_reports_window() {
        let sandbox = Sandbox::new();
        write_grok_log(
            &sandbox.home,
            "unified.jsonl",
            &[grok_billing_record_no_percent()],
        );
        let result = collect_grok(&sandbox.home);
        assert_eq!(result["windows"].as_array().unwrap().len(), 1);
        assert_eq!(result["windows"][0]["used_percent"], json!(0.0));
    }
}
