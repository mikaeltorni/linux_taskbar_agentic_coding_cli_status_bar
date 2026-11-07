//! Verify the public reset forecast discovers Polymarket's active event.
use agentic_cli_status::collectors::codex_reset_forecast::collect_codex_reset_forecast;
use agentic_cli_status::config::default_config;
use agentic_cli_status::http::{HttpError, HttpOpener, HttpRequest, HttpResponse};
use serde_json::json;

#[derive(Debug)]
struct Gamma;

impl HttpOpener for Gamma {
    /// Return an independent active market fixture.
    /// Parameters: self - fake source; request - public endpoint request.
    /// Returns: a Gamma response or transport error.
    fn open(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        println!("self={self:?} request={request:?}");
        let result = if request.url.contains("gamma-api.polymarket.com") {
            Ok(HttpResponse { status: 200, body: json!({"events":[{"title":"How many Codex resets week of November 2?", "slug":"how-many-codex-resets-week-of-november-2", "active":true, "closed":false, "markets":[{"active":true,"closed":false,"groupItemTitle":"0","endDate":"2030-11-09T04:59:00Z","outcomes":"[\"Yes\",\"No\"]","outcomePrices":"[\"0.24\",\"0.76\"]"}, {"active":true,"closed":false,"groupItemTitle":"1","endDate":"2030-11-09T04:59:00Z","outcomes":"[\"Yes\",\"No\"]","outcomePrices":"[\"0.45\",\"0.55\"]"}]}]}).to_string().into_bytes(), headers: Default::default() })
        } else { Err(HttpError::transport("unexpected endpoint")) };
        println!("{result:?}");
        result
    }
}

/// Include Polymarket's priced open market without changing consensus.
/// Parameters: none.
/// Returns: None.
#[test]
fn active_polymarket_event_is_discovered() {
    println!("parameters=none");
    let config = default_config();
    assert_eq!(config["codex_reset_forecast"]["sites"]["polymarket"]["enabled"], true);
    let sites = json!({"codex_reset_monitor":{"enabled":false},"whenreset":{"enabled":false},"polymarket":{"enabled":true}});
    let data = collect_codex_reset_forecast(None, Some(1794000000.0), None, None, Some(&sites), Some(&Gamma));
    let row = data["sites"].as_array().unwrap().iter().find(|row| row["id"] == "polymarket").expect("Polymarket source");
    assert_eq!(row["predictions"][0]["percent"], 24.0);
    assert_eq!(row["predictions"][0]["label"], "0");
    assert_eq!(row["predictions"][1]["label"], "1");
    assert_eq!(row["url"], "https://polymarket.com/event/how-many-codex-resets-week-of-november-2");
    assert!(data["consensus_percent_24h"].is_null());
    let expired = collect_codex_reset_forecast(Some(&data), Some(2_000_000_000.0), None, None, Some(&sites), Some(&Gamma));
    assert!(expired["sites"][0]["predictions"].as_array().unwrap().is_empty());
    println!("None");
}
