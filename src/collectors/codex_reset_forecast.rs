//! Codex weekly-limit reset forecasts from public community trackers.
//!
//! OpenAI grants discretionary ("goodwill") Codex weekly-limit resets on no
//! published schedule, so several independent public trackers publish a
//! probability that the next one lands inside a given horizon. This collector
//! reads those numbers so the status bar can show one consensus percentage
//! next to the Codex segments.
//!
//! Major components:
//! - [`FORECAST_SITES`] — the vetted source registry (id, display name, page,
//!   endpoint, politeness floor, parser). Adding a source is a registry entry
//!   plus a parser, never a new fetch/throttle path.
//! - [`collect_codex_reset_forecast`] — per-site throttled fetch and parse,
//!   merged into one snapshot carrying 24h and 48h consensus percentages
//!   (plus `consensus_percent` for the configured horizon).
//!
//! Data safety: every endpoint is a public, unauthenticated, read-only URL
//! that the site's own `robots.txt` allows for generic agents. Nothing here
//! touches an OpenAI account, consumes plan limits, or sends local data
//! upstream — it only reads third-party published forecasts.
//!
//! Politeness: each site carries its own `min_interval_seconds` floor and the
//! effective interval is `max(configured, floor)`, so raising the shared fetch
//! cadence can never poll a source faster than that source tolerates.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::collectors::util::{now_epoch, parse_iso_epoch};
use crate::http::{HttpOpener, HttpRequest, UreqOpener};
use crate::logging;

/// Shared fetch cadence floor when the config does not override it.
pub const FORECAST_FETCH_MIN_INTERVAL_SECONDS: f64 = 30.0;

/// Horizon used for the bar percentage when the config does not override it.
pub const DEFAULT_HORIZON: &str = "24h";

/// Horizons a site may publish that the bar can aggregate into a consensus.
pub const CONSENSUS_HORIZONS: &[&str] = &["24h", "48h"];

/// Per-request timeout. Forecast pages are cached upstream and small, apart
/// from the monitor's full HTML page, which still lands well inside this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Honest, identifying User-Agent so operators can see (and contact) us.
const USER_AGENT: &str = concat!(
    "agentic-cli-status/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/mikaeltorni/linux_taskbar_agentic_coding_cli_status_bar)"
);

/// One published probability from a site, e.g. `24h` → `22%`.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    /// Horizon label. `24h`/`48h` feed the consensus; anything else (e.g. a
    /// prediction-market expiry like `by September 14`) is menu-only.
    pub label: String,
    pub percent: f64,
    /// Cumulative traded volume in dollars for a Polymarket expiry, when published.
    pub volume_usd: Option<f64>,
}

/// Everything one site published on this fetch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SiteForecast {
    pub predictions: Vec<Prediction>,
    /// A result-specific public page, when the endpoint identifies one.
    pub url: Option<String>,
    /// Short provenance line shown under the site name in the bar menu.
    pub detail: Option<String>,
    /// Upstream publication time, when the site states one.
    pub observed_at: Option<f64>,
}

/// A vetted forecast source.
///
/// `endpoint` is what the collector fetches; `url` is the human page shown as
/// provenance. `min_interval_seconds` is this source's politeness floor.
pub struct ForecastSite {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub endpoint: &'static str,
    pub min_interval_seconds: f64,
    pub parse: fn(&str) -> Result<SiteForecast, String>,
}

/// The vetted source registry — the single catalog of forecast sites.
///
/// Selection criteria: a public read-only endpoint, a `robots.txt` that allows
/// it, a stated methodology, and a machine-readable published probability.
///
/// - `codex-reset.com` — documents a public JSON API (`/api/forecast`) and
///   publishes 24h/48h probabilities derived from verified reset history.
/// - `codexreset.org` — "Codex Reset Monitor"; HTML only, so it is parsed from
///   stable `data-testid` attributes. Its page is ~1 MB and its own data
///   refreshes hourly, hence the 300 s floor.
/// - Polymarket — a real-money prediction market on exactly this question,
///   read through the documented public Gamma API.
pub const FORECAST_SITES: &[ForecastSite] = &[
    ForecastSite {
        id: "codex_reset",
        name: "codex-reset.com",
        url: "https://codex-reset.com/",
        endpoint: "https://codex-reset.com/api/forecast",
        min_interval_seconds: 30.0,
        parse: parse_codex_reset,
    },
    ForecastSite {
        id: "codex_reset_monitor",
        name: "codexreset.org",
        url: "https://codexreset.org/",
        endpoint: "https://codexreset.org/",
        min_interval_seconds: 300.0,
        parse: parse_codex_reset_monitor,
    },
    ForecastSite {
        id: "polymarket",
        name: "Polymarket",
        url: "https://polymarket.com/",
        endpoint: "https://gamma-api.polymarket.com/public-search?q=OpenAI%20resets%20Codex%20weekly%20usage%20limit&limit_per_type=5",
        min_interval_seconds: 30.0,
        parse: parse_polymarket,
    },
];

/// Look up a registry entry by id.
pub fn forecast_site(id: &str) -> Option<&'static ForecastSite> {
    FORECAST_SITES.iter().find(|site| site.id == id)
}

/// Clamp a raw percentage into `0..=100`, rejecting non-finite values.
fn clamp_percent(value: Option<f64>) -> Option<f64> {
    let value = value?;
    if !value.is_finite() {
        return None;
    }
    Some(value.clamp(0.0, 100.0))
}

/// Median of a percentage sample; `None` when the sample is empty.
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle])
    } else {
        Some((values[middle - 1] + values[middle]) / 2.0)
    }
}

/// Parse `codex-reset.com`'s documented `/api/forecast` JSON.
fn parse_codex_reset(body: &str) -> Result<SiteForecast, String> {
    let root: Value =
        serde_json::from_str(body).map_err(|exc| format!("invalid forecast JSON ({exc})"))?;
    let probabilities = root
        .get("probabilities")
        .ok_or_else(|| "forecast JSON has no probabilities object".to_string())?;

    let mut predictions = Vec::new();
    for (rounded_key, raw_key, label) in [
        ("rounded_24h", "raw_24h", "24h"),
        ("rounded_48h", "raw_48h", "48h"),
    ] {
        let percent = clamp_percent(
            probabilities
                .get(rounded_key)
                .and_then(Value::as_f64)
                .or_else(|| {
                    probabilities
                        .get(raw_key)
                        .and_then(Value::as_f64)
                        .map(|fraction| fraction * 100.0)
                }),
        );
        if let Some(percent) = percent {
            predictions.push(Prediction {
                label: label.to_string(),
                percent,
                volume_usd: None,
            });
        }
    }
    if predictions.is_empty() {
        return Err("forecast JSON published no 24h/48h probability".into());
    }

    // `mode` distinguishes a history-only baseline from a signal-driven
    // forecast; `confidence` is the site's own calibration caveat.
    let mut parts = Vec::new();
    if let Some(mode) = root.get("mode").and_then(Value::as_str) {
        parts.push(mode.to_string());
    }
    if let Some(confidence) = root.get("confidence").and_then(Value::as_str) {
        parts.push(format!("{confidence} confidence"));
    }

    Ok(SiteForecast {
        predictions,
        url: None,
        detail: (!parts.is_empty()).then(|| parts.join(" · ")),
        observed_at: root
            .get("updated_at")
            .and_then(Value::as_str)
            .and_then(parse_iso_epoch)
            .map(|epoch| epoch as f64),
    })
}

/// Read one `data-testid`-tagged probability ring out of the monitor's HTML.
///
/// The rings carry the value twice — as `data-target-value` on the element and
/// inside the `aria-label` — so the second form is a fallback if the markup's
/// attribute order changes.
fn monitor_ring_percent(body: &str, horizon: &str, hours: &str) -> Option<f64> {
    let by_attribute = format!(
        r#"data-testid="probability-ring-{horizon}"[^>]*?data-target-value="([0-9]+(?:\.[0-9]+)?)""#
    );
    let by_label = format!(r#"within {hours} hours:\s*([0-9]+(?:\.[0-9]+)?)%"#);
    for pattern in [by_attribute, by_label] {
        let Ok(regex) = regex::Regex::new(&pattern) else {
            continue;
        };
        if let Some(captures) = regex.captures(body) {
            if let Some(percent) = captures
                .get(1)
                .and_then(|m| m.as_str().parse::<f64>().ok())
                .and_then(|v| clamp_percent(Some(v)))
            {
                return Some(percent);
            }
        }
    }
    None
}

/// Parse `codexreset.org`'s rendered monitor page.
fn parse_codex_reset_monitor(body: &str) -> Result<SiteForecast, String> {
    let mut predictions = Vec::new();
    for (horizon, hours) in [("24h", "24"), ("48h", "48")] {
        if let Some(percent) = monitor_ring_percent(body, horizon, hours) {
            predictions.push(Prediction {
                label: horizon.to_string(),
                percent,
                volume_usd: None,
            });
        }
    }
    if predictions.is_empty() {
        return Err("monitor page published no probability ring".into());
    }

    // The page states its own backtested hit rate; surface it as provenance.
    let detail = regex::Regex::new(r#"data-testid="prediction-hit-rate"[^>]*>([^<]{1,60})<"#)
        .ok()
        .and_then(|regex| regex.captures(body))
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str().trim().to_string())
        .filter(|text| !text.is_empty());

    Ok(SiteForecast {
        predictions,
        url: None,
        detail,
        observed_at: None,
    })
}

/// Decode Polymarket's stringified JSON arrays (`outcomes`, `outcomePrices`).
fn polymarket_string_array(market: &Value, key: &str) -> Vec<String> {
    market
        .get(key)
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}

/// Yes-share price (0..1) of a binary market, as a percentage.
fn polymarket_yes_percent(market: &Value) -> Option<f64> {
    let outcomes = polymarket_string_array(market, "outcomes");
    let prices = polymarket_string_array(market, "outcomePrices");
    let index = outcomes
        .iter()
        .position(|outcome| outcome.eq_ignore_ascii_case("yes"))?;
    let price: f64 = prices.get(index)?.parse().ok()?;
    clamp_percent(Some(price * 100.0))
}

/// Gamma publishes cumulative per-market volume as `volumeNum` or a numeric string.
fn polymarket_volume_usd(market: &Value) -> Option<f64> {
    market
        .get("volumeNum")
        .and_then(Value::as_f64)
        .or_else(|| market.get("volume").and_then(Value::as_str)?.parse().ok())
        .filter(|volume: &f64| volume.is_finite() && *volume >= 0.0)
}

/// Shorten a market question into a horizon label, e.g. `by September 14`.
fn polymarket_label(question: &str) -> String {
    match question.to_lowercase().rfind(" by ") {
        Some(index) => format!("by {}", question[index + 4..].trim_end_matches('?').trim()),
        None => question.trim_end_matches('?').trim().to_string(),
    }
}

/// Build a canonical Polymarket event page from a documented event slug.
fn polymarket_event_url(event: &Value) -> Option<String> {
    let slug = event.get("slug").and_then(Value::as_str)?;
    if slug.is_empty()
        || !slug
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return None;
    }
    Some(format!("https://polymarket.com/event/{slug}"))
}

/// Parse the newest matching Gamma event into its nearest open markets.
fn parse_polymarket(body: &str) -> Result<SiteForecast, String> {
    let root: Value =
        serde_json::from_str(body).map_err(|exc| format!("invalid Gamma JSON ({exc})"))?;
    let events = root
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| "Gamma response has no events array".to_string())?;

    // Guard against the search returning an unrelated event.
    let mut newest: Option<(i64, Option<String>, Vec<(i64, String, f64, Option<f64>)>)> = None;
    for event in events {
        let title = event
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        if !(title.contains("codex") && title.contains("reset")) {
            continue;
        }
        if event.get("closed").and_then(Value::as_bool).unwrap_or(true)
            || !event
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        let mut open = Vec::new();
        for market in event
            .get("markets")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            if market
                .get("closed")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                continue;
            }
            if !market
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(percent) = polymarket_yes_percent(market) else {
                continue;
            };
            let end = market
                .get("endDate")
                .and_then(Value::as_str)
                .and_then(parse_iso_epoch)
                .unwrap_or(i64::MAX);
            let label =
                polymarket_label(market.get("question").and_then(Value::as_str).unwrap_or(""));
            open.push((end, label, percent, polymarket_volume_usd(market)));
        }
        if open.is_empty() {
            continue;
        }
        let created = event
            .get("creationDate")
            .and_then(Value::as_str)
            .and_then(parse_iso_epoch)
            .unwrap_or(i64::MIN);
        if newest
            .as_ref()
            .map_or(true, |(current, _, _)| created > *current)
        {
            newest = Some((created, polymarket_event_url(event), open));
        }
    }
    let Some((_, url, mut open)) = newest else {
        return Err("no open Codex reset market in the Gamma response".into());
    };

    // Nearest expiries first — those are the ones that answer "reset soon?".
    open.sort_by_key(|(end, _, _, _)| *end);
    open.truncate(2);

    Ok(SiteForecast {
        predictions: open
            .iter()
            .map(|(_, label, percent, volume_usd)| Prediction {
                label: label.clone(),
                percent: *percent,
                volume_usd: *volume_usd,
            })
            .collect(),
        url,
        detail: Some("nearest open markets · Yes price".to_string()),
        observed_at: None,
    })
}

/// Fetch one site's endpoint and parse it.
fn fetch_site(
    site: &ForecastSite,
    opener: Option<&dyn HttpOpener>,
) -> Result<SiteForecast, String> {
    let request = HttpRequest {
        url: site.endpoint.to_string(),
        method: "GET".into(),
        headers: vec![
            ("Accept".into(), "application/json, text/html;q=0.9".into()),
            ("Accept-Encoding".into(), "gzip".into()),
            ("User-Agent".into(), USER_AGENT.into()),
        ],
        body: None,
        timeout: REQUEST_TIMEOUT,
    };
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    let response = opener.open(&request).map_err(|exc| exc.to_string())?;
    (site.parse)(&response.text())
}

/// Serialize a site's state for `status.json`.
fn site_value(
    site: &ForecastSite,
    forecast: &SiteForecast,
    checked_at: f64,
    error: Option<String>,
) -> Value {
    json!({
        "id": site.id,
        "name": site.name,
        "url": forecast.url.as_deref().unwrap_or(site.url),
        "predictions": forecast
            .predictions
            .iter()
            .map(|prediction| {
                let mut row = json!({"label": prediction.label, "percent": prediction.percent});
                if let Some(volume) = prediction.volume_usd {
                    row["volume_usd"] = json!(volume);
                }
                row
            })
            .collect::<Vec<Value>>(),
        "detail": forecast.detail,
        "observed_at": forecast.observed_at,
        "checked_at": checked_at,
        "error": error,
    })
}

/// Read a previously stored site entry back into a [`SiteForecast`].
fn carried_forecast(previous: Option<&Value>) -> SiteForecast {
    let Some(previous) = previous else {
        return SiteForecast::default();
    };
    let predictions = previous
        .get("predictions")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some(Prediction {
                        label: row.get("label").and_then(Value::as_str)?.to_string(),
                        percent: clamp_percent(row.get("percent").and_then(Value::as_f64))?,
                        volume_usd: row
                            .get("volume_usd")
                            .and_then(Value::as_f64)
                            .filter(|volume| volume.is_finite() && *volume >= 0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    SiteForecast {
        predictions,
        url: previous
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        detail: previous
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
        observed_at: previous.get("observed_at").and_then(Value::as_f64),
    }
}

/// Index the previous snapshot's site rows by id.
fn previous_sites(last_result: Option<&Value>) -> Map<String, Value> {
    let mut indexed = Map::new();
    if let Some(rows) = last_result
        .and_then(|last| last.get("sites"))
        .and_then(Value::as_array)
    {
        for row in rows {
            if let Some(id) = row.get("id").and_then(Value::as_str) {
                indexed.insert(id.to_string(), row.clone());
            }
        }
    }
    indexed
}

/// Median consensus and contributor count for one published horizon label.
///
/// Sites that do not publish that label (Polymarket market expiries, a
/// missing 48h ring) are omitted rather than filled in, so a missing side
/// stays missing.
fn consensus_for(rows: &[Value], horizon: &str) -> (Option<f64>, usize) {
    let sample: Vec<f64> = rows
        .iter()
        .filter_map(|row| row.get("predictions").and_then(Value::as_array))
        .flatten()
        .filter(|prediction| prediction.get("label").and_then(Value::as_str) == Some(horizon))
        .filter_map(|prediction| prediction.get("percent").and_then(Value::as_f64))
        .collect();
    let contributors = sample.len();
    (median(sample), contributors)
}

/// Whether a site is enabled in `codex_reset_forecast.sites.<id>.enabled`.
fn site_enabled(site_config: Option<&Value>, id: &str) -> bool {
    site_config
        .and_then(|sites| sites.get(id))
        .and_then(|site| site.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Collect Codex reset forecasts from every enabled site.
///
/// Each site self-throttles against its own `checked_at` in `last_result`, so
/// this is safe to call on every poll tick: between fetches it republishes the
/// carried values unchanged, which is what keeps the bar updating at the bar's
/// tick rate rather than at the fetch cadence.
///
/// Parameters:
/// - `last_result` — the previous `codex_reset_forecast` snapshot, if any.
/// - `now` — epoch seconds override (tests pass a fixed clock).
/// - `min_interval` — configured fetch cadence; raised per site to that site's
///   own `min_interval_seconds` floor.
/// - `horizon` — which published horizon `consensus_percent` uses. Both 24h
///   and 48h medians are always published alongside it.
/// - `site_config` — the `sites` object of per-site enable toggles.
/// - `opener` — HTTP seam; production passes `None` for [`UreqOpener`].
pub fn collect_codex_reset_forecast(
    last_result: Option<&Value>,
    now: Option<f64>,
    min_interval: Option<f64>,
    horizon: Option<&str>,
    site_config: Option<&Value>,
    opener: Option<&dyn HttpOpener>,
) -> Value {
    let now = now.unwrap_or_else(now_epoch);
    let min_interval = min_interval.unwrap_or(FORECAST_FETCH_MIN_INTERVAL_SECONDS);
    let horizon = horizon.unwrap_or(DEFAULT_HORIZON);
    let previous = previous_sites(last_result);

    let mut rows: Vec<Value> = Vec::new();
    for site in FORECAST_SITES {
        if !site_enabled(site_config, site.id) {
            continue;
        }
        let last_site = previous.get(site.id);
        let interval = min_interval.max(site.min_interval_seconds);
        if let Some(checked) = last_site
            .and_then(|row| row.get("checked_at"))
            .and_then(Value::as_f64)
        {
            if now - checked < interval {
                // Still inside this source's politeness window: republish it.
                rows.push(last_site.cloned().unwrap_or(Value::Null));
                continue;
            }
        }
        match fetch_site(site, opener) {
            Ok(forecast) => rows.push(site_value(site, &forecast, now, None)),
            Err(exc) => {
                let carried = carried_forecast(last_site);
                let previous_error = last_site
                    .and_then(|row| row.get("error"))
                    .and_then(Value::as_str);
                if previous_error != Some(exc.as_str()) {
                    logging::warn(format!("codex reset forecast {}: {exc}", site.id));
                }
                rows.push(site_value(site, &carried, now, Some(exc)));
            }
        }
    }

    let (consensus_24h, contrib_24h) = consensus_for(&rows, "24h");
    let (consensus_48h, contrib_48h) = consensus_for(&rows, "48h");
    let (consensus, contributors) = if horizon == "48h" {
        (consensus_48h, contrib_48h)
    } else {
        (consensus_24h, contrib_24h)
    };
    logging::debug(format!(
        "codex reset forecast consensus horizon={horizon} 24h={consensus_24h:?} 48h={consensus_48h:?}"
    ));

    let observed_at = rows
        .iter()
        .filter_map(|row| {
            row.get("observed_at")
                .and_then(Value::as_f64)
                .or_else(|| row.get("checked_at").and_then(Value::as_f64))
        })
        .fold(None::<f64>, |acc, value| {
            Some(acc.map_or(value, |current: f64| current.max(value)))
        });

    let error = if rows.is_empty() {
        Some("no forecast site enabled".to_string())
    } else if consensus.is_none() {
        Some(format!("no site published a {horizon} probability"))
    } else {
        None
    };

    json!({
        "source": "codex_reset_forecast",
        "horizon": horizon,
        "consensus_percent": consensus,
        "consensus_percent_24h": consensus_24h,
        "consensus_percent_48h": consensus_48h,
        "contributing_sites": contributors,
        "observed_at": observed_at,
        "checked_at": now,
        "sites": rows,
        "error": error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::http::{HttpError, HttpResponse};

    /// Opener that replays a canned body (or error) per endpoint and counts hits.
    struct FakeOpener {
        bodies: Vec<(String, Result<String, String>)>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl HttpOpener for FakeOpener {
        fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
            self.calls.lock().unwrap().push(request.url.clone());
            for (fragment, body) in &self.bodies {
                if request.url.contains(fragment.as_str()) {
                    return match body {
                        Ok(text) => Ok(HttpResponse {
                            status: 200,
                            body: text.clone().into_bytes(),
                            headers: Default::default(),
                        }),
                        Err(message) => Err(HttpError::transport(message.clone())),
                    };
                }
            }
            Err(HttpError::transport("unexpected url"))
        }
    }

    fn forecast_json() -> String {
        json!({
            "mode": "model",
            "confidence": "low",
            "updated_at": "2026-09-10T23:42:50.300Z",
            "probabilities": {"rounded_24h": 22, "rounded_48h": 39,
                              "raw_24h": 0.2178, "raw_48h": 0.3882},
        })
        .to_string()
    }

    fn monitor_html() -> String {
        r#"<div class="probability-ring" role="img"
             aria-label="GPT-estimated probability of another global reset within 24 hours: 24%"
             data-testid="probability-ring-24h" data-animated-value="0" data-target-value="24"></div>
           <div class="probability-ring" role="img"
             aria-label="GPT-estimated probability of another global reset within 48 hours: 42%"
             data-testid="probability-ring-48h" data-animated-value="0" data-target-value="42"></div>
           <strong data-testid="prediction-hit-rate">78% reset hit rate</strong>"#
            .to_string()
    }

    fn gamma_json() -> String {
        json!({"events": [{
        "title": "OpenAI resets Codex weekly usage limit by...?",
        "slug": "openai-resets-codex-weekly-usage-limit-by-20260914",
        "active": true,
        "closed": false,
        "creationDate": "2026-09-14T22:24:23.483583Z",
        "markets": [
            {"question": "Will OpenAI reset Codex weekly usage limit by September 21?",
             "closed": false, "active": true, "endDate": "2026-09-22T03:59:00Z",
             "volumeNum": 1234.56,
             "outcomes": "[\"Yes\", \"No\"]", "outcomePrices": "[\"0.535\", \"0.465\"]"},
            {"question": "Will OpenAI reset Codex weekly usage limit by September 14?",
             "closed": false, "active": true, "endDate": "2026-09-15T03:59:00Z",
             "volume": "78.9",
             "outcomes": "[\"Yes\", \"No\"]", "outcomePrices": "[\"0.49\", \"0.51\"]"},
            {"question": "Will OpenAI reset Codex weekly usage limit by September 7?",
             "closed": true, "active": true, "endDate": "2026-09-08T03:59:00Z",
             "outcomes": "[\"Yes\", \"No\"]", "outcomePrices": "[\"1\", \"0\"]"}
        ]}, {
        "title": "OpenAI resets Codex weekly usage limit by...?",
        "slug": "openai-resets-codex-weekly-usage-limit-by-20260901",
        "active": true,
        "closed": false,
        "creationDate": "2026-09-01T22:30:52.649997Z",
        "markets": [{
            "question": "Will OpenAI reset Codex weekly usage limit by September 12?",
            "closed": false, "active": true, "endDate": "2026-09-13T03:59:00Z",
            "outcomes": "[\"Yes\", \"No\"]", "outcomePrices": "[\"0.8\", \"0.2\"]"
        }]}]})
        .to_string()
    }

    fn all_sites_opener(calls: Arc<Mutex<Vec<String>>>) -> FakeOpener {
        FakeOpener {
            bodies: vec![
                ("codex-reset.com".into(), Ok(forecast_json())),
                ("codexreset.org".into(), Ok(monitor_html())),
                ("gamma-api".into(), Ok(gamma_json())),
            ],
            calls,
        }
    }

    #[test]
    fn parses_the_documented_forecast_api() {
        let parsed = parse_codex_reset(&forecast_json()).expect("parsed");
        assert_eq!(
            parsed.predictions,
            vec![
                Prediction {
                    label: "24h".into(),
                    percent: 22.0,
                    volume_usd: None,
                },
                Prediction {
                    label: "48h".into(),
                    percent: 39.0,
                    volume_usd: None,
                },
            ]
        );
        assert_eq!(parsed.detail.as_deref(), Some("model · low confidence"));
        assert!(parsed.observed_at.is_some());
    }

    #[test]
    fn falls_back_to_raw_probabilities() {
        let body = json!({"probabilities": {"raw_24h": 0.25, "raw_48h": 0.5}}).to_string();
        let parsed = parse_codex_reset(&body).expect("parsed");
        assert_eq!(parsed.predictions[0].percent, 25.0);
        assert_eq!(parsed.predictions[1].percent, 50.0);
    }

    #[test]
    fn rejects_a_forecast_payload_without_probabilities() {
        assert!(parse_codex_reset("{}").is_err());
        assert!(parse_codex_reset("not json").is_err());
        assert!(parse_codex_reset(&json!({"probabilities": {}}).to_string()).is_err());
    }

    #[test]
    fn parses_the_monitor_probability_rings() {
        let parsed = parse_codex_reset_monitor(&monitor_html()).expect("parsed");
        assert_eq!(
            parsed.predictions,
            vec![
                Prediction {
                    label: "24h".into(),
                    percent: 24.0,
                    volume_usd: None,
                },
                Prediction {
                    label: "48h".into(),
                    percent: 42.0,
                    volume_usd: None,
                },
            ]
        );
        assert_eq!(parsed.detail.as_deref(), Some("78% reset hit rate"));
    }

    #[test]
    fn falls_back_to_the_monitor_aria_label() {
        // Same numbers, but the data-target-value attribute is gone.
        let html = r#"<div aria-label="probability of another global reset within 24 hours: 31%"
                        data-testid="probability-ring-24h"></div>"#;
        let parsed = parse_codex_reset_monitor(html).expect("parsed");
        assert_eq!(
            parsed.predictions,
            vec![Prediction {
                label: "24h".into(),
                percent: 31.0,
                volume_usd: None,
            }]
        );
    }

    #[test]
    fn rejects_monitor_html_without_a_ring() {
        assert!(
            parse_codex_reset_monitor("<html><body>down for maintenance</body></html>").is_err()
        );
    }

    #[test]
    fn parses_the_nearest_open_polymarket_markets() {
        let parsed = parse_polymarket(&gamma_json()).expect("parsed");
        // Older event and closed market dropped; nearest current expiries first.
        assert_eq!(
            parsed.predictions,
            vec![
                Prediction {
                    label: "by September 14".into(),
                    percent: 49.0,
                    volume_usd: Some(78.9),
                },
                Prediction {
                    label: "by September 21".into(),
                    percent: 53.5,
                    volume_usd: Some(1234.56),
                },
            ]
        );
        assert_eq!(
            parsed.url.as_deref(),
            Some("https://polymarket.com/event/openai-resets-codex-weekly-usage-limit-by-20260914")
        );
    }

    #[test]
    fn ignores_unrelated_polymarket_events() {
        let body = json!({"events": [{
            "title": "Who wins the election?",
            "markets": [{"question": "Will X win by November 3?", "closed": false, "active": true,
                         "endDate": "2026-11-04T03:59:00Z",
                         "outcomes": "[\"Yes\", \"No\"]", "outcomePrices": "[\"0.6\", \"0.4\"]"}]}]})
        .to_string();
        assert!(parse_polymarket(&body).is_err());
    }

    #[test]
    fn ignores_invalid_polymarket_volume_without_losing_the_market() {
        let mut payload: Value = serde_json::from_str(&gamma_json()).unwrap();
        payload["events"][0]["markets"][0]["volumeNum"] = json!(-5);
        payload["events"][0]["markets"][1]["volume"] = json!("unknown");
        let parsed = parse_polymarket(&payload.to_string()).expect("market remains available");
        assert!(parsed
            .predictions
            .iter()
            .all(|row| row.volume_usd.is_none()));
    }

    #[test]
    fn merges_every_site_into_one_consensus() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opener = all_sites_opener(calls.clone());
        let result =
            collect_codex_reset_forecast(None, Some(1_000.0), None, None, None, Some(&opener));

        assert_eq!(result["horizon"], json!("24h"));
        // Median of the two published 24h numbers (22, 24); Polymarket has no
        // 24h horizon, so it stays menu-only.
        assert_eq!(result["consensus_percent"], json!(23.0));
        assert_eq!(result["consensus_percent_24h"], json!(23.0));
        // Median of the two published 48h numbers (39, 42).
        assert_eq!(result["consensus_percent_48h"], json!(40.5));
        assert_eq!(result["contributing_sites"], json!(2));
        assert_eq!(result["error"], Value::Null);
        assert_eq!(result["sites"].as_array().unwrap().len(), 3);
        assert_eq!(
            result["sites"][2]["predictions"][0]["volume_usd"],
            json!(78.9)
        );
        assert_eq!(
            result["sites"][2]["predictions"][1]["volume_usd"],
            json!(1234.56)
        );
        assert_eq!(
            result["sites"][2]["url"],
            json!(
                "https://polymarket.com/event/openai-resets-codex-weekly-usage-limit-by-20260914"
            )
        );
        assert_eq!(calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn honors_the_configured_horizon() {
        let opener = all_sites_opener(Arc::new(Mutex::new(Vec::new())));
        let result = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            Some("48h"),
            None,
            Some(&opener),
        );
        // Median of 39 and 42. Both horizons stay published so the bar can
        // show 24-48h consensus regardless of which horizon is configured.
        assert_eq!(result["consensus_percent"], json!(40.5));
        assert_eq!(result["consensus_percent_24h"], json!(23.0));
        assert_eq!(result["consensus_percent_48h"], json!(40.5));
    }

    #[test]
    fn skips_sites_disabled_in_config() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opener = all_sites_opener(calls.clone());
        let sites =
            json!({"codex_reset_monitor": {"enabled": false}, "polymarket": {"enabled": false}});
        let result = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            Some(&sites),
            Some(&opener),
        );
        assert_eq!(result["sites"].as_array().unwrap().len(), 1);
        assert_eq!(result["sites"][0]["id"], json!("codex_reset"));
        assert_eq!(result["consensus_percent"], json!(22.0));
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn throttles_each_site_against_its_own_floor() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let first = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        calls.lock().unwrap().clear();

        // 60 s later: the 30 s sites refetch, the 300 s monitor does not.
        let second = collect_codex_reset_forecast(
            Some(&first),
            Some(1_060.0),
            None,
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        let fetched = calls.lock().unwrap().clone();
        assert_eq!(fetched.len(), 2);
        assert!(fetched.iter().all(|url| !url.contains("codexreset.org")));
        // The throttled site still reports its carried value.
        assert_eq!(second["consensus_percent"], json!(23.0));
    }

    #[test]
    fn republishes_carried_values_inside_the_throttle_window() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let first = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        calls.lock().unwrap().clear();
        let second = collect_codex_reset_forecast(
            Some(&first),
            Some(1_005.0),
            None,
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "no site refetched after 5s"
        );
        assert_eq!(second["consensus_percent"], first["consensus_percent"]);
        assert_eq!(second["sites"], first["sites"]);
    }

    #[test]
    fn a_configured_interval_can_only_slow_a_site_down() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let first = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            Some(5.0),
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        calls.lock().unwrap().clear();
        // 10 s later: even with a 5 s config, every floor (30 s / 300 s) holds.
        collect_codex_reset_forecast(
            Some(&first),
            Some(1_010.0),
            Some(5.0),
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        assert!(calls.lock().unwrap().is_empty());

        // A larger configured interval wins over the smaller per-site floor.
        calls.lock().unwrap().clear();
        collect_codex_reset_forecast(
            Some(&first),
            Some(1_060.0),
            Some(600.0),
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn failed_sites_carry_their_last_values_links_and_errors() {
        let good = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            None,
            Some(&all_sites_opener(Arc::new(Mutex::new(Vec::new())))),
        );
        let broken = FakeOpener {
            bodies: vec![
                ("codex-reset.com".into(), Err("connection refused".into())),
                ("codexreset.org".into(), Ok(monitor_html())),
                ("gamma-api".into(), Err("connection refused".into())),
            ],
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let result = collect_codex_reset_forecast(
            Some(&good),
            Some(2_000.0),
            None,
            None,
            None,
            Some(&broken),
        );

        let failed = &result["sites"][0];
        assert_eq!(failed["id"], json!("codex_reset"));
        assert_eq!(failed["error"], json!("connection refused"));
        assert_eq!(failed["predictions"][0]["percent"], json!(22.0));
        let failed_market = &result["sites"][2];
        assert_eq!(failed_market["error"], json!("connection refused"));
        assert_eq!(failed_market["predictions"][0]["volume_usd"], json!(78.9));
        assert_eq!(
            failed_market["url"],
            json!(
                "https://polymarket.com/event/openai-resets-codex-weekly-usage-limit-by-20260914"
            )
        );
        // The bar keeps a consensus rather than blanking on one bad source.
        assert_eq!(result["consensus_percent"], json!(23.0));
    }

    #[test]
    fn reports_an_error_when_no_site_publishes_the_horizon() {
        let opener = FakeOpener {
            bodies: vec![("gamma-api".into(), Ok(gamma_json()))],
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let sites =
            json!({"codex_reset": {"enabled": false}, "codex_reset_monitor": {"enabled": false}});
        let result = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            Some(&sites),
            Some(&opener),
        );
        assert_eq!(result["consensus_percent"], Value::Null);
        assert_eq!(result["consensus_percent_24h"], Value::Null);
        assert_eq!(result["consensus_percent_48h"], Value::Null);
        assert_eq!(
            result["error"],
            json!("no site published a 24h probability")
        );
        // Polymarket still contributes its market rows to the menu.
        assert_eq!(
            result["sites"][0]["predictions"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn reports_an_error_when_every_site_is_disabled() {
        let sites = json!({
            "codex_reset": {"enabled": false},
            "codex_reset_monitor": {"enabled": false},
            "polymarket": {"enabled": false},
        });
        let result =
            collect_codex_reset_forecast(None, Some(1_000.0), None, None, Some(&sites), None);
        assert_eq!(result["error"], json!("no forecast site enabled"));
        assert_eq!(result["sites"], json!([]));
    }

    #[test]
    fn every_registry_entry_is_a_public_https_endpoint() {
        assert!(FORECAST_SITES.len() >= 3, "multiple sites are the point");
        for site in FORECAST_SITES {
            assert!(site.endpoint.starts_with("https://"), "{}", site.id);
            assert!(site.url.starts_with("https://"), "{}", site.id);
            assert!(!site.name.is_empty(), "{}", site.id);
            assert!(
                site.min_interval_seconds >= FORECAST_FETCH_MIN_INTERVAL_SECONDS,
                "{}",
                site.id
            );
            assert!(forecast_site(site.id).is_some());
        }
    }

    #[test]
    fn median_handles_both_sample_parities() {
        assert_eq!(median(vec![]), None);
        assert_eq!(median(vec![10.0]), Some(10.0));
        assert_eq!(median(vec![10.0, 20.0]), Some(15.0));
        assert_eq!(median(vec![30.0, 10.0, 20.0]), Some(20.0));
    }

    #[test]
    fn percentages_are_clamped_and_finite() {
        assert_eq!(clamp_percent(Some(140.0)), Some(100.0));
        assert_eq!(clamp_percent(Some(-4.0)), Some(0.0));
        assert_eq!(clamp_percent(Some(f64::NAN)), None);
        assert_eq!(clamp_percent(None), None);
    }

    #[test]
    fn market_questions_shorten_to_a_horizon_label() {
        assert_eq!(
            polymarket_label("Will OpenAI reset Codex weekly usage limit by September 14?"),
            "by September 14"
        );
        assert_eq!(polymarket_label("Something else"), "Something else");
    }
}
