use std::path::PathBuf;

#[test]
fn cold_start_probe() {
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
    for inst in snap["instances"].as_array().unwrap() {
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
}
