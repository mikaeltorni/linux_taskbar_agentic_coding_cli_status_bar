//! Claude Code: OAuth usage endpoint with Claude Code's own credentials.

use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};

use super::util::{now_epoch, to_epoch, to_percent};
use crate::config::CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS;
use crate::http::{header_ci, HttpError, HttpOpener, HttpRequest, UreqOpener};
use crate::logging;
use crate::snapshot::{carry_last_reading, snapshot, Window};

pub const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const CLAUDE_OAUTH_BETA: &str = "oauth-2025-04-20";
pub const CLAUDE_UA_FALLBACK_VERSION: &str = "2.0.0";
pub const CLAUDE_OAUTH_REFRESH_LOCK_NAME: &str = ".oauth_refresh.lock";
pub const CLAUDE_OAUTH_REFRESH_LOCK_TIMEOUT_SECONDS: f64 = 5.0;

fn oauth_token_url() -> String {
    std::env::var("CLAUDE_CODE_OAUTH_TOKEN_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com/v1/oauth/token".into())
}

fn oauth_client_id() -> String {
    std::env::var("CLAUDE_CODE_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| "9d1c250a-e61b-44d9-88ed-5944d1962f5e".into())
}

const DEFAULT_SCOPES: &[&str] = &[
    "user:file_upload",
    "user:inference",
    "user:mcp_servers",
    "user:profile",
    "user:sessions:claude_code",
];

fn version_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[0-9][0-9A-Za-z_.-]*$").unwrap())
}

/// Return the `claude-code/<version>` User-Agent the endpoint expects.
pub fn claude_user_agent(home: &Path) -> String {
    for candidate in [
        home.join(".local")
            .join("state")
            .join("claude")
            .join("latest-version"),
        home.join(".claude").join("version"),
    ] {
        if let Ok(version) = fs::read_to_string(&candidate) {
            let version = version.trim();
            if version_re().is_match(version) {
                return format!("claude-code/{version}");
            }
        }
    }
    let installed = home.join(".local").join("bin").join("claude");
    if let Ok(resolved) = fs::canonicalize(&installed) {
        if let Some(version) = resolved.file_name().and_then(|n| n.to_str()) {
            if version_re().is_match(version) {
                return format!("claude-code/{version}");
            }
        }
    }
    format!("claude-code/{CLAUDE_UA_FALLBACK_VERSION}")
}

fn claude_access_token(home: &Path) -> (Option<String>, Option<Value>, Option<String>) {
    let creds_file = home.join(".claude").join(".credentials.json");
    let creds: Value = match fs::read_to_string(&creds_file) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(exc) => return (None, None, Some(format!("credentials unavailable: {exc}"))),
        },
        Err(exc) => return (None, None, Some(format!("credentials unavailable: {exc}"))),
    };
    let oauth = creds.get("claudeAiOauth").cloned().unwrap_or(json!({}));
    let token = oauth.get("accessToken").and_then(Value::as_str);
    if token.map(str::is_empty).unwrap_or(true) {
        return (
            None,
            oauth.get("subscriptionType").cloned(),
            Some("no accessToken in credentials".into()),
        );
    }
    if let Some(expires_at) = oauth.get("expiresAt").and_then(Value::as_f64) {
        if expires_at / 1000.0 < now_epoch() {
            return (
                None,
                oauth.get("subscriptionType").cloned(),
                Some("access token expired".into()),
            );
        }
    }
    (
        token.map(str::to_string),
        oauth.get("subscriptionType").cloned(),
        None,
    )
}

/// Return True when a Claude Code process appears to be running.
pub fn claude_is_running(proc_root: &Path) -> bool {
    let entries = match fs::read_dir(proc_root) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry
            .file_name()
            .to_string_lossy()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let raw = match fs::read(entry.path().join("cmdline")) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        if raw.is_empty() {
            continue;
        }
        for part in raw.split(|b| *b == 0) {
            if part.is_empty() {
                continue;
            }
            let arg = String::from_utf8_lossy(part);
            let base = Path::new(arg.as_ref())
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if base == "claude" {
                return true;
            }
            if base == "cli.js" && arg.contains("claude-code") {
                return true;
            }
        }
    }
    false
}

fn claude_refresh_lock_path(home: &Path) -> PathBuf {
    home.join(".claude").join(CLAUDE_OAUTH_REFRESH_LOCK_NAME)
}

fn try_flock_exclusive(fd: i32) -> bool {
    unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

fn unlock_fd(fd: i32) {
    unsafe {
        libc::flock(fd, libc::LOCK_UN);
    }
}

/// Read and parse Claude Code's credentials document.
///
/// Parameters:
///   creds_file: path to `~/.claude/.credentials.json`.
///
/// Returns the parsed document, or a human-readable error string.
fn read_credentials(creds_file: &Path) -> Result<Value, String> {
    let text =
        fs::read_to_string(creds_file).map_err(|exc| format!("credentials unavailable: {exc}"))?;
    serde_json::from_str(&text).map_err(|exc| format!("credentials unavailable: {exc}"))
}

/// Return the stored access token when it is present and not yet expired.
///
/// Expiry is judged against the wall clock (`now_epoch`), the same clock
/// :func:`claude_access_token` uses, so both agree on whether the credentials
/// file currently holds a usable token; the collector's injectable `now` only
/// drives polling intervals and refresh-token lifetimes.
///
/// Parameters:
///   oauth: the `claudeAiOauth` object from the credentials document.
///
/// Returns the usable access token, or None when it is missing or expired.
fn fresh_access_token(oauth: &Value) -> Option<String> {
    let token = oauth.get("accessToken").and_then(Value::as_str)?;
    if token.is_empty() {
        return None;
    }
    match oauth.get("expiresAt").and_then(Value::as_f64) {
        Some(expires_at) if expires_at / 1000.0 < now_epoch() => None,
        _ => Some(token.to_string()),
    }
}

/// Refresh Claude Code's OAuth access token using the stored refresh token.
///
/// A still valid token is returned untouched. An *expired* token is refreshed
/// even while Claude Code is running: sessions launched with a long-lived
/// `CLAUDE_CODE_OAUTH_TOKEN` (Agent Command Center's token file) never rewrite
/// `~/.claude/.credentials.json`, so nothing else would ever renew it and the
/// usage tile would stay frozen forever. The refresh runs under the shared
/// advisory lock, re-reads the credentials once the lock is held (so a token
/// another writer just rotated in is used instead of a stale one), and writes
/// the result back atomically.
///
/// Parameters:
///   home: home directory holding `.claude/.credentials.json`.
///   now: epoch seconds override for tests.
///   opener: HTTP opener override for tests.
///   is_running: predicate reporting whether Claude Code is running.
///   lock_timeout: seconds to wait for the refresh lock.
///
/// Returns `(access token, subscription type, error message)`.
pub fn claude_refresh_oauth_token(
    home: &Path,
    now: Option<f64>,
    opener: Option<&dyn HttpOpener>,
    is_running: Option<&dyn Fn() -> bool>,
    lock_timeout: Option<f64>,
) -> (Option<String>, Option<Value>, Option<String>) {
    let now = now.unwrap_or_else(now_epoch);
    let running = is_running.unwrap_or(&|| claude_is_running(Path::new("/proc")));
    let lock_timeout = lock_timeout.unwrap_or(CLAUDE_OAUTH_REFRESH_LOCK_TIMEOUT_SECONDS);
    let creds_file = home.join(".claude").join(".credentials.json");
    let mut creds: Value = match read_credentials(&creds_file) {
        Ok(value) => value,
        Err(error) => return (None, None, Some(error)),
    };
    let mut oauth = creds.get("claudeAiOauth").cloned().unwrap_or(json!({}));
    if running() {
        if let Some(token) = fresh_access_token(&oauth) {
            logging::info("claude oauth token still valid; refresh skipped");
            return (Some(token), oauth.get("subscriptionType").cloned(), None);
        }
    }
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    let lock_path = claude_refresh_lock_path(home);
    let _ = fs::create_dir_all(lock_path.parent().unwrap_or(Path::new(".")));
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&lock_path)
        .ok();
    if let Some(ref file) = lock_file {
        let fd = file.as_raw_fd();
        let deadline = Instant::now() + Duration::from_secs_f64(lock_timeout);
        loop {
            if try_flock_exclusive(fd) {
                break;
            }
            if Instant::now() >= deadline {
                logging::warn("claude oauth refresh lock timed out");
                return (
                    None,
                    oauth.get("subscriptionType").cloned(),
                    Some("refresh lock timed out".into()),
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Whoever held the lock (or Claude Code itself) may have refreshed the
        // credentials while we waited; re-read them so we never spend a refresh
        // token that has already been rotated away.
        if let Ok(latest) = read_credentials(&creds_file) {
            let latest_oauth = latest.get("claudeAiOauth").cloned().unwrap_or(json!({}));
            if let Some(token) = fresh_access_token(&latest_oauth) {
                unlock_fd(fd);
                logging::info("claude oauth token already refreshed by another writer");
                return (
                    Some(token),
                    latest_oauth.get("subscriptionType").cloned(),
                    None,
                );
            }
            creds = latest;
            oauth = latest_oauth;
        }
    } else {
        logging::warn("claude oauth refresh lock unavailable");
    }
    let Some(refresh_token) = oauth
        .get("refreshToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
    else {
        if let Some(file) = lock_file.as_ref() {
            unlock_fd(file.as_raw_fd());
        }
        return (
            None,
            oauth.get("subscriptionType").cloned(),
            Some("no refresh token in credentials".into()),
        );
    };
    if let Some(refresh_expires_at) = oauth.get("refreshTokenExpiresAt").and_then(Value::as_f64) {
        if refresh_expires_at / 1000.0 < now {
            if let Some(file) = lock_file.as_ref() {
                unlock_fd(file.as_raw_fd());
            }
            return (
                None,
                oauth.get("subscriptionType").cloned(),
                Some("refresh token expired".into()),
            );
        }
    }
    let scopes: Vec<String> = match oauth.get("scopes").and_then(Value::as_array) {
        Some(list) if !list.is_empty() => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => DEFAULT_SCOPES.iter().map(|s| (*s).to_string()).collect(),
    };
    let body = json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": oauth_client_id(),
        "scope": scopes.join(" "),
    });
    let request = HttpRequest {
        url: oauth_token_url(),
        method: "POST".into(),
        headers: vec![
            ("Content-Type".into(), "application/json".into()),
            ("Accept".into(), "application/json".into()),
            ("User-Agent".into(), claude_user_agent(home)),
            ("anthropic-beta".into(), CLAUDE_OAUTH_BETA.into()),
        ],
        body: Some(serde_json::to_vec(&body).unwrap_or_default()),
        timeout: Duration::from_secs(15),
    };
    logging::info("claude oauth access token expired; refreshing");
    let result = opener.open(&request);
    let payload = match result {
        Ok(response) => match serde_json::from_str::<Value>(&response.text()) {
            Ok(v) => v,
            Err(exc) => {
                if let Some(file) = lock_file {
                    unlock_fd(file.as_raw_fd());
                }
                logging::error(format!("claude oauth refresh failed: {exc}"));
                return (
                    None,
                    oauth.get("subscriptionType").cloned(),
                    Some(format!("refresh failed: {exc}")),
                );
            }
        },
        Err(exc) => {
            if let Some(file) = lock_file {
                unlock_fd(file.as_raw_fd());
            }
            if let Some(status) = exc.status {
                let body_text: String = String::from_utf8_lossy(&exc.body)
                    .chars()
                    .take(200)
                    .collect();
                logging::error(format!("claude oauth refresh HTTP {status}: {body_text}"));
                return (
                    None,
                    oauth.get("subscriptionType").cloned(),
                    Some(format!("refresh HTTP {status}")),
                );
            }
            logging::error(format!("claude oauth refresh failed: {exc}"));
            return (
                None,
                oauth.get("subscriptionType").cloned(),
                Some(format!("refresh failed: {exc}")),
            );
        }
    };
    let Some(new_access) = payload
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        if let Some(file) = lock_file {
            unlock_fd(file.as_raw_fd());
        }
        return (
            None,
            oauth.get("subscriptionType").cloned(),
            Some("refresh response missing access_token".into()),
        );
    };
    oauth["accessToken"] = json!(new_access);
    if let Some(new_refresh) = payload.get("refresh_token").and_then(Value::as_str) {
        if !new_refresh.is_empty() {
            oauth["refreshToken"] = json!(new_refresh);
        }
    }
    if let Some(expires_in) = payload.get("expires_in").and_then(Value::as_f64) {
        oauth["expiresAt"] = json!(((now + expires_in) * 1000.0) as i64);
    }
    if let Some(refresh_expires_in) = payload
        .get("refresh_token_expires_in")
        .and_then(Value::as_f64)
    {
        oauth["refreshTokenExpiresAt"] = json!(((now + refresh_expires_in) * 1000.0) as i64);
    }
    creds["claudeAiOauth"] = oauth.clone();
    let write_result = (|| -> std::io::Result<()> {
        if let Some(parent) = creds_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = creds_file.with_file_name(".credentials.tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            serde_json::to_writer_pretty(&mut file, &creds)?;
        }
        let mode = fs::metadata(&creds_file).map(|m| m.permissions()).ok();
        if let Some(perms) = mode {
            let _ = fs::set_permissions(&tmp, perms);
        } else {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(&tmp, &creds_file)?;
        Ok(())
    })();
    if let Some(file) = lock_file {
        unlock_fd(file.as_raw_fd());
    }
    if let Err(exc) = write_result {
        logging::error(format!("claude oauth refresh write-back failed: {exc}"));
        return (
            Some(new_access.to_string()),
            oauth.get("subscriptionType").cloned(),
            Some(format!("refresh write-back failed: {exc}")),
        );
    }
    logging::info("claude oauth refresh succeeded; credentials updated");
    (
        Some(new_access.to_string()),
        oauth.get("subscriptionType").cloned(),
        None,
    )
}

fn retry_after_seconds(exc: &HttpError, fallback: f64) -> f64 {
    header_ci(&exc.headers, "Retry-After")
        .filter(|v| v.chars().all(|c| c.is_ascii_digit()))
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

fn claude_windows(payload: &Value) -> Vec<Window> {
    let labels = [
        ("five_hour", "5h"),
        ("seven_day", "week"),
        ("seven_day_opus", "opus"),
    ];
    let mut windows = Vec::new();
    for (key, label) in labels {
        let Some(window) = payload.get(key).filter(|v| v.is_object()) else {
            continue;
        };
        let used_value = if window
            .get("utilization")
            .map(|v| !v.is_null())
            .unwrap_or(false)
        {
            window.get("utilization")
        } else {
            window.get("used_percentage")
        };
        let Some(used) = used_value.and_then(to_percent) else {
            continue;
        };
        windows.push(Window {
            label: label.into(),
            used_percent: used,
            resets_at: window.get("resets_at").and_then(to_epoch),
        });
    }
    let mut existing: std::collections::HashSet<String> =
        windows.iter().map(|w| w.label.clone()).collect();
    if let Some(limits) = payload.get("limits").and_then(Value::as_array) {
        for limit in limits {
            if !limit.is_object()
                || limit.get("kind").and_then(Value::as_str) != Some("weekly_scoped")
            {
                continue;
            }
            let display_name = limit
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let Some(display_name) = display_name else {
                continue;
            };
            let label = display_name.to_lowercase();
            let Some(used) = limit.get("percent").and_then(to_percent) else {
                continue;
            };
            if existing.contains(&label) {
                continue;
            }
            windows.push(Window {
                label: label.clone(),
                used_percent: used,
                resets_at: limit.get("resets_at").and_then(to_epoch),
            });
            existing.insert(label);
        }
    }
    windows
}

/// Collect Claude Code rate limits from the OAuth usage endpoint.
pub fn collect_claude(
    home: &Path,
    last_result: Option<&Value>,
    now: Option<f64>,
    min_interval: Option<f64>,
    opener: Option<&dyn HttpOpener>,
    refresh_opener: Option<&dyn HttpOpener>,
    is_running: Option<&dyn Fn() -> bool>,
) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let min_interval = min_interval.unwrap_or(CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS as f64);
    if let Some(last) = last_result {
        if let Some(checked) = last.get("checked_at").and_then(Value::as_f64) {
            let retry_until = last
                .get("retry_after_until")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            if now - checked < min_interval || now < retry_until {
                return last.clone();
            }
        }
    }
    let (mut token, mut subscription, mut error) = claude_access_token(home);
    if token.is_none() {
        if error.as_deref() == Some("access token expired") {
            let (refreshed, sub, refresh_error) =
                claude_refresh_oauth_token(home, Some(now), refresh_opener, is_running, None);
            if let Some(sub) = sub {
                subscription = Some(sub);
            }
            if let Some(refreshed) = refreshed {
                token = Some(refreshed);
                error = None;
            } else if let Some(refresh_error) = &refresh_error {
                logging::info(format!(
                    "claude oauth refresh not attempted: {refresh_error}"
                ));
            }
        }
        if token.is_none() {
            let mut snap = snapshot(
                "claude_oauth_endpoint",
                None,
                subscription,
                Vec::new(),
                error.clone(),
            );
            snap["checked_at"] = json!(now);
            snap = carry_last_reading(snap, last_result);
            if error.as_deref() == Some("access token expired")
                && snap
                    .get("windows")
                    .and_then(Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
            {
                snap["error"] = Value::Null;
            }
            return snap;
        }
    }
    let request = HttpRequest {
        url: CLAUDE_USAGE_URL.into(),
        method: "GET".into(),
        headers: vec![
            ("Authorization".into(), format!("Bearer {}", token.unwrap())),
            ("anthropic-beta".into(), CLAUDE_OAUTH_BETA.into()),
            ("User-Agent".into(), claude_user_agent(home)),
            ("Content-Type".into(), "application/json".into()),
        ],
        body: None,
        timeout: Duration::from_secs(20),
    };
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    match opener.open(&request) {
        Ok(response) => match serde_json::from_str::<Value>(&response.text()) {
            Ok(payload) => {
                let windows = claude_windows(&payload);
                if windows.is_empty() {
                    let mut snap = snapshot(
                        "claude_oauth_endpoint",
                        None,
                        subscription,
                        Vec::new(),
                        Some("no rate-limit windows in endpoint response".into()),
                    );
                    snap["checked_at"] = json!(now);
                    carry_last_reading(snap, last_result)
                } else {
                    let mut snap = snapshot(
                        "claude_oauth_endpoint",
                        Some(now),
                        subscription,
                        windows,
                        None,
                    );
                    snap["checked_at"] = json!(now);
                    snap
                }
            }
            Err(exc) => {
                logging::error(format!("claude usage endpoint failed: {exc}"));
                let mut snap = snapshot(
                    "claude_oauth_endpoint",
                    None,
                    subscription,
                    Vec::new(),
                    Some(exc.to_string()),
                );
                snap["checked_at"] = json!(now);
                carry_last_reading(snap, last_result)
            }
        },
        Err(exc) => {
            if let Some(status) = exc.status {
                logging::error(format!("claude usage endpoint HTTP {status}"));
                let mut snap = snapshot(
                    "claude_oauth_endpoint",
                    None,
                    subscription,
                    Vec::new(),
                    Some(format!("HTTP {status}")),
                );
                snap["checked_at"] = json!(now);
                if status == 429 {
                    let retry_after = retry_after_seconds(&exc, min_interval);
                    snap["retry_after_until"] = json!(now + retry_after);
                    logging::warn(format!(
                        "claude usage endpoint locked out; retrying after {retry_after:.0} s"
                    ));
                }
                carry_last_reading(snap, last_result)
            } else {
                logging::error(format!("claude usage endpoint failed: {exc}"));
                let mut snap = snapshot(
                    "claude_oauth_endpoint",
                    None,
                    subscription,
                    Vec::new(),
                    Some(exc.to_string()),
                );
                snap["checked_at"] = json!(now);
                carry_last_reading(snap, last_result)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpResponse;
    use crate::test_support::{write_claude_credentials, write_claude_credentials_full, Sandbox};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct JsonOpener {
        body: String,
        status: u16,
        headers: HashMap<String, String>,
        calls: AtomicUsize,
    }

    impl JsonOpener {
        fn ok(body: Value) -> Self {
            Self {
                body: body.to_string(),
                status: 200,
                headers: HashMap::new(),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl HttpOpener for JsonOpener {
        fn open(&self, _request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.status >= 400 {
                return Err(HttpError {
                    message: format!("HTTP {}", self.status),
                    status: Some(self.status),
                    body: self.body.as_bytes().to_vec(),
                    headers: self.headers.clone(),
                });
            }
            Ok(HttpResponse {
                status: self.status,
                body: self.body.as_bytes().to_vec(),
                headers: self.headers.clone(),
            })
        }
    }

    fn usage_payload() -> Value {
        json!({
            "five_hour": {"utilization": 12.0, "resets_at": 1783080000_i64},
            "seven_day": {"utilization": 40.0, "resets_at": 1783570000_i64},
        })
    }

    #[test]
    fn user_agent_follows_installed_claude_symlink_version() {
        let sandbox = Sandbox::new();
        let versions = sandbox
            .home
            .join(".local")
            .join("share")
            .join("claude")
            .join("versions");
        fs::create_dir_all(&versions).unwrap();
        let target = versions.join("2.1.4");
        fs::write(&target, "").unwrap();
        let bin_dir = sandbox.home.join(".local").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        std::os::unix::fs::symlink(&target, bin_dir.join("claude")).unwrap();
        assert_eq!(claude_user_agent(&sandbox.home), "claude-code/2.1.4");
    }

    #[test]
    fn fetches_windows_from_endpoint() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", 3_600_000);
        let opener = JsonOpener::ok(usage_payload());
        let result = collect_claude(
            &sandbox.home,
            None,
            Some(1000.0),
            None,
            Some(&opener),
            None,
            Some(&|| false),
        );
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(result["windows"][0]["label"], json!("5h"));
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
        assert_eq!(result["windows"][1]["used_percent"], json!(40.0));
        assert_eq!(result["plan"], json!("max"));
    }

    #[test]
    fn used_percentage_field_variant() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", 3_600_000);
        let opener = JsonOpener::ok(json!({
            "five_hour": {"used_percentage": 8.0},
            "seven_day": {"used_percentage": 20.0},
        }));
        let result = collect_claude(
            &sandbox.home,
            None,
            Some(1000.0),
            None,
            Some(&opener),
            None,
            Some(&|| false),
        );
        assert_eq!(result["windows"][0]["used_percent"], json!(8.0));
    }

    #[test]
    fn fetches_fable_scoped_weekly_limit() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", 3_600_000);
        let opener = JsonOpener::ok(json!({
            "five_hour": {"utilization": 12.0},
            "seven_day": {"utilization": 40.0},
            "limits": [{
                "kind": "weekly_scoped",
                "percent": 62.0,
                "resets_at": 1784000000_i64,
                "scope": {"model": {"display_name": "Fable"}},
            }],
        }));
        let result = collect_claude(
            &sandbox.home,
            None,
            Some(1000.0),
            None,
            Some(&opener),
            None,
            Some(&|| false),
        );
        let labels: Vec<_> = result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["label"].as_str().unwrap())
            .collect();
        assert_eq!(labels, ["5h", "week", "fable"]);
        assert_eq!(result["windows"][2]["used_percent"], json!(62.0));
    }

    #[test]
    fn endpoint_throttled_by_min_interval() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", 3_600_000);
        let opener = JsonOpener::ok(usage_payload());
        let first = collect_claude(
            &sandbox.home,
            None,
            Some(1000.0),
            Some(180.0),
            Some(&opener),
            None,
            Some(&|| false),
        );
        let _second = collect_claude(
            &sandbox.home,
            Some(&first),
            Some(1100.0),
            Some(180.0),
            Some(&opener),
            None,
            Some(&|| false),
        );
        assert_eq!(opener.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn error_preserves_previous_windows() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", 3_600_000);
        let last = json!({
            "windows": [{"label": "5h", "used_percent": 12.0, "resets_at": null}],
            "observed_at": 1.0,
            "plan": "max",
        });
        let opener = JsonOpener {
            body: String::new(),
            status: 500,
            headers: HashMap::new(),
            calls: AtomicUsize::new(0),
        };
        let result = collect_claude(
            &sandbox.home,
            Some(&last),
            Some(1000.0),
            Some(1.0),
            Some(&opener),
            None,
            Some(&|| false),
        );
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
        assert_eq!(result["error"], json!("HTTP 500"));
    }

    #[test]
    fn honors_429_retry_after() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", 3_600_000);
        let mut headers = HashMap::new();
        headers.insert("Retry-After".into(), "300".into());
        let opener = JsonOpener {
            body: String::new(),
            status: 429,
            headers,
            calls: AtomicUsize::new(0),
        };
        let last = json!({
            "windows": [{"label": "5h", "used_percent": 12.0, "resets_at": null}],
            "observed_at": 1.0,
        });
        let result = collect_claude(
            &sandbox.home,
            Some(&last),
            Some(1000.0),
            Some(60.0),
            Some(&opener),
            None,
            Some(&|| false),
        );
        assert_eq!(result["retry_after_until"], json!(1300.0));
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
    }

    #[test]
    fn missing_credentials_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_claude(
            &sandbox.home,
            None,
            Some(1.0),
            None,
            None,
            None,
            Some(&|| false),
        );
        assert!(result["error"].as_str().unwrap().contains("credentials"));
    }

    #[test]
    fn expired_token_keeps_last_reading_without_alarming() {
        let sandbox = Sandbox::new();
        write_claude_credentials(&sandbox.home, "sk-test", -1_000);
        let last = json!({
            "windows": [{"label": "5h", "used_percent": 12.0, "resets_at": null}],
            "observed_at": 1.0,
            "plan": "max",
        });
        let result = collect_claude(
            &sandbox.home,
            Some(&last),
            Some(1000.0),
            Some(1.0),
            None,
            None,
            Some(&|| true),
        );
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
        assert!(result.get("error").unwrap().is_null());
    }

    #[test]
    fn refreshes_expired_token_then_fetches_usage() {
        let sandbox = Sandbox::new();
        write_claude_credentials_full(
            &sandbox.home,
            "old",
            -1_000,
            Some("refresh-1"),
            86_400_000 * 30,
            None,
        );
        struct RefreshThenUsage {
            calls: Mutex<Vec<String>>,
        }
        impl HttpOpener for RefreshThenUsage {
            fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
                self.calls.lock().unwrap().push(request.url.clone());
                if request.url.contains("/oauth/token") {
                    return Ok(HttpResponse {
                        status: 200,
                        body: json!({
                            "access_token": "new-token",
                            "refresh_token": "refresh-2",
                            "expires_in": 3600,
                        })
                        .to_string()
                        .into_bytes(),
                        headers: HashMap::new(),
                    });
                }
                Ok(HttpResponse {
                    status: 200,
                    body: usage_payload().to_string().into_bytes(),
                    headers: HashMap::new(),
                })
            }
        }
        let opener = RefreshThenUsage {
            calls: Mutex::new(Vec::new()),
        };
        let result = collect_claude(
            &sandbox.home,
            None,
            Some(1000.0),
            Some(1.0),
            Some(&opener),
            Some(&opener),
            Some(&|| false),
        );
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
        let creds: Value = serde_json::from_str(
            &fs::read_to_string(sandbox.home.join(".claude").join(".credentials.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(creds["claudeAiOauth"]["accessToken"], json!("new-token"));
    }

    #[test]
    fn refreshes_expired_token_while_claude_is_running() {
        // ACC-launched Claude sessions authenticate with a long-lived token and
        // never rewrite .credentials.json, so the poller must refresh an expired
        // token itself even while Claude Code is running.
        let sandbox = Sandbox::new();
        write_claude_credentials_full(
            &sandbox.home,
            "old",
            -1_000,
            Some("refresh-1"),
            86_400_000 * 30,
            None,
        );
        struct RefreshThenUsage;
        impl HttpOpener for RefreshThenUsage {
            fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
                if request.url.contains("oauth/token") {
                    return Ok(HttpResponse {
                        status: 200,
                        body: json!({"access_token": "new-token", "expires_in": 3600})
                            .to_string()
                            .into_bytes(),
                        headers: HashMap::new(),
                    });
                }
                Ok(HttpResponse {
                    status: 200,
                    body: usage_payload().to_string().into_bytes(),
                    headers: HashMap::new(),
                })
            }
        }
        let result = collect_claude(
            &sandbox.home,
            None,
            None,
            Some(1.0),
            Some(&RefreshThenUsage),
            Some(&RefreshThenUsage),
            Some(&|| true),
        );
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(result["windows"][0]["used_percent"], json!(12.0));
        let creds: Value = serde_json::from_str(
            &fs::read_to_string(sandbox.home.join(".claude").join(".credentials.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(creds["claudeAiOauth"]["accessToken"], json!("new-token"));
    }

    #[test]
    fn keeps_valid_token_while_claude_is_running() {
        let sandbox = Sandbox::new();
        write_claude_credentials_full(
            &sandbox.home,
            "still-good",
            3_600_000,
            Some("refresh-1"),
            86_400_000 * 30,
            None,
        );
        struct Boom;
        impl HttpOpener for Boom {
            fn open(&self, _request: &HttpRequest) -> Result<HttpResponse, HttpError> {
                panic!("should not call network");
            }
        }
        let (token, subscription, error) =
            claude_refresh_oauth_token(&sandbox.home, None, Some(&Boom), Some(&|| true), Some(1.0));
        assert_eq!(token.as_deref(), Some("still-good"));
        assert_eq!(subscription, Some(json!("max")));
        assert!(error.is_none());
    }

    #[test]
    fn is_running_detects_claude_binary() {
        let sandbox = Sandbox::new();
        let proc = sandbox.home.join("proc");
        fs::create_dir_all(proc.join("1")).unwrap();
        fs::write(proc.join("1").join("cmdline"), b"/usr/bin/claude\x00").unwrap();
        assert!(claude_is_running(&proc));
    }

    #[test]
    fn is_running_detects_node_launched_claude_code() {
        let sandbox = Sandbox::new();
        let proc = sandbox.home.join("proc");
        fs::create_dir_all(proc.join("2")).unwrap();
        fs::write(
            proc.join("2").join("cmdline"),
            b"/usr/bin/node\x00/home/u/.npm/claude-code/cli.js\x00",
        )
        .unwrap();
        assert!(claude_is_running(&proc));
    }

    #[test]
    fn is_running_false_when_no_claude() {
        let sandbox = Sandbox::new();
        let proc = sandbox.home.join("proc");
        fs::create_dir_all(proc.join("1")).unwrap();
        fs::write(
            proc.join("1").join("cmdline"),
            b"/usr/bin/python3\x00script.py\x00",
        )
        .unwrap();
        assert!(!claude_is_running(&proc));
    }
}
