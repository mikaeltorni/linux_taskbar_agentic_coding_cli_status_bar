//! Public regressions for the retained Codex reset source catalog.
use std::sync::Mutex;

use agentic_cli_status::collectors::codex_reset_forecast::collect_codex_reset_forecast;
use agentic_cli_status::config::{default_config, load_config};
use agentic_cli_status::http::{HttpError, HttpOpener, HttpRequest, HttpResponse};
use serde_json::{json, Value};

#[derive(Debug, Default)]
struct PublicSources {
    calls: Mutex<Vec<String>>,
    offline: bool,
}

impl HttpOpener for PublicSources {
    /// Replay retained public sources and reject any unapproved request.
    /// Parameters: self - fake source state; request - outgoing public HTTP request.
    /// Returns: retained source response, or a transport failure.
    fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        println!("self={self:?}, request={request:?}");
        self.calls.lock().unwrap().push(request.url.clone());
        let result = if self.offline {
            Err(HttpError::transport("offline"))
        } else {
            let body = match request.url.as_str() {
                url if url.starts_with("https://gamma-api.polymarket.com/") => r#"{"events":[]}"#.to_string(),
                "https://codexreset.org/" => r#"<div data-testid="probability-ring-24h" data-target-value="24"></div><div data-testid="probability-ring-48h" data-target-value="42"></div>"#.to_string(),
                "https://whenreset.dev/api/reset-feed?provider=codex" => "<rss><channel><item><title>Usage reset</title><pubDate>Tue, 22 Sep 2026 18:23:37 GMT</pubDate></item></channel></rss>".to_string(),
                _ => {
                    let error = Err(HttpError::transport("retired source requested"));
                    println!("{error:?}");
                    return error;
                },
            };
            Ok(HttpResponse {
                status: 200,
                body: body.into_bytes(),
                headers: Default::default(),
            })
        };
        println!("{result:?}");
        result
    }
}

/// Check the catalog and independent forecast and history results.
/// Parameters: result - public snapshot to inspect.
/// Returns: None.
fn assert_retained_snapshot(result: &Value) {
    println!("result={result}");
    let rows = result["sites"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["polymarket", "codex_reset_monitor", "whenreset"]
    );
    assert_eq!(rows[1]["url"], "https://codexreset.org/");
    assert_eq!(result["consensus_percent_24h"], 24.0);
    assert_eq!(result["consensus_percent_48h"], 42.0);
    assert_eq!(result["contributing_sites"], 1);
    assert_eq!(rows[2]["reference"], true);
    assert_eq!(rows[2]["predictions"], json!([]));
    assert!(rows[2]["detail"]
        .as_str()
        .unwrap()
        .contains("latest direct reset"));
    println!("None");
}

#[test]
/// Verify new installations expose precisely the retained sources.
/// Parameters: none.
/// Returns: None.
fn defaults_and_fresh_collection_use_only_retained_sources() {
    println!("parameters=none");
    let config = default_config();
    let sites = &config["codex_reset_forecast"]["sites"];
    assert_eq!(sites.as_object().unwrap().len(), 3);
    assert!(sites.get("codex_reset").is_none());
    assert!(sites.get("will_codex_reset").is_none());
    assert_eq!(sites["polymarket"]["enabled"], true);
    let opener = PublicSources::default();
    let result =
        collect_codex_reset_forecast(None, Some(1000.0), None, None, Some(sites), Some(&opener));
    assert_retained_snapshot(&result);
    assert_eq!(opener.calls.lock().unwrap().len(), 3);
    println!("None");
}

#[test]
/// Verify cached retired forecasts cannot affect fresh or throttled snapshots.
/// Parameters: none.
/// Returns: None.
fn legacy_enabled_sources_and_cached_rows_are_removed() {
    println!("parameters=none");
    let opener = PublicSources::default();
    let mut first =
        collect_codex_reset_forecast(None, Some(1000.0), None, None, None, Some(&opener));
    first["sites"].as_array_mut().unwrap().extend([
        json!({"id":"codex_reset", "name":"codex-reset.com", "checked_at":1000.0, "predictions":[{"label":"24h","percent":99.0},{"label":"48h","percent":99.0}], "commitment":{"percent":99.0,"deadline_at":2000.0}}),
        json!({"id":"will_codex_reset", "name":"Will Codex Reset", "checked_at":1000.0, "predictions":[]})
    ]);
    let legacy = json!({"codex_reset":{"enabled":true},"will_codex_reset":{"enabled":true},"polymarket":{"enabled":true}});
    opener.calls.lock().unwrap().clear();
    for now in [1005.0, 1060.0, 1400.0] {
        let next = collect_codex_reset_forecast(
            Some(&first),
            Some(now),
            None,
            None,
            Some(&legacy),
            Some(&opener),
        );
        assert_retained_snapshot(&next);
        let calls = opener.calls.lock().unwrap();
        assert!(calls.iter().all(|url| !url.contains("codex-reset.com")
            && !url.contains("willcodexreset.org")));
        drop(calls);
        first = next;
    }
    println!("None");
}

#[test]
/// Verify rejected network reads preserve retained forecast and history data.
/// Parameters: none.
/// Returns: None.
fn offline_reads_keep_retained_values_and_history() {
    println!("parameters=none");
    let good = collect_codex_reset_forecast(
        None,
        Some(1000.0),
        None,
        None,
        None,
        Some(&PublicSources::default()),
    );
    let opener = PublicSources {
        offline: true,
        ..Default::default()
    };
    let failed =
        collect_codex_reset_forecast(Some(&good), Some(1400.0), None, None, None, Some(&opener));
    assert_retained_snapshot(&failed);
    for site in failed["sites"].as_array().unwrap() {
        assert_eq!(site["error"], "offline");
    }
    assert!(failed["sites"][2]["detail"]
        .as_str()
        .unwrap()
        .ends_with(" · last known"));
    let throttled =
        collect_codex_reset_forecast(Some(&failed), Some(1405.0), None, None, None, Some(&opener));
    assert_retained_snapshot(&throttled);
    assert_eq!(opener.calls.lock().unwrap().len(), 3);
    println!("None");
}

#[test]
/// Verify user opt-outs survive and enabling retired sources makes no requests.
/// Parameters: none.
/// Returns: None.
fn retained_opt_outs_cannot_be_overridden_by_legacy_sources() {
    println!("parameters=none");
    let config = json!({"codex_reset_monitor":{"enabled":false},"whenreset":{"enabled":false},"codex_reset":{"enabled":true},"will_codex_reset":{"enabled":true},"polymarket":{"enabled":false}});
    let opener = PublicSources::default();
    let result =
        collect_codex_reset_forecast(None, Some(1000.0), None, None, Some(&config), Some(&opener));
    assert_eq!(result["sites"], json!([]));
    assert_eq!(result["consensus_percent"], Value::Null);
    assert_eq!(result["error"], "no forecast site enabled");
    assert!(opener.calls.lock().unwrap().is_empty());
    println!("None");
}

#[test]
/// Verify persisted legacy settings migrate while retaining current opt-outs.
/// Parameters: none.
/// Returns: None.
fn loading_legacy_config_drops_retired_source_settings() {
    println!("parameters=none");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, json!({"codex_reset_forecast":{"sites":{"codex_reset":{"enabled":true},"will_codex_reset":{"enabled":true},"polymarket":{"enabled":true},"codex_reset_monitor":{"enabled":false},"whenreset":{"enabled":true}}}}).to_string()).unwrap();
    let config = load_config(Some(&path));
    let sites = &config["codex_reset_forecast"]["sites"];
    assert_eq!(sites.as_object().unwrap().len(), 3);
    assert!(sites.get("codex_reset").is_none());
    assert!(sites.get("will_codex_reset").is_none());
    assert_eq!(sites["polymarket"]["enabled"], true);
    assert_eq!(sites["codex_reset_monitor"]["enabled"], false);
    assert_eq!(sites["whenreset"]["enabled"], true);
    println!("None");
}
