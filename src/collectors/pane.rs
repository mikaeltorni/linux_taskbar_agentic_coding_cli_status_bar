//! tmux pane fallback: parse rendered `/status` / `/usage` TUI text.

use std::sync::OnceLock;

use chrono::{Datelike, Duration, Local, TimeZone, Timelike};
use regex::Regex;
use serde_json::{json, Value};

use super::util::{now_epoch, to_percent};
use crate::logging;
use crate::snapshot::{snapshot, Window};

struct WindowKeyword {
    label: &'static str,
    pattern: &'static str,
    /// How long the window itself lasts, in seconds, when that is known.
    ///
    /// A reset can never be further away than one full window, so this bounds
    /// what a parsed reset moment is allowed to claim. `None` means the window
    /// length is unknown and no bound is applied.
    max_seconds: Option<i64>,
}

/// Grace added to a window length before a parsed reset is called impossible.
///
/// Pane text is rendered to whole minutes and can be captured a moment after
/// the CLI drew it, so a genuine "resets in one full window" reading may land
/// slightly beyond the exact window length.
const RESET_HORIZON_SLACK_SECONDS: i64 = 180;

const FIVE_HOUR_SECONDS: Option<i64> = Some(5 * 3600);
const WEEK_SECONDS: Option<i64> = Some(7 * 86400);
const MONTH_SECONDS: Option<i64> = Some(31 * 86400);

fn pane_window_keywords(cli_id: &str) -> &'static [WindowKeyword] {
    match cli_id {
        "codex" => &[
            WindowKeyword {
                label: "5h",
                pattern: r"(?i)5h(?:our)?\s+limit",
                max_seconds: FIVE_HOUR_SECONDS,
            },
            WindowKeyword {
                label: "week",
                pattern: r"(?i)week(?:ly)?\s+limit",
                max_seconds: WEEK_SECONDS,
            },
            WindowKeyword {
                label: "month",
                pattern: r"(?i)month(?:ly)?\s+limit",
                max_seconds: MONTH_SECONDS,
            },
        ],
        "claude_code" => &[
            WindowKeyword {
                label: "5h",
                pattern: r"(?i)current\s+session|5-?h(?:our)?\s+limit",
                max_seconds: FIVE_HOUR_SECONDS,
            },
            WindowKeyword {
                label: "week",
                pattern: r"(?i)current\s+week|week(?:ly)?\s+limit",
                max_seconds: WEEK_SECONDS,
            },
            WindowKeyword {
                label: "fable",
                pattern: r"(?i)(?:current\s+week[^\n]*\b)?fable(?:\s+only)?",
                max_seconds: WEEK_SECONDS,
            },
        ],
        "grok" => &[WindowKeyword {
            label: "week",
            pattern: r"(?i)week(?:ly)?\s+limit|credits?|usage",
            max_seconds: WEEK_SECONDS,
        }],
        "opencode_go" => &[
            WindowKeyword {
                label: "rolling",
                pattern: r"(?i)\brolling\b",
                max_seconds: None,
            },
            WindowKeyword {
                label: "week",
                pattern: r"(?i)\bweekly\b",
                max_seconds: WEEK_SECONDS,
            },
            WindowKeyword {
                label: "month",
                pattern: r"(?i)\bmonthly\b",
                max_seconds: MONTH_SECONDS,
            },
        ],
        _ => &[],
    }
}

fn re(pattern: &'static str) -> &'static Regex {
    compiled_regex(pattern)
}

fn compiled_regex(pattern: &'static str) -> &'static Regex {
    static MAP: OnceLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static Regex>>,
    > = OnceLock::new();
    let map = MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(existing) = guard.get(pattern) {
        return existing;
    }
    let compiled = Box::leak(Box::new(Regex::new(pattern).expect("static regex")));
    guard.insert(pattern, compiled);
    compiled
}

const PANE_PERCENT: &str = r"(\d+(?:\.\d+)?)\s*%";
const CODEX_RESETS_AVAILABLE: &str = r"(?i)\b(\d+)\s+(?:usage\s+limit\s+)?resets?\s+available\b";
const PANE_RESET: &str = r"(?i)\bresets?\b\s*(?:at\s+)?(.+)$";
const PANE_RESET_CLOCK: &str = r"\b([01]?\d|2[0-3]):([0-5]\d)\b";
const PANE_RESET_DATE: &str = r"(?i)\b(\d{1,2})\s+([A-Za-z]{3})";
const PANE_RESET_IN: &str = r"(?i)\bresets?\s+in\s+(?:(\d+)d\s*)?(?:(\d+)h\s*)?(?:(\d+)m)?";
const PANE_PERIOD_END: &str = r"(?i)\bperiod\s+ends?\b\s*(.+)$";
const LEFT_WORD: &str = r"(?i)\bleft\b";
const DAY_MONTH: &str = r"(?i)\b(\d{1,2})\s+([A-Za-z]{3})\b";
const MONTH_DAY: &str = r"(?i)\b([A-Za-z]{3})\s+(\d{1,2})\b";

fn month_number(abbr: &str) -> Option<u32> {
    match abbr.to_ascii_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn local_from_timestamp(now: f64) -> chrono::DateTime<Local> {
    Local
        .timestamp_opt(now as i64, 0)
        .single()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).single().unwrap())
}

fn pane_reset_duration_epoch(line: &str, now: f64) -> Option<i64> {
    let caps = re(PANE_RESET_IN).captures(line)?;
    if caps.get(1).is_none() && caps.get(2).is_none() && caps.get(3).is_none() {
        return None;
    }
    let days: i64 = caps
        .get(1)
        .map(|m| m.as_str().parse().unwrap_or(0))
        .unwrap_or(0);
    let hours: i64 = caps
        .get(2)
        .map(|m| m.as_str().parse().unwrap_or(0))
        .unwrap_or(0);
    let minutes: i64 = caps
        .get(3)
        .map(|m| m.as_str().parse().unwrap_or(0))
        .unwrap_or(0);
    let total = days * 86400 + hours * 3600 + minutes * 60;
    if total <= 0 {
        return None;
    }
    Some(now as i64 + total)
}

fn pane_period_end_epoch(line: &str, now: f64) -> Option<i64> {
    let caps = re(PANE_PERIOD_END).captures(line)?;
    let mut descriptor = caps.get(1)?.as_str().trim().to_string();
    if descriptor.ends_with(')') {
        descriptor.pop();
    }
    if let Some(duration) = pane_reset_duration_epoch(&format!("resets {descriptor}"), now) {
        return Some(duration);
    }
    let (day, month_abbr) = if let Some(dm) = re(DAY_MONTH).captures(&descriptor) {
        (
            dm.get(1)?.as_str().parse::<u32>().ok()?,
            dm.get(2)?.as_str().to_string(),
        )
    } else {
        let md = re(MONTH_DAY).captures(&descriptor)?;
        (
            md.get(2)?.as_str().parse::<u32>().ok()?,
            md.get(1)?.as_str().to_string(),
        )
    };
    let month = month_number(&month_abbr)?;
    let base = local_from_timestamp(now);
    let candidate = base
        .with_month(month)?
        .with_day(day)?
        .with_hour(23)?
        .with_minute(59)?
        .with_second(59)?
        .with_nanosecond(0)?;
    let mut candidate = candidate;
    if candidate.timestamp() < now as i64 - 86400 {
        candidate = candidate.with_year(candidate.year() + 1)?;
    }
    Some(candidate.timestamp())
}

/// Whether a parsed reset moment can belong to a window of `max_seconds`.
///
/// A window resets at most one window length from now, so anything further out
/// came from a mis-read line (or an invented day rollover) and is dropped.
/// Moments already in the past stay plausible: they mean the window rolled over
/// while the pane text aged, which consumers render as replenished.
fn reset_within_window(epoch: i64, now: f64, max_seconds: Option<i64>) -> bool {
    match max_seconds {
        Some(max) => epoch - now as i64 <= max + RESET_HORIZON_SLACK_SECONDS,
        None => true,
    }
}

/// Parse a "resets HH:MM" clock descriptor into an epoch.
///
/// A bare clock time carries no date, so a time that already passed today
/// normally means tomorrow. That inference is only valid when tomorrow still
/// fits inside the window: a 5h window cannot reset 24h from now. When it does
/// not fit, the pane text is simply older than the reset it names, so the
/// already-passed moment is returned as-is and the window counts as reset.
///
/// * `line` - One pane line, expected to contain a `resets ...` descriptor.
/// * `now` - Epoch seconds the pane text was observed at.
/// * `max_seconds` - Window length bounding the rollover, when known.
fn pane_reset_epoch(line: &str, now: f64, max_seconds: Option<i64>) -> Option<i64> {
    let reset = re(PANE_RESET).captures(line)?;
    let descriptor = reset.get(1)?.as_str();
    let clock = re(PANE_RESET_CLOCK).captures(descriptor)?;
    let hour: u32 = clock.get(1)?.as_str().parse().ok()?;
    let minute: u32 = clock.get(2)?.as_str().parse().ok()?;
    let base = local_from_timestamp(now);
    if let Some(day) = re(PANE_RESET_DATE).captures(descriptor) {
        let month = month_number(day.get(2)?.as_str())?;
        let day_num: u32 = day.get(1)?.as_str().parse().ok()?;
        let candidate = base
            .with_month(month)?
            .with_day(day_num)?
            .with_hour(hour)?
            .with_minute(minute)?
            .with_second(0)?
            .with_nanosecond(0)?;
        let mut candidate = candidate;
        if candidate.timestamp() < now as i64 - 86400 {
            candidate = candidate.with_year(candidate.year() + 1)?;
        }
        return Some(candidate.timestamp());
    }
    let mut candidate = base
        .with_hour(hour)?
        .with_minute(minute)?
        .with_second(0)?
        .with_nanosecond(0)?;
    if candidate.timestamp() <= now as i64 {
        let rolled = candidate + Duration::days(1);
        if reset_within_window(rolled.timestamp(), now, max_seconds) {
            candidate = rolled;
        } else {
            logging::debug(format!(
                "pane reset {} already passed and a day rollover exceeds the window; \
treating the window as reset",
                candidate.format("%H:%M")
            ));
        }
    }
    Some(candidate.timestamp())
}

fn compiled_keyword(pattern: &'static str) -> &'static Regex {
    compiled_regex(pattern)
}

/// Find the reset moment printed near a window's usage line.
///
/// * `lines` - Pane text split into lines.
/// * `keyword` - Regex identifying the window's own line.
/// * `stop_keywords` - Other windows' regexes; scanning stops at them so one
///   window never adopts another's reset.
/// * `now` - Epoch seconds the pane text was observed at.
/// * `max_seconds` - Window length, used to bound the bare-clock rollover
///   inference. Explicit dated descriptors ("period ends Jul 12") are trusted
///   as printed, since they carry their own date and can legitimately sit
///   further out than the nominal window length.
fn pane_window_reset(
    lines: &[&str],
    keyword: &Regex,
    stop_keywords: &[&Regex],
    now: f64,
    max_seconds: Option<i64>,
) -> Option<i64> {
    for (index, line) in lines.iter().enumerate() {
        if !keyword.is_match(line) {
            continue;
        }
        if line.to_ascii_lowercase().contains("gpt-reserve") {
            continue;
        }
        for offset in 0..3 {
            let probe = index + offset;
            if probe >= lines.len() {
                break;
            }
            if offset > 0 && stop_keywords.iter().any(|k| k.is_match(lines[probe])) {
                break;
            }
            if let Some(epoch) = pane_period_end_epoch(lines[probe], now)
                .or_else(|| pane_reset_duration_epoch(lines[probe], now))
                .or_else(|| pane_reset_epoch(lines[probe], now, max_seconds))
            {
                return Some(epoch);
            }
        }
        return None;
    }
    None
}

fn pane_window_percent(lines: &[&str], keyword: &Regex) -> Option<f64> {
    for (index, line) in lines.iter().enumerate() {
        if !keyword.is_match(line) {
            continue;
        }
        if line.to_ascii_lowercase().contains("gpt-reserve") {
            continue;
        }
        let mut source_line = *line;
        let mut matched = re(PANE_PERCENT).captures(line);
        if matched.is_none() && index + 1 < lines.len() {
            source_line = lines[index + 1];
            matched = re(PANE_PERCENT).captures(source_line);
        }
        let caps = matched?;
        let percent_val = caps.get(1)?.as_str().parse::<f64>().ok()?;
        let mut percent = to_percent(&json!(percent_val))?;
        let combined = format!("{line} {source_line}");
        if re(LEFT_WORD).is_match(&combined) {
            percent = 100.0 - percent;
        }
        return Some(percent);
    }
    None
}

/// Parse captured tmux pane text into the shared snapshot shape.
pub fn parse_pane(cli_id: &str, text: &str, now: Option<f64>) -> Value {
    let lines: Vec<&str> = text.lines().collect();
    let now = now.unwrap_or_else(now_epoch);
    let keywords = pane_window_keywords(cli_id);
    let compiled: Vec<&Regex> = keywords
        .iter()
        .map(|k| compiled_keyword(k.pattern))
        .collect();
    let mut windows = Vec::new();
    for (idx, spec) in keywords.iter().enumerate() {
        let keyword = compiled[idx];
        let Some(percent) = pane_window_percent(&lines, keyword) else {
            continue;
        };
        let others: Vec<&Regex> = compiled
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != idx)
            .map(|(_, r)| *r)
            .collect();
        windows.push(Window {
            label: spec.label.into(),
            used_percent: percent,
            resets_at: pane_window_reset(&lines, keyword, &others, now, spec.max_seconds),
        });
    }
    let error = if windows.is_empty() {
        Some("no usage percentages found in pane".into())
    } else {
        None
    };
    let observed = if windows.is_empty() { None } else { Some(now) };
    let mut snap = snapshot("tmux_pane", observed, None, windows, error);
    if cli_id == "codex" {
        if let Some(caps) = re(CODEX_RESETS_AVAILABLE).captures(text) {
            if let Ok(count) = caps.get(1).unwrap().as_str().parse::<i64>() {
                snap["resets_available"] = json!(count);
            }
        }
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Local;

    fn local_ts(year: i32, month: u32, day: u32, hour: u32, min: u32) -> f64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, min, 0)
            .single()
            .unwrap()
            .timestamp() as f64
    }

    fn local_ts_hms(year: i32, month: u32, day: u32, hour: u32, min: u32, sec: u32) -> i64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, min, sec)
            .single()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn reads_available_resets_from_usage() {
        let pane = "5h limit: 61% used\nWeekly limit: 22% used\n1 reset available\n";
        let result = parse_pane("codex", pane, Some(1000.0));
        assert_eq!(result["resets_available"], json!(1));
    }

    #[test]
    fn heatmap_without_reset_count_is_not_authoritative() {
        let result = parse_pane(
            "codex",
            "Token activity   last 12 months\n5h limit: 61% used\nWeekly limit: 22% used\n",
            Some(1000.0),
        );
        assert!(result.get("resets_available").is_none());
    }

    #[test]
    fn reads_reset_count_even_when_usage_breakdown_is_missing() {
        let result = parse_pane(
            "codex",
            "You have 2 usage limit resets available.\n",
            Some(1000.0),
        );
        assert_eq!(result["windows"], json!([]));
        assert_eq!(result["resets_available"], json!(2));
    }

    #[test]
    fn codex_status_pane_used_variant() {
        let text = concat!(
            "  /status\n",
            "  Token usage: 32,950 total\n",
            "  5h limit:     [########  ] 57% used (resets 14:30)\n",
            "  Weekly limit: [####      ] 43% used (resets Mon)\n",
        );
        let result = parse_pane("codex", text, Some(5.0));
        assert_eq!(result["observed_at"], json!(5.0));
        let pairs: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(pairs, vec![("5h".into(), 57.0), ("week".into(), 43.0)]);
    }

    #[test]
    fn codex_status_pane_left_variant() {
        let text = concat!(
            "│  5h limit:             [████████████████████] 99% left (resets 20:12) │\n",
            "│  Weekly limit:         [██████████░░░░░░░░░░] 49% left                │\n",
            "│                        (resets 09:10 on 9 Jul)                        │\n",
        );
        let result = parse_pane("codex", text, Some(5.0));
        let pairs: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(pairs, vec![("5h".into(), 1.0), ("week".into(), 51.0)]);
    }

    #[test]
    fn codex_status_pane_monthly_only_when_subscription_inactive() {
        let now = local_ts(2026, 7, 25, 10, 0);
        let text = concat!(
            " Token activity   last 12 months\n",
            " Lifetime 3.02B · Peak 287M\n",
            "│  Account:              user@example.com (Plus)                │\n",
            "│  Monthly limit:        [████████████████████] 100% left              │\n",
            "│                        (resets 10:03 on 24 Aug)                      │\n",
        );
        let result = parse_pane("codex", text, Some(now));
        assert!(result.get("error").unwrap().is_null());
        let pairs: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(pairs, vec![("month".into(), 0.0)]);
        assert_eq!(
            result["windows"][0]["resets_at"],
            json!(local_ts_hms(2026, 8, 24, 10, 3, 0))
        );
    }

    #[test]
    fn codex_status_pane_subscribed_5h_week_ignores_absent_month() {
        let text = concat!(
            "│  5h limit:             [████████████████████] 99% left (resets 20:12) │\n",
            "│  Weekly limit:         [██████████░░░░░░░░░░] 49% left                │\n",
            "│                        (resets 09:10 on 9 Jul)                        │\n",
        );
        let result = parse_pane("codex", text, Some(5.0));
        let labels: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["5h", "week"]);
    }

    #[test]
    fn codex_status_pane_skips_gpt_reserve_and_reads_zero_remaining() {
        let text = concat!(
            "│  gpt-reserve Weekly limit:   [████████████████████] 100% left (resets 14:30 on 5 Sep) │\n",
            "│  5h limit:                   [░░░░░░░░░░░░░░░░░░░░] 0% left (resets 18:34)            │\n",
            "│  Weekly limit:               [█████████████████░░░] 84% left (resets 13:34 on 5 Sep)  │\n",
        );
        let result = parse_pane("codex", text, Some(5.0));
        let pairs: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(pairs, vec![("5h".into(), 100.0), ("week".into(), 16.0)]);
    }

    #[test]
    fn codex_pane_parses_reset_times_into_epochs() {
        let now = local_ts(2026, 7, 4, 17, 25);
        let text = concat!(
            "│  5h limit:      [████████████████████] 99% left (resets 20:12) │\n",
            "│  Weekly limit:  [██████████░░░░░░░░░░] 49% left                │\n",
            "│                 (resets 09:10 on 9 Jul)                        │\n",
        );
        let result = parse_pane("codex", text, Some(now));
        let by_label: std::collections::HashMap<_, _> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["resets_at"].as_i64(),
                )
            })
            .collect();
        assert_eq!(by_label["5h"], Some(local_ts_hms(2026, 7, 4, 20, 12, 0)));
        assert_eq!(by_label["week"], Some(local_ts_hms(2026, 7, 9, 9, 10, 0)));
    }

    #[test]
    fn codex_pane_clock_already_past_rolls_to_tomorrow() {
        // A weekly window is long enough for tomorrow to be a plausible reset.
        let now = local_ts(2026, 7, 4, 21, 0);
        let text = "  Weekly limit: [####      ] 43% used (resets 20:12)\n";
        let result = parse_pane("codex", text, Some(now));
        assert_eq!(
            result["windows"][0]["resets_at"],
            json!(local_ts_hms(2026, 7, 5, 20, 12, 0))
        );
    }

    #[test]
    fn codex_pane_five_hour_reset_never_rolls_a_full_day() {
        // Reproduces the 24h countdown: a stale 5h pane whose reset already
        // passed must not be pushed to tomorrow, because a 5h window cannot
        // reset ~23h from now. It reads as already reset instead.
        let now = local_ts(2026, 7, 4, 21, 0);
        let text = "  5h limit: [########  ] 57% used (resets 20:12)\n";
        let result = parse_pane("codex", text, Some(now));
        let resets_at = result["windows"][0]["resets_at"].as_i64().unwrap();
        assert_eq!(resets_at, local_ts_hms(2026, 7, 4, 20, 12, 0));
        assert!(resets_at - now as i64 <= 5 * 3600 + RESET_HORIZON_SLACK_SECONDS);
    }

    #[test]
    fn codex_pane_five_hour_reset_still_rolls_within_the_window() {
        // 23:40 now, reset 01:15 -> tomorrow, and that is only ~1.5h out, so
        // the rollover is legitimate and must be kept.
        let now = local_ts(2026, 7, 4, 23, 40);
        let text = "  5h limit: [##        ] 20% used (resets 01:15)\n";
        let result = parse_pane("codex", text, Some(now));
        assert_eq!(
            result["windows"][0]["resets_at"],
            json!(local_ts_hms(2026, 7, 5, 1, 15, 0))
        );
    }

    #[test]
    fn codex_pane_day_only_reset_stays_null() {
        let now = local_ts(2026, 7, 4, 17, 25);
        let text = "  Weekly limit: [####      ] 43% used (resets Mon)\n";
        let result = parse_pane("codex", text, Some(now));
        assert!(result["windows"][0]["resets_at"].is_null());
    }

    #[test]
    fn grok_weekly_limit_left_pane() {
        let text = "╰────────────────── Weekly limit left: 0% · Composer 2.5 ─╯\n";
        let result = parse_pane("grok", text, Some(1.0));
        assert_eq!(
            result["windows"],
            json!([{"label": "week", "used_percent": 100.0, "resets_at": null}])
        );
    }

    #[test]
    fn claude_usage_pane() {
        let text = concat!(
            "Current session (5-hour limit)\n",
            "  12% used · resets at 3pm\n",
            "Current week (all models)\n",
            "  40% used · resets Tuesday\n",
        );
        let result = parse_pane("claude_code", text, Some(1.0));
        let pairs: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(pairs, vec![("5h".into(), 12.0), ("week".into(), 40.0)]);
    }

    #[test]
    fn claude_usage_pane_includes_fable() {
        let text = concat!(
            "Current session (5-hour limit)\n",
            "  12% used · resets at 3pm\n",
            "Current week (all models)\n",
            "  40% used · resets Tuesday\n",
            "Current week (Fable only)\n",
            "  62% used · resets Tuesday\n",
        );
        let result = parse_pane("claude_code", text, Some(1.0));
        let pairs: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("5h".into(), 12.0),
                ("week".into(), 40.0),
                ("fable".into(), 62.0)
            ]
        );
    }

    #[test]
    fn grok_usage_pane() {
        let now = local_ts(2026, 7, 4, 12, 0);
        let text = "Credits used this week: 100% (period ends Jul 9)\n";
        let result = parse_pane("grok", text, Some(now));
        assert_eq!(
            result["windows"],
            json!([{
                "label": "week",
                "used_percent": 100.0,
                "resets_at": local_ts_hms(2026, 7, 9, 23, 59, 59)
            }])
        );
    }

    #[test]
    fn grok_weekly_limit_left_with_period_ends() {
        let now = local_ts(2026, 7, 4, 12, 0);
        let text = concat!(
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 ",
            "(period ends Jul 12) ─╯\n",
        );
        let result = parse_pane("grok", text, Some(now));
        assert_eq!(result["windows"][0]["used_percent"], json!(55.0));
        assert_eq!(
            result["windows"][0]["resets_at"],
            json!(local_ts_hms(2026, 7, 12, 23, 59, 59))
        );
    }

    #[test]
    fn grok_period_ends_in_duration() {
        let now = 1_000_000.0;
        let text = "Weekly limit left: 10% (period ends in 2d 5h)\n";
        let result = parse_pane("grok", text, Some(now));
        assert_eq!(result["windows"][0]["used_percent"], json!(90.0));
        assert_eq!(
            result["windows"][0]["resets_at"],
            json!(now as i64 + 2 * 86400 + 5 * 3600)
        );
    }

    #[test]
    fn unparseable_pane_reports_error() {
        let result = parse_pane("codex", "welcome to codex\n", Some(1.0));
        assert_eq!(result["windows"], json!([]));
        assert!(result["error"].as_str().unwrap().contains("no usage"));
    }

    #[test]
    fn opencode_go_ogc_usage_pane() {
        let text = concat!(
            "rolling  0%  resets in 4h 57m\n",
            "weekly   17% resets in 2d 17h\n",
            "monthly  8%  resets in 29d 23h\n",
        );
        let result = parse_pane("opencode_go", text, Some(1_000.0));
        let labels: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                ("rolling".into(), 0.0),
                ("week".into(), 17.0),
                ("month".into(), 8.0)
            ]
        );
    }
}
