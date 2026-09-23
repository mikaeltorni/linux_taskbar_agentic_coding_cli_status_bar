//! Codex collector: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::util::{now_epoch, tail_lines, to_epoch, to_percent, TAIL_READ_BYTES};
use crate::logging;
use crate::snapshot::{carry_last_reading, carry_reset_metadata, snapshot, Window};

pub const CODEX_INSTANCES_FILE: &str = "codex-instances.json";
pub const CODEX_TRACKER_STATE_DIR: &str = "codex-agent-tracker";

pub const CODEX_SCAN_DAYS: usize = 7;
pub const CODEX_SAME_RESET_TOLERANCE_SECONDS: i64 = 60;
/// Minimum interval between read-only per-account reset-credit reads.
pub const CODEX_RESET_CREDITS_MIN_INTERVAL_SECONDS: f64 = 60.0;

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

/// Collect Codex rate limits from this account's newest observed rollout.
pub fn collect_codex(home: &Path, now: Option<f64>) -> Value {
    collect_codex_at(&home.join(".codex"), now)
}

/// Collect Codex rate limits from one isolated `CODEX_HOME`.
///
/// `codex_home` is the Codex directory itself (for example `~/.codex` or
/// `~/.codex-account-2`), not the user home.
pub fn collect_codex_at(codex_home: &Path, now: Option<f64>) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let sessions = codex_home.join("sessions");
    if !sessions.is_dir() {
        return snapshot(
            "codex_sessions",
            None,
            None,
            Vec::new(),
            Some(format!("no {}/sessions directory", codex_home.display())),
        );
    }
    let mut best: Option<Value> = None;
    for rollout in recent_rollouts(&sessions) {
        let observed = best
            .as_ref()
            .and_then(|snap| snap["observed_at"].as_f64())
            .unwrap_or(0.0);
        if file_mtime(&rollout) < observed {
            break;
        }
        // ACC can hard-link a resumed rollout into another CODEX_HOME. Its
        // inode then contains one login's events under multiple account names.
        let Ok(metadata) = rollout.symlink_metadata() else {
            continue;
        };
        if metadata.file_type().is_symlink() || metadata.nlink() > 1 {
            logging::debug(format!(
                "ignoring shared Codex rollout {}",
                rollout.display()
            ));
            continue;
        }
        if let Some(found) = codex_last_rate_limits(&rollout, now) {
            let found_at = found["observed_at"].as_f64().unwrap_or(0.0);
            if found_at > observed {
                best = Some(found);
            }
        }
    }
    if let Some(found) = best {
        return found;
    }
    snapshot(
        "codex_sessions",
        None,
        None,
        Vec::new(),
        Some("no rate_limits events found".into()),
    )
}

/// Return the ACC tracker state directory that owns `codex-instances.json`.
///
/// Matches Agent Command Center: `CODEX_AGENT_TRACKER_STATE_DIR`, then
/// `AGENT_COMMAND_CENTER_STATE_DIR`, then `~/.local/state/codex-agent-tracker`.
pub fn tracker_state_dir(home: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("CODEX_AGENT_TRACKER_STATE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(dir) = std::env::var("AGENT_COMMAND_CENTER_STATE_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join(CODEX_TRACKER_STATE_DIR);
        }
    }
    home.join(".local")
        .join("state")
        .join(CODEX_TRACKER_STATE_DIR)
}

/// One ChatGPT Codex account instance from ACC's registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexAccountInstance {
    pub id: i64,
    pub home: PathBuf,
    pub label: String,
}

/// Return whether ``codex_home`` has finished ChatGPT login.
///
/// A missing ``auth.json`` means the home is still waiting on OAuth. The
/// poller must not start a ``codex`` TUI there: that TUI opens a browser
/// login every few seconds and keeps doing so after the dedicated ACC
/// login later writes this file.
pub fn codex_home_authenticated(codex_home: &Path) -> bool {
    let authenticated = codex_home.join("auth.json").is_file();
    if !authenticated {
        logging::debug(format!(
            "Codex home {} has no auth.json yet",
            codex_home.display()
        ));
    }
    authenticated
}

/// Load ACC Codex homes and the persisted selected instance id.
///
/// Missing or unreadable registries yield an empty list so the poller keeps
/// the historical single-home `~/.codex` path. `selected` defaults to 1.
pub fn load_codex_account_registry(home: &Path) -> (Vec<CodexAccountInstance>, i64) {
    let instances = load_codex_account_instances(home);
    let path = tracker_state_dir(home).join(CODEX_INSTANCES_FILE);
    let selected = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|parsed| parsed.get("selected").and_then(Value::as_i64))
        .unwrap_or(1);
    let selected = if instances.iter().any(|row| row.id == selected) {
        selected
    } else {
        instances.first().map(|row| row.id).unwrap_or(1)
    };
    (instances, selected)
}

/// Return the ACC-selected Codex home, or ``~/.codex`` when none is registered.
pub fn selected_codex_home(user_home: &Path) -> PathBuf {
    let (rows, selected) = load_codex_account_registry(user_home);
    rows.iter()
        .find(|row| row.id == selected)
        .map(|row| row.home.clone())
        .unwrap_or_else(|| user_home.join(".codex"))
}

/// Load extra ChatGPT Codex homes registered by Agent Command Center.
///
/// Missing or unreadable registries yield an empty list so the poller keeps
/// the historical single-home `~/.codex` path.
pub fn load_codex_account_instances(home: &Path) -> Vec<CodexAccountInstance> {
    let path = tracker_state_dir(home).join(CODEX_INSTANCES_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(exc) => {
            logging::error(format!(
                "cannot read ACC Codex instance registry {}: {exc}",
                path.display()
            ));
            return Vec::new();
        }
    };
    let parsed: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(exc) => {
            logging::error(format!(
                "invalid ACC Codex instance registry {}: {exc}",
                path.display()
            ));
            return Vec::new();
        }
    };
    let Some(rows) = parsed.get("instances").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut instances = Vec::new();
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let Some(home_value) = row.get("home").and_then(Value::as_str) else {
            continue;
        };
        if home_value.is_empty() {
            continue;
        }
        let label = row
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        instances.push(CodexAccountInstance {
            id,
            home: PathBuf::from(home_value),
            label,
        });
    }
    instances.sort_by_key(|row| row.id);
    instances
}

fn instance_snapshot(row: &CodexAccountInstance, collected: Value, selected_id: i64) -> Value {
    let mut snap = json!({
        "id": row.id,
        "label": row.label,
        "home": row.home.to_string_lossy(),
        "selected": row.id == selected_id,
        "source": collected.get("source").cloned().unwrap_or(Value::Null),
        "observed_at": collected.get("observed_at").cloned().unwrap_or(Value::Null),
        "plan": collected.get("plan").cloned().unwrap_or(Value::Null),
        "windows": collected.get("windows").cloned().unwrap_or(json!([])),
        "credits": collected.get("credits").cloned().unwrap_or(Value::Null),
        "error": collected.get("error").cloned().unwrap_or(Value::Null),
    });
    for key in ["resets_available", "resets_checked_at", "reset_probe_at"] {
        if let Some(value) = collected.get(key) {
            snap[key] = value.clone();
        }
    }
    snap
}

fn last_instance_by_id(last: Option<&Value>, id: i64) -> Option<&Value> {
    last.and_then(|snapshot| snapshot.get("instances"))
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("id").and_then(Value::as_i64) == Some(id))
        })
}

/// Recover account identity metadata from the last complete status snapshot.
///
/// The ACC registry can be briefly unavailable while the desktop session is
/// starting. Retaining these rows prevents a transient read failure from
/// collapsing a multi-account panel to the historical single-home shape.
fn last_account_registry(last: Option<&Value>) -> (Vec<CodexAccountInstance>, i64) {
    let selected = last
        .and_then(|snapshot| snapshot.get("selected"))
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let mut rows: Vec<_> = last
        .and_then(|snapshot| snapshot.get("instances"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_i64)?;
            let home = row.get("home").and_then(Value::as_str)?;
            if home.is_empty() {
                return None;
            }
            Some(CodexAccountInstance {
                id,
                home: PathBuf::from(home),
                label: row
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect();
    rows.sort_by_key(|row| row.id);
    (rows, selected)
}

/// Collect every ACC-registered ChatGPT Codex account independently.
///
/// A missing registry returns the historical single-home `~/.codex` snapshot
/// tagged as selected instance 1. One or more registered homes add an
/// `instances` array and a `selected` id. Top-level windows belong to the
/// selected instance so the panel can render `CX2` with that account's usage.
pub fn collect_codex_accounts(home: &Path, now: Option<f64>, last: Option<&Value>) -> Value {
    let (mut rows, mut selected_id) = load_codex_account_registry(home);
    if rows.is_empty() {
        let (previous_rows, previous_selected) = last_account_registry(last);
        if !previous_rows.is_empty() {
            logging::debug(format!(
                "ACC Codex registry unavailable; retaining {} previously tracked instances",
                previous_rows.len()
            ));
            rows = previous_rows;
            selected_id = previous_selected;
        }
    }
    if rows.is_empty() {
        let mut snapshot = collect_codex(home, now);
        snapshot["selected"] = json!(1);
        return snapshot;
    }
    if rows.len() > 1 {
        logging::info(format!(
            "collecting {} ACC Codex account instances (selected={selected_id})",
            rows.len()
        ));
    }
    let mut collected_rows = Vec::new();
    for row in &rows {
        let last_inst = last_instance_by_id(last, row.id);
        let mut snap = collect_codex_at(&row.home, now);
        snap = carry_last_reading(snap, last_inst);
        if let Some(previous) = last_inst {
            let previous_at = previous["observed_at"].as_f64().unwrap_or(0.0);
            let current_at = snap["observed_at"].as_f64().unwrap_or(0.0);
            if previous_at > current_at
                && previous["windows"]
                    .as_array()
                    .is_some_and(|w| !w.is_empty())
            {
                for key in [
                    "source",
                    "observed_at",
                    "windows",
                    "plan",
                    "credits",
                    "error",
                    "pane_checked_at",
                ] {
                    if let Some(value) = previous.get(key) {
                        snap[key] = value.clone();
                    }
                }
            }
        }
        snap = carry_reset_metadata(snap, last_inst);
        collected_rows.push(instance_snapshot(row, snap, selected_id));
    }
    let mut combined = collected_rows
        .iter()
        .find(|row| row.get("id").and_then(Value::as_i64) == Some(selected_id))
        .cloned()
        .unwrap_or_else(|| collected_rows[0].clone());
    if let Some(map) = combined.as_object_mut() {
        map.remove("id");
        map.remove("label");
        map.remove("home");
        map.remove("selected");
    }
    combined["selected"] = json!(selected_id);
    combined["instances"] = Value::Array(collected_rows);
    combined
}

/// Refresh manual-reset availability from Codex's read-only account metadata.
/// The account ID from each home's auth file is checked against the response by
/// `read`, so a selected-login change cannot attach one account's credits to
/// another. Failed reads hide old counts until the next retry.
pub fn refresh_codex_reset_counts(
    mut snapshot: Value,
    user_home: &Path,
    now: f64,
    read: &dyn Fn(&Path, &str) -> Result<i64, String>,
) -> Value {
    fn account_id(codex_home: &Path) -> Option<String> {
        let text = std::fs::read_to_string(codex_home.join("auth.json")).ok()?;
        let auth: Value = serde_json::from_str(&text).ok()?;
        auth.pointer("/tokens/account_id")?
            .as_str()
            .map(str::to_owned)
    }

    fn refresh_one(
        row: &mut Value,
        codex_home: &Path,
        now: f64,
        read: &dyn Fn(&Path, &str) -> Result<i64, String>,
    ) {
        let Some(account_id) = account_id(codex_home) else {
            return;
        };
        if row["reset_probe_at"]
            .as_f64()
            .is_some_and(|last| now - last < CODEX_RESET_CREDITS_MIN_INTERVAL_SECONDS)
        {
            return;
        }
        row["reset_probe_at"] = json!(now);
        match read(codex_home, &account_id) {
            Ok(count) if count >= 0 => {
                let old = row["resets_available"].as_i64();
                row["resets_available"] = json!(count);
                row["resets_checked_at"] = json!(now);
                if old != Some(count) {
                    logging::info(format!(
                        "Codex reset allowance for {} changed from {old:?} to {count}",
                        codex_home.display()
                    ));
                }
            }
            Ok(_) => {
                row["resets_available"] = Value::Null;
                row["resets_checked_at"] = Value::Null;
                logging::warn(format!(
                    "Codex reset availability for {} was negative",
                    codex_home.display()
                ));
            }
            Err(error) => {
                row["resets_available"] = Value::Null;
                row["resets_checked_at"] = Value::Null;
                logging::warn(format!(
                    "Codex reset availability for {}: {error}",
                    codex_home.display()
                ));
            }
        }
    }

    if let Some(rows) = snapshot.get_mut("instances").and_then(Value::as_array_mut) {
        for row in rows {
            if let Some(home) = row.get("home").and_then(Value::as_str).map(PathBuf::from) {
                refresh_one(row, &home, now, read);
            }
        }
        let selected = snapshot["selected"].as_i64();
        if let Some(row) = snapshot["instances"].as_array().and_then(|rows| {
            rows.iter()
                .find(|row| row["id"].as_i64() == selected)
                .cloned()
        }) {
            for key in ["resets_available", "resets_checked_at", "reset_probe_at"] {
                if let Some(value) = row.get(key) {
                    snapshot[key] = value.clone();
                }
            }
        }
    } else {
        refresh_one(&mut snapshot, &user_home.join(".codex"), now, read);
    }
    snapshot
}

/// Return a file's modification time in epoch seconds, or 0 when unknown.
fn file_mtime(path: &Path) -> f64 {
    path.metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|delta| delta.as_secs_f64())
        .unwrap_or(0.0)
}

/// Return rollout files worth reading, most recently written first.
///
/// Codex names a rollout after the moment the session *started*, but keeps
/// appending to it while the session is resumed. Ordering by file name
/// therefore hands the newest limits of a long-lived `ca1` console to a dead
/// session that merely started later, which freezes one account's percentages
/// (and makes two accounts look like they share one reading). Ranking by
/// modification time instead always reaches the session that is actually
/// running. `CODEX_SCAN_DAYS` still bounds the work: only the day directories
/// holding the most recently written rollouts are considered.
///
/// Args:
///     sessions: `CODEX_HOME/sessions` directory for one account.
///
/// Returns:
///     Rollout paths ordered newest modification first.
fn recent_rollouts(sessions: &Path) -> Vec<PathBuf> {
    let mut newest_by_day: HashMap<PathBuf, f64> = HashMap::new();
    let mut files: Vec<(f64, PathBuf, PathBuf)> = Vec::new();
    for day_dir in walk_day_dirs(sessions) {
        let Ok(entries) = std::fs::read_dir(&day_dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let is_rollout = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
                .unwrap_or(false);
            if !is_rollout {
                continue;
            }
            let mtime = file_mtime(&path);
            newest_by_day
                .entry(day_dir.clone())
                .and_modify(|best| {
                    if mtime > *best {
                        *best = mtime;
                    }
                })
                .or_insert(mtime);
            files.push((mtime, day_dir.clone(), path));
        }
    }
    let mut days: Vec<(f64, PathBuf)> = newest_by_day
        .into_iter()
        .map(|(day, mtime)| (mtime, day))
        .collect();
    days.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let kept: HashSet<PathBuf> = days
        .into_iter()
        .take(CODEX_SCAN_DAYS)
        .map(|(_, day)| day)
        .collect();
    files.retain(|(_, day, _)| kept.contains(day));
    files.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.2.cmp(&a.2)));
    files.into_iter().map(|(_, _, path)| path).collect()
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

/// Human-readable reason Codex refused a turn for credit reasons.
///
/// Args:
///     reached_type: Raw `rate_limit_reached_type` from a rollout event.
///
/// Returns:
///     A short phrase, or None when the value is not a credit block.
pub fn credit_block_phrase(reached_type: &str) -> Option<String> {
    match reached_type.trim() {
        "" => None,
        "workspace_member_credits_depleted" => Some("team credits out".into()),
        other if other.contains("credit") => Some("credits out".into()),
        _ => None,
    }
}

/// Read the credit half of one `rate_limits` event.
///
/// Codex reports two limit families per turn: `limit_id: "codex"` carries the
/// 5h/weekly windows, while `limit_id: "premium"` carries the pay-as-you-go
/// credit balance and, when a turn was actually refused,
/// `rate_limit_reached_type`. A window countdown says nothing about a credit
/// block — the window can reset without unblocking the account — so the credit
/// state is published alongside the windows instead of being dropped.
///
/// Args:
///     limits: The `rate_limits` object from one rollout event.
///     observed_at: Epoch seconds the event was written.
///
/// Returns:
///     A publishable credit snapshot, or None when the event carries no
///     credit information at all.
fn credit_state(limits: &serde_json::Map<String, Value>, observed_at: f64) -> Option<Value> {
    let credits = limits.get("credits").and_then(Value::as_object);
    let reached = limits
        .get("rate_limit_reached_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if credits.is_none() && reached.is_none() {
        return None;
    }
    let get = |key: &str| credits.and_then(|map| map.get(key)).cloned();
    // A team subscription without a credit facility emits this premium-family
    // reason when its included allowance runs out. Use its ordinary usage
    // windows, just like a personal subscription; it is not a billing block.
    let reached = if reached == Some("workspace_member_credits_depleted")
        && get("has_credits") == Some(Value::Bool(false))
    {
        None
    } else {
        reached
    };
    Some(json!({
        "has_credits": get("has_credits").unwrap_or(Value::Null),
        "unlimited": get("unlimited").unwrap_or(Value::Null),
        "balance": get("balance").unwrap_or(Value::Null),
        "reached_type": reached.map(Value::from).unwrap_or(Value::Null),
        "blocked": reached.and_then(credit_block_phrase).is_some(),
        "observed_at": observed_at,
    }))
}

fn codex_last_rate_limits(rollout: &Path, now: f64) -> Option<Value> {
    let mut candidates: std::collections::BTreeMap<String, Vec<Candidate>> =
        std::collections::BTreeMap::new();
    let mut plan_type: Option<Value> = None;
    // Newest credit state seen in this rollout, whatever `limit_id` carried it.
    let mut credits: Option<(f64, usize, Value)> = None;
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
        }
        if let Some(plan) = limits.get("plan_type") {
            if !plan.is_null() && plan != &json!("") {
                plan_type = Some(plan.clone());
            }
        }
        if let Some(state) = credit_state(limits, observed_at) {
            let newer = credits
                .as_ref()
                .map(|(seen_at, seen_order, _)| (observed_at, order) >= (*seen_at, *seen_order))
                .unwrap_or(true);
            if newer {
                credits = Some((observed_at, order, state));
            }
        }
    }
    let mut selected: Vec<(String, Candidate)> = Vec::new();
    for (label, label_candidates) in &candidates {
        let active: Vec<&Candidate> = label_candidates
            .iter()
            .filter(|c| c.resets_at.map(|r| r as f64 > now).unwrap_or(true))
            .collect();
        if active.is_empty() {
            // Every reading for this window expired before `now` — typically an
            // idle account whose newest rollout predates a reboot. The window
            // has since rolled over, so the recorded percentage no longer
            // describes it. Publish the reset instead of a stale usage bar with
            // a countdown that already elapsed.
            let expired_at = label_candidates
                .iter()
                .filter_map(|c| c.resets_at)
                .max()
                .map(|r| r as f64);
            if let Some(expired_at) = expired_at {
                logging::debug(format!(
                    "codex window {label} expired at {expired_at}; reporting it as reset"
                ));
                selected.push((
                    label.clone(),
                    Candidate {
                        used_percent: 0.0,
                        resets_at: None,
                        observed_at: label_candidates
                            .iter()
                            .map(|c| c.observed_at)
                            .fold(0.0, f64::max),
                        order: 0,
                    },
                ));
                continue;
            }
        }
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
    let mut result = snapshot(
        "codex_sessions",
        Some(newest_observed),
        plan_type,
        windows,
        None,
    );
    if let Some((_, _, state)) = credits {
        result["credits"] = state;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        codex_token_count_event, codex_token_count_event_at, write_codex_auth, write_codex_rollout,
        Sandbox,
    };
    use serde_json::json;

    /// Wall clock used by the fixtures below: just after their rollout
    /// timestamps and before every `resets_at` they carry, so window
    /// selection is exercised without the readings counting as expired.
    const FIXTURE_NOW: f64 = 1_783_075_290.0;

    #[test]
    fn reset_credit_reads_replace_stale_counts_for_each_account() {
        let sandbox = Sandbox::new();
        let one = sandbox.home.join("one");
        let three = sandbox.home.join("three");
        for (home, account) in [(&one, "account-one"), (&three, "account-three")] {
            std::fs::create_dir_all(home).unwrap();
            std::fs::write(
                home.join("auth.json"),
                json!({"tokens": {"account_id": account}}).to_string(),
            )
            .unwrap();
        }
        let first = json!({"id": 1, "home": one, "resets_available": 2, "resets_checked_at": 10.0});
        let third =
            json!({"id": 3, "home": three, "resets_available": 1, "resets_checked_at": 10.0});
        let snapshot = json!({"selected": 3, "instances": [first, third], "resets_available": 1});
        let calls = std::cell::RefCell::new(Vec::new());
        let read = |home: &Path, id: &str| {
            calls
                .borrow_mut()
                .push((home.to_path_buf(), id.to_string()));
            Ok(if home.ends_with("one") { 1 } else { 0 })
        };
        let fresh = refresh_codex_reset_counts(snapshot, &sandbox.home, 1000.0, &read);
        assert_eq!(fresh["instances"][0]["resets_available"], json!(1));
        assert_eq!(fresh["instances"][1]["resets_available"], json!(0));
        assert_eq!(fresh["resets_available"], json!(0));
        assert_eq!(fresh["instances"][0]["resets_checked_at"], json!(1000.0));
        assert_eq!(calls.borrow().len(), 2);
        assert_eq!(calls.borrow()[0].1, "account-one");
        assert_eq!(calls.borrow()[1].1, "account-three");
        let held = refresh_codex_reset_counts(fresh, &sandbox.home, 1030.0, &read);
        assert_eq!(calls.borrow().len(), 2);
        assert_eq!(held["instances"][1]["resets_available"], json!(0));
    }

    #[test]
    fn failed_reset_read_hides_stale_allowance_until_retry() {
        let sandbox = Sandbox::new();
        let one = sandbox.home.join("one");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::write(
            one.join("auth.json"),
            json!({"tokens": {"account_id": "account-one"}}).to_string(),
        )
        .unwrap();
        let snapshot = json!({"selected": 1, "instances": [{"id": 1, "home": one, "resets_available": 2, "resets_checked_at": 10.0}]});
        let failed = refresh_codex_reset_counts(snapshot, &sandbox.home, 1000.0, &|_, _| {
            Err("offline".into())
        });
        assert!(failed["instances"][0]["resets_available"].is_null());
        assert!(failed["resets_available"].is_null());
        assert_eq!(failed["instances"][0]["reset_probe_at"], json!(1000.0));
    }

    #[test]
    fn windows_whose_reset_already_passed_report_as_reset() {
        // The reboot case: an idle account's newest rollout is older than its
        // own reset moment, so both windows rolled over while the machine was
        // off. The instance must report a reset window, not the pre-reboot
        // percentage with a countdown that already elapsed.
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-before-reboot.jsonl",
            &[codex_token_count_event(57.0, 43.0)],
        );
        // Well past both fixture resets (1783080635 and 1783577400).
        let result = collect_codex(&sandbox.home, Some(1_783_600_000.0));
        let windows = result["windows"].as_array().unwrap();
        for window in windows {
            assert_eq!(window["used_percent"], json!(0.0), "{window}");
            assert!(window["resets_at"].is_null(), "{window}");
        }
    }

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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
    fn newest_observation_wins_when_an_old_rollout_was_touched_later() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-recent.jsonl",
            &[codex_token_count_event_at(
                80.0,
                90.0,
                "2026-07-03T10:41:25Z",
                "plus",
            )],
        );
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-touched.jsonl",
            &[codex_token_count_event_at(
                10.0,
                20.0,
                "2026-07-03T09:41:25Z",
                "plus",
            )],
        );
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
        assert_eq!(result["windows"][0]["used_percent"], json!(80.0));
        assert_eq!(result["windows"][1]["used_percent"], json!(90.0));
    }

    #[test]
    fn prefers_the_resumed_session_still_being_written_over_a_newer_named_one() {
        // A `ca1` console started on 07/02 and is still appending; a short
        // session started on 07/03 finished long ago. Name ordering picks the
        // dead one and freezes the account at 0%.
        let sandbox = Sandbox::new();
        let live = write_codex_rollout(
            &sandbox.home,
            "2026/07/02",
            "rollout-2026-07-02T09-00-00-live.jsonl",
            &[codex_token_count_event(100.0, 16.0)],
        );
        let dead = write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-2026-07-03T10-32-59-dead.jsonl",
            &[codex_token_count_event(0.0, 0.0)],
        );
        let base =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_783_000_000);
        std::fs::File::options()
            .write(true)
            .open(&dead)
            .unwrap()
            .set_modified(base)
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&live)
            .unwrap()
            .set_modified(base + std::time::Duration::from_secs(3_600))
            .unwrap();

        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
        let windows = result["windows"].as_array().unwrap();
        assert_eq!(windows[0]["used_percent"], json!(100.0));
        assert_eq!(windows[1]["used_percent"], json!(16.0));
    }

    #[test]
    fn team_without_credit_facility_keeps_personal_usage_display() {
        // A team login emits a premium depletion event after the included
        // window runs out, even though it has no credit facility.
        let sandbox = Sandbox::new();
        let events = vec![
            json!({
                "timestamp": "2026-07-03T10:40:00Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "limit_id": "codex",
                        "primary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": 1783085000_i64},
                        "secondary": {"used_percent": 16.0, "window_minutes": 10080, "resets_at": 1783579000_i64},
                        "credits": {"has_credits": false, "unlimited": false, "balance": null},
                        "plan_type": "team",
                        "rate_limit_reached_type": null,
                    }
                },
            }),
            json!({
                "timestamp": "2026-07-03T10:41:00Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "limit_id": "premium",
                        "primary": null,
                        "secondary": null,
                        "credits": {"has_credits": false, "unlimited": false, "balance": null},
                        "plan_type": "team",
                        "rate_limit_reached_type": "workspace_member_credits_depleted",
                    }
                },
            }),
        ];
        write_codex_rollout(&sandbox.home, "2026/07/03", "rollout-a.jsonl", &events);
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
        // plan_type now survives even though it arrived on a windowless event.
        assert_eq!(result["plan"], json!("team"));
        assert_eq!(result["credits"]["reached_type"], Value::Null);
        assert_eq!(result["credits"]["blocked"], json!(false));
        assert_eq!(result["windows"][1]["used_percent"], json!(16.0));
        assert_eq!(result["windows"][0]["resets_at"], json!(1783085000_i64));
        assert_eq!(result["credits"]["has_credits"], json!(false));
        assert_eq!(result["windows"][0]["used_percent"], json!(100.0));
    }

    #[test]
    fn actual_credit_facility_still_reports_depletion() {
        let limits = json!({
            "credits": {"has_credits": true, "balance": "0"},
            "rate_limit_reached_type": "workspace_member_credits_depleted"
        });
        let state = credit_state(limits.as_object().unwrap(), FIXTURE_NOW).unwrap();
        assert_eq!(state["blocked"], true);
        assert_eq!(state["reached_type"], "workspace_member_credits_depleted");
    }

    #[test]
    fn reports_credit_balance_without_a_block_for_an_unblocked_login() {
        let sandbox = Sandbox::new();
        let events = vec![json!({
            "timestamp": "2026-07-03T10:40:00Z",
            "type": "event_msg",
            "payload": {
                "rate_limits": {
                    "limit_id": "codex",
                    "primary": {"used_percent": 12.0, "window_minutes": 300, "resets_at": 1783085000_i64},
                    "credits": {"has_credits": false, "unlimited": false, "balance": "0"},
                    "plan_type": "plus",
                    "rate_limit_reached_type": null,
                }
            },
        })];
        write_codex_rollout(&sandbox.home, "2026/07/03", "rollout-b.jsonl", &events);
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
        assert_eq!(result["credits"]["balance"], json!("0"));
        assert_eq!(result["credits"]["reached_type"], json!(null));
        assert_eq!(result["credits"]["blocked"], json!(false));
    }

    #[test]
    fn credit_block_phrase_names_the_plan_that_ran_out() {
        assert_eq!(
            credit_block_phrase("workspace_member_credits_depleted").as_deref(),
            Some("team credits out")
        );
        assert_eq!(
            credit_block_phrase("user_credits_depleted").as_deref(),
            Some("credits out")
        );
        assert_eq!(credit_block_phrase("usage_limit_reached"), None);
        assert_eq!(credit_block_phrase(""), None);
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
        assert_eq!(result["windows"][0]["used_percent"], json!(11.0));
    }

    #[test]
    fn missing_sessions_dir_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("sessions directory"));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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
        let result = collect_codex(&sandbox.home, Some(FIXTURE_NOW));
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

    fn write_registry(home: &Path, instances: &Value, selected: i64) {
        let dir = crate::collectors::codex::tracker_state_dir(home);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(crate::collectors::codex::CODEX_INSTANCES_FILE),
            serde_json::to_string(&json!({
                "version": 1,
                "selected": selected,
                "instances": instances
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_rollout_at(codex_home: &Path, day: &str, name: &str, events: &[Value]) {
        let day_dir = codex_home.join("sessions").join(day);
        std::fs::create_dir_all(&day_dir).unwrap();
        let mut text = String::new();
        for event in events {
            text.push_str(&serde_json::to_string(event).unwrap());
            text.push('\n');
        }
        std::fs::write(day_dir.join(name), text).unwrap();
    }

    #[test]
    fn missing_registry_collects_only_default_home() {
        let sandbox = Sandbox::new();
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(57.0, 43.0)],
        );
        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), None);
        assert!(result.get("instances").is_none());
        assert_eq!(result["selected"], json!(1));
        assert_eq!(result["windows"][0]["used_percent"], json!(57.0));
    }

    #[test]
    fn tracker_state_dir_prefers_codex_agent_tracker_state_dir() {
        let sandbox = Sandbox::new();
        let custom = sandbox.home.join("custom-tracker");
        unsafe {
            std::env::set_var("CODEX_AGENT_TRACKER_STATE_DIR", &custom);
        }
        assert_eq!(tracker_state_dir(&sandbox.home), custom);
    }

    #[test]
    fn tracker_state_dir_falls_back_to_agent_command_center_state_dir() {
        let sandbox = Sandbox::new();
        let custom = sandbox.home.join("acc-state");
        unsafe {
            std::env::set_var("AGENT_COMMAND_CENTER_STATE_DIR", &custom);
        }
        assert_eq!(tracker_state_dir(&sandbox.home), custom);
    }

    #[test]
    fn two_registered_homes_are_collected_independently() {
        let sandbox = Sandbox::new();
        let primary = sandbox.home.join(".codex");
        let extra = sandbox.home.join(".codex-account-2");
        write_rollout_at(
            &primary,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(57.0, 43.0)],
        );
        write_rollout_at(
            &extra,
            "2026/07/03",
            "rollout-b.jsonl",
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
        write_registry(
            &sandbox.home,
            &json!([
                {"id": 1, "home": primary.to_string_lossy(), "label": "", "builtin": true},
                {"id": 2, "home": extra.to_string_lossy(), "label": "trial", "builtin": false},
            ]),
            2,
        );
        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), None);
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
        assert_eq!(result["windows"][0]["label"], json!("month"));
        assert_eq!(result["plan"], json!("free"));
        assert_eq!(result["selected"], json!(2));
        let instances = result["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(instances[0]["id"], json!(1));
        assert_eq!(instances[0]["selected"], json!(false));
        assert_eq!(instances[0]["windows"][0]["used_percent"], json!(57.0));
        assert_eq!(instances[1]["id"], json!(2));
        assert_eq!(instances[1]["selected"], json!(true));
        assert_eq!(instances[1]["label"], json!("trial"));
        assert_eq!(instances[1]["plan"], json!("free"));
        assert_eq!(instances[1]["windows"][0]["label"], json!("month"));
        assert_eq!(instances[1]["windows"][0]["used_percent"], json!(12.0));
        assert_eq!(selected_codex_home(&sandbox.home), extra);
    }

    #[test]
    fn shared_rollout_cannot_replace_an_accounts_own_limits() {
        let sandbox = Sandbox::new();
        let first = sandbox.home.join(".codex");
        let third = sandbox.home.join(".codex-account-3");
        write_rollout_at(
            &first,
            "2026/07/03",
            "rollout-first.jsonl",
            &[codex_token_count_event(70.0, 80.0)],
        );
        let shared = write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-shared.jsonl",
            &[codex_token_count_event(3.0, 96.0)],
        );
        let target = third.join("sessions/2026/07/03/rollout-shared.jsonl");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::hard_link(shared, target).unwrap();
        write_rollout_at(
            &third,
            "2026/07/03",
            "rollout-third.jsonl",
            &[codex_token_count_event(4.0, 95.0)],
        );
        write_registry(
            &sandbox.home,
            &json!([
                {"id": 1, "home": first.to_string_lossy()},
                {"id": 3, "home": third.to_string_lossy()}
            ]),
            3,
        );
        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), None);
        assert_eq!(
            result["instances"][0]["windows"][0]["used_percent"],
            json!(70.0)
        );
        assert_eq!(
            result["instances"][1]["windows"][0]["used_percent"],
            json!(4.0)
        );
    }

    #[test]
    fn newer_console_weekly_reset_is_kept_for_its_own_account() {
        let sandbox = Sandbox::new();
        let first = sandbox.home.join(".codex");
        let second = sandbox.home.join(".codex-account-2");
        write_rollout_at(
            &first,
            "2026/07/03",
            "rollout-first.jsonl",
            &[codex_token_count_event(1.0, 2.0)],
        );
        write_rollout_at(
            &second,
            "2026/07/03",
            "rollout-second.jsonl",
            &[codex_token_count_event(3.0, 12.0)],
        );
        write_registry(
            &sandbox.home,
            &json!([
                {"id": 1, "home": first.to_string_lossy()},
                {"id": 2, "home": second.to_string_lossy()}
            ]),
            1,
        );
        let previous = json!({"instances": [
            {"id": 1, "observed_at": FIXTURE_NOW + 20.0, "source": "codex_pane", "windows": [{"label": "week", "used_percent": 50.0, "resets_at": 1783577400_i64}]},
            {"id": 2, "observed_at": FIXTURE_NOW + 20.0, "source": "codex_pane", "windows": [{"label": "week", "used_percent": 100.0, "resets_at": 1783577400_i64}]}
        ]});
        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), Some(&previous));
        assert_eq!(
            result["instances"][0]["windows"][0]["used_percent"],
            json!(50.0)
        );
        assert_eq!(
            result["instances"][1]["windows"][0]["used_percent"],
            json!(100.0)
        );
        assert_eq!(
            result["instances"][1]["windows"][0]["resets_at"],
            json!(1783577400_i64)
        );
    }

    #[test]
    fn extra_instance_keeps_its_own_carried_reading() {
        let sandbox = Sandbox::new();
        let primary = sandbox.home.join(".codex");
        let extra = sandbox.home.join(".codex-account-2");
        write_rollout_at(
            &primary,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(10.0, 5.0)],
        );
        write_registry(
            &sandbox.home,
            &json!([
                {"id": 1, "home": primary.to_string_lossy(), "label": ""},
                {"id": 2, "home": extra.to_string_lossy(), "label": "trial"},
            ]),
            1,
        );
        let last = json!({
            "instances": [
                {"id": 1, "windows": [{"label": "5h", "used_percent": 1.0}]},
                {
                    "id": 2,
                    "plan": "free",
                    "windows": [{"label": "month", "used_percent": 33.0, "resets_at": 1}],
                    "observed_at": 99.0,
                },
            ]
        });
        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), Some(&last));
        assert_eq!(
            result["instances"][1]["windows"][0]["used_percent"],
            json!(33.0)
        );
        assert_eq!(result["instances"][1]["plan"], json!("free"));
        assert_eq!(
            result["instances"][0]["windows"][0]["used_percent"],
            json!(10.0)
        );
    }

    #[test]
    fn each_instance_keeps_its_own_reset_count_when_rollouts_have_windows() {
        let sandbox = Sandbox::new();
        let primary = sandbox.home.join(".codex");
        let extra = sandbox.home.join(".codex-account-2");
        write_rollout_at(
            &primary,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(10.0, 5.0)],
        );
        write_rollout_at(
            &extra,
            "2026/07/03",
            "rollout-b.jsonl",
            &[codex_token_count_event(20.0, 8.0)],
        );
        write_registry(
            &sandbox.home,
            &json!([
                {"id": 1, "home": primary.to_string_lossy(), "label": ""},
                {"id": 2, "home": extra.to_string_lossy(), "label": "main"},
            ]),
            1,
        );
        let last = json!({
            "instances": [
                {
                    "id": 1,
                    "resets_available": 2,
                    "resets_checked_at": 50.0,
                    "windows": [{"label": "5h", "used_percent": 10.0}]
                },
                {
                    "id": 2,
                    "resets_available": 1,
                    "resets_checked_at": 50.0,
                    "windows": [{"label": "5h", "used_percent": 20.0}]
                },
            ]
        });
        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), Some(&last));
        assert_eq!(result["instances"][0]["resets_available"], json!(2));
        assert_eq!(result["instances"][1]["resets_available"], json!(1));
        assert_eq!(result["resets_available"], json!(2));
    }

    #[test]
    fn transiently_missing_registry_keeps_every_previous_instance() {
        let sandbox = Sandbox::new();
        let primary = sandbox.home.join(".codex");
        let extra = sandbox.home.join(".codex-account-2");
        let third = sandbox.home.join(".codex-account-3");
        write_rollout_at(
            &primary,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(10.0, 5.0)],
        );
        write_rollout_at(
            &extra,
            "2026/07/03",
            "rollout-b.jsonl",
            &[codex_token_count_event(20.0, 15.0)],
        );
        write_rollout_at(
            &third,
            "2026/07/03",
            "rollout-c.jsonl",
            &[codex_token_count_event(30.0, 25.0)],
        );
        write_registry(
            &sandbox.home,
            &json!([
                {"id": 1, "home": primary.to_string_lossy(), "label": ""},
                {"id": 2, "home": extra.to_string_lossy(), "label": "trial"},
                {"id": 3, "home": third.to_string_lossy(), "label": "spare"},
            ]),
            3,
        );
        let previous = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), None);
        std::fs::remove_file(tracker_state_dir(&sandbox.home).join(CODEX_INSTANCES_FILE)).unwrap();

        let result = collect_codex_accounts(&sandbox.home, Some(FIXTURE_NOW), Some(&previous));

        assert_eq!(result["selected"], json!(3));
        assert_eq!(result["instances"].as_array().unwrap().len(), 3);
        assert_eq!(result["instances"][0]["id"], json!(1));
        assert_eq!(result["instances"][1]["id"], json!(2));
        assert_eq!(result["instances"][2]["id"], json!(3));
    }

    #[test]
    fn authentication_is_the_auth_json_file() {
        let sandbox = Sandbox::new();
        let home = sandbox.home.join(".codex-account-3");
        assert!(!codex_home_authenticated(&home));
        write_codex_auth(&home);
        assert!(codex_home_authenticated(&home));
    }
}
