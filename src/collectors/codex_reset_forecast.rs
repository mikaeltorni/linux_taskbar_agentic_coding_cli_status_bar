//! Codex weekly-limit reset forecasts from public community trackers.
//!
//! OpenAI grants discretionary ("goodwill") Codex weekly-limit resets on no
//! published schedule. This collector reads Codex Reset Monitor probabilities,
//! and WhenReset history. Only published 24h/48h
//! probabilities contribute to the percentage next to the Codex segments.
//!
//! Major components:
//! - [`FORECAST_SITES`] — the vetted source registry (id, display name, page,
//!   endpoint, politeness floor, parser). Adding a source is a registry entry
//!   plus a parser, never a new fetch/throttle path.
//! - [`RESET_REFERENCE_SITES`] — public reset-history sources
//!   summarized in linked rows without adding a consensus vote.
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

use crate::collectors::util::now_epoch;
use crate::http::{HttpOpener, HttpRequest, UreqOpener};

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
    /// a dated forecast) is menu-only.
    pub label: String,
    pub percent: f64,
    /// Optional published volume metadata from older snapshots.
    pub volume_usd: Option<f64>,
    /// Optional validity deadline as a UTC epoch; 24h/48h horizons leave it unset.
    pub end_at: Option<f64>,
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
    /// A dated official commitment can raise the probability for horizons
    /// containing its deadline. The model predictions remain available after it expires.
    commitment: Option<DatedCommitment>,
}

#[derive(Debug, Clone, PartialEq)]
struct DatedCommitment {
    percent: f64,
    deadline_at: f64,
}

/// A vetted forecast source.
///
/// `endpoint` is what the collector fetches; `url` is the human page shown as
/// provenance. `min_interval_seconds` is this source's politeness floor;
/// `display_order` places its row in the popup.
#[derive(Debug)]
pub struct ForecastSite {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub endpoint: &'static str,
    pub min_interval_seconds: f64,
    pub display_order: usize,
    pub parse: fn(&str) -> Result<SiteForecast, String>,
}

/// The vetted source registry — the single catalog of forecast sites.
///
/// Selection criteria: a public read-only endpoint, a `robots.txt` that allows
/// it, a stated methodology, and a machine-readable published probability.
///
/// - `codexreset.org` — "Codex Reset Monitor"; HTML only, so it is parsed from
///   stable `data-testid` attributes. Its page is ~1 MB and its own data
///   refreshes hourly, hence the 300 s floor.
pub const FORECAST_SITES: &[ForecastSite] = &[ForecastSite {
    id: "codex_reset_monitor",
    name: "codexreset.org",
    url: "https://codexreset.org/",
    endpoint: "https://codexreset.org/",
    min_interval_seconds: 300.0,
    display_order: 3,
    parse: parse_codex_reset_monitor,
}];

/// A vetted public supplemental source whose data is not an independent
/// numeric forecast for the consensus.
pub struct ResetReferenceSite {
    pub id: &'static str,
    pub name: &'static str,
    pub url: &'static str,
    pub endpoint: &'static str,
    pub parse: fn(&str) -> Result<SiteForecast, String>,
    pub min_interval_seconds: f64,
    pub display_order: usize,
}

/// Supplemental sources with live summaries, excluded from the consensus.
pub const RESET_REFERENCE_SITES: &[ResetReferenceSite] = &[ResetReferenceSite {
    id: "whenreset",
    name: "WhenReset",
    url: "https://whenreset.dev/codex",
    endpoint: "https://whenreset.dev/api/reset-feed?provider=codex",
    parse: parse_whenreset_feed,
    min_interval_seconds: 300.0,
    display_order: 4,
}];

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

/// Format a published UTC event timestamp for a compact source row.
fn event_time(epoch: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|date| date.format("%b %-d %H:%M").to_string())
        .ok_or_else(|| "source published an invalid event timestamp".to_string())
}

/// Read the newest announced event from WhenReset's public Codex RSS feed.
/// A bonus reset is a banked credit; it does not refill usage immediately.
fn parse_whenreset_feed(body: &str) -> Result<SiteForecast, String> {
    let item = body
        .split("<item>")
        .nth(1)
        .and_then(|part| part.split("</item>").next())
        .ok_or_else(|| "WhenReset feed published no Codex event".to_string())?;
    let title = item
        .split("<title>")
        .nth(1)
        .and_then(|part| part.split("</title>").next())
        .ok_or_else(|| "WhenReset event has no title".to_string())?;
    let kind = if title.contains("Bonus reset") {
        "banked reset"
    } else if title.contains("Usage reset") {
        "direct reset"
    } else {
        return Err("WhenReset event has an unknown reset type".into());
    };
    let date = item
        .split("<pubDate>")
        .nth(1)
        .and_then(|part| part.split("</pubDate>").next())
        .and_then(|date| chrono::DateTime::parse_from_rfc2822(date).ok())
        .ok_or_else(|| "WhenReset event has no valid publication time".to_string())?;
    Ok(SiteForecast {
        detail: Some(format!(
            "latest {kind} · {} UTC",
            event_time(date.timestamp())?
        )),
        observed_at: Some(date.timestamp() as f64),
        ..SiteForecast::default()
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
                end_at: None,
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
        commitment: None,
    })
}

/// Fetch one site's endpoint and parse it.
fn fetch_site(
    endpoint: &str,
    parse: fn(&str) -> Result<SiteForecast, String>,
    opener: Option<&dyn HttpOpener>,
) -> Result<SiteForecast, String> {
    let request = HttpRequest {
        url: endpoint.to_string(),
        method: "GET".into(),
        headers: vec![
            (
                "Accept".into(),
                "application/json, text/html;q=0.9, application/rss+xml;q=0.8".into(),
            ),
            ("Accept-Encoding".into(), "gzip".into()),
            ("User-Agent".into(), USER_AGENT.into()),
        ],
        body: None,
        timeout: REQUEST_TIMEOUT,
    };
    let default_opener = UreqOpener;
    let opener: &dyn HttpOpener = opener.unwrap_or(&default_opener);
    let response = opener.open(&request).map_err(|exc| exc.to_string())?;
    parse(&response.text())
}

/// Whether a dated prediction is still inside its vote window at `now`.
///
/// An optional cached validity deadline is compared with the collector clock.
/// Parameters: prediction - cached prediction; now - current epoch.
/// Returns: whether the optional validity deadline has not passed.
fn vote_window_open(prediction: &Prediction, now: f64) -> bool {
    println!("prediction={prediction:?}, now={now}");
    let result = match prediction.end_at {
        Some(end_at) => now < end_at,
        None => true,
    };
    println!("{result}");
    result
}

/// Predictions still valid at `now`. Dated rows keep the two
/// earliest remaining windows; undated 24h/48h rows are unchanged.
/// Parameters: predictions - cached forecast values; now - current epoch.
/// Returns: the still-valid predictions.
fn visible_predictions(predictions: &[Prediction], now: f64) -> Vec<Prediction> {
    println!("predictions={predictions:?}, now={now}");
    let mut open: Vec<Prediction> = predictions
        .iter()
        .filter(|prediction| vote_window_open(prediction, now))
        .cloned()
        .collect();
    if open.iter().any(|prediction| prediction.end_at.is_some()) {
        open.sort_by(|left, right| {
            left.end_at
                .unwrap_or(f64::INFINITY)
                .partial_cmp(&right.end_at.unwrap_or(f64::INFINITY))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        open.truncate(2);
    }
    println!("{open:?}");
    open
}

/// Serialize a site's state for `status.json`.
/// Parameters: site - source metadata; forecast - parsed data; checked_at - fetch epoch; error - optional failure; now - current epoch.
/// Returns: the serialized forecast source row.
fn site_value(
    site: &ForecastSite,
    forecast: &SiteForecast,
    checked_at: f64,
    error: Option<String>,
    now: f64,
) -> Value {
    println!(
        "site={site:?}, forecast={forecast:?}, checked_at={checked_at}, error={error:?}, now={now}"
    );
    let visible = visible_predictions(&forecast.predictions, now);
    let baseline_predictions: Vec<Value> = visible
        .iter()
        .map(|prediction| {
            let mut row = json!({"label": prediction.label, "percent": prediction.percent});
            if let Some(volume) = prediction.volume_usd {
                row["volume_usd"] = json!(volume);
            }
            if let Some(end_at) = prediction.end_at {
                row["end_at"] = json!(end_at);
            }
            row
        })
        .collect();
    let mut predictions = baseline_predictions.clone();
    let mut commitment_applied = false;
    if let Some(commitment) = &forecast.commitment {
        for prediction in &mut predictions {
            let horizon_seconds = match prediction.get("label").and_then(Value::as_str) {
                Some("24h") => 24.0 * 3600.0,
                Some("48h") => 48.0 * 3600.0,
                _ => continue,
            };
            if commitment.deadline_at > now && commitment.deadline_at <= now + horizon_seconds {
                let model_percent = prediction["percent"].as_f64().unwrap_or(0.0);
                if commitment.percent > model_percent {
                    prediction["percent"] = json!(commitment.percent);
                    commitment_applied = true;
                }
            }
        }
    }
    let mut row = json!({
        "id": site.id,
        "name": if commitment_applied {
            format!("{} (official commitment)", site.name)
        } else {
            site.name.to_string()
        },
        "url": forecast.url.as_deref().unwrap_or(site.url),
        "predictions": predictions,
        "detail": forecast.detail,
        "observed_at": forecast.observed_at,
        "checked_at": checked_at,
        "error": error,
    });
    if let Some(commitment) = &forecast.commitment {
        row["baseline_predictions"] = json!(baseline_predictions);
        row["commitment"] = json!({
            "percent": commitment.percent,
            "deadline_at": commitment.deadline_at,
        });
        row["commitment_applied"] = json!(commitment_applied);
    }
    println!("{row}");
    row
}

/// Serialize a supplemental source in the same shape as a forecast row.
fn reference_site_value(
    site: &ResetReferenceSite,
    forecast: &SiteForecast,
    checked_at: f64,
    error: Option<String>,
) -> Value {
    let detail = forecast.detail.as_deref().map(|detail| {
        if site.id == "will_codex_reset" {
            detail.replace(" · mirrored outlook", "")
        } else {
            detail.to_string()
        }
    });
    json!({
        "id": site.id,
        "name": site.name,
        "url": site.url,
        "reference": true,
        "predictions": [],
        "detail": match (detail.as_deref(), error.as_deref()) {
            (Some(detail), Some(_)) => {
                format!("{} · last known", detail.trim_end_matches(" · last known"))
            },
            (Some(detail), None) => detail.to_string(),
            (None, _) => "live data unavailable".to_string(),
        },
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
        .get("baseline_predictions")
        .or_else(|| previous.get("predictions"))
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
                        end_at: row.get("end_at").and_then(Value::as_f64),
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
        commitment: previous.get("commitment").and_then(|value| {
            Some(DatedCommitment {
                percent: clamp_percent(value.get("percent").and_then(Value::as_f64))?,
                deadline_at: value.get("deadline_at").and_then(Value::as_f64)?,
            })
        }),
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
/// Sources without that horizon (history rows or a missing 48h ring)
/// are omitted rather than filled in, so a missing side
/// stays missing.
/// Parameters: rows - public source rows; horizon - requested forecast horizon.
/// Returns: median estimate and contributor count.
fn consensus_for(rows: &[Value], horizon: &str) -> (Option<f64>, usize) {
    println!("rows={rows:?}, horizon={horizon}");
    let sample: Vec<f64> = rows
        .iter()
        .filter_map(|row| row.get("predictions").and_then(Value::as_array))
        .flatten()
        .filter(|prediction| prediction.get("label").and_then(Value::as_str) == Some(horizon))
        .filter_map(|prediction| prediction.get("percent").and_then(Value::as_f64))
        .collect();
    let contributors = sample.len();
    let result = (median(sample), contributors);
    println!("{result:?}");
    result
}

/// Whether a site is enabled in `codex_reset_forecast.sites.<id>.enabled`.
fn site_enabled(site_config: Option<&Value>, id: &str) -> bool {
    site_config
        .and_then(|sites| sites.get(id))
        .and_then(|site| site.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Return the last check time while this source remains inside its fetch floor.
fn checked_within_interval(previous: Option<&Value>, now: f64, interval: f64) -> Option<f64> {
    previous
        .and_then(|row| row.get("checked_at"))
        .and_then(Value::as_f64)
        .filter(|checked| now - checked < interval)
}

/// Collect Codex reset forecasts from every enabled site.
///
/// Each site self-throttles against its own `checked_at` in `last_result`, so
/// this is safe to call on every poll tick: between fetches it republishes the
/// carried values unchanged, which is what keeps the bar updating at the bar's
/// tick rate rather than at the fetch cadence.
///
/// Parameters: last_result - previous forecast snapshot; now - optional epoch;
/// min_interval - fetch cadence; horizon - selected forecast horizon;
/// site_config - source toggles; opener - optional HTTP transport.
/// Returns: the retained-source forecast snapshot.
///
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
    println!("last_result={last_result:?}, now={now:?}, min_interval={min_interval:?}, horizon={horizon:?}, site_config={site_config:?}, opener={:?}", opener.map(|value| value as *const dyn HttpOpener));
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
        if let Some(checked) = checked_within_interval(last_site, now, interval) {
            // Re-serialize any dated commitment that can expire before refetch.
            if last_site.and_then(|row| row.get("commitment")).is_some() {
                let carried = carried_forecast(last_site);
                let error = last_site
                    .and_then(|row| row.get("error"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                rows.push(site_value(site, &carried, checked, error, now));
            } else {
                rows.push(last_site.cloned().unwrap_or(Value::Null));
            }
            continue;
        }
        match fetch_site(site.endpoint, site.parse, opener) {
            Ok(forecast) => rows.push(site_value(site, &forecast, now, None, now)),
            Err(exc) => {
                let carried = carried_forecast(last_site);
                let previous_error = last_site
                    .and_then(|row| row.get("error"))
                    .and_then(Value::as_str);
                if previous_error != Some(exc.as_str()) {
                    println!("codex reset forecast {}: {exc}", site.id);
                }
                rows.push(site_value(site, &carried, now, Some(exc), now));
            }
        }
    }

    let forecast_rows = rows.len();
    for site in RESET_REFERENCE_SITES {
        if !site_enabled(site_config, site.id) {
            continue;
        }
        let last_site = previous.get(site.id);
        let interval = min_interval.max(site.min_interval_seconds);
        if let Some(checked) = checked_within_interval(last_site, now, interval) {
            if site.id == "will_codex_reset" {
                // Re-serialize the cached row so the retired caption
                // disappears immediately after upgrading, without losing data.
                let carried = carried_forecast(last_site);
                let error = last_site
                    .and_then(|row| row.get("error"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                rows.push(reference_site_value(site, &carried, checked, error));
            } else {
                rows.push(last_site.cloned().unwrap_or(Value::Null));
            }
            continue;
        }
        match fetch_site(site.endpoint, site.parse, opener) {
            Ok(forecast) => rows.push(reference_site_value(site, &forecast, now, None)),
            Err(exc) => {
                let carried = carried_forecast(last_site);
                let previous_error = last_site
                    .and_then(|row| row.get("error"))
                    .and_then(Value::as_str);
                if previous_error != Some(exc.as_str()) {
                    println!("codex reset reference {}: {exc}", site.id);
                }
                rows.push(reference_site_value(site, &carried, now, Some(exc)));
            }
        }
    }

    // Sort the combined forecast and reference rows by their registry-owned
    // popup positions. Fetch order remains independent of presentation.
    rows.sort_by_key(|row| {
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        FORECAST_SITES
            .iter()
            .find(|site| site.id == id)
            .map(|site| site.display_order)
            .or_else(|| {
                RESET_REFERENCE_SITES
                    .iter()
                    .find(|site| site.id == id)
                    .map(|site| site.display_order)
            })
            .unwrap_or(usize::MAX)
    });

    if let Some(current) = rows.iter().find(|row| row["id"] == "codex_reset") {
        let active = current["commitment_applied"].as_bool().unwrap_or(false);
        let was_active = previous
            .get("codex_reset")
            .and_then(|row| row.get("commitment_applied"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if active != was_active {
            println!(
                "{}",
                format!(
                    "codex reset forecast official commitment {}",
                    if active {
                        "active"
                    } else {
                        "expired or withdrawn"
                    }
                )
            );
        }
    }

    let (consensus_24h, contrib_24h) = consensus_for(&rows, "24h");
    let (consensus_48h, contrib_48h) = consensus_for(&rows, "48h");
    let (consensus, contributors) = if horizon == "48h" {
        (consensus_48h, contrib_48h)
    } else {
        (consensus_24h, contrib_24h)
    };
    println!(
        "codex reset forecast consensus horizon={horizon} 24h={consensus_24h:?} 48h={consensus_48h:?}"
    );

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

    let error = if forecast_rows == 0 {
        Some("no forecast site enabled".to_string())
    } else if consensus.is_none() {
        Some(format!("no site published a {horizon} probability"))
    } else {
        None
    };

    let result = json!({
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
    });
    println!("{result}");
    result
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

    #[test]
    /// Verify reference sites publish live context without changing consensus.
    /// Parameters: none.
    /// Returns: None.
    fn reference_sites_publish_live_context_without_changing_consensus() {
        println!("parameters=none");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let result = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        let sites = result["sites"].as_array().unwrap();
        for (id, url) in [("whenreset", "https://whenreset.dev/codex")] {
            let site = sites.iter().find(|site| site["id"] == id).expect(id);
            assert_eq!(site["url"], json!(url));
            assert_eq!(site["reference"], json!(true));
            assert_eq!(site["predictions"], json!([]));
            assert!(site["detail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty()));
        }
        assert_eq!(
            calls.lock().unwrap().len(),
            FORECAST_SITES.len() + RESET_REFERENCE_SITES.len()
        );
        assert_eq!(result["contributing_sites"], json!(1));
        println!("None");
    }

    #[test]
    fn parses_whenreset_banked_reset_without_implying_immediate_refill() {
        let feed = r#"<rss><channel><item><title>Tibo · Bonus reset</title><pubDate>Tue, 22 Sep 2026 18:23:37 GMT</pubDate></item></channel></rss>"#;
        let parsed = parse_whenreset_feed(feed).expect("valid feed");
        assert_eq!(
            parsed.detail.as_deref(),
            Some("latest banked reset · Sep 22 18:23 UTC")
        );
        assert!(parsed.predictions.is_empty());
        assert!(parse_whenreset_feed("<rss><channel></channel></rss>").is_err());
    }

    #[test]
    fn failed_reference_keeps_one_stale_marker() {
        let site = &RESET_REFERENCE_SITES[0];
        let forecast = SiteForecast {
            detail: Some("latest banked reset · Sep 22 18:23 UTC · last known".into()),
            ..SiteForecast::default()
        };
        let row = reference_site_value(site, &forecast, 1_000.0, Some("offline".into()));
        assert_eq!(
            row["detail"],
            "latest banked reset · Sep 22 18:23 UTC · last known"
        );
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

    /// Replay all retained public source endpoints.
    /// Parameters: calls - shared outgoing-request recorder.
    /// Returns: the fake opener with retained source bodies.
    fn all_sites_opener(calls: Arc<Mutex<Vec<String>>>) -> FakeOpener {
        println!("calls={calls:?}");
        let result = FakeOpener {
            bodies: vec![
                ("codexreset.org".into(), Ok(monitor_html())),
                ("whenreset.dev".into(), Ok("<rss><channel><item><title>Tibo · Bonus reset</title><pubDate>Tue, 22 Sep 2026 18:23:37 GMT</pubDate></item></channel></rss>".into())),
            ],
            calls,
        };
        println!("bodies={:?}, calls={:?}", result.bodies, result.calls);
        result
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
                    end_at: None,
                },
                Prediction {
                    label: "48h".into(),
                    percent: 42.0,
                    volume_usd: None,
                    end_at: None,
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
                end_at: None,
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
    /// Verify merges every site into one consensus.
    /// Parameters: none.
    /// Returns: None.
    fn merges_every_site_into_one_consensus() {
        println!("parameters=none");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opener = all_sites_opener(calls.clone());
        let result =
            collect_codex_reset_forecast(None, Some(1_000.0), None, None, None, Some(&opener));

        assert_eq!(result["horizon"], json!("24h"));
        // Historical reference rows do not vote in the forecast.
        assert_eq!(result["consensus_percent"], json!(24.0));
        assert_eq!(result["consensus_percent_24h"], json!(24.0));
        assert_eq!(result["consensus_percent_48h"], json!(42.0));
        assert_eq!(result["contributing_sites"], json!(1));
        assert_eq!(result["error"], Value::Null);
        assert_eq!(result["sites"].as_array().unwrap().len(), 2);
        assert_eq!(
            result["sites"]
                .as_array()
                .unwrap()
                .iter()
                .map(|site| site["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["codex_reset_monitor", "whenreset"]
        );
        assert!(!result["sites"]
            .as_array()
            .unwrap()
            .iter()
            .any(|site| site["id"] == "nextreset"));
        assert_eq!(result["sites"][0]["url"], json!("https://codexreset.org/"));
        assert_eq!(calls.lock().unwrap().len(), 2);
        println!("None");
    }

    #[test]
    /// Verify honors the configured horizon.
    /// Parameters: none.
    /// Returns: None.
    fn honors_the_configured_horizon() {
        println!("parameters=none");
        let opener = all_sites_opener(Arc::new(Mutex::new(Vec::new())));
        let result = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            Some("48h"),
            None,
            Some(&opener),
        );
        // Both horizons stay published regardless of which is configured.
        assert_eq!(result["consensus_percent"], json!(42.0));
        assert_eq!(result["consensus_percent_24h"], json!(24.0));
        assert_eq!(result["consensus_percent_48h"], json!(42.0));
        println!("None");
    }

    #[test]
    /// Verify skips sites disabled in config.
    /// Parameters: none.
    /// Returns: None.
    fn skips_sites_disabled_in_config() {
        println!("parameters=none");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opener = all_sites_opener(calls.clone());
        let sites = json!({
            "codex_reset_monitor": {"enabled": true},
            "polymarket": {"enabled": false},
            "whenreset": {"enabled": false},
            "will_codex_reset": {"enabled": false}
        });
        let result = collect_codex_reset_forecast(
            None,
            Some(1_000.0),
            None,
            None,
            Some(&sites),
            Some(&opener),
        );
        assert_eq!(result["sites"].as_array().unwrap().len(), 1);
        assert_eq!(result["sites"][0]["id"], json!("codex_reset_monitor"));
        assert_eq!(result["consensus_percent"], json!(24.0));
        assert_eq!(calls.lock().unwrap().len(), 1);
        println!("None");
    }

    #[test]
    /// Verify throttles each site against its own floor.
    /// Parameters: none.
    /// Returns: None.
    fn throttles_each_site_against_its_own_floor() {
        println!("parameters=none");
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

        // Both retained sources hold their 300 s minimum interval.
        let second = collect_codex_reset_forecast(
            Some(&first),
            Some(1_060.0),
            None,
            None,
            None,
            Some(&all_sites_opener(calls.clone())),
        );
        let fetched = calls.lock().unwrap().clone();
        assert!(fetched.is_empty());
        assert!(fetched.iter().all(|url| !url.contains("codexreset.org")));
        // The throttled site still reports its carried value.
        assert_eq!(second["consensus_percent"], json!(24.0));
        println!("None");
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
    /// Verify failed sites carry their last values links and errors.
    /// Parameters: none.
    /// Returns: None.
    fn failed_sites_carry_their_last_values_links_and_errors() {
        println!("parameters=none");
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
                ("codexreset.org".into(), Err("connection refused".into())),
                ("whenreset.dev".into(), Err("connection refused".into())),
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
        assert_eq!(failed["id"], json!("codex_reset_monitor"));
        assert_eq!(failed["error"], json!("connection refused"));
        assert_eq!(failed["predictions"][0]["percent"], json!(24.0));
        assert_eq!(failed["url"], json!("https://codexreset.org/"));
        assert_eq!(result["sites"][1]["id"], json!("whenreset"));
        assert_eq!(result["sites"][1]["error"], json!("connection refused"));
        assert!(result["sites"][1]["detail"]
            .as_str()
            .unwrap()
            .ends_with(" · last known"));
        // The bar keeps a consensus rather than blanking on one bad source.
        assert_eq!(result["consensus_percent"], json!(24.0));
        println!("None");
    }

    #[test]
    /// Verify a missing forecast horizon stays unavailable without fabricated estimates.
    /// Parameters: none.
    /// Returns: None.
    fn reports_an_error_when_no_site_publishes_the_horizon() {
        println!("parameters=none");
        let opener = FakeOpener {
            bodies: vec![(
                "codexreset.org".into(),
                Ok(
                    r#"<div data-testid="probability-ring-24h" data-target-value="24"></div>"#
                        .into(),
                ),
            )],
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let sites = json!({"whenreset": {"enabled": false}});
        let result = collect_codex_reset_forecast(
            None,
            Some(1000.0),
            None,
            Some("48h"),
            Some(&sites),
            Some(&opener),
        );
        assert_eq!(result["consensus_percent"], Value::Null);
        assert_eq!(result["consensus_percent_24h"], json!(24.0));
        assert_eq!(result["consensus_percent_48h"], Value::Null);
        assert_eq!(
            result["error"],
            json!("no site published a 48h probability")
        );
        assert_eq!(
            result["sites"][0]["predictions"].as_array().unwrap().len(),
            1
        );
        println!("None");
    }

    #[test]
    fn reports_an_error_when_every_site_is_disabled() {
        let sites = json!({
            "codex_reset": {"enabled": false},
            "codex_reset_monitor": {"enabled": false},
            "polymarket": {"enabled": false},
            "whenreset": {"enabled": false},
            "will_codex_reset": {"enabled": false},
        });
        let result =
            collect_codex_reset_forecast(None, Some(1_000.0), None, None, Some(&sites), None);
        assert_eq!(result["error"], json!("no forecast site enabled"));
        assert_eq!(result["sites"], json!([]));
    }

    #[test]
    /// Verify every registry entry is a public https endpoint.
    /// Parameters: none.
    /// Returns: None.
    fn every_registry_entry_is_a_public_https_endpoint() {
        println!("parameters=none");
        assert_eq!(FORECAST_SITES.len(), 1);
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
        for site in RESET_REFERENCE_SITES {
            assert!(site.url.starts_with("https://"), "{}", site.id);
            assert!(site.endpoint.starts_with("https://"), "{}", site.id);
            assert!(!site.name.is_empty(), "{}", site.id);
            assert!(site.min_interval_seconds >= FORECAST_FETCH_MIN_INTERVAL_SECONDS);
            assert!(forecast_site(site.id).is_none(), "{}", site.id);
        }
        println!("None");
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
}
