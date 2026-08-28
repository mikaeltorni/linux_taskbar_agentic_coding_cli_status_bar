//! Extract GNOME extension helpers and run them under `gjs`.
//!
//! `extension.js` files import `gi://…`, so they cannot be loaded by a test
//! process. Pure helpers can still be lifted out with brace counting and
//! executed under gjs, matching the former Python `js_probe.py`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn extract_function(source: &str, name: &str) -> String {
    let needle = format!("function {name}(");
    let start = source
        .find(&format!("\n{needle}"))
        .map(|i| i + 1)
        .or_else(|| source.starts_with(&needle).then_some(0))
        .unwrap_or_else(|| panic!("function {name} not found"));
    let mut depth = 0i32;
    for (offset, ch) in source[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=start + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces extracting {name}");
}

fn extract_constant(source: &str, name: &str) -> String {
    for line in source.lines() {
        if line.starts_with(&format!("const {name} = ")) && line.ends_with(';') {
            return line.to_string();
        }
    }
    panic!("const {name} not found");
}

fn run_gjs(snippets: &[String], probe: &str) -> Value {
    let gjs = which("gjs").unwrap_or_else(|| {
        panic!("skip: gjs not installed");
    });
    let script = format!("{}\n{probe}", snippets.join("\n"));
    let output = Command::new(gjs)
        .args(["-c", &script])
        .env("TZ", "UTC")
        .output()
        .expect("spawn gjs");
    assert!(
        output.status.success(),
        "gjs failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("gjs JSON")
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let candidate = dir.join(name);
            candidate.is_file().then_some(candidate)
        })
    })
}

fn gjs_available() -> bool {
    which("gjs").is_some()
}

fn lerp_color(left: f64) -> String {
    let full = (0x8F, 0xF0, 0xA4);
    let mid = (0xF8, 0xE4, 0x5C);
    let low = (0xFF, 0x7B, 0x63);
    let midpoint = (1.0 + 100.0) / 2.0;
    let (start, end, t) = if left <= midpoint {
        (low, mid, (left - 1.0) / (midpoint - 1.0))
    } else {
        (mid, full, (left - midpoint) / (100.0 - midpoint))
    };
    let channels = [
        (start.0 as f64 + (end.0 as f64 - start.0 as f64) * t).round() as i32,
        (start.1 as f64 + (end.1 as f64 - start.1 as f64) * t).round() as i32,
        (start.2 as f64 + (end.2 as f64 - start.2 as f64) * t).round() as i32,
    ];
    format!("#{:02x}{:02x}{:02x}", channels[0], channels[1], channels[2])
}

fn agentic_results() -> Option<Value> {
    if !gjs_available() {
        eprintln!("gjs not installed; skipping");
        return None;
    }
    let source = fs::read_to_string(
        repo_root()
            .join("agentic-cli-status@local")
            .join("extension.js"),
    )
    .unwrap();
    let constants = [
        "GRADIENT_FULL_RGB",
        "GRADIENT_MID_RGB",
        "GRADIENT_LOW_RGB",
        "EMPTY_COLOR",
        "NEUTRAL_COLOR",
    ];
    let functions = [
        "resetPassed",
        "percentLeft",
        "percentColor",
        "resetClock",
        "resetDate",
        "resetCountdown",
        "resetsText",
        "resetAvailabilityText",
        "segmentMarkup",
        "codexBarMarkup",
        "codexInstanceTitle",
    ];
    let mut snippets: Vec<String> = constants
        .iter()
        .map(|name| extract_constant(&source, name))
        .collect();
    snippets.extend(functions.iter().map(|name| extract_function(&source, name)));
    let probe = include_str!("extension_color_probe.js");
    Some(run_gjs(&snippets, probe))
}

#[test]
fn gradient_and_segments() {
    let Some(js) = agentic_results() else {
        return;
    };
    assert_eq!(js["color_100"], "#8ff0a4");
    assert_eq!(js["color_1"], "#ff7b63");
    assert_eq!(js["color_0"], "#ff0000");
    assert_eq!(js["color_99"], lerp_color(99.0));
    assert_eq!(js["color_50"], lerp_color(50.0));
    assert_eq!(js["color_2"], lerp_color(2.0));
    assert_ne!(js["color_99"], js["color_100"]);
    assert_ne!(js["color_2"], js["color_1"]);
    assert_eq!(js["left_clamped"], 0);
    assert_eq!(js["dash"], "<span foreground=\"#9a9996\">CX –</span>");
    assert_eq!(
        js["errored_stale_segment"],
        "<span foreground=\"#9a9996\">CC –?</span>"
    );
    let segment = js["segment"].as_str().unwrap();
    assert!(segment.starts_with("<span foreground=\"#8ff0a4\">CC "));
    assert!(segment.contains(">100<") && segment.contains("#8ff0a4"));
    assert!(segment.contains("<span foreground=\"#ff7b63\">1</span>"));
    assert!(segment.contains("<span foreground=\"#ff0000\">0</span>"));
    assert!(segment.contains("<span foreground=\"#9a9996\">·</span>"));
    assert!(!segment.contains('%'));
    let expected_week = concat!(
        "<span foreground=\"#ff0000\">GK </span>",
        "<span foreground=\"#ff0000\">Thu 17:10</span>"
    );
    assert_eq!(js["weekly_exhausted_grok"], expected_week);
    assert_eq!(
        js["weekly_exhausted_codex"].as_str().unwrap(),
        expected_week.replace("GK", "CX")
    );
    assert!(!js["weekly_exhausted_codex"]
        .as_str()
        .unwrap()
        .contains("5h"));
    let red_countdown = "<span foreground=\"#ff0000\">4h59m</span>";
    let separator = "<span foreground=\"#9a9996\">-</span>";
    let weekly = "<span foreground=\"#f9d05d\">41</span>";
    assert_eq!(
        js["five_hour_exhausted_codex"].as_str().unwrap(),
        format!("<span foreground=\"#ff0000\">CX </span>{red_countdown}{separator}{weekly}")
    );
    assert_eq!(
        js["five_hour_exhausted_claude"].as_str().unwrap(),
        format!("<span foreground=\"#ff0000\">CC </span>{red_countdown}{separator}{weekly}{separator}{weekly}")
    );
    assert_eq!(js["countdown_59m"], "59m");
    assert_eq!(js["countdown_60m"], "1h00m");
    assert_eq!(js["reset_passed_left_full"], 100);
    assert_eq!(js["reset_future_left_exhausted"], 0);
    assert_eq!(
        js["boot_stale_five_hour_replenished"].as_str().unwrap(),
        concat!(
            "<span foreground=\"#8ff0a4\">CX </span>",
            "<span foreground=\"#8ff0a4\">100</span>",
            "<span foreground=\"#9a9996\">·</span>",
            "<span foreground=\"#fd9f61\">18</span>"
        )
    );
    let weekly_seg = js["boot_stale_weekly_replenished"].as_str().unwrap();
    assert!(weekly_seg.contains("<span foreground=\"#8ff0a4\">100</span>"));
    assert!(weekly_seg.contains(">60<"));
    assert!(!weekly_seg.contains("#ff0000"));
    let grok_reset = js["grok_details_reset"].as_str().unwrap();
    assert!(grok_reset.starts_with(" — resets "));
    assert_eq!(js["rounded_reset_clock"], "Sun 14:00");
    assert_eq!(js["rounded_details_reset"], " — resets Sun 14:00");
    assert_eq!(js["one_reset_available"], " — 1 reset available");
    assert_eq!(js["multiple_resets_available"], " — 2 resets available");
    assert_eq!(js["missing_resets_available"], "");
    assert_eq!(js["zero_resets_available"], "");
    let two = js["two_codex_accounts"].as_str().unwrap();
    assert!(two.contains(">CX <") || two.contains("CX "));
    assert!(two.contains(" | "));
    assert!(two.contains(">2 <") || two.contains("2 "));
    assert_eq!(js["second_codex_title"], "Codex [2] trial");
    assert_eq!(js["primary_codex_title"], "Codex [1]");
    let or_seg = js["openrouter_segment"].as_str().unwrap();
    assert!(or_seg.contains(">58<"));
    let og = js["opencode_go_segment"].as_str().unwrap();
    assert!(og.starts_with("<span foreground=\"#8ff0a4\">OG "));
    assert!(og.contains("<span foreground=\"#8ff0a4\">100</span>"));
    let month = js["opencode_go_month_exhausted"].as_str().unwrap();
    assert!(month.contains("<span foreground=\"#ff0000\">3 Aug</span>"));
    assert_eq!(js["monthly_reset_date"], "3 Aug");
    assert_eq!(js["monthly_details_reset"], " — resets 3 Aug");
    let source = fs::read_to_string(
        repo_root()
            .join("agentic-cli-status@local")
            .join("extension.js"),
    )
    .unwrap();
    let og_index = source.find("['opencode_go', 'OG']").unwrap();
    let or_index = source.find("['openrouter', 'OR']").unwrap();
    assert!(og_index < or_index);
}

#[test]
fn workspace_attention_badges() {
    if !gjs_available() {
        eprintln!("gjs not installed; skipping");
        return;
    }
    let source = fs::read_to_string(
        repo_root()
            .join("acc-model-status@local")
            .join("extension.js"),
    )
    .unwrap();
    let payload = r#"
const PAYLOAD = {providers: [
  {kind: 'claude', agents: [
    {phase: 'RUNNING', workspace: 5, current_workspace: true},
    {phase: 'ATTN', workspace: 4, current_workspace: false}]},
  {kind: 'codex', agents: [
    {phase: 'DRAFT', workspace: 4, current_workspace: false},
    {phase: 'DONE', workspace: 2, current_workspace: false},
    {phase: 'done', workspace: 2, current_workspace: false},
    {phase: 'INTERRUPTED', workspace: 5, current_workspace: true},
    {phase: 'DONE', workspace: 0, current_workspace: false}]},
  {kind: 'grok', agents: [
    {phase: 'STOPPED', workspace: 5, current_workspace: true}]},
]};
"#;
    let snippets = vec![
        extract_constant(&source, "NEEDS_USER_PHASES"),
        extract_function(&source, "workspaceAttentionCounts"),
        payload.to_string(),
    ];
    let js = run_gjs(
        &snippets,
        "print(JSON.stringify({entries: workspaceAttentionCounts(PAYLOAD)}));",
    );
    let labels: Vec<_> = js["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["W2:2", "W4:1", "W5:2"]);
    assert!(source.contains("this._renderWorkspaceAttention("));
    assert!(source.contains("FLASH_INTERVAL_MS"));
    let css = fs::read_to_string(
        repo_root()
            .join("acc-model-status@local")
            .join("stylesheet.css"),
    )
    .unwrap();
    for style in ["ams-ws-attn", "ams-ws-attn-dim", "ams-ws-attn-current"] {
        assert!(css.contains(&format!(".{style} {{")), "{style}");
    }
}

fn code_only(text: &str) -> String {
    let no_block = regex::Regex::new(r"(?s)/\*.*?\*/")
        .unwrap()
        .replace_all(text, "");
    regex::Regex::new(r"(?m)^\s*//.*$")
        .unwrap()
        .replace_all(&no_block, "")
        .into_owned()
}

#[test]
fn model_status_extension_contract() {
    let root = repo_root();
    let ext = root.join("acc-model-status@local");
    let js = fs::read_to_string(ext.join("extension.js")).unwrap();
    let code = code_only(&js);
    assert!(code.contains("model_status_bar.json"));
    assert!(code.contains("CODEX_AGENT_TRACKER_STATE_DIR"));
    for forbidden in ["GLib.spawn", "Gio.Subprocess", "Soup", "subprocess"] {
        assert!(!code.contains(forbidden), "{forbidden}");
    }
    let metadata: Value =
        serde_json::from_str(&fs::read_to_string(ext.join("metadata.json")).unwrap()).unwrap();
    assert_eq!(metadata["uuid"], "acc-model-status@local");
    let install = fs::read_to_string(root.join("install.sh")).unwrap();
    assert!(install.contains("MODEL_STATUS_UUID=\"acc-model-status@local\""));
    let setup = fs::read_to_string(root.join("lib").join("status_setup.sh")).unwrap();
    assert!(setup.contains("ACS_MODEL_STATUS_FILES"));
    let components = fs::read_to_string(root.join("installer").join("components.sh")).unwrap();
    assert!(components.contains("model_status_bar|"));
    assert!(components.contains("|off|acs_install_model_status_bar"));
}

#[test]
fn systemd_unit_starts_the_rust_binary() {
    let unit = fs::read_to_string(
        repo_root()
            .join("systemd")
            .join("agentic-cli-status.service"),
    )
    .unwrap();
    assert!(unit.contains("agentic-cli-status --daemon"));
    assert!(unit.contains("EnvironmentFile=-%h/.config/agentic-cli-status/.env"));
    assert!(unit.contains("WantedBy=default.target"));
    assert!(unit.contains("StandardError=journal"));
    assert!(!unit.contains("python3"));
    assert!(!unit.contains("PYTHONPATH"));
}

#[test]
fn installer_delegates_readonly_routing() {
    let text = fs::read_to_string(repo_root().join("install.sh")).unwrap();
    assert!(text.contains("isc_route_readonly_args"));
    assert!(!text.contains("--list-components|--export-selection|--detect"));
}
