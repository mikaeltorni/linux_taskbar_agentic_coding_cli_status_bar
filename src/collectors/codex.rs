//! Codex collector: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.

use std::path::Path;

use serde_json::{json, Value};

use super::util::{now_epoch, tail_lines, to_epoch, to_percent, TAIL_READ_BYTES};
use crate::logging;
use crate::snapshot::{snapshot, Window};

pub const CODEX_SCAN_DAYS: usize = 7;
pub const CODEX_SAME_RESET_TOLERANCE_SECONDS: i64 = 60;

const WINDOW_DURATION_LABELS: [(f64, &str); 3] =
    [(300.0, "5h"), (10080.0, "week"), (43200.0, "month")];

/// Map a Codex rate-limit window duration to a status-bar label.
pub fn codex_window_label(window_minutes: Option<f64>, key: &str) -> String {
    if let Some(minutes) = window_minutes {
        if minutes > 0.0 {
            let label = WINDOW_DURATION_LABELS
                .iter()
                .min_by(|a, b| {
                    (a.0 - minutes)
                        .abs()
                        .partial_cmp(&(b.0 - minutes).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|item| item.1)
                .unwrap_or("5h");
            return label.to_string();
        }
    }
    if key == "primary" {
        "5h".into()
    } else {
        "week".into()
    }
}

/// Collect Codex rate limits from the newest session rollout file.
pub fn collect_codex(home: &Path, now: Option<f64>) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let sessions = home.join(".codex").join("sessions");
    if !sessions.is_dir() {
        return snapshot(
            "codex_sessions",
            None,
            None,
            Vec::new(),
            Some("no ~/.codex/sessions directory".into()),
        );
    }
    let mut day_dirs: Vec<_> = walk_day_dirs(&sessions);
    day_dirs.sort_by(|a, b| b.as_os_str().cmp(a.as_os_str()));
    for day_dir in day_dirs.into_iter().take(CODEX_SCAN_DAYS) {
        let mut files: Vec<_> = match std::fs::read_dir(&day_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                        .unwrap_or(false)
                })
                .collect(),
            Err(_) => continue,
        };
        files.sort_by(|a, b| b.as_os_str().cmp(a.as_os_str()));
        for rollout in files {
            if let Some(found) = codex_last_rate_limits(&rollout, now) {
                return found;
            }
        }
    }
    snapshot(
        "codex_sessions",
        None,
        None,
        Vec::new(),
        Some("no rate_limits events found".into()),
    )
}

fn walk_day_dirs(sessions: &Path) -> Vec<std::path::PathBuf> {
    let mut days = Vec::new();
    let Ok(years) = std::fs::read_dir(sessions) else {
        return days;
    };
    for year in years.filter_map(|e| e.ok()) {
        let year_path = year.path();
        if !year_path.is_dir() {
            continue;
        }
        let Ok(months) = std::fs::read_dir(&year_path) else {
            continue;
        };
        for month in months.filter_map(|e| e.ok()) {
            let month_path = month.path();
            if !month_path.is_dir() {
                continue;
            }
            let Ok(day_entries) = std::fs::read_dir(&month_path) else {
                continue;
            };
            for day in day_entries.filter_map(|e| e.ok()) {
                let day_path = day.path();
                if day_path.is_dir() {
                    days.push(day_path);
                }
            }
        }
    }
    days
}

#[derive(Clone)]
struct Candidate {
    used_percent: f64,
    resets_at: Option<i64>,
    observed_at: f64,
    order: usize,
}

fn codex_last_rate_limits(rollout: &Path, now: f64) -> Option<Value> {
    let mut candidates: std::collections::BTreeMap<String, Vec<Candidate>> =
        std::collections::BTreeMap::new();
    let mut plan_type: Option<Value> = None;
    let lines = match tail_lines(rollout, TAIL_READ_BYTES) {
        Ok(lines) => lines,
        Err(exc) => {
            logging::error(format!(
                "cannot read codex rollout {}: {exc}",
                rollout.display()
            ));
            return None;
        }
    };
    let mtime = rollout
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    for (order, line) in lines.iter().enumerate() {
        if !line.contains("\"rate_limits\"") {
            continue;
        }
        let event: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let payload = event.get("payload").cloned().unwrap_or(json!({}));
        let limits = match payload.get("rate_limits") {
            Some(Value::Object(map)) => map,
            _ => continue,
        };
        let ts = event.get("timestamp").and_then(to_epoch);
        let observed_at = ts.map(|t| t as f64).unwrap_or(mtime);
        for key in ["primary", "secondary"] {
            let window = match limits.get(key) {
                Some(Value::Object(w)) => w,
                _ => continue,
            };
            let Some(used) = to_percent(window.get("used_percent").unwrap_or(&Value::Null)) else {
                continue;
            };
            let minutes = window.get("window_minutes").and_then(Value::as_f64);
            let label = codex_window_label(minutes, key);
            let reset = window.get("resets_at").and_then(to_epoch);
            let candidate = Candidate {
                used_percent: used,
                resets_at: reset,
                observed_at,
                order,
            };
            let label_candidates = candidates.entry(label).or_default();
            let same_index = label_candidates.iter().position(|existing| {
                reset.is_some()
                    && existing.resets_at.is_some()
                    && (reset.unwrap() - existing.resets_at.unwrap()).abs()
                        <= CODEX_SAME_RESET_TOLERANCE_SECONDS
            });
            match same_index {
                None => label_candidates.push(candidate),
                Some(idx) => {
                    let existing = &label_candidates[idx];
                    if (candidate.observed_at, candidate.order)
                        >= (existing.observed_at, existing.order)
                    {
                        label_candidates[idx] = candidate;
                    }
                }
            }
            if let Some(plan) = limits.get("plan_type") {
                if !plan.is_null() && plan != &json!("") {
                    plan_type = Some(plan.clone());
                }
            }
        }
    }
    let mut selected: Vec<(String, Candidate)> = Vec::new();
    for (label, label_candidates) in &candidates {
        let active: Vec<&Candidate> = label_candidates
            .iter()
            .filter(|c| c.resets_at.map(|r| r as f64 > now).unwrap_or(true))
            .collect();
        let selectable: Vec<&Candidate> = if active.is_empty() {
            label_candidates.iter().collect()
        } else {
            active
        };
        let winner = selectable.into_iter().max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    a.observed_at
                        .partial_cmp(&b.observed_at)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.order.cmp(&b.order))
        })?;
        selected.push((label.clone(), winner.clone()));
    }
    if selected.is_empty() {
        return None;
    }
    let newest_observed = selected
        .iter()
        .map(|(_, c)| c.observed_at)
        .fold(f64::NEG_INFINITY, f64::max);
    let windows: Vec<Window> = selected
        .iter()
        .map(|(label, candidate)| Window {
            label: label.clone(),
            used_percent: candidate.used_percent,
            resets_at: candidate.resets_at,
        })
        .collect();
    let labels: Vec<_> = windows.iter().map(|w| w.label.as_str()).collect();
    logging::debug(format!(
        "codex rate_limits from {} plan={:?} labels={labels:?}",
        rollout.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
        plan_type
    ));
    Some(snapshot(
        "codex_sessions",
        Some(newest_observed),
        plan_type,
        windows,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        codex_token_count_event, codex_token_count_event_at, write_codex_rollout, Sandbox,
    };
    use serde_json::json;

    #[test]
    fn reads_last_rate_limits_event_of_newest_rollout() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/02",
            "rollout-old.jsonl",
            &[codex_token_count_event(10.0, 5.0)],
        );
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-new.jsonl",
            &[
                codex_token_count_event(20.0, 15.0),
                codex_token_count_event(57.0, 43.0),
            ],
        );
        let result = collect_codex(&sandbox.home, None);
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(result["plan"], json!("plus"));
        assert!(result.get("resets_available").is_none());
        let windows = result["windows"].as_array().unwrap();
        assert_eq!(windows[0]["used_percent"], json!(57.0));
        assert_eq!(windows[1]["used_percent"], json!(43.0));
        assert_eq!(
            windows[0],
            json!({"label": "5h", "used_percent": 57.0, "resets_at": 1783080635_i64})
        );
        let observed = result["observed_at"].as_f64().unwrap();
        assert!((observed - 1_783_075_285.0).abs() < 2.0);
    }

    #[test]
    fn reports_most_depleted_window_when_concurrent_5h_windows_interleave() {
        let sandbox = Sandbox::new();
        let events = vec![
            codex_token_count_event_at(25.0, 4.0, "2026-07-03T10:40:00Z", "plus"),
            json!({
                "timestamp": "2026-07-03T10:45:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {"total_token_usage": {"total_tokens": 1}},
                    "rate_limits": {
                        "limit_id": "codex",
                        "primary": {
                            "used_percent": 3.0,
                            "window_minutes": 300,
                            "resets_at": 1783085000_i64,
                        },
                        "secondary": {
                            "used_percent": 1.0,
                            "window_minutes": 10080,
                            "resets_at": 1783579000_i64,
                        },
                        "plan_type": "plus",
                    },
                },
            }),
        ];
        write_codex_rollout(&sandbox.home, "2026/07/03", "rollout-x.jsonl", &events);
        let result = collect_codex(&sandbox.home, None);
        let by_label: std::collections::HashMap<_, _> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (w["label"].as_str().unwrap().to_string(), w.clone()))
            .collect();
        assert_eq!(by_label["5h"]["used_percent"], json!(25.0));
        assert_eq!(by_label["week"]["used_percent"], json!(4.0));
    }

    #[test]
    fn uses_newest_snapshot_when_reset_epochs_describe_same_window() {
        let sandbox = Sandbox::new();
        let events = vec![
            json!({
                "timestamp": "2026-07-09T23:24:02Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "primary": {"used_percent": 100.0, "resets_at": 1783653454_i64},
                        "secondary": {"used_percent": 16.0, "resets_at": 1784240254_i64},
                        "plan_type": "plus",
                    }
                },
            }),
            json!({
                "timestamp": "2026-07-09T23:24:20Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "primary": {"used_percent": 6.0, "resets_at": 1783653479_i64},
                        "secondary": {"used_percent": 1.0, "resets_at": 1784240279_i64},
                        "plan_type": "plus",
                    }
                },
            }),
        ];
        write_codex_rollout(&sandbox.home, "2026/07/10", "rollout-x.jsonl", &events);
        let result = collect_codex(&sandbox.home, None);
        let by_label: std::collections::HashMap<_, _> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (w["label"].as_str().unwrap().to_string(), w.clone()))
            .collect();
        assert_eq!(
            by_label["5h"],
            json!({"label": "5h", "used_percent": 6.0, "resets_at": 1783653479_i64})
        );
        assert_eq!(
            by_label["week"],
            json!({"label": "week", "used_percent": 1.0, "resets_at": 1784240279_i64})
        );
        let observed = result["observed_at"].as_f64().unwrap();
        assert!((observed - 1_783_639_460.0).abs() < 2.0);
    }

    #[test]
    fn ignores_exhausted_window_after_its_reset_has_passed() {
        let sandbox = Sandbox::new();
        let events = vec![
            json!({
                "timestamp": "1970-01-01T00:15:00Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "primary": {"used_percent": 100.0, "resets_at": 950},
                        "secondary": {"used_percent": 48.0, "resets_at": 3000},
                        "plan_type": "plus",
                    }
                },
            }),
            json!({
                "timestamp": "1970-01-01T00:16:40Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "primary": {"used_percent": 8.0, "resets_at": 2000},
                        "secondary": {"used_percent": 49.0, "resets_at": 3000},
                        "plan_type": "plus",
                    }
                },
            }),
        ];
        write_codex_rollout(&sandbox.home, "1970/01/01", "rollout-x.jsonl", &events);
        let result = collect_codex(&sandbox.home, Some(1000.0));
        assert_eq!(
            result["windows"],
            json!([
                {"label": "5h", "used_percent": 8.0, "resets_at": 2000},
                {"label": "week", "used_percent": 49.0, "resets_at": 3000},
            ])
        );
    }

    #[test]
    fn skips_window_missing_used_percent() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-a.jsonl",
            &[json!({
                "timestamp": "2026-07-03T10:41:25Z",
                "payload": {"rate_limits": {"primary": {"window_minutes": 300, "resets_at": 1}}},
            })],
        );
        let result = collect_codex(&sandbox.home, None);
        assert_eq!(result["error"], json!("no rate_limits events found"));
    }

    #[test]
    fn skips_rollouts_without_rate_limits() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-empty.jsonl",
            &[json!({"timestamp": "2026-07-03T10:41:25Z", "payload": {"type": "other"}})],
        );
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-good.jsonl",
            &[codex_token_count_event(11.0, 22.0)],
        );
        let result = collect_codex(&sandbox.home, None);
        assert_eq!(result["windows"][0]["used_percent"], json!(11.0));
    }

    #[test]
    fn missing_sessions_dir_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_codex(&sandbox.home, None);
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("no ~/.codex/sessions"));
    }

    #[test]
    fn malformed_lines_are_ignored() {
        let sandbox = Sandbox::new();
        let day_dir = sandbox
            .home
            .join(".codex")
            .join("sessions")
            .join("2026/07/03");
        std::fs::create_dir_all(&day_dir).unwrap();
        std::fs::write(
            day_dir.join("rollout-a.jsonl"),
            format!(
                "not json\n{}\n",
                serde_json::to_string(&codex_token_count_event(57.0, 43.0)).unwrap()
            ),
        )
        .unwrap();
        let result = collect_codex(&sandbox.home, None);
        assert_eq!(result["windows"][0]["used_percent"], json!(57.0));
    }

    #[test]
    fn free_plan_monthly_primary_labeled_month() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-a.jsonl",
            &[json!({
                "timestamp": "2026-07-03T10:41:25.148Z",
                "payload": {
                    "rate_limits": {
                        "primary": {
                            "used_percent": 12.0,
                            "window_minutes": 43200,
                            "resets_at": 1784998206_i64,
                        },
                        "plan_type": "free",
                    }
                }
            })],
        );
        let result = collect_codex(&sandbox.home, None);
        assert_eq!(
            result["windows"],
            json!([{"label": "month", "used_percent": 12.0, "resets_at": 1784998206_i64}])
        );
    }

    #[test]
    fn plus_plan_keeps_5h_and_week_labels() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(57.0, 43.0)],
        );
        let result = collect_codex(&sandbox.home, None);
        let labels: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["5h", "week"]);
    }

    #[test]
    fn primary_only_week_length_window_labeled_week() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-a.jsonl",
            &[json!({
                "timestamp": "2026-07-03T10:41:25.148Z",
                "payload": {
                    "rate_limits": {
                        "primary": {
                            "used_percent": 93.0,
                            "window_minutes": 10080,
                            "resets_at": 1784998206_i64,
                        },
                        "plan_type": "plus",
                    }
                }
            })],
        );
        let result = collect_codex(&sandbox.home, None);
        assert_eq!(
            result["windows"],
            json!([{"label": "week", "used_percent": 93.0, "resets_at": 1784998206_i64}])
        );
    }

    #[test]
    fn codex_window_label_maps_known_durations() {
        assert_eq!(codex_window_label(Some(300.0), "primary"), "5h");
        assert_eq!(codex_window_label(Some(10080.0), "primary"), "week");
        assert_eq!(codex_window_label(Some(43200.0), "primary"), "month");
        assert_eq!(codex_window_label(None, "primary"), "5h");
        assert_eq!(codex_window_label(None, "secondary"), "week");
    }
}
