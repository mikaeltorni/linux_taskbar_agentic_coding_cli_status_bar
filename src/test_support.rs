//! Shared test sandbox and CLI data fixtures.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tempfile::TempDir;

use crate::env_test_lock;

pub const GROK_BILLING_PERIOD_END: i64 = 1_783_577_040;
pub const NOW: f64 = 1_783_075_290.0;

pub const OPENCODE_GO_HTML_FIXTURE: &str = concat!(
    "<html><body>",
    "rollingUsage:$R[0]={usagePercent:0,resetInSec:17820}",
    "weeklyUsage:$R[1]={usagePercent:17,resetInSec:234000}",
    "monthlyUsage:$R[2]={usagePercent:8,resetInSec:2588400}",
    "</body></html>"
);

pub fn openrouter_analytics_fixture() -> Vec<Value> {
    vec![
        json!({
            "model": "stealth/ox-alpha",
            "variant": "standard",
            "request_count": "420",
        }),
        json!({
            "model": "qwen/qwen3.5-flash-20260224",
            "variant": "standard",
            "request_count": "3",
        }),
    ]
}

/// Isolated home directory with XDG env vars pointed inside.
pub struct Sandbox {
    _lock: MutexGuard<'static, ()>,
    _tmp: TempDir,
    pub home: PathBuf,
    originals: HashMap<String, Option<OsString>>,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox {
    pub fn new() -> Self {
        let lock = env_test_lock::lock();
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let mut originals = HashMap::new();
        let assignments = [
            ("HOME", home.as_os_str().to_os_string()),
            ("XDG_CONFIG_HOME", home.join(".config").into_os_string()),
            (
                "XDG_STATE_HOME",
                home.join(".local").join("state").into_os_string(),
            ),
            ("ACS_LOG_DIR", tmp.path().join("logs").into_os_string()),
        ];
        for (key, value) in assignments {
            originals.insert(key.to_string(), std::env::var_os(key));
            unsafe { std::env::set_var(key, value) };
        }
        originals.insert(
            "OPENROUTER_MANAGEMENT_API_KEY".into(),
            std::env::var_os("OPENROUTER_MANAGEMENT_API_KEY"),
        );
        unsafe { std::env::remove_var("OPENROUTER_MANAGEMENT_API_KEY") };
        Self {
            _lock: lock,
            _tmp: tmp,
            home,
            originals,
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for (key, original) in &self.originals {
            unsafe {
                match original {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

pub fn write_codex_rollout(home: &Path, day: &str, name: &str, events: &[Value]) -> PathBuf {
    let day_dir = home.join(".codex").join("sessions").join(day);
    fs::create_dir_all(&day_dir).unwrap();
    let target = day_dir.join(name);
    let mut text = String::new();
    for event in events {
        text.push_str(&serde_json::to_string(event).unwrap());
        text.push('\n');
    }
    fs::write(&target, text).unwrap();
    target
}

pub fn codex_token_count_event(primary_pct: f64, secondary_pct: f64) -> Value {
    codex_token_count_event_at(
        primary_pct,
        secondary_pct,
        "2026-07-03T10:41:25.148Z",
        "plus",
    )
}

pub fn codex_token_count_event_at(
    primary_pct: f64,
    secondary_pct: f64,
    timestamp: &str,
    plan: &str,
) -> Value {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {"total_token_usage": {"total_tokens": 1}},
            "rate_limits": {
                "limit_id": "codex",
                "primary": {
                    "used_percent": primary_pct,
                    "window_minutes": 300,
                    "resets_at": 1783080635_i64,
                },
                "secondary": {
                    "used_percent": secondary_pct,
                    "window_minutes": 10080,
                    "resets_at": 1783577400_i64,
                },
                "plan_type": plan,
            },
        },
    })
}

pub fn write_grok_log(home: &Path, name: &str, records: &[Value]) -> PathBuf {
    let logs_dir = home.join(".grok").join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let target = logs_dir.join(name);
    let mut text = String::new();
    for record in records {
        text.push_str(&serde_json::to_string(record).unwrap());
        text.push('\n');
    }
    fs::write(&target, text).unwrap();
    target
}

pub fn grok_billing_record(percent: f64) -> Value {
    grok_billing_record_at(percent, "2026-07-03T10:36:48.739Z", "SuperGrok")
}

pub fn grok_billing_record_at(percent: f64, ts: &str, tier: &str) -> Value {
    json!({
        "ts": ts,
        "src": "shell",
        "lvl": "info",
        "msg": "billing: fetched credits config",
        "ctx": {
            "config": {
                "creditUsagePercent": percent,
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-07-02T06:04:00.368952+00:00",
                    "end": "2026-07-09T06:04:00.368952+00:00",
                },
                "billingPeriodEnd": "2026-07-09T06:04:00.368952+00:00",
            },
            "subscriptionTier": tier,
        },
    })
}

pub fn grok_balance_exhausted_record(ts: &str) -> Value {
    json!({
        "ts": ts,
        "src": "grok-pager",
        "lvl": "error",
        "msg": "agent response failed",
        "ctx": {
            "error": "Internal error: {\n  \"message\": \"API error (status 402 Payment Required): Grok Build usage balance exhausted\",\n  \"http_status\": 402\n}"
        },
    })
}

pub fn grok_billing_record_no_percent() -> Value {
    json!({
        "ts": "2026-07-09T09:10:47.647Z",
        "src": "shell",
        "lvl": "info",
        "msg": "billing: fetched credits config",
        "ctx": {
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-07-09T06:04:00.368952+00:00",
                    "end": "2026-07-16T06:04:00.368952+00:00",
                },
                "billingPeriodStart": "2026-07-09T06:04:00.368952+00:00",
                "billingPeriodEnd": "2026-07-16T06:04:00.368952+00:00",
                "historyLen": 0,
            },
            "subscriptionTier": "SuperGrok",
        },
    })
}

pub fn write_grok_pane(home: &Path, text: &str) -> PathBuf {
    let pane_dir = crate::config::state_dir(Some(home)).join("panes");
    fs::create_dir_all(&pane_dir).unwrap();
    let target = pane_dir.join("grok.txt");
    fs::write(&target, text).unwrap();
    target
}

pub fn write_opencode_go_credentials(home: &Path) -> PathBuf {
    write_opencode_go_credentials_at(home, "wrk_test123", "Fe26.2**testcookie")
}

pub fn write_opencode_go_credentials_at(
    home: &Path,
    workspace_id: &str,
    auth_cookie: &str,
) -> PathBuf {
    let config_dir = home.join(".config").join("opencode");
    fs::create_dir_all(&config_dir).unwrap();
    let target = config_dir.join("opencode-go-usage.json");
    fs::write(
        &target,
        serde_json::to_string_pretty(&json!({
            "workspaceId": workspace_id,
            "authCookie": auth_cookie,
            "refreshIntervalMinutes": 5,
        }))
        .unwrap(),
    )
    .unwrap();
    target
}

pub fn write_openrouter_env(home: &Path) -> PathBuf {
    write_openrouter_env_key(home, "sk-or-v1-test-management-key")
}

pub fn write_openrouter_env_key(home: &Path, api_key: &str) -> PathBuf {
    let config_dir = home.join(".config").join("agentic-cli-status");
    fs::create_dir_all(&config_dir).unwrap();
    let target = config_dir.join(".env");
    fs::write(
        &target,
        format!("OPENROUTER_MANAGEMENT_API_KEY={api_key}\n"),
    )
    .unwrap();
    let mut perms = fs::metadata(&target).unwrap().permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&target, perms).unwrap();
    target
}

pub fn write_claude_credentials(home: &Path, token: &str, expires_in_ms: i64) -> PathBuf {
    write_claude_credentials_full(home, token, expires_in_ms, None, 86_400_000 * 30, None)
}

pub fn write_claude_credentials_full(
    home: &Path,
    token: &str,
    expires_in_ms: i64,
    refresh_token: Option<&str>,
    refresh_expires_in_ms: i64,
    scopes: Option<Vec<&str>>,
) -> PathBuf {
    let creds_dir = home.join(".claude");
    fs::create_dir_all(&creds_dir).unwrap();
    let target = creds_dir.join(".credentials.json");
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut oauth = json!({
        "accessToken": token,
        "expiresAt": now_ms + expires_in_ms,
        "subscriptionType": "max",
    });
    if let Some(refresh) = refresh_token {
        oauth["refreshToken"] = json!(refresh);
        oauth["refreshTokenExpiresAt"] = json!(now_ms + refresh_expires_in_ms);
        oauth["scopes"] = json!(scopes.unwrap_or_else(|| vec![
            "user:file_upload",
            "user:inference",
            "user:mcp_servers",
            "user:profile",
            "user:sessions:claude_code",
        ]));
    }
    fs::write(
        &target,
        serde_json::to_string(&json!({"claudeAiOauth": oauth})).unwrap(),
    )
    .unwrap();
    target
}

/// Append one line to a file (used when building large tails).
pub fn append_line(path: &Path, line: &str) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{line}").unwrap();
}
