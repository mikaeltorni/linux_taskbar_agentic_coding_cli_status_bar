//! Polling daemon that aggregates agentic CLI rate limits into a status file.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::collectors::openrouter::OpenRouterFetch;
use crate::collectors::{
    collect_claude, collect_codex_accounts, collect_codex_reset_forecast, collect_cursor,
    collect_grok, collect_opencode_go, collect_openrouter, load_codex_account_registry, parse_pane,
    selected_codex_home,
};
use crate::config::{
    load_config, slash_command, state_dir, CLI_IDS, CODEX_RESET_FORECAST_STALE_AFTER_SECONDS,
};
use crate::http::HttpOpener;
use crate::logging;
use crate::snapshot::carry_last_reading;
use crate::tmux::{
    codex_console_window, console_codex_home, ensure_console, real_sleep, refresh_and_capture,
    take_codex_console_refresh, tmux_available, tmux_console_cli_ids, CodexConsole, RealTmux,
    TmuxRunner,
};

pub const STATUS_FILE_NAME: &str = "status.json";
pub const PANE_DIR_NAME: &str = "panes";
pub const TMUX_REFRESH_MIN_INTERVAL_SECONDS: f64 = 60.0;
pub const REMOTE_HEADLESS_CLI_IDS: &[&str] =
    &["claude_code", "opencode_go", "openrouter", "cursor"];

/// Job id used to run the Codex reset forecast through the same background
/// worker as the remote CLI collectors. It is not a CLI, so it is kept out of
/// `REMOTE_HEADLESS_CLI_IDS` and only widens the worker pool.
pub const CODEX_RESET_FORECAST_JOB_ID: &str = "codex_reset_forecast";

/// Every id the background worker pool must be able to service concurrently.
pub const REMOTE_HEADLESS_JOB_IDS: &[&str] = &[
    "claude_code",
    "opencode_go",
    "openrouter",
    "cursor",
    CODEX_RESET_FORECAST_JOB_ID,
];

/// Optional test/production hooks for collectors and tmux.
#[derive(Clone, Default)]
pub struct PollHooks {
    pub http: Option<Arc<dyn HttpOpener>>,
    pub refresh_http: Option<Arc<dyn HttpOpener>>,
    pub openrouter_fetch: Option<OpenRouterFetch>,
    pub tmux: Option<Arc<dyn TmuxRunner>>,
    pub tmux_available: Option<bool>,
    pub claude_running: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    pub sleeper: Option<Arc<dyn Fn(f64) + Send + Sync>>,
}

fn now_epoch() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Return the aggregated status file path.
pub fn status_path(home: Option<&Path>) -> PathBuf {
    state_dir(home).join(STATUS_FILE_NAME)
}

fn is_stale(snapshot: &Value, stale_after: i64, now: f64) -> bool {
    let windows_empty = snapshot
        .get("windows")
        .and_then(Value::as_array)
        .map(|a| a.is_empty())
        .unwrap_or(true);
    let observed = snapshot.get("observed_at").and_then(Value::as_f64);
    if windows_empty || observed.is_none() {
        return true;
    }
    (now - observed.unwrap()) > stale_after as f64
}

fn carry_fresher_reading(snapshot: Value, last: Option<&Value>) -> Value {
    let Some(last) = last else {
        return snapshot;
    };
    if last
        .get("windows")
        .and_then(Value::as_array)
        .map(|a| a.is_empty())
        .unwrap_or(true)
    {
        return snapshot;
    }
    let last_obs = last
        .get("observed_at")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let snap_obs = snapshot
        .get("observed_at")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if last_obs <= snap_obs {
        return snapshot;
    }
    let mut carried = snapshot;
    for key in ["source", "observed_at", "windows"] {
        if let Some(value) = last.get(key) {
            carried[key] = value.clone();
        }
    }
    for key in ["resets_available", "resets_checked_at"] {
        if let Some(value) = last.get(key) {
            carried[key] = value.clone();
        }
    }
    if carried.get("plan").map(|p| p.is_null()).unwrap_or(true) {
        carried["plan"] = last.get("plan").cloned().unwrap_or(Value::Null);
    }
    carried
}

/// Return the previous snapshot only when it describes the same account.
///
/// Every non-Codex CLI is a single login, so its previous reading always
/// belongs to it. Codex can hold several ChatGPT logins, and the top-level
/// fields belong to the selected one; when the selection moved between ticks
/// the previous reading is another account's and carrying it would publish one
/// login's limits under another's name.
///
/// Args:
///     cli_id: Poller CLI id being collected.
///     snapshot: Snapshot built for this tick.
///     last: Previous tick's snapshot for the same CLI.
///
/// Returns:
///     `last` when it is safe to carry forward, otherwise None.
fn codex_same_account_last<'a>(
    cli_id: &str,
    snapshot: &Value,
    last: Option<&'a Value>,
) -> Option<&'a Value> {
    let last = last?;
    if cli_id != "codex" {
        return Some(last);
    }
    if snapshot.get("instances").is_none() {
        return Some(last);
    }
    let now_selected = snapshot.get("selected").and_then(Value::as_i64);
    let then_selected = last.get("selected").and_then(Value::as_i64);
    match (now_selected, then_selected) {
        (Some(now), Some(then)) if now != then => {
            logging::info(format!(
                "codex selection moved {then} -> {now}; not carrying the previous account's reading"
            ));
            None
        }
        _ => Some(last),
    }
}

fn carry_codex_reset_metadata(snapshot: Value, last: Option<&Value>) -> Value {
    let Some(last) = last else {
        return snapshot;
    };
    if snapshot.get("resets_available").is_some() {
        return snapshot;
    }
    let mut carried = snapshot;
    for key in ["resets_available", "resets_checked_at"] {
        if let Some(value) = last.get(key) {
            carried[key] = value.clone();
        }
    }
    carried
}

fn carry_window_resets(snapshot: Value, fallbacks: &[&Value]) -> Value {
    let Some(windows) = snapshot.get("windows").and_then(Value::as_array) else {
        return snapshot;
    };
    if windows.is_empty() {
        return snapshot;
    }
    let mut known: HashMap<String, Value> = HashMap::new();
    for source in fallbacks {
        if let Some(list) = source.get("windows").and_then(Value::as_array) {
            for window in list {
                if let Some(label) = window.get("label").and_then(Value::as_str) {
                    if let Some(reset) = window.get("resets_at") {
                        if !reset.is_null() && !known.contains_key(label) {
                            known.insert(label.to_string(), reset.clone());
                        }
                    }
                }
            }
        }
    }
    let mut filled = Vec::new();
    let mut changed = false;
    for window in windows {
        let mut window = window.clone();
        if window.get("resets_at").map(|v| v.is_null()).unwrap_or(true) {
            if let Some(label) = window.get("label").and_then(Value::as_str) {
                if let Some(reset) = known.get(label) {
                    window["resets_at"] = reset.clone();
                    changed = true;
                }
            }
        }
        filled.push(window);
    }
    if !changed {
        return snapshot;
    }
    let mut out = snapshot;
    out["windows"] = Value::Array(filled);
    out
}

fn load_pane_capture(home: &Path, cli_id: &str) -> (Option<String>, Option<f64>) {
    let pane = state_dir(Some(home))
        .join(PANE_DIR_NAME)
        .join(format!("{cli_id}.txt"));
    match pane.metadata() {
        Ok(meta) => {
            let text = fs::read_to_string(&pane).ok();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64());
            (text, mtime)
        }
        Err(_) => (None, None),
    }
}

fn save_pane_capture(home: &Path, cli_id: &str, text: &str) {
    let pane_dir = state_dir(Some(home)).join(PANE_DIR_NAME);
    if let Err(exc) = fs::create_dir_all(&pane_dir) {
        logging::warn(format!("cannot save pane capture for {cli_id}: {exc}"));
        return;
    }
    let target = pane_dir.join(format!("{cli_id}.txt"));
    let temporary = pane_dir.join(format!(".{cli_id}.txt.tmp"));
    if let Err(exc) = fs::write(&temporary, text) {
        logging::warn(format!("cannot save pane capture for {cli_id}: {exc}"));
        return;
    }
    if let Err(exc) = fs::rename(&temporary, &target) {
        logging::warn(format!("cannot save pane capture for {cli_id}: {exc}"));
    }
}

/// Merge Grok billing and console readings with a fixed priority.
pub fn resolve_grok_snapshot(
    billing: &Value,
    live_text: Option<&str>,
    saved_text: Option<&str>,
    saved_at: Option<f64>,
    last: Option<&Value>,
    source_mode: &str,
    tmux_enabled: bool,
    now: f64,
    stale_after: i64,
    tmux_throttled: bool,
) -> Value {
    let plan = billing.get("plan").cloned();
    if source_mode == "headless"
        || !tmux_enabled
        || billing
            .get("windows")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
    {
        return billing.clone();
    }
    let from_pane = |text: &str, observed_at: f64| -> Option<Value> {
        let pane = parse_pane("grok", text, Some(now));
        if pane
            .get("windows")
            .and_then(Value::as_array)
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            return None;
        }
        let mut merged = pane;
        merged["plan"] = if plan.as_ref().map(|p| !p.is_null()).unwrap_or(false) {
            plan.clone().unwrap()
        } else {
            merged.get("plan").cloned().unwrap_or(Value::Null)
        };
        merged["observed_at"] = json!(observed_at);
        Some(merged)
    };
    if let Some(live_text) = live_text {
        if let Some(live_snap) = from_pane(live_text, now) {
            return live_snap;
        }
    }
    if let (Some(saved_text), Some(saved_at)) = (saved_text, saved_at) {
        if now - saved_at <= stale_after as f64 {
            if let Some(saved_snap) = from_pane(saved_text, saved_at) {
                return saved_snap;
            }
        }
    }
    if tmux_throttled {
        if let Some(last) = last {
            if last.get("source").and_then(Value::as_str) == Some("tmux_pane")
                && last
                    .get("windows")
                    .and_then(Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
                && !is_stale(last, stale_after, now)
            {
                let mut carried = billing.clone();
                for key in ["source", "observed_at", "windows", "error"] {
                    if let Some(value) = last.get(key) {
                        carried[key] = value.clone();
                    }
                }
                if carried.get("plan").map(|p| p.is_null()).unwrap_or(true) {
                    carried["plan"] = last
                        .get("plan")
                        .cloned()
                        .filter(|p| !p.is_null())
                        .unwrap_or_else(|| plan.clone().unwrap_or(Value::Null));
                }
                return carried;
            }
        }
    }
    billing.clone()
}

fn collect_headless(
    cli_id: &str,
    home: &Path,
    last: Option<&Value>,
    cli_config: &Value,
    now: f64,
    hooks: &PollHooks,
) -> Value {
    match cli_id {
        "codex" => collect_codex_accounts(home, Some(now), last),
        "claude_code" => collect_claude(
            home,
            last,
            Some(now),
            cli_config
                .get("endpoint_min_interval_seconds")
                .and_then(Value::as_f64)
                .or_else(|| {
                    cli_config
                        .get("endpoint_min_interval_seconds")
                        .and_then(Value::as_i64)
                        .map(|v| v as f64)
                }),
            hooks.http.as_deref(),
            hooks.refresh_http.as_deref(),
            hooks
                .claude_running
                .as_ref()
                .map(|f| f.as_ref() as &dyn Fn() -> bool),
        ),
        "grok" => collect_grok(home),
        "opencode_go" => collect_opencode_go(
            home,
            last,
            Some(now),
            cli_config
                .get("fetch_min_interval_seconds")
                .and_then(Value::as_f64)
                .or_else(|| {
                    cli_config
                        .get("fetch_min_interval_seconds")
                        .and_then(Value::as_i64)
                        .map(|v| v as f64)
                }),
            hooks.http.as_deref(),
        ),
        "openrouter" => collect_openrouter(
            home,
            last,
            Some(now),
            cli_config
                .get("fetch_min_interval_seconds")
                .and_then(Value::as_f64)
                .or_else(|| {
                    cli_config
                        .get("fetch_min_interval_seconds")
                        .and_then(Value::as_i64)
                        .map(|v| v as f64)
                }),
            cli_config.get("model_slug").and_then(Value::as_str),
            cli_config.get("daily_limit").and_then(Value::as_i64),
            hooks.openrouter_fetch.clone(),
        ),
        "cursor" => collect_cursor(
            home,
            last,
            Some(now),
            cli_config
                .get("fetch_min_interval_seconds")
                .and_then(Value::as_f64)
                .or_else(|| {
                    cli_config
                        .get("fetch_min_interval_seconds")
                        .and_then(Value::as_i64)
                        .map(|v| v as f64)
                }),
            hooks.http.as_deref(),
        ),
        CODEX_RESET_FORECAST_JOB_ID => collect_codex_reset_forecast(
            last,
            Some(now),
            config_seconds(cli_config, "fetch_min_interval_seconds"),
            cli_config.get("horizon").and_then(Value::as_str),
            cli_config.get("sites"),
            hooks.http.as_deref(),
        ),
        _ => json!({"windows": []}),
    }
}

/// Read a seconds-valued config key that may be stored as an int or a float.
fn config_seconds(config: &Value, key: &str) -> Option<f64> {
    config.get(key).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|seconds| seconds as f64))
    })
}

fn do_refresh_and_capture(
    session: &str,
    cli_id: &str,
    window: &str,
    hooks: &PollHooks,
) -> Option<String> {
    let sleeper = hooks
        .sleeper
        .clone()
        .unwrap_or_else(|| Arc::new(real_sleep));
    if let Some(tmux) = &hooks.tmux {
        refresh_and_capture(session, cli_id, window, sleeper.as_ref(), tmux.as_ref())
    } else {
        refresh_and_capture(session, cli_id, window, sleeper.as_ref(), &RealTmux)
    }
}

/// Return the console window every registered Codex account should own.
///
/// One window per ACC login keeps both accounts' `/status` readings live, so
/// the panel shows every instance's 5h limit rather than only the selected
/// one's. Installs without a registry keep the single historical `codex`
/// window on `~/.codex`.
fn codex_console_plan(home: &Path) -> Vec<CodexConsole> {
    let (rows, _) = load_codex_account_registry(home);
    if rows.is_empty() {
        return vec![CodexConsole {
            window: codex_console_window(1),
            home: selected_codex_home(home),
            instance_id: None,
        }];
    }
    rows.into_iter()
        .map(|row| CodexConsole {
            window: codex_console_window(row.id),
            home: row.home,
            instance_id: Some(row.id),
        })
        .collect()
}

/// One CLI console window and the account it is expected to be logged into.
struct ConsoleTarget {
    /// tmux window name.
    window: String,
    /// ACC Codex instance id this window reports for, when there is one.
    instance_id: Option<i64>,
    /// `CODEX_HOME` the window should be running.
    home: Option<PathBuf>,
}

/// Return every console window that reports for one CLI.
///
/// Non-Codex CLIs have a single login and so a single window. Codex gets one
/// window per ACC-registered account, which is what lets both logins publish
/// their own 5h limits on the same tick — including right after a reboot, when
/// no account has a rollout newer than the boot.
///
/// Args:
///     cli_id: CLI being polled.
///     snapshot: Headless reading, carrying `instances` for Codex.
///     home: User home used to resolve the default Codex login.
///
/// Returns:
///     Console targets in instance order.
fn console_targets(cli_id: &str, snapshot: &Value, home: &Path) -> Vec<ConsoleTarget> {
    if cli_id != "codex" {
        return vec![ConsoleTarget {
            window: cli_id.to_string(),
            instance_id: None,
            home: None,
        }];
    }
    let rows = snapshot.get("instances").and_then(Value::as_array);
    let Some(rows) = rows.filter(|rows| !rows.is_empty()) else {
        return vec![ConsoleTarget {
            window: codex_console_window(1),
            instance_id: None,
            home: Some(selected_codex_home(home)),
        }];
    };
    rows.iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_i64)?;
            Some(ConsoleTarget {
                window: codex_console_window(id),
                instance_id: Some(id),
                home: row.get("home").and_then(Value::as_str).map(PathBuf::from),
            })
        })
        .collect()
}

/// Return when this console's account last had a pane capture applied.
fn console_pane_checked_at(last: Option<&Value>, instance_id: Option<i64>) -> f64 {
    let top_level = || {
        last.and_then(|l| l.get("pane_checked_at").and_then(Value::as_f64))
            .unwrap_or(0.0)
    };
    let Some(instance_id) = instance_id else {
        return top_level();
    };
    last.and_then(|l| l.get("instances"))
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("id").and_then(Value::as_i64) == Some(instance_id))
        })
        .and_then(|row| row.get("pane_checked_at").and_then(Value::as_f64))
        .unwrap_or(0.0)
}

/// Read the `CODEX_HOME` a console window is logged into.
fn console_home_of(hooks: &PollHooks, session: &str, window: &str) -> Option<String> {
    match &hooks.tmux {
        Some(tmux) => console_codex_home(tmux.as_ref(), session, window),
        None => console_codex_home(&RealTmux, session, window),
    }
}

fn collect_grok_cli(
    cli_config: &Value,
    tmux_config: &Value,
    home: &Path,
    last: Option<&Value>,
    now: f64,
    tmux_refresh_request: Option<&dyn Fn(&str, &str, &str) -> bool>,
    hooks: &PollHooks,
) -> Value {
    let source_mode = cli_config
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let stale_after = cli_config
        .get("stale_after_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(1800);
    let tmux_enabled = tmux_config
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let billing = collect_grok(home);
    let mut last_tmux_attempt = last.and_then(|l| l.get("tmux_attempt_at").and_then(Value::as_f64));
    let tmux_wanted = tmux_enabled
        && (source_mode == "auto" || source_mode == "tmux")
        && (source_mode == "tmux" || is_stale(&billing, stale_after, now) || source_mode == "auto")
        && last_tmux_attempt
            .map(|prev| now - prev >= TMUX_REFRESH_MIN_INTERVAL_SECONDS)
            .unwrap_or(true);
    let mut live_text = None;
    if tmux_wanted {
        let session = tmux_config
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or("agentic-cli-status");
        if let Some(request) = tmux_refresh_request {
            if request(session, "grok", "grok") {
                last_tmux_attempt = Some(now);
            }
        } else {
            last_tmux_attempt = Some(now);
            live_text = do_refresh_and_capture(session, "grok", "grok", hooks);
        }
    }
    let mut saved_text = None;
    let mut saved_at = None;
    if (source_mode == "auto" || source_mode == "tmux") && tmux_enabled {
        let loaded = load_pane_capture(home, "grok");
        saved_text = loaded.0;
        saved_at = loaded.1;
    }
    let mut snapshot = resolve_grok_snapshot(
        &billing,
        live_text.as_deref(),
        saved_text.as_deref(),
        saved_at,
        last,
        source_mode,
        tmux_enabled,
        now,
        stale_after,
        !tmux_wanted,
    );
    snapshot = carry_window_resets(snapshot, &[&billing]);
    snapshot = carry_last_reading(snapshot, last);
    if let Some(live_text) = live_text.as_deref() {
        if parse_pane("grok", live_text, Some(now))
            .get("windows")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            save_pane_capture(home, "grok", live_text);
        }
    }
    if let Some(attempt) = last_tmux_attempt {
        snapshot["tmux_attempt_at"] = json!(attempt);
    }
    snapshot["stale"] = json!(is_stale(&snapshot, stale_after, now));
    snapshot
}

fn merge_cli_pane(
    mut snapshot: Value,
    cli_id: &str,
    pane_text: &str,
    pane_observed_at: f64,
    source_mode: &str,
    stale_after: i64,
    last: Option<&Value>,
    now: f64,
) -> Value {
    let pane_snapshot = parse_pane(cli_id, pane_text, Some(pane_observed_at));
    let pane_has_reset_metadata = pane_snapshot.get("resets_available").is_some();
    let pane_is_newer = pane_observed_at
        >= snapshot
            .get("observed_at")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    let mut prior_reset_metadata = json!({});
    for key in ["resets_available", "resets_checked_at"] {
        if let Some(value) = snapshot.get(key) {
            prior_reset_metadata[key] = value.clone();
        }
    }
    let use_pane = source_mode == "tmux"
        || snapshot
            .get("windows")
            .and_then(Value::as_array)
            .map(|a| a.is_empty())
            .unwrap_or(true)
        || is_stale(&snapshot, stale_after, now)
        || (cli_id == "codex" && source_mode == "auto" && pane_is_newer);
    if pane_snapshot
        .get("windows")
        .and_then(Value::as_array)
        .map(|a| !a.is_empty())
        .unwrap_or(false)
        && use_pane
    {
        if cli_id == "codex"
            && source_mode == "auto"
            && snapshot
                .get("windows")
                .and_then(Value::as_array)
                .map(|a| !a.is_empty())
                .unwrap_or(false)
            && pane_is_newer
        {
            logging::debug("codex pane reading superseded rollout snapshot");
        }
        let mut pane_snapshot = pane_snapshot.clone();
        pane_snapshot["plan"] = snapshot
            .get("plan")
            .cloned()
            .filter(|p| !p.is_null())
            .unwrap_or_else(|| pane_snapshot.get("plan").cloned().unwrap_or(Value::Null));
        pane_snapshot["observed_at"] = json!(pane_observed_at);
        // `/status` prints windows only. Dropping the rollout's credit state
        // here would hide a credit block behind a window countdown that will
        // not unblock the account.
        if let Some(credits) = snapshot.get("credits") {
            if !credits.is_null() {
                pane_snapshot["credits"] = credits.clone();
            }
        }
        if cli_id == "codex" && pane_snapshot.get("resets_available").is_none() {
            if let Some(obj) = prior_reset_metadata.as_object() {
                for (key, value) in obj {
                    pane_snapshot[key] = value.clone();
                }
            }
        }
        snapshot = pane_snapshot;
    }
    if cli_id == "codex" && pane_has_reset_metadata {
        let previous_count = last
            .and_then(|l| l.get("resets_available").and_then(Value::as_i64))
            .unwrap_or(0);
        let current_count = pane_snapshot
            .get("resets_available")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if current_count > 0 {
            snapshot["resets_available"] = json!(current_count);
        } else {
            if let Some(obj) = snapshot.as_object_mut() {
                obj.remove("resets_available");
            }
        }
        snapshot["resets_checked_at"] = json!(pane_observed_at);
        if previous_count != current_count {
            logging::info(format!(
                "codex reset allowance changed from {previous_count} to {current_count}"
            ));
        }
    }
    snapshot["pane_checked_at"] = json!(pane_observed_at);
    snapshot
}

fn collect_cli(
    cli_id: &str,
    cli_config: &Value,
    tmux_config: &Value,
    home: &Path,
    last: Option<&Value>,
    now: f64,
    tmux_refresh_request: Option<&dyn Fn(&str, &str, &str) -> bool>,
    headless_snapshot: Option<Value>,
    hooks: &PollHooks,
) -> Value {
    if cli_id == "grok" {
        return collect_grok_cli(
            cli_config,
            tmux_config,
            home,
            last,
            now,
            tmux_refresh_request,
            hooks,
        );
    }
    let source_mode = cli_config
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let stale_after = cli_config
        .get("stale_after_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(1800);
    let mut snapshot = json!({"windows": []});
    if source_mode == "auto" || source_mode == "headless" {
        snapshot = headless_snapshot
            .unwrap_or_else(|| collect_headless(cli_id, home, last, cli_config, now, hooks));
    }
    // Carrying a previous reading forward is only ever valid inside one
    // account. The top-level Codex snapshot is a projection of whichever
    // instance ACC has selected, so once `caN` moves the selection the previous
    // tick describes a *different* ChatGPT login and must not be carried onto
    // the new one.
    let account_last = codex_same_account_last(cli_id, &snapshot, last);
    snapshot = carry_fresher_reading(snapshot, account_last);
    if cli_id == "codex" {
        snapshot = carry_codex_reset_metadata(snapshot, account_last);
    }
    let headless_snapshot = snapshot.clone();
    let mut last_tmux_attempt = last.and_then(|l| l.get("tmux_attempt_at").and_then(Value::as_f64));
    // Windows rebuilt for a different login this tick: their saved pane text
    // still shows the old account, so it is dropped and captured again now.
    let recreated_consoles = if cli_id == "codex" {
        take_codex_console_refresh()
    } else {
        HashSet::new()
    };
    let force_codex_refresh = !recreated_consoles.is_empty();
    for window in &recreated_consoles {
        let stale_pane = state_dir(Some(home))
            .join(PANE_DIR_NAME)
            .join(format!("{window}.txt"));
        if stale_pane.is_file() {
            let _ = fs::remove_file(&stale_pane);
            logging::info(format!(
                "dropped stale Codex pane capture for {window} after console login change"
            ));
        }
    }
    let tmux_wanted = slash_command(cli_id).is_some()
        && tmux_config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        && (source_mode == "auto" || source_mode == "tmux")
        && (source_mode == "tmux"
            || is_stale(&snapshot, stale_after, now)
            || (cli_id == "codex" && source_mode == "auto"))
        && (force_codex_refresh
            || last_tmux_attempt
                .map(|prev| now - prev >= TMUX_REFRESH_MIN_INTERVAL_SECONDS)
                .unwrap_or(true));
    let mut endpoint_marks = json!({});
    for key in ["checked_at", "retry_after_until"] {
        if let Some(value) = snapshot.get(key) {
            endpoint_marks[key] = value.clone();
        }
    }
    let session = tmux_config
        .get("session")
        .and_then(Value::as_str)
        .unwrap_or("agentic-cli-status");
    let tmux_enabled = tmux_config
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    // Every CLI owns one console window, and Codex owns one per ACC account,
    // so an idle login keeps reporting its own limits instead of waiting for
    // the selected account's window to be pointed at it.
    for console in console_targets(cli_id, &snapshot, home) {
        let mut pane_text: Option<String> = None;
        let mut pane_observed_at: Option<f64> = None;
        // Identity of the login this console is running, read from the pane
        // process environment. Every `/status` capture belongs to that account
        // and to no other, which is what keeps two ChatGPT logins from sharing
        // one reading.
        let console_home: Option<String> = if cli_id == "codex" && tmux_enabled {
            console_home_of(hooks, session, &console.window)
        } else {
            None
        };
        // A window dedicated to one account stays authoritative for it, so its
        // capture is re-applied even when the pane text has not changed.
        let console_owns_account = console
            .home
            .as_deref()
            .zip(console_home.as_deref())
            .is_some_and(|(want, got)| Path::new(got) == want);
        if tmux_wanted {
            if let Some(request) = tmux_refresh_request {
                if request(session, cli_id, &console.window) {
                    last_tmux_attempt = Some(now);
                }
            } else {
                last_tmux_attempt = Some(now);
                pane_text = do_refresh_and_capture(session, cli_id, &console.window, hooks);
                pane_observed_at = Some(now);
                if let Some(text) = pane_text.as_deref() {
                    save_pane_capture(home, &console.window, text);
                }
            }
        }
        if tmux_refresh_request.is_some()
            && tmux_enabled
            && (source_mode == "auto" || source_mode == "tmux")
        {
            let (saved_text, saved_at) = load_pane_capture(home, &console.window);
            if let (Some(saved_text), Some(saved_at)) = (saved_text, saved_at) {
                let last_checked = console_pane_checked_at(last, console.instance_id);
                let pane_fresh = now - saved_at <= stale_after as f64;
                if pane_fresh && (saved_at > last_checked || console_owns_account) {
                    pane_text = Some(saved_text);
                    pane_observed_at = Some(saved_at);
                }
            }
        }
        if let (Some(text), Some(observed_at)) = (pane_text.as_deref(), pane_observed_at) {
            snapshot = if cli_id == "codex" && snapshot.get("instances").is_some() {
                apply_codex_live_pane(
                    snapshot,
                    text,
                    observed_at,
                    source_mode,
                    stale_after,
                    last,
                    now,
                    console_home.as_deref(),
                )
            } else {
                merge_cli_pane(
                    snapshot,
                    cli_id,
                    text,
                    observed_at,
                    source_mode,
                    stale_after,
                    last,
                    now,
                )
            };
        }
    }
    snapshot = carry_window_resets(
        snapshot,
        &[&headless_snapshot, account_last.unwrap_or(&json!({}))],
    );
    if let Some(obj) = endpoint_marks.as_object() {
        for (key, value) in obj {
            snapshot[key] = value.clone();
        }
    }
    if let Some(attempt) = last_tmux_attempt {
        snapshot["tmux_attempt_at"] = json!(attempt);
    }
    if let Some(last) = last {
        if last.get("pane_checked_at").is_some() && snapshot.get("pane_checked_at").is_none() {
            snapshot["pane_checked_at"] = last["pane_checked_at"].clone();
        }
    }
    snapshot["stale"] = json!(is_stale(&snapshot, stale_after, now));
    if cli_id == "codex" {
        snapshot = promote_codex_selected_instance(snapshot);
    }
    snapshot
}

/// Largest L1 gap (used-percent points) still treated as the same Codex account.
const CODEX_PANE_MATCH_MAX: f64 = 15.0;

fn window_used_percents(snapshot: &Value) -> Vec<(String, f64)> {
    snapshot
        .get("windows")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|window| {
                    Some((
                        window.get("label")?.as_str()?.to_string(),
                        window.get("used_percent")?.as_f64()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Distance between pane windows and one instance's headless reading.
///
/// Sums absolute used-percent gaps on labels both sides share. `None` means
/// there is no overlapping window to compare.
fn codex_pane_distance(instance: &Value, pane: &Value) -> Option<f64> {
    let pane_windows = window_used_percents(pane);
    if pane_windows.is_empty() {
        return None;
    }
    let instance_windows = window_used_percents(instance);
    let mut total = 0.0;
    let mut matched = 0;
    for (label, used) in &pane_windows {
        if let Some((_, other)) = instance_windows.iter().find(|(name, _)| name == label) {
            total += (used - other).abs();
            matched += 1;
        }
    }
    (matched > 0).then_some(total)
}

/// Return the ACC instance registered at `home`, if any.
///
/// Identity beats inference: when the debug console's `CODEX_HOME` names a
/// registered account, its `/status` capture belongs to that account with no
/// guessing, so two logins at similar usage can never be handed each other's
/// numbers.
///
/// Args:
///     snapshot: Combined Codex snapshot carrying the `instances` array.
///     home: `CODEX_HOME` read from the console pane process, if known.
///
/// Returns:
///     The matching instance id, or None when `home` is unknown or
///     unregistered.
fn codex_instance_for_home(snapshot: &Value, home: Option<&str>) -> Option<i64> {
    let home = Path::new(home?);
    snapshot
        .get("instances")?
        .as_array()?
        .iter()
        .find(|row| {
            row.get("home")
                .and_then(Value::as_str)
                .is_some_and(|registered| Path::new(registered) == home)
        })
        .and_then(|row| row.get("id").and_then(Value::as_i64))
}

/// Return the ACC instance whose usage matches a captured `/status` pane.
///
/// The debug console is a single Codex login (usually `~/.codex`). Applying
/// that pane to the last `caN` selection copies one account onto another.
fn match_codex_pane_instance(snapshot: &Value, pane: &Value) -> Option<i64> {
    let rows = snapshot.get("instances")?.as_array()?;
    if rows.len() == 1 {
        return rows[0].get("id").and_then(Value::as_i64);
    }
    let mut scored: Vec<(i64, f64)> = rows
        .iter()
        .filter_map(|row| {
            let id = row.get("id").and_then(Value::as_i64)?;
            Some((id, codex_pane_distance(row, pane)?))
        })
        .collect();
    scored.sort_by(|left, right| left.1.total_cmp(&right.1));
    let (best_id, best_dist) = scored.first()?;
    if *best_dist > CODEX_PANE_MATCH_MAX {
        return None;
    }
    if scored
        .get(1)
        .is_some_and(|(_, second)| *second - *best_dist < 5.0)
    {
        return None;
    }
    Some(*best_id)
}

/// Copy the selected instance's live fields onto the top-level Codex snapshot.
fn promote_codex_selected_instance(mut snapshot: Value) -> Value {
    let selected_id = snapshot
        .get("selected")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let Some(chosen) = snapshot
        .get("instances")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("id").and_then(Value::as_i64) == Some(selected_id))
                .cloned()
        })
    else {
        return snapshot;
    };
    for key in [
        "windows",
        "plan",
        "credits",
        "observed_at",
        "error",
        "source",
        "resets_available",
        "resets_checked_at",
        "stale",
    ] {
        if let Some(value) = chosen.get(key) {
            snapshot[key] = value.clone();
        }
    }
    snapshot
}

/// Merge a Codex `/status` pane into the instance it belongs to.
///
/// Extra registered logins keep their own headless readings. The compact bar
/// then shows the selected instance, not whichever login the debug console
/// happens to be.
fn apply_codex_live_pane(
    mut snapshot: Value,
    pane_text: &str,
    pane_observed_at: f64,
    source_mode: &str,
    stale_after: i64,
    last: Option<&Value>,
    now: f64,
    console_home: Option<&str>,
) -> Value {
    let selected_id = snapshot
        .get("selected")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let console_owner = codex_instance_for_home(&snapshot, console_home);
    let owner_id = if let Some(owner_id) = console_owner {
        logging::debug(format!(
            "codex pane /status applied to instance {owner_id} (console CODEX_HOME={})",
            console_home.unwrap_or("unset")
        ));
        owner_id
    } else {
        if let Some(home) = console_home {
            logging::warn(format!(
                "codex console CODEX_HOME={home} is not a registered ACC instance; falling back to usage matching"
            ));
        }
        let pane = parse_pane("codex", pane_text, Some(pane_observed_at));
        let Some(owner_id) = match_codex_pane_instance(&snapshot, &pane) else {
            logging::warn(
                "codex pane /status did not match any ACC instance; keeping independent headless readings",
            );
            snapshot["pane_checked_at"] = json!(pane_observed_at);
            return promote_codex_selected_instance(snapshot);
        };
        if owner_id != selected_id {
            logging::info(format!(
                "codex pane /status matched instance {owner_id}, not selected {selected_id}; leaving the selected account's limits unchanged"
            ));
        } else {
            logging::debug(format!(
                "codex pane /status matched selected instance {owner_id}"
            ));
        }
        owner_id
    };
    let Some(Value::Array(mut rows)) = snapshot.get("instances").cloned() else {
        return merge_cli_pane(
            snapshot,
            "codex",
            pane_text,
            pane_observed_at,
            source_mode,
            stale_after,
            last,
            now,
        );
    };
    let Some(index) = rows
        .iter()
        .position(|row| row.get("id").and_then(Value::as_i64) == Some(owner_id))
    else {
        snapshot["pane_checked_at"] = json!(pane_observed_at);
        return promote_codex_selected_instance(snapshot);
    };
    let owner = rows[index].clone();
    let mut base = owner.clone();
    if let Some(map) = base.as_object_mut() {
        for key in ["id", "label", "home", "selected"] {
            map.remove(key);
        }
    }
    // A capture whose account is known from the console process environment is
    // authoritative for that account, even when its percentages look older
    // than the rollout reading.
    let merge_mode = if console_owner.is_some() {
        "tmux"
    } else {
        source_mode
    };
    let merged = merge_cli_pane(
        base,
        "codex",
        pane_text,
        pane_observed_at,
        merge_mode,
        stale_after,
        last,
        now,
    );
    if let Some(target) = rows.get_mut(index) {
        for key in [
            "windows",
            "plan",
            "credits",
            "observed_at",
            "error",
            "source",
            "resets_available",
            "resets_checked_at",
            "stale",
            "pane_checked_at",
        ] {
            if let Some(value) = merged.get(key) {
                target[key] = value.clone();
            }
        }
        for key in ["id", "label", "home", "selected"] {
            if let Some(value) = owner.get(key) {
                target[key] = value.clone();
            }
        }
    }
    snapshot["instances"] = Value::Array(rows);
    if let Some(value) = merged.get("pane_checked_at") {
        snapshot["pane_checked_at"] = value.clone();
    }
    promote_codex_selected_instance(snapshot)
}

fn write_status(status: &Value, home: &Path) {
    let target = status_path(Some(home));
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = target.with_extension("tmp");
    if let Ok(mut text) = serde_json::to_string_pretty(status) {
        text.push('\n');
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, &target);
            logging::debug(format!("status written to {}", target.display()));
        }
    }
}

/// Load the last status snapshot so restart-only metadata survives.
pub fn load_previous_status(home: &Path) -> Option<Value> {
    let target = status_path(Some(home));
    match fs::read_to_string(&target) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(previous)
                if previous.is_object()
                    && previous.get("clis").map(Value::is_object).unwrap_or(false) =>
            {
                logging::info(format!(
                    "reloaded previous status from {}",
                    target.display()
                ));
                Some(previous)
            }
            Ok(_) => {
                logging::warn(format!(
                    "ignoring invalid previous status shape in {}",
                    target.display()
                ));
                None
            }
            Err(exc) => {
                logging::warn(format!(
                    "cannot reload previous status from {}: {exc}",
                    target.display()
                ));
                None
            }
        },
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => None,
        Err(exc) => {
            logging::warn(format!(
                "cannot reload previous status from {}: {exc}",
                target.display()
            ));
            None
        }
    }
}

/// Return seconds remaining until a start-to-start polling deadline.
pub fn next_tick_delay(started_at: f64, interval: i64, now: f64) -> f64 {
    (started_at + interval as f64 - now).max(0.0)
}

/// Run one poll tick and write the aggregated status file.
pub fn poll_once(
    home: Option<&Path>,
    previous: Option<&Value>,
    now: Option<f64>,
    tmux_refresh_request: Option<&dyn Fn(&str, &str, &str) -> bool>,
    headless_refresh_worker: Option<&HeadlessRefreshWorker>,
    hooks: &PollHooks,
) -> Value {
    let home_buf = home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())));
    let home = home_buf.as_path();
    let now = now.unwrap_or_else(now_epoch);
    let config = load_config(None);
    let enabled: Vec<String> = CLI_IDS
        .iter()
        .filter(|cli| config["clis"][*cli]["enabled"].as_bool().unwrap_or(false))
        .map(|s| (*s).to_string())
        .collect();
    let console_clis = tmux_console_cli_ids(&enabled);
    if config["tmux"]
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
        && !console_clis.is_empty()
    {
        let session = config["tmux"]["session"]
            .as_str()
            .unwrap_or("agentic-cli-status");
        let available = hooks.tmux_available.unwrap_or_else(tmux_available);
        let codex_consoles = codex_console_plan(home);
        if let Some(tmux) = &hooks.tmux {
            ensure_console(
                session,
                &console_clis,
                home,
                tmux.as_ref(),
                available,
                &codex_consoles,
            );
        } else {
            ensure_console(
                session,
                &console_clis,
                home,
                &RealTmux,
                available,
                &codex_consoles,
            );
        }
    }
    let previous_clis = previous
        .and_then(|p| p.get("clis"))
        .cloned()
        .unwrap_or(json!({}));
    let mut status = json!({
        "generated_at": now,
        "poll_interval_seconds": config["poll_interval_seconds"],
        "clis": {},
    });
    for cli_id in CLI_IDS {
        let cli_config = &config["clis"][cli_id];
        if !cli_config["enabled"].as_bool().unwrap_or(false) {
            status["clis"][cli_id] = json!({"enabled": false});
            continue;
        }
        let last_cli = previous_clis.get(*cli_id);
        let mut headless_snapshot = None;
        if let Some(worker) = headless_refresh_worker {
            if REMOTE_HEADLESS_CLI_IDS.contains(cli_id)
                && matches!(
                    cli_config["source"].as_str(),
                    Some("auto") | Some("headless")
                )
            {
                headless_snapshot = worker
                    .take_result(cli_id)
                    .or_else(|| last_cli.cloned())
                    .or_else(|| Some(json!({"windows": []})));
                worker.request(
                    cli_id,
                    home,
                    headless_snapshot.clone(),
                    cli_config.clone(),
                    now,
                    hooks,
                );
            }
        }
        let mut snapshot = collect_cli(
            cli_id,
            cli_config,
            &config["tmux"],
            home,
            last_cli,
            now,
            tmux_refresh_request,
            headless_snapshot,
            hooks,
        );
        snapshot["enabled"] = json!(true);
        if snapshot.get("error").map(|e| !e.is_null()).unwrap_or(false) {
            let prev_err = last_cli.and_then(|l| l.get("error"));
            if prev_err != snapshot.get("error") {
                logging::warn(format!("cli {cli_id}: {}", snapshot["error"]));
            }
        }
        status["clis"][cli_id] = snapshot;
    }
    status["codex_reset_forecast"] =
        poll_codex_reset_forecast(&config, previous, home, now, headless_refresh_worker, hooks);
    write_status(&status, home);
    status
}

/// Refresh the Codex reset forecast for this tick.
///
/// The fetch itself runs on the shared background worker, so a slow or hanging
/// third-party site can never stall the poll loop; the tick publishes whatever
/// reading is already in hand. That is what makes the bar percentage move at
/// the bar's tick rate while the sites are only contacted on their own cadence.
fn poll_codex_reset_forecast(
    config: &Value,
    previous: Option<&Value>,
    home: &Path,
    now: f64,
    headless_refresh_worker: Option<&HeadlessRefreshWorker>,
    hooks: &PollHooks,
) -> Value {
    let forecast_config = &config["codex_reset_forecast"];
    let codex_enabled = config["clis"]["codex"]
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !codex_enabled
        || !forecast_config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return json!({"enabled": false});
    }
    let last = previous.and_then(|p| p.get("codex_reset_forecast"));
    let last = last.filter(|value| value.get("sites").is_some());

    let mut snapshot = match headless_refresh_worker {
        Some(worker) => {
            let carried = worker
                .take_result(CODEX_RESET_FORECAST_JOB_ID)
                .or_else(|| last.cloned());
            worker.request(
                CODEX_RESET_FORECAST_JOB_ID,
                home,
                carried.clone(),
                forecast_config.clone(),
                now,
                hooks,
            );
            // Before the very first background fetch lands there is nothing to
            // publish yet; report that rather than inventing a number.
            carried.unwrap_or_else(|| json!({"sites": [], "consensus_percent": Value::Null}))
        }
        // Single-shot callers (`status --once`, tests) collect inline.
        None => collect_headless(
            CODEX_RESET_FORECAST_JOB_ID,
            home,
            last,
            forecast_config,
            now,
            hooks,
        ),
    };

    snapshot["enabled"] = json!(true);
    let stale_after = config_seconds(forecast_config, "stale_after_seconds")
        .unwrap_or(CODEX_RESET_FORECAST_STALE_AFTER_SECONDS as f64);
    let observed_at = snapshot
        .get("observed_at")
        .and_then(Value::as_f64)
        .or_else(|| snapshot.get("checked_at").and_then(Value::as_f64));
    let has_consensus = snapshot
        .get("consensus_percent")
        .map(|value| value.is_number())
        .unwrap_or(false);
    snapshot["stale"] =
        json!(!has_consensus || observed_at.map(|at| now - at > stale_after).unwrap_or(true));

    if let Some(error) = snapshot.get("error").and_then(Value::as_str) {
        let previous_error = last
            .and_then(|value| value.get("error"))
            .and_then(Value::as_str);
        if previous_error != Some(error) {
            logging::warn(format!("codex reset forecast: {error}"));
        }
    }
    snapshot
}

type HeadlessJob = (String, PathBuf, Option<Value>, Value, f64, PollHooks);

/// Run remote headless collectors without blocking the shared poll tick.
pub struct HeadlessRefreshWorker {
    queue: Arc<Mutex<VecDeque<Option<HeadlessJob>>>>,
    pending: Arc<Mutex<HashSet<String>>>,
    results: Arc<Mutex<HashMap<String, Value>>>,
    started: Arc<Mutex<bool>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for HeadlessRefreshWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadlessRefreshWorker {
    pub fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
            pending: Arc::new(Mutex::new(HashSet::new())),
            results: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(false)),
            threads: Mutex::new(Vec::new()),
        }
    }

    pub fn start(&self) {
        let mut started = self.started.lock().unwrap();
        if *started {
            return;
        }
        *started = true;
        drop(started);
        let mut threads = self.threads.lock().unwrap();
        for index in 0..REMOTE_HEADLESS_JOB_IDS.len() {
            let queue = self.queue.clone();
            let pending = self.pending.clone();
            let results = self.results.clone();
            threads.push(
                thread::Builder::new()
                    .name(format!("agentic-cli-status-remote-{index}"))
                    .spawn(move || loop {
                        let job = loop {
                            if let Some(item) = queue.lock().unwrap().pop_front() {
                                break item;
                            }
                            thread::sleep(Duration::from_millis(20));
                        };
                        let Some((cli_id, home, last, cli_config, now, hooks)) = job else {
                            return;
                        };
                        let result = collect_headless(
                            &cli_id,
                            &home,
                            last.as_ref(),
                            &cli_config,
                            now,
                            &hooks,
                        );
                        results.lock().unwrap().insert(cli_id.clone(), result);
                        pending.lock().unwrap().remove(&cli_id);
                    })
                    .expect("spawn remote collector"),
            );
        }
        logging::info("background remote collector workers started");
    }

    pub fn request(
        &self,
        cli_id: &str,
        home: &Path,
        last: Option<Value>,
        cli_config: Value,
        now: f64,
        hooks: &PollHooks,
    ) -> bool {
        {
            let started = self.started.lock().unwrap();
            let mut pending = self.pending.lock().unwrap();
            if !*started || pending.contains(cli_id) {
                return false;
            }
            pending.insert(cli_id.to_string());
        }
        self.queue.lock().unwrap().push_back(Some((
            cli_id.to_string(),
            home.to_path_buf(),
            last,
            cli_config,
            now,
            hooks.clone(),
        )));
        true
    }

    pub fn take_result(&self, cli_id: &str) -> Option<Value> {
        self.results.lock().unwrap().remove(cli_id)
    }

    pub fn close(&self) {
        let mut started = self.started.lock().unwrap();
        if !*started {
            return;
        }
        *started = false;
        drop(started);
        for _ in 0..REMOTE_HEADLESS_JOB_IDS.len() {
            self.queue.lock().unwrap().push_back(None);
        }
        let mut threads = self.threads.lock().unwrap();
        let mut alive = false;
        for handle in threads.drain(..) {
            if handle.join().is_err() {
                alive = true;
            }
        }
        if alive {
            logging::info("background remote collectors still exiting asynchronously");
        } else {
            logging::info("background remote collector workers stopped");
        }
    }
}

/// Console refresh job: `(session, cli_id, window)`.
type TmuxJob = (String, String, String);

/// Run slow tmux slash-command captures outside the five-second poll tick.
pub struct TmuxRefreshWorker {
    home: PathBuf,
    hooks: PollHooks,
    queue: Arc<Mutex<VecDeque<Option<TmuxJob>>>>,
    pending: Arc<Mutex<HashSet<(String, String)>>>,
    started: Arc<Mutex<bool>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl TmuxRefreshWorker {
    pub fn new(home: PathBuf, hooks: PollHooks) -> Self {
        Self {
            home,
            hooks,
            queue: Arc::new(Mutex::new(VecDeque::new())),
            pending: Arc::new(Mutex::new(HashSet::new())),
            started: Arc::new(Mutex::new(false)),
            thread: Mutex::new(None),
        }
    }

    pub fn start(&self) {
        let mut started = self.started.lock().unwrap();
        if *started {
            return;
        }
        *started = true;
        drop(started);
        let queue = self.queue.clone();
        let pending = self.pending.clone();
        let home = self.home.clone();
        let hooks = self.hooks.clone();
        let handle = thread::Builder::new()
            .name("agentic-cli-status-tmux-refresh".into())
            .spawn(move || loop {
                let job = loop {
                    if let Some(item) = queue.lock().unwrap().pop_front() {
                        break item;
                    }
                    thread::sleep(Duration::from_millis(20));
                };
                let Some((session, cli_id, window)) = job else {
                    return;
                };
                if let Some(text) = do_refresh_and_capture(&session, &cli_id, &window, &hooks) {
                    save_pane_capture(&home, &window, &text);
                }
                pending.lock().unwrap().remove(&(session, window));
            })
            .expect("spawn tmux worker");
        *self.thread.lock().unwrap() = Some(handle);
        logging::info("background tmux refresh worker started");
    }

    /// Queue a `/status` capture for one console window.
    ///
    /// Jobs are keyed by window, not by CLI, so each ACC Codex account's
    /// console is refreshed on its own instead of one login crowding out the
    /// other.
    pub fn request(&self, session: &str, cli_id: &str, window: &str) -> bool {
        let key = (session.to_string(), window.to_string());
        {
            let started = self.started.lock().unwrap();
            let mut pending = self.pending.lock().unwrap();
            if !*started || pending.contains(&key) {
                return false;
            }
            pending.insert(key.clone());
        }
        self.queue.lock().unwrap().push_back(Some((
            session.to_string(),
            cli_id.to_string(),
            window.to_string(),
        )));
        logging::debug(format!("queued background tmux refresh for {window}"));
        true
    }

    pub fn close(&self) {
        let mut started = self.started.lock().unwrap();
        if !*started {
            return;
        }
        *started = false;
        drop(started);
        self.queue.lock().unwrap().push_back(None);
        if let Some(handle) = self.thread.lock().unwrap().take() {
            let _ = handle.join();
        }
        logging::info("background tmux refresh worker stopped");
    }
}

/// Poll forever at the configured rate until SIGTERM/SIGINT.
pub fn run_daemon(home: Option<&Path>, hooks: PollHooks) {
    logging::info(format!(
        "agentic-cli-status poller starting (pid {})",
        std::process::id()
    ));
    let stop = Arc::new(Mutex::new(false));
    install_stop_handler(stop.clone());
    let home_buf = home
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())));
    let mut previous = load_previous_status(&home_buf);
    let tmux_worker = TmuxRefreshWorker::new(home_buf.clone(), hooks.clone());
    let headless_worker = HeadlessRefreshWorker::new();
    tmux_worker.start();
    headless_worker.start();
    let mut overrun = false;
    while !*stop.lock().unwrap() {
        let tick_started = Instant::now();
        let tick = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            poll_once(
                Some(&home_buf),
                previous.as_ref(),
                None,
                Some(&|session, cli_id, window| tmux_worker.request(session, cli_id, window)),
                Some(&headless_worker),
                &hooks,
            )
        }));
        match tick {
            Ok(status) => previous = Some(status),
            Err(_) => logging::error("poll tick failed: panic in poll_once"),
        }
        let interval = previous
            .as_ref()
            .and_then(|p| p.get("poll_interval_seconds").and_then(Value::as_i64))
            .unwrap_or_else(|| {
                load_config(None)["poll_interval_seconds"]
                    .as_i64()
                    .unwrap_or(5)
            });
        let delay = next_tick_delay(0.0, interval, tick_started.elapsed().as_secs_f64());
        if delay == 0.0 && !overrun {
            logging::warn(format!("poll tick exceeded configured {interval}s cadence"));
            overrun = true;
        } else if delay > 0.0 && overrun {
            logging::info(format!("poll tick cadence recovered to {interval}s"));
            overrun = false;
        }
        let deadline = Instant::now() + Duration::from_secs_f64(delay);
        while !*stop.lock().unwrap() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            thread::sleep(remaining.min(Duration::from_secs(1)));
        }
    }
    tmux_worker.close();
    headless_worker.close();
    logging::info("agentic-cli-status poller stopped");
}

/// Install SIGTERM/SIGINT to flip `stop`.
fn install_stop_handler(stop: Arc<Mutex<bool>>) {
    thread::spawn(move || unsafe {
        let mut set = std::mem::zeroed::<libc::sigset_t>();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        let mut sig: libc::c_int = 0;
        if libc::sigwait(&set, &mut sig) == 0 {
            logging::info(format!("received signal {sig}; stopping"));
            *stop.lock().unwrap() = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::save_config;
    use crate::http::{HttpError, HttpResponse};
    use crate::test_support::{
        codex_token_count_event, grok_billing_record, write_codex_rollout,
        write_cursor_credentials, write_grok_log, Sandbox, CURSOR_SAND_USAGE_FIXTURE,
        CURSOR_SPENDING_RESET_EPOCH, CURSOR_USAGE_SUMMARY_FIXTURE, NOW,
    };

    fn configure(tmux_enabled: bool) {
        let mut cfg = load_config(None);
        cfg["tmux"]["enabled"] = json!(tmux_enabled);
        save_config(&cfg, None).unwrap();
    }

    #[test]
    fn aggregates_enabled_clis_and_writes_status() {
        let sandbox = Sandbox::new();
        configure(false);
        write_codex_rollout(
            &sandbox.home,
            "2026/07/03",
            "rollout-a.jsonl",
            &[codex_token_count_event(57.0, 43.0)],
        );
        write_grok_log(&sandbox.home, "grok.log", &[grok_billing_record(88.0)]);
        let status = poll_once(
            Some(&sandbox.home),
            None,
            Some(NOW),
            None,
            None,
            &PollHooks::default(),
        );
        assert_eq!(status["generated_at"], json!(NOW));
        assert_eq!(status["poll_interval_seconds"], json!(5));
        let codex = &status["clis"]["codex"];
        assert_eq!(codex["enabled"], json!(true));
        assert_eq!(codex["stale"], json!(false));
        assert_eq!(codex["windows"][0]["used_percent"], json!(57.0));
        assert!(codex.get("resets_available").is_none());
        assert_eq!(
            status["clis"]["grok"]["windows"][0]["used_percent"],
            json!(88.0)
        );
        assert_eq!(status["clis"]["claude_code"]["enabled"], json!(false));
        assert_eq!(status["clis"]["cursor"]["enabled"], json!(false));
        let on_disk: Value =
            serde_json::from_str(&fs::read_to_string(status_path(Some(&sandbox.home))).unwrap())
                .unwrap();
        assert_eq!(
            on_disk["clis"]["codex"]["windows"][0]["used_percent"],
            json!(57.0)
        );
    }

    #[test]
    fn poll_once_writes_cursor_included_pools_from_spending_page() {
        let sandbox = Sandbox::new();
        configure(false);
        write_cursor_credentials(&sandbox.home);
        let mut cfg = load_config(None);
        cfg["clis"]["cursor"]["enabled"] = json!(true);
        save_config(&cfg, None).unwrap();
        struct SpendingOpener;
        impl crate::http::HttpOpener for SpendingOpener {
            fn open(&self, request: &crate::http::HttpRequest) -> Result<HttpResponse, HttpError> {
                let body = if request.url.contains("/api/usage-summary") {
                    CURSOR_USAGE_SUMMARY_FIXTURE.as_bytes().to_vec()
                } else if request.url.contains("/api/dashboard/get-sand-usage-status") {
                    CURSOR_SAND_USAGE_FIXTURE.as_bytes().to_vec()
                } else {
                    panic!("unexpected cursor URL {}", request.url);
                };
                Ok(HttpResponse {
                    status: 200,
                    body,
                    headers: std::collections::HashMap::new(),
                })
            }
        }
        let hooks = PollHooks {
            http: Some(Arc::new(SpendingOpener)),
            ..PollHooks::default()
        };
        let status = poll_once(Some(&sandbox.home), None, Some(NOW), None, None, &hooks);
        let cursor = &status["clis"]["cursor"];
        assert_eq!(cursor["enabled"], json!(true));
        assert_eq!(cursor["windows"][0]["label"], json!("cursor_models"));
        assert_eq!(cursor["windows"][0]["used_percent"], json!(23.1));
        assert_eq!(cursor["windows"][1]["used_percent"], json!(45.0));
        assert_eq!(cursor["windows"][2]["used_percent"], json!(12.0));
        assert_eq!(
            cursor["usage_limit_reset"],
            json!(CURSOR_SPENDING_RESET_EPOCH)
        );
        let blob = cursor.to_string().to_ascii_lowercase();
        assert!(!blob.contains("99.9"), "{blob}");
        assert!(!blob.contains("ondemand"), "{blob}");
    }

    #[test]
    fn previous_status_survives_daemon_restart() {
        let sandbox = Sandbox::new();
        let target = status_path(Some(&sandbox.home));
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            json!({
                "generated_at": NOW,
                "poll_interval_seconds": 5,
                "clis": {"codex": {"resets_available": 1}},
            })
            .to_string(),
        )
        .unwrap();
        let previous = load_previous_status(&sandbox.home).unwrap();
        assert_eq!(previous["clis"]["codex"]["resets_available"], json!(1));
    }

    #[test]
    fn disabled_cli_is_skipped() {
        let sandbox = Sandbox::new();
        let mut cfg = load_config(None);
        cfg["tmux"]["enabled"] = json!(false);
        cfg["clis"]["codex"]["enabled"] = json!(false);
        save_config(&cfg, None).unwrap();
        let status = poll_once(
            Some(&sandbox.home),
            None,
            Some(NOW),
            None,
            None,
            &PollHooks::default(),
        );
        assert_eq!(status["clis"]["codex"], json!({"enabled": false}));
    }

    #[test]
    fn codex_reset_forecast_is_skipped_when_codex_is_disabled() {
        let sandbox = Sandbox::new();
        let mut cfg = load_config(None);
        cfg["tmux"]["enabled"] = json!(false);
        cfg["clis"]["codex"]["enabled"] = json!(false);
        cfg["codex_reset_forecast"]["enabled"] = json!(true);
        save_config(&cfg, None).unwrap();

        struct UnexpectedHttp;
        impl crate::http::HttpOpener for UnexpectedHttp {
            fn open(&self, request: &crate::http::HttpRequest) -> Result<HttpResponse, HttpError> {
                panic!("forecast HTTP must be skipped: {}", request.url);
            }
        }
        let hooks = PollHooks {
            http: Some(Arc::new(UnexpectedHttp)),
            ..PollHooks::default()
        };
        let status = poll_once(Some(&sandbox.home), None, Some(NOW), None, None, &hooks);

        assert_eq!(status["codex_reset_forecast"], json!({"enabled": false}));
    }

    #[test]
    fn deadline_is_measured_from_tick_start() {
        assert_eq!(next_tick_delay(0.0, 5, 1.5), 3.5);
        assert_eq!(next_tick_delay(0.0, 5, 5.0), 0.0);
        assert_eq!(next_tick_delay(0.0, 5, 6.0), 0.0);
    }

    #[test]
    fn fills_missing_reset_from_first_fallback() {
        let snapshot = json!({
            "windows": [{"label": "week", "used_percent": 10.0, "resets_at": null}],
        });
        let fallback = json!({
            "windows": [{"label": "week", "used_percent": 10.0, "resets_at": 99}],
        });
        let filled = carry_window_resets(snapshot, &[&fallback]);
        assert_eq!(filled["windows"][0]["resets_at"], json!(99));
    }

    #[test]
    fn own_reset_wins_and_earlier_fallback_is_preferred() {
        let snapshot = json!({
            "windows": [{"label": "week", "used_percent": 10.0, "resets_at": 1}],
        });
        let fallback = json!({
            "windows": [{"label": "week", "used_percent": 10.0, "resets_at": 99}],
        });
        let filled = carry_window_resets(snapshot.clone(), &[&fallback]);
        assert_eq!(filled["windows"][0]["resets_at"], json!(1));
    }

    /// tmux fake that answers a different `CODEX_HOME` per console window.
    struct PerWindowTmux {
        homes: std::collections::HashMap<String, String>,
    }

    impl crate::tmux::TmuxRunner for PerWindowTmux {
        fn run(&self, args: &[String]) -> (i32, String, String) {
            match args.first().map(String::as_str) {
                // The pane "pid" is the window name so each console can be
                // told apart without a real process.
                Some("list-panes") => {
                    let target = args.get(2).cloned().unwrap_or_default();
                    let window = target.split(':').next_back().unwrap_or("").to_string();
                    (0, format!("{window}\n"), String::new())
                }
                _ => (0, String::new(), String::new()),
            }
        }

        fn process_codex_home(&self, pid: &str) -> Option<String> {
            self.homes.get(pid).cloned()
        }
    }

    fn write_instance_registry(home: &Path, homes: &[(i64, PathBuf)], selected: i64) {
        let dir = crate::collectors::codex::tracker_state_dir(home);
        fs::create_dir_all(&dir).unwrap();
        let rows: Vec<Value> = homes
            .iter()
            .map(|(id, path)| json!({"id": id, "home": path.display().to_string()}))
            .collect();
        fs::write(
            dir.join(crate::collectors::codex::CODEX_INSTANCES_FILE),
            json!({"version": 1, "selected": selected, "instances": rows}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn both_codex_logins_report_their_own_5h_limit_after_a_reboot() {
        // After a reboot no rollout is newer than boot, so the only source of
        // a 5h limit is each account's own console capture. Both must land.
        let sandbox = Sandbox::new();
        configure(true);
        let first = sandbox.home.join(".codex");
        let second = sandbox.home.join(".codex-account-2");
        write_instance_registry(&sandbox.home, &[(1, first.clone()), (2, second.clone())], 1);
        save_pane_capture(
            &sandbox.home,
            "codex",
            concat!(
                "\u{2502}  5h limit:      [\u{2588}\u{2588}\u{2591}\u{2591}] 40% left (resets 17:05) \u{2502}\n",
                "\u{2502}  Weekly limit:  [\u{2588}\u{2591}\u{2591}\u{2591}] 90% left               \u{2502}\n",
            ),
        );
        save_pane_capture(
            &sandbox.home,
            "codex-2",
            concat!(
                "\u{2502}  5h limit:      [\u{2588}\u{2591}\u{2591}\u{2591}] 75% left (resets 18:20) \u{2502}\n",
                "\u{2502}  Weekly limit:  [\u{2588}\u{2591}\u{2591}\u{2591}] 95% left               \u{2502}\n",
            ),
        );
        let hooks = PollHooks {
            tmux: Some(Arc::new(PerWindowTmux {
                homes: std::collections::HashMap::from([
                    ("codex".to_string(), first.display().to_string()),
                    ("codex-2".to_string(), second.display().to_string()),
                ]),
            })),
            tmux_available: Some(true),
            ..PollHooks::default()
        };
        let never_refresh = |_: &str, _: &str, _: &str| false;
        let status = poll_once(
            Some(&sandbox.home),
            None,
            Some(NOW),
            Some(&never_refresh),
            None,
            &hooks,
        );
        let instances = status["clis"]["codex"]["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 2);
        // 100 - "left" percentage, per account, from that account's console.
        assert_eq!(instances[0]["windows"][0]["used_percent"], json!(60.0));
        assert_eq!(instances[1]["windows"][0]["used_percent"], json!(25.0));
        assert_eq!(
            instances[0]["windows"][0]["used_percent"],
            status["clis"]["codex"]["windows"][0]["used_percent"],
            "the selected account is mirrored at the top level"
        );
    }

    #[test]
    fn pane_status_does_not_copy_onto_the_other_codex_account() {
        // Reproduces the 13:47 glitch: the debug console /status is the
        // primary team account (0% / 43% left) while ca2 / CX2 is plus.
        let snapshot = json!({
            "selected": 2,
            "windows": [
                {"label": "5h", "used_percent": 29.0},
                {"label": "week", "used_percent": 4.0}
            ],
            "plan": "plus",
            "observed_at": 50.0,
            "instances": [
                {
                    "id": 1,
                    "plan": "team",
                    "windows": [
                        {"label": "5h", "used_percent": 100.0},
                        {"label": "week", "used_percent": 57.0}
                    ]
                },
                {
                    "id": 2,
                    "plan": "plus",
                    "windows": [
                        {"label": "5h", "used_percent": 29.0},
                        {"label": "week", "used_percent": 4.0}
                    ]
                },
            ]
        });
        let pane = concat!(
            "│  5h limit:             [░░░░░░░░░░░░░░░░░░░░] 0% left (resets 17:05) │\n",
            "│  Weekly limit:         [█████████░░░░░░░░░░░] 43% left               │\n",
        );
        let applied = apply_codex_live_pane(snapshot, pane, 80.0, "auto", 1800, None, 80.0, None);
        assert_eq!(applied["selected"], json!(2));
        assert_eq!(applied["plan"], json!("plus"));
        assert_eq!(applied["windows"][0]["used_percent"], json!(29.0));
        assert_eq!(applied["windows"][1]["used_percent"], json!(4.0));
        assert_eq!(
            applied["instances"][0]["windows"][0]["used_percent"],
            json!(100.0)
        );
        assert_eq!(
            applied["instances"][0]["windows"][1]["used_percent"],
            json!(57.0)
        );
        assert_eq!(
            applied["instances"][1]["windows"][0]["used_percent"],
            json!(29.0)
        );
        assert_eq!(applied["instances"][1]["plan"], json!("plus"));
    }

    #[test]
    fn pane_status_updates_selected_account_when_console_follows_its_home() {
        // Hitting 0 remaining jumps used-percent by more than the 15-point
        // identity window. The console process CODEX_HOME is the selected
        // login, so /status must still replace the stale 6-85 reading.
        let snapshot = json!({
            "selected": 2,
            "windows": [
                {"label": "5h", "used_percent": 94.0},
                {"label": "week", "used_percent": 15.0}
            ],
            "plan": "plus",
            "observed_at": 50.0,
            "instances": [
                {
                    "id": 1,
                    "home": "/home/u/.codex",
                    "plan": "team",
                    "windows": [
                        {"label": "5h", "used_percent": 100.0},
                        {"label": "week", "used_percent": 57.0}
                    ]
                },
                {
                    "id": 2,
                    "home": "/home/u/.codex-account-2",
                    "plan": "plus",
                    "windows": [
                        {"label": "5h", "used_percent": 94.0},
                        {"label": "week", "used_percent": 15.0}
                    ]
                },
            ]
        });
        let pane = concat!(
            "│  5h limit:             [░░░░░░░░░░░░░░░░░░░░] 0% left (resets 18:34) │\n",
            "│  Weekly limit:         [█████████████████░░░] 86% left               │\n",
        );
        let applied = apply_codex_live_pane(
            snapshot,
            pane,
            80.0,
            "auto",
            1800,
            None,
            80.0,
            Some("/home/u/.codex-account-2"),
        );
        assert_eq!(applied["windows"][0]["used_percent"], json!(100.0));
        assert_eq!(applied["windows"][1]["used_percent"], json!(14.0));
        assert_eq!(
            applied["instances"][1]["windows"][0]["used_percent"],
            json!(100.0)
        );
        assert_eq!(
            applied["instances"][0]["windows"][0]["used_percent"],
            json!(100.0)
        );
    }

    #[test]
    fn console_home_owns_the_pane_when_both_accounts_sit_at_the_same_usage() {
        // Two exhausted logins read identically, so usage matching cannot tell
        // them apart. The console CODEX_HOME can, and must.
        let snapshot = json!({
            "selected": 1,
            "windows": [
                {"label": "5h", "used_percent": 100.0},
                {"label": "week", "used_percent": 16.0}
            ],
            "plan": "team",
            "observed_at": 50.0,
            "instances": [
                {
                    "id": 1,
                    "home": "/home/u/.codex",
                    "plan": "team",
                    "windows": [
                        {"label": "5h", "used_percent": 100.0},
                        {"label": "week", "used_percent": 16.0}
                    ]
                },
                {
                    "id": 2,
                    "home": "/home/u/.codex-account-2",
                    "plan": "plus",
                    "windows": [
                        {"label": "5h", "used_percent": 100.0},
                        {"label": "week", "used_percent": 16.0}
                    ]
                },
            ]
        });
        let pane = concat!(
            "│  5h limit:             [████████████████████] 70% left (resets 17:05) │\n",
            "│  Weekly limit:         [████████████████████] 90% left               │\n",
        );
        let applied = apply_codex_live_pane(
            snapshot,
            pane,
            80.0,
            "auto",
            1800,
            None,
            80.0,
            Some("/home/u/.codex-account-2"),
        );
        // Instance 2 took the fresh console reading; instance 1 kept its own.
        assert_eq!(
            applied["instances"][1]["windows"][0]["used_percent"],
            json!(30.0)
        );
        assert_eq!(
            applied["instances"][0]["windows"][0]["used_percent"],
            json!(100.0)
        );
        // Selected is still 1, so the compact bar keeps showing account 1.
        assert_eq!(applied["windows"][0]["used_percent"], json!(100.0));
    }

    #[test]
    fn selection_change_does_not_carry_the_previous_account_forward() {
        let fresh = json!({
            "selected": 2,
            "observed_at": 100.0,
            "windows": [{"label": "5h", "used_percent": 3.0}],
            "instances": [{"id": 2, "home": "/home/u/.codex-account-2"}],
        });
        let previous = json!({
            "selected": 1,
            "observed_at": 900.0,
            "windows": [{"label": "5h", "used_percent": 100.0}],
        });
        assert!(codex_same_account_last("codex", &fresh, Some(&previous)).is_none());

        let same_account = json!({
            "selected": 2,
            "observed_at": 900.0,
            "windows": [{"label": "5h", "used_percent": 100.0}],
        });
        assert!(codex_same_account_last("codex", &fresh, Some(&same_account)).is_some());
        assert!(codex_same_account_last("claude_code", &fresh, Some(&previous)).is_some());
    }

    #[test]
    fn pane_status_updates_the_account_it_matches() {
        let snapshot = json!({
            "selected": 2,
            "windows": [
                {"label": "5h", "used_percent": 29.0},
                {"label": "week", "used_percent": 4.0}
            ],
            "plan": "plus",
            "observed_at": 50.0,
            "instances": [
                {
                    "id": 1,
                    "plan": "team",
                    "windows": [
                        {"label": "5h", "used_percent": 100.0},
                        {"label": "week", "used_percent": 57.0}
                    ]
                },
                {
                    "id": 2,
                    "plan": "plus",
                    "windows": [
                        {"label": "5h", "used_percent": 29.0},
                        {"label": "week", "used_percent": 4.0}
                    ]
                },
            ]
        });
        let pane = concat!(
            "│  5h limit:             [████████████████████] 65% left (resets 17:05) │\n",
            "│  Weekly limit:         [████████████████████] 94% left               │\n",
        );
        let applied = apply_codex_live_pane(snapshot, pane, 80.0, "auto", 1800, None, 80.0, None);
        assert_eq!(applied["windows"][0]["used_percent"], json!(35.0));
        assert_eq!(applied["windows"][1]["used_percent"], json!(6.0));
        assert_eq!(
            applied["instances"][1]["windows"][0]["used_percent"],
            json!(35.0)
        );
        assert_eq!(
            applied["instances"][0]["windows"][0]["used_percent"],
            json!(100.0)
        );
    }

    #[test]
    fn grok_billing_authoritative_over_live_pane() {
        let billing = json!({
            "source": "grok_logs",
            "observed_at": 10.0,
            "plan": "SuperGrok",
            "windows": [{"label": "week", "used_percent": 88.0, "resets_at": null}],
            "error": null,
        });
        let result = resolve_grok_snapshot(
            &billing,
            Some("Weekly limit left: 45%\n"),
            None,
            None,
            None,
            "auto",
            true,
            20.0,
            1800,
            false,
        );
        assert_eq!(result["windows"][0]["used_percent"], json!(88.0));
        assert_eq!(result["source"], json!("grok_logs"));
    }
}
