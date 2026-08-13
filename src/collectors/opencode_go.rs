//! OpenCode Go: opencode.ai workspace `/go` page (opencode-go-usage plugin).

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

use super::util::{now_epoch, to_percent};
use crate::config::OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS;
use crate::http::{HttpOpener, HttpRequest, UreqOpener};
use crate::logging;
use crate::snapshot::{carry_last_reading, snapshot, Window};

const OPENCODE_GO_USAGE_URL: &str = "https://opencode.ai/workspace/{workspace_id}/go";
const OPENCODE_GO_CONFIG_FILE: &str = "opencode-go-usage.json";

fn usage_patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                "rolling",
                Regex::new(r"rollingUsage:\$R\[\d+\]=(\{[^}]+\})").unwrap(),
            ),
            (
                "week",
                Regex::new(r"weeklyUsage:\$R\[\d+\]=(\{[^}]+\})").unwrap(),
            ),
            (
                "month",
                Regex::new(r"monthlyUsage:\$R\[\d+\]=(\{[^}]+\})").unwrap(),
            ),
        ]
    })
}

fn js_keys() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"([{,]\s*)([a-zA-Z_][a-zA-Z0-9_]*)(\s*:)").unwrap())
}

fn workspace_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^wrk_[a-zA-Z0-9]+$").unwrap())
}

fn opencode_go_config_paths(home: &Path) -> [std::path::PathBuf; 2] {
    [
        home.join(".config")
            .join("opencode")
            .join(OPENCODE_GO_CONFIG_FILE),
        home.join(".opencode").join(OPENCODE_GO_CONFIG_FILE),
    ]
}

fn opencode_go_credentials(home: &Path) -> Result<(String, String), String> {
    let env_workspace = std::env::var("OPENCODE_GO_WORKSPACE_ID").ok();
    let env_cookie = std::env::var("OPENCODE_GO_AUTH_COOKIE").ok();
    if let (Some(workspace), Some(cookie)) = (env_workspace, env_cookie) {
        if !workspace.is_empty() && !cookie.is_empty() {
            return Ok((workspace, cookie));
        }
    }
    for path in opencode_go_config_paths(home) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(payload) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let workspace = payload.get("workspaceId").and_then(Value::as_str);
        let cookie = payload.get("authCookie").and_then(Value::as_str);
        if let (Some(workspace), Some(cookie)) = (workspace, cookie) {
            if !workspace.is_empty() && !cookie.is_empty() {
                return Ok((workspace.to_string(), cookie.to_string()));
            }
        }
    }
    Err("OpenCode Go credentials unavailable".into())
}

fn opencode_go_usage_from_html(html: &str, now: f64) -> Vec<Window> {
    let mut windows = Vec::new();
    for (label, pattern) in usage_patterns() {
        let Some(caps) = pattern.captures(html) else {
            continue;
        };
        let json_str = js_keys().replace_all(&caps[1], r#"$1"$2"$3"#);
        let Ok(payload) = serde_json::from_str::<Value>(&json_str) else {
            continue;
        };
        let Some(used) = to_percent(payload.get("usagePercent").unwrap_or(&Value::Null)) else {
            continue;
        };
        let resets_at = payload.get("resetInSec").and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().map(|f| f as i64))
                .map(|reset_in| now as i64 + reset_in)
        });
        windows.push(Window {
            label: (*label).into(),
            used_percent: used,
            resets_at,
        });
    }
    windows
}

/// Collect OpenCode Go usage from the workspace /go page.
pub fn collect_opencode_go(
    home: &Path,
    last_result: Option<&Value>,
    now: Option<f64>,
    min_interval: Option<f64>,
    opener: Option<&dyn HttpOpener>,
) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let min_interval = min_interval.unwrap_or(OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS as f64);
    if let Some(last) = last_result {
        if let Some(checked) = last.get("checked_at").and_then(Value::as_f64) {
            if now - checked < min_interval
                && last
                    .get("windows")
                    .and_then(Value::as_array)
                    .map(|a| !a.is_empty())
                    .unwrap_or(false)
                && last.get("error").map(|e| e.is_null()).unwrap_or(true)
            {
                return last.clone();
            }
        }
    }
    let (workspace, cookie) = match opencode_go_credentials(home) {
        Ok(creds) => creds,
        Err(error) => {
            let mut snap = snapshot("opencode_go_page", None, None, Vec::new(), Some(error));
            snap["checked_at"] = json!(now);
            return carry_last_reading(snap, last_result);
        }
    };
    if !workspace_id_re().is_match(&workspace) {
        let mut snap = snapshot(
            "opencode_go_page",
            None,
            None,
            Vec::new(),
            Some("invalid workspace ID format".into()),
        );
        snap["checked_at"] = json!(now);
        return carry_last_reading(snap, last_result);
    }
    let url = OPENCODE_GO_USAGE_URL.replace("{workspace_id}", &workspace);
    let request = HttpRequest {
        url,
        method: "GET".into(),
        headers: vec![
            (
                "User-Agent".into(),
                "Mozilla/5.0 (X11; Linux x86_64; rv:148.0) Gecko/20100101 Firefox/148.0".into(),
            ),
            (
                "Accept".into(),
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into(),
            ),
            ("Cookie".into(), format!("auth={cookie}")),
        ],
        body: None,
        timeout: Duration::from_secs(20),
    };
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    match opener.open(&request) {
        Ok(response) => {
            let html = response.text();
            let windows = opencode_go_usage_from_html(&html, now);
            if windows.is_empty() {
                let mut snap = snapshot(
                    "opencode_go_page",
                    None,
                    None,
                    Vec::new(),
                    Some("no usage windows in page response".into()),
                );
                snap["checked_at"] = json!(now);
                carry_last_reading(snap, last_result)
            } else {
                let mut snap = snapshot("opencode_go_page", Some(now), None, windows, None);
                snap["checked_at"] = json!(now);
                snap
            }
        }
        Err(exc) => {
            let error = if let Some(status) = exc.status {
                logging::error(format!("opencode go page HTTP {status}"));
                format!("HTTP {status}")
            } else {
                logging::error(format!("opencode go page failed: {exc}"));
                exc.to_string()
            };
            let mut snap = snapshot("opencode_go_page", None, None, Vec::new(), Some(error));
            snap["checked_at"] = json!(now);
            carry_last_reading(snap, last_result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpError, HttpResponse};
    use crate::test_support::{write_opencode_go_credentials, Sandbox, OPENCODE_GO_HTML_FIXTURE};
    use std::collections::HashMap;

    struct HtmlOpener {
        html: String,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl HttpOpener for HtmlOpener {
        fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.lock().unwrap().push(request.url.clone());
            Ok(HttpResponse {
                status: 200,
                body: self.html.as_bytes().to_vec(),
                headers: HashMap::new(),
            })
        }
    }

    #[test]
    fn parses_html_fixture() {
        let sandbox = Sandbox::new();
        write_opencode_go_credentials(&sandbox.home);
        let opener = HtmlOpener {
            html: OPENCODE_GO_HTML_FIXTURE.into(),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let now = 2000.0;
        let result = collect_opencode_go(&sandbox.home, None, Some(now), None, Some(&opener));
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
        assert_eq!(
            pairs,
            vec![
                ("rolling".into(), 0.0),
                ("week".into(), 17.0),
                ("month".into(), 8.0)
            ]
        );
        assert_eq!(opener.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn throttles_within_fetch_interval() {
        let sandbox = Sandbox::new();
        write_opencode_go_credentials(&sandbox.home);
        let opener = HtmlOpener {
            html: OPENCODE_GO_HTML_FIXTURE.into(),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let first = collect_opencode_go(&sandbox.home, None, Some(2000.0), None, Some(&opener));
        let second = collect_opencode_go(
            &sandbox.home,
            Some(&first),
            Some(2100.0),
            None,
            Some(&opener),
        );
        assert_eq!(second["windows"][1]["used_percent"], json!(17.0));
        assert_eq!(opener.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn empty_last_reading_bypasses_fetch_throttle() {
        let sandbox = Sandbox::new();
        write_opencode_go_credentials(&sandbox.home);
        let opener = HtmlOpener {
            html: OPENCODE_GO_HTML_FIXTURE.into(),
            calls: std::sync::Mutex::new(Vec::new()),
        };
        let last = json!({"checked_at": 2000.0, "windows": [], "error": null});
        collect_opencode_go(
            &sandbox.home,
            Some(&last),
            Some(2100.0),
            None,
            Some(&opener),
        );
        assert_eq!(opener.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn missing_credentials_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_opencode_go(&sandbox.home, None, Some(1.0), None, None);
        assert!(result["error"].as_str().unwrap().contains("credentials"));
    }

    #[test]
    fn failed_fetch_keeps_last_reading() {
        let sandbox = Sandbox::new();
        write_opencode_go_credentials(&sandbox.home);
        struct Fail;
        impl HttpOpener for Fail {
            fn open(&self, _request: &HttpRequest) -> Result<HttpResponse, HttpError> {
                Err(HttpError {
                    message: "HTTP 500".into(),
                    status: Some(500),
                    body: Vec::new(),
                    headers: HashMap::new(),
                })
            }
        }
        let last = json!({
            "windows": [{"label": "week", "used_percent": 17.0, "resets_at": null}],
            "observed_at": 1.0,
            "plan": null,
        });
        let result = collect_opencode_go(
            &sandbox.home,
            Some(&last),
            Some(400.0),
            Some(1.0),
            Some(&Fail),
        );
        assert_eq!(result["windows"][0]["used_percent"], json!(17.0));
        assert_eq!(result["error"], json!("HTTP 500"));
    }
}
