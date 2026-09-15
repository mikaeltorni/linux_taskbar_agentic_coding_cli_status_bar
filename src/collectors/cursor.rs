//! Cursor spending page: headless scrape of cursor.com/dashboard/spending.
//!
//! Cursor is not a coding CLI. This collector only GETs the Spending tab
//! (session cookie, never a prompt) and parses **included** Pro usage:
//! Cursor Models, Other Models, Grok Bot, the usage-limit reset, and any
//! other included-usage fields on the page. On-demand / spend-limit /
//! pay-as-you-go subtrees are dropped at parse time, not hidden later.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::util::{now_epoch, parse_iso_epoch, to_percent};
use crate::config::{CURSOR_FETCH_MIN_INTERVAL_SECONDS, CURSOR_SPENDING_URL_DEFAULT};
use crate::http::{HttpOpener, HttpRequest, UreqOpener};
use crate::logging;
use crate::snapshot::{carry_last_reading, snapshot, Window};

const CONFIG_FILE_NAME: &str = "cursor-spending.json";
const SOURCE: &str = "cursor_spending_page";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:148.0) Gecko/20100101 Firefox/148.0";

const POOL_CURSOR_MODELS: &str = "cursor_models";
const POOL_OTHER_MODELS: &str = "other_models";
const POOL_GROK_BOT: &str = "grok_bot";

/// One extra included-usage field from the spending page (never on-demand).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncludedField {
    pub label: String,
    pub text: String,
}

/// Included-usage parse result from a Cursor spending-page document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpendingPage {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub usage_limit_reset: Option<i64>,
    pub included_fields: Vec<IncludedField>,
}

fn cookie_header_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed
        .to_ascii_lowercase()
        .starts_with("workoscursorsessiontoken=")
    {
        trimmed.to_string()
    } else {
        format!("WorkosCursorSessionToken={trimmed}")
    }
}

fn is_on_demand_key(key: &str) -> bool {
    let n: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    n.contains("ondemand")
        || n.contains("spendlimit")
        || n.contains("payasyougo")
        || n.contains("usagebased")
}

fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn pool_id(name: &str) -> Option<&'static str> {
    let n = normalize_name(name);
    if n.contains("cursormodel") {
        Some(POOL_CURSOR_MODELS)
    } else if n.contains("othermodel") {
        Some(POOL_OTHER_MODELS)
    } else if n.contains("grokbot") {
        Some(POOL_GROK_BOT)
    } else {
        None
    }
}

fn is_reset_key(key: &str) -> bool {
    let n = normalize_name(key);
    n.contains("usagelimitreset") || n == "billingcycleend" || n == "resetdate" || n == "resetsat"
}

fn epoch_from_value(value: &Value) -> Option<i64> {
    if let Some(n) = value
        .as_i64()
        .or_else(|| value.as_u64().map(|u| u as i64))
        .or_else(|| value.as_f64().map(|f| f as i64))
    {
        return Some(if n.abs() > 10_000_000_000 {
            n / 1000
        } else {
            n
        });
    }
    value.as_str().and_then(epoch_from_str)
}

fn epoch_from_str(raw: &str) -> Option<i64> {
    let trimmed = raw.trim();
    if let Ok(n) = trimmed.parse::<i64>() {
        return Some(if n.abs() > 10_000_000_000 {
            n / 1000
        } else {
            n
        });
    }
    parse_iso_epoch(trimmed).or_else(|| parse_human_date(trimmed))
}

fn parse_human_date(raw: &str) -> Option<i64> {
    let trimmed = raw.trim().trim_end_matches('.');
    const FMTS: &[&str] = &[
        "%Y-%m-%d",
        "%b %d, %Y",
        "%B %d, %Y",
        "%d %b %Y",
        "%d %B %Y",
        "%b %d %Y",
        "%B %d %Y",
    ];
    for fmt in FMTS {
        if let Ok(date) = NaiveDate::parse_from_str(trimmed, fmt) {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|naive| naive.and_utc().timestamp());
        }
    }
    None
}

fn percent_from_value(value: &Value) -> Option<f64> {
    to_percent(value).or_else(|| {
        value
            .as_str()
            .and_then(parse_percent_str)
            .map(|n| n.clamp(0.0, 100.0))
    })
}

fn parse_percent_str(raw: &str) -> Option<f64> {
    let trimmed = raw.trim().trim_end_matches('%').trim();
    trimmed.parse::<f64>().ok()
}

fn percent_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\d+(?:\.\d+)?)\s*%").unwrap())
}

fn script_json_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<script[^>]*>(.*?)</script>"#).unwrap())
}

fn included_region(html: &str) -> &str {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("included usage").unwrap_or(0);
    let rest = &html[start..];
    let rest_lower = rest.to_ascii_lowercase();
    let cut = [
        "on-demand usage",
        "on demand usage",
        "ondemand usage",
        "spend limit",
        "pay-as-you-go",
        "pay as you go",
    ]
    .iter()
    .filter_map(|marker| rest_lower.find(marker))
    .min()
    .unwrap_or(rest.len());
    &rest[..cut]
}

fn field_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn push_field(out: &mut SpendingPage, label: &str, text: String) {
    if text.trim().is_empty() {
        return;
    }
    if is_on_demand_key(label) || is_on_demand_key(&text) {
        return;
    }
    if out
        .included_fields
        .iter()
        .any(|f| f.label == label && f.text == text)
    {
        return;
    }
    out.included_fields.push(IncludedField {
        label: label.to_string(),
        text,
    });
}

fn cents_to_dollars(cents: f64) -> String {
    format!("${:.2}", cents / 100.0)
}

fn ingest_pool(id: &'static str, value: &Value, now: f64, out: &mut SpendingPage) {
    let used = percent_from_value(value)
        .or_else(|| {
            value
                .get("usedPercent")
                .or_else(|| value.get("used_percent"))
                .or_else(|| value.get("percentUsed"))
                .or_else(|| value.get("percent_used"))
                .and_then(percent_from_value)
        })
        .or_else(|| {
            value
                .get("usagePercent")
                .or_else(|| value.get("usage_percent"))
                .and_then(percent_from_value)
        });
    let mut resets_at = value
        .get("resetsAt")
        .or_else(|| value.get("resets_at"))
        .or_else(|| value.get("resetAt"))
        .or_else(|| value.get("resetDate"))
        .and_then(epoch_from_value);
    if resets_at.is_none() {
        if let Some(reset_in) = value
            .get("resetInSec")
            .or_else(|| value.get("reset_in_sec"))
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        {
            resets_at = Some(now as i64 + reset_in);
        }
    }
    if let Some(used_percent) = used {
        if !out.windows.iter().any(|w| w.label == id) {
            out.windows.push(Window {
                label: id.into(),
                used_percent,
                resets_at,
            });
        }
    }
    if let Some(obj) = value.as_object() {
        for (key, child) in obj {
            if is_on_demand_key(key) {
                continue;
            }
            let n = normalize_name(key);
            if n.contains("remainingcents") {
                if let Some(cents) = child.as_f64().or_else(|| child.as_i64().map(|i| i as f64)) {
                    push_field(out, &format!("{id} remaining"), cents_to_dollars(cents));
                }
            } else if n.contains("limitcents") {
                if let Some(cents) = child.as_f64().or_else(|| child.as_i64().map(|i| i as f64)) {
                    push_field(out, &format!("{id} limit"), cents_to_dollars(cents));
                }
            } else if n == "cadence" || n == "period" {
                if let Some(text) = field_text(child) {
                    push_field(out, &format!("{id} cadence"), text);
                }
            } else if n == "remaining" || n == "limit" {
                if let Some(text) = field_text(child) {
                    push_field(out, &format!("{id} {key}"), text);
                }
            }
        }
    }
}

fn walk_json(value: &Value, now: f64, out: &mut SpendingPage) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if is_on_demand_key(key) {
                    continue;
                }
                if let Some(id) = pool_id(key) {
                    ingest_pool(id, child, now, out);
                }
                if is_reset_key(key) {
                    if let Some(epoch) = epoch_from_value(child) {
                        out.usage_limit_reset = Some(epoch);
                    }
                }
                let n = normalize_name(key);
                if matches!(n.as_str(), "plan" | "planname" | "membershiptype") {
                    if out.plan.is_none() {
                        if let Some(text) = field_text(child) {
                            out.plan = Some(text);
                        }
                    }
                }
                walk_json(child, now, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_json(item, now, out);
            }
        }
        _ => {}
    }
}

fn parse_json_blobs(html: &str, now: f64, out: &mut SpendingPage) {
    for caps in script_json_re().captures_iter(html) {
        let body = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if body.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(body) {
            walk_json(&value, now, out);
        }
    }
}

fn parse_html_pools(html: &str, now: f64, out: &mut SpendingPage) {
    let region = included_region(html);
    let region_lower = region.to_ascii_lowercase();
    let headings = [
        ("cursor models", POOL_CURSOR_MODELS),
        ("other models", POOL_OTHER_MODELS),
        ("grok bot", POOL_GROK_BOT),
    ];
    for (heading, id) in headings {
        if out.windows.iter().any(|w| w.label == id) {
            continue;
        }
        let Some(pos) = region_lower.find(heading) else {
            continue;
        };
        let slice = &region[pos..region.len().min(pos + 800)];
        let Some(caps) = percent_re().captures(slice) else {
            continue;
        };
        let Some(used) = parse_percent_str(&caps[1]) else {
            continue;
        };
        out.windows.push(Window {
            label: id.into(),
            used_percent: used.clamp(0.0, 100.0),
            resets_at: None,
        });
        if let Some(remain) = Regex::new(r"(\$[\d,.]+)\s+remaining(?:\s+of\s+(\$[\d,.]+))?")
            .ok()
            .and_then(|re| re.captures(slice))
        {
            let mut text = remain[1].to_string();
            if let Some(limit) = remain.get(2) {
                text = format!("{} of {}", remain[1].to_string(), limit.as_str());
            }
            push_field(out, &format!("{id} remaining"), text);
        }
        let _ = now;
    }
    if out.usage_limit_reset.is_none() {
        if let Some(idx) = region_lower.find("usage limit reset") {
            let after = region[idx + "usage limit reset".len()..]
                .chars()
                .take(48)
                .collect::<String>();
            let cleaned = after
                .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | ',' | '-'))
                .collect::<String>();
            out.usage_limit_reset = epoch_from_str(cleaned.trim());
        }
    }
    if out.plan.is_none() {
        if let Some(caps) = Regex::new(r"(?i)\bplan\s+([A-Za-z][A-Za-z0-9 +]+)")
            .ok()
            .and_then(|re| re.captures(region))
        {
            let name = caps[1].trim();
            if !name.is_empty() && !normalize_name(name).contains("ondemand") {
                out.plan = Some(name.to_string());
            }
        }
    }
}

fn apply_monthly_reset(page: &mut SpendingPage) {
    let Some(reset) = page.usage_limit_reset else {
        return;
    };
    for window in &mut page.windows {
        if window.label == POOL_GROK_BOT {
            continue;
        }
        if window.resets_at.is_none() {
            window.resets_at = Some(reset);
        }
    }
}

/// Parse included-usage windows from a Cursor spending-page HTML/payload.
///
/// On-demand, spend-limit, and pay-as-you-go fields are ignored. This is the
/// same function the poller uses after a successful HTTP GET.
pub fn parse_spending_page(html: &str, now: f64) -> SpendingPage {
    let mut page = SpendingPage::default();
    parse_json_blobs(html, now, &mut page);
    parse_html_pools(html, now, &mut page);
    apply_monthly_reset(&mut page);
    page
}

fn cursor_config_path(home: &Path) -> std::path::PathBuf {
    home.join(".config")
        .join("agentic-cli-status")
        .join(CONFIG_FILE_NAME)
}

fn cursor_cookie(home: &Path) -> Result<String, String> {
    if let Ok(cookie) = std::env::var("CRS_AUTH_COOKIE") {
        if !cookie.trim().is_empty() {
            return Ok(cookie);
        }
    }
    let path = cursor_config_path(home);
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Cursor spending credentials unavailable ({})",
            path.display()
        )
    })?;
    let payload: Value = serde_json::from_str(&text)
        .map_err(|_| "Cursor spending credentials file is not valid JSON".to_string())?;
    payload
        .get("authCookie")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Cursor spending authCookie is missing".into())
}

fn spending_url() -> String {
    std::env::var("CRS_SPENDING_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| CURSOR_SPENDING_URL_DEFAULT.to_string())
}

fn env_fetch_interval() -> Option<f64> {
    std::env::var("CRS_FETCH_INTERVAL_SECONDS")
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0)
}

fn attach_extras(mut snap: Value, page: &SpendingPage) -> Value {
    snap["usage_limit_reset"] = match page.usage_limit_reset {
        Some(epoch) => json!(epoch),
        None => Value::Null,
    };
    snap["included_fields"] = json!(page.included_fields);
    snap
}

fn copy_cursor_extras(mut snap: Value, last: Option<&Value>) -> Value {
    let empty_fields = snap
        .get("included_fields")
        .and_then(Value::as_array)
        .map(|a| a.is_empty())
        .unwrap_or(true);
    if empty_fields {
        if let Some(fields) = last.and_then(|v| v.get("included_fields")) {
            if fields.as_array().map(|a| !a.is_empty()).unwrap_or(false) {
                snap["included_fields"] = fields.clone();
            }
        }
    }
    if snap
        .get("usage_limit_reset")
        .map(|v| v.is_null())
        .unwrap_or(true)
    {
        if let Some(reset) = last.and_then(|v| v.get("usage_limit_reset")) {
            if !reset.is_null() {
                snap["usage_limit_reset"] = reset.clone();
            }
        }
    }
    snap
}

/// Collect Cursor included usage from the spending page.
pub fn collect_cursor(
    home: &Path,
    last_result: Option<&Value>,
    now: Option<f64>,
    min_interval: Option<f64>,
    opener: Option<&dyn HttpOpener>,
) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let min_interval = env_fetch_interval()
        .or(min_interval)
        .unwrap_or(CURSOR_FETCH_MIN_INTERVAL_SECONDS as f64);
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
                logging::debug(format!(
                    "cursor spending scrape throttled ({min_interval}s floor)"
                ));
                return last.clone();
            }
        }
    }
    let cookie = match cursor_cookie(home) {
        Ok(cookie) => cookie,
        Err(error) => {
            logging::warn(error.clone());
            let mut snap = snapshot(SOURCE, None, None, Vec::new(), Some(error));
            snap["checked_at"] = json!(now);
            return copy_cursor_extras(carry_last_reading(snap, last_result), last_result);
        }
    };
    let url = spending_url();
    logging::info(format!("cursor spending scrape {url}"));
    let request = HttpRequest {
        url,
        method: "GET".into(),
        headers: vec![
            ("User-Agent".into(), USER_AGENT.into()),
            (
                "Accept".into(),
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into(),
            ),
            ("Cookie".into(), cookie_header_value(&cookie)),
        ],
        body: None,
        timeout: Duration::from_secs(20),
    };
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    match opener.open(&request) {
        Ok(response) => {
            let html = response.text();
            let page = parse_spending_page(&html, now);
            if page.windows.is_empty() {
                logging::warn("cursor spending page had no included-usage windows");
                let mut snap = snapshot(
                    SOURCE,
                    None,
                    page.plan.clone().map(Value::from),
                    Vec::new(),
                    Some("no included-usage windows in page response".into()),
                );
                snap["checked_at"] = json!(now);
                let snap = attach_extras(snap, &page);
                copy_cursor_extras(carry_last_reading(snap, last_result), last_result)
            } else {
                logging::info(format!(
                    "cursor spending scrape parsed {} included windows",
                    page.windows.len()
                ));
                let mut snap = snapshot(
                    SOURCE,
                    Some(now),
                    page.plan.clone().map(Value::from),
                    page.windows.clone(),
                    None,
                );
                snap["checked_at"] = json!(now);
                attach_extras(snap, &page)
            }
        }
        Err(exc) => {
            let error = if let Some(status) = exc.status {
                logging::error(format!("cursor spending page HTTP {status}"));
                format!("HTTP {status}")
            } else {
                logging::error(format!("cursor spending page failed: {exc}"));
                exc.to_string()
            };
            let mut snap = snapshot(SOURCE, None, None, Vec::new(), Some(error));
            snap["checked_at"] = json!(now);
            copy_cursor_extras(carry_last_reading(snap, last_result), last_result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{HttpError, HttpResponse};
    use crate::test_support::{
        write_cursor_credentials, Sandbox, CURSOR_SPENDING_HTML_FIXTURE,
        CURSOR_SPENDING_RESET_EPOCH,
    };
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct HtmlOpener {
        html: String,
        calls: Mutex<Vec<String>>,
    }

    impl HttpOpener for HtmlOpener {
        fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.lock().unwrap().push(request.url.clone());
            assert!(
                request
                    .headers
                    .iter()
                    .any(|(n, v)| n == "Cookie" && v.contains("WorkosCursorSessionToken=")),
                "spending scrape must send the session cookie"
            );
            Ok(HttpResponse {
                status: 200,
                body: self.html.as_bytes().to_vec(),
                headers: HashMap::new(),
            })
        }
    }

    fn window_pairs(result: &Value) -> Vec<(String, f64)> {
        result["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["label"].as_str().unwrap().to_string(),
                    w["used_percent"].as_f64().unwrap(),
                )
            })
            .collect()
    }

    fn snapshot_has_on_demand(result: &Value) -> bool {
        let blob = result.to_string().to_ascii_lowercase();
        blob.contains("ondemand")
            || blob.contains("on-demand")
            || blob.contains("on demand")
            || blob.contains("spendlimit")
            || blob.contains("spend-limit")
            || blob.contains("pay-as-you-go")
            || blob.contains("999.00")
            || blob.contains("99.9")
    }

    #[test]
    fn parse_spending_page_reads_included_pools_and_reset() {
        let page = parse_spending_page(CURSOR_SPENDING_HTML_FIXTURE, 2000.0);
        let pairs: Vec<_> = page
            .windows
            .iter()
            .map(|w| (w.label.as_str(), w.used_percent))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (POOL_CURSOR_MODELS, 23.1),
                (POOL_OTHER_MODELS, 45.0),
                (POOL_GROK_BOT, 12.0),
            ]
        );
        assert_eq!(page.usage_limit_reset, Some(CURSOR_SPENDING_RESET_EPOCH));
        assert_eq!(page.plan.as_deref(), Some("Pro"));
        let cursor_reset = page
            .windows
            .iter()
            .find(|w| w.label == POOL_CURSOR_MODELS)
            .unwrap()
            .resets_at;
        assert_eq!(cursor_reset, Some(CURSOR_SPENDING_RESET_EPOCH));
        let grok_reset = page
            .windows
            .iter()
            .find(|w| w.label == POOL_GROK_BOT)
            .unwrap()
            .resets_at;
        assert_eq!(grok_reset, Some(2000 + 259_200));
        let blob = format!("{:?}{:?}", page.windows, page.included_fields);
        let lower = blob.to_ascii_lowercase();
        assert!(!lower.contains("ondemand"));
        assert!(!lower.contains("99.9"));
        assert!(!lower.contains("999"));
        assert!(page
            .included_fields
            .iter()
            .any(|f| f.label.contains("cursor_models") && f.text.contains("$15.38")));
    }

    #[test]
    fn parse_spending_page_html_only_fixture() {
        let html = concat!(
            "<html><body>",
            "<h2>Included Usage</h2>",
            "<h3>Cursor Models</h3><p>10% used</p><p>$9.00 remaining of $20.00</p>",
            "<h3>Other Models</h3><p>20% used</p>",
            "<h3>Grok Bot</h3><p>30% used</p>",
            "<p>Usage limit reset Oct 12, 2026</p>",
            "<h2>On-demand usage</h2><p>99.9% used</p><p>$999.00 spent</p>",
            "</body></html>"
        );
        let page = parse_spending_page(html, 1.0);
        let pairs: Vec<_> = page
            .windows
            .iter()
            .map(|w| (w.label.as_str(), w.used_percent))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (POOL_CURSOR_MODELS, 10.0),
                (POOL_OTHER_MODELS, 20.0),
                (POOL_GROK_BOT, 30.0),
            ]
        );
        assert_eq!(page.usage_limit_reset, Some(CURSOR_SPENDING_RESET_EPOCH));
        let blob = format!("{:?}{:?}", page.windows, page.included_fields);
        assert!(!blob.contains("99.9"));
        assert!(!blob.contains("999"));
    }

    #[test]
    fn collect_cursor_parses_html_fixture() {
        let sandbox = Sandbox::new();
        write_cursor_credentials(&sandbox.home);
        let opener = HtmlOpener {
            html: CURSOR_SPENDING_HTML_FIXTURE.into(),
            calls: Mutex::new(Vec::new()),
        };
        let result = collect_cursor(&sandbox.home, None, Some(2000.0), None, Some(&opener));
        assert!(result.get("error").unwrap().is_null());
        assert_eq!(
            window_pairs(&result),
            vec![
                (POOL_CURSOR_MODELS.into(), 23.1),
                (POOL_OTHER_MODELS.into(), 45.0),
                (POOL_GROK_BOT.into(), 12.0),
            ]
        );
        assert_eq!(
            result["usage_limit_reset"],
            json!(CURSOR_SPENDING_RESET_EPOCH)
        );
        assert_eq!(result["plan"], json!("Pro"));
        assert!(!snapshot_has_on_demand(&result));
        assert_eq!(opener.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn throttles_within_fetch_interval() {
        let sandbox = Sandbox::new();
        write_cursor_credentials(&sandbox.home);
        let opener = HtmlOpener {
            html: CURSOR_SPENDING_HTML_FIXTURE.into(),
            calls: Mutex::new(Vec::new()),
        };
        let first = collect_cursor(&sandbox.home, None, Some(2000.0), None, Some(&opener));
        let second = collect_cursor(
            &sandbox.home,
            Some(&first),
            Some(2010.0),
            None,
            Some(&opener),
        );
        assert_eq!(second["windows"][0]["used_percent"], json!(23.1));
        assert_eq!(opener.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn env_interval_overrides_config_floor() {
        let sandbox = Sandbox::new();
        write_cursor_credentials(&sandbox.home);
        let opener = HtmlOpener {
            html: CURSOR_SPENDING_HTML_FIXTURE.into(),
            calls: Mutex::new(Vec::new()),
        };
        unsafe { std::env::set_var("CRS_FETCH_INTERVAL_SECONDS", "5") };
        let first = collect_cursor(&sandbox.home, None, Some(2000.0), Some(30.0), Some(&opener));
        let second = collect_cursor(
            &sandbox.home,
            Some(&first),
            Some(2008.0),
            Some(30.0),
            Some(&opener),
        );
        unsafe { std::env::remove_var("CRS_FETCH_INTERVAL_SECONDS") };
        assert_eq!(opener.calls.lock().unwrap().len(), 2);
        assert_eq!(second["windows"][1]["used_percent"], json!(45.0));
    }

    #[test]
    fn missing_credentials_reports_error() {
        let sandbox = Sandbox::new();
        let result = collect_cursor(&sandbox.home, None, Some(1.0), None, None);
        assert!(result["error"].as_str().unwrap().contains("credentials"));
    }

    #[test]
    fn failed_fetch_keeps_last_reading() {
        let sandbox = Sandbox::new();
        write_cursor_credentials(&sandbox.home);
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
            "windows": [{"label": "cursor_models", "used_percent": 23.1, "resets_at": null}],
            "observed_at": 1.0,
            "plan": "Pro",
            "usage_limit_reset": CURSOR_SPENDING_RESET_EPOCH,
            "included_fields": [{"label": "cursor_models remaining", "text": "$15.38"}],
        });
        let result = collect_cursor(
            &sandbox.home,
            Some(&last),
            Some(400.0),
            Some(1.0),
            Some(&Fail),
        );
        assert_eq!(result["windows"][0]["used_percent"], json!(23.1));
        assert_eq!(result["error"], json!("HTTP 500"));
        assert_eq!(
            result["usage_limit_reset"],
            json!(CURSOR_SPENDING_RESET_EPOCH)
        );
        assert!(!snapshot_has_on_demand(&result));
    }
}
