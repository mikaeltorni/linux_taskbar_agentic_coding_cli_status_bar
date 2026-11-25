use std::path::PathBuf;

#[test]
/// Check cold-start collection with or without an ACC account registry.
/// Parameters: none.
/// Returns: () after validating the selected instance.
fn cold_start_probe() {
    println!("parameters=none");
    let home = PathBuf::from(std::env::var("HOME").unwrap());
    let now: f64 = std::env::var("PROBE_NOW")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
        });
    let snap =
        agentic_cli_status::collectors::codex::collect_codex_accounts(&home, Some(now), None);
    println!("COLDSTART now={now} selected={}", snap["selected"]);
    if let Some(instances) = snap["instances"].as_array() {
        assert!(instances
            .iter()
            .any(|instance| instance["id"] == snap["selected"]));
        for inst in instances {
            println!(
                "  id={} home={} source={} plan={} obs={} error={} windows={}",
                inst["id"],
                inst["home"],
                inst["source"],
                inst["plan"],
                inst["observed_at"],
                inst["error"],
                inst["windows"]
            );
        }
    } else {
        assert_eq!(snap["selected"], 1);
    }
    println!("()");
}
