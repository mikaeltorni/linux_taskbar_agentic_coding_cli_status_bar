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

fn extract_method(source: &str, name: &str) -> String {
    let needle = format!("\n    {name}(");
    let start = source
        .find(&needle)
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("method {name} not found"));
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
    panic!("unbalanced braces extracting method {name}");
}

/// Style-class tokens of `segment.add_child(...)` widgets, in source order.
fn segment_add_child_classes(body: &str) -> Vec<String> {
    let mut classes = Vec::new();
    let mut rest = body;
    while let Some(idx) = rest.find("segment.add_child(") {
        rest = &rest[idx + "segment.add_child(".len()..];
        let Some(cls_idx) = rest.find("style_class:") else {
            continue;
        };
        let after = rest[cls_idx + "style_class:".len()..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| matches!(c, '\'' | '"' | '`')) else {
            continue;
        };
        let inner = &after[1..];
        let Some(end) = inner.find(quote) else {
            continue;
        };
        let token = inner[..end]
            .split_whitespace()
            .next()
            .unwrap_or(&inner[..end]);
        classes.push(token.to_string());
    }
    classes
}

fn extract_constant(source: &str, name: &str) -> String {
    for line in source.lines() {
        if line.starts_with(&format!("const {name} = ")) && line.ends_with(';') {
            return line.to_string();
        }
    }
    panic!("const {name} not found");
}

fn extract_const_block(source: &str, name: &str) -> String {
    let needle = format!("const {name} = ");
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("const {name} not found"));
    let rest = &source[start..];
    if !rest.contains('{') {
        return extract_constant(source, name);
    }
    let mut depth = 0i32;
    for (offset, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let mut end = offset + 1;
                    if rest[end..].starts_with(';') {
                        end += 1;
                    }
                    return rest[..end].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces extracting const {name}");
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
        "CLUTTER_MARKUP_PREFIX",
        "INSTANCE_TAG_FIRST_RGB",
        "INSTANCE_TAG_LAST_RGB",
    ];
    let functions = [
        "resetPassed",
        "percentLeft",
        "percentColor",
        "instanceTagColor",
        "resetClock",
        "resetDate",
        "resetCountdown",
        "resetsText",
        "resetAvailabilityText",
        "creditBlocked",
        "zeroBalance",
        "creditDetailText",
        "segmentMarkup",
        "lunaReserveWindow",
        "codexLunaReserveWindow",
        "lunaReserveMarkup",
        "codexBarMarkup",
        "codexResetForecastMarkup",
        "codexBarWithForecastMarkup",
        "roundedForecastPercent",
        "forecastHorizonPercents",
        "formatForecastConsensusPair",
        "forecastPercentMarkup",
        "forecastMenuMarkup",
        "formatForecastConsensusMarkup",
        "formatForecastPercent",
        "formatForecastVolume",
        "formatPolymarketDate",
        "codexResetForecastMenuEntries",
        "openExternalUrl",
        "cursorBarMarkup",
        "cursorExtraDetailTexts",
        "isOnDemandField",
        "codexInstanceTitle",
        "codexWindowTitle",
        "cliSectionHeading",
        "cliEnabledFromConfig",
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
    assert_eq!(js["missing_forecast_volume"], "");
    assert_eq!(js["invalid_forecast_volume"], "");
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
    assert_eq!(js["zero_resets_available"], " — 0 resets available");
    assert_eq!(
        js["team_instance_heading"],
        "Codex [1] (team) — 2 resets available"
    );
    assert_eq!(
        js["personal_instance_heading"],
        "Codex [2] main (team) — 1 reset available"
    );
    assert_eq!(js["missing_instance_heading"], "Codex [1] (team)");
    assert_eq!(
        js["zero_instance_heading"],
        "Codex [1] (team) — 0 resets available"
    );
    let two = js["two_codex_accounts"].as_str().unwrap();
    assert!(two.contains("CX1"));
    assert!(two.contains("CX2"));
    assert!(two.contains(" | "));
    let luna = js["luna_reserve_on_cx3"].as_str().unwrap();
    assert!(!luna.contains("5h-weekly-luna "));
    assert!(luna.contains(">92<"), "unexpected Luna counter: {luna}");
    assert!(
        luna.find("CX3").unwrap() < luna.find(">92<").unwrap(),
        "Luna counter should follow its CX3 account: {luna}"
    );
    let direct_luna = js["luna_reserve_direct_on_selected_cx3"].as_str().unwrap();
    assert!(
        !direct_luna.contains("5h-weekly-luna ")
            && direct_luna.find("CX3").unwrap() < direct_luna.find(">92<").unwrap(),
        "selected-account Luna counter should follow CX3: {direct_luna}"
    );
    assert_eq!(js["depleted_luna_reserve_markup"], "");
    assert_eq!(js["luna_reserve_window_title"], "Luna Reserve");
    let first = js["first_codex_account"].as_str().unwrap();
    assert!(first.contains("CX1"));
    assert!(first.contains("CX2"));
    assert!(first.contains(" | "));
    let forecast = js["codex_with_forecast"].as_str().unwrap();
    assert!(forecast.contains(">64%</span>"), "{forecast}");
    assert!(forecast.find("64%").unwrap() < forecast.find("CX1").unwrap());
    assert!(forecast.contains("> | </span>"), "{forecast}");
    let stale_forecast = js["codex_with_stale_forecast"].as_str().unwrap();
    assert!(stale_forecast.contains("64%"), "{stale_forecast}");
    assert!(stale_forecast.contains(">?</span>"), "{stale_forecast}");
    assert_eq!(js["codex_without_forecast"], js["codex_without_consensus"]);
    assert!(!js["codex_without_forecast"].as_str().unwrap().contains('%'));
    let pango = regex::Regex::new(r"<[^>]+>").unwrap();
    let forecast_24_48 = js["codex_with_24_48_forecast"].as_str().unwrap();
    let forecast_24_48_plain = pango.replace_all(forecast_24_48, "");
    assert!(
        forecast_24_48_plain.contains("26%-48%"),
        "compact 24-48h consensus must be visible as 26%-48%: {forecast_24_48}"
    );
    assert!(
        forecast_24_48.find("26%").unwrap() < forecast_24_48.find("48%").unwrap(),
        "24h consensus must precede 48h: {forecast_24_48}"
    );
    let only_48 = js["codex_with_only_48h_forecast"].as_str().unwrap();
    let only_48_plain = pango.replace_all(only_48, "");
    assert!(
        only_48_plain.contains("48%"),
        "48h-only compact forecast missing: {only_48}"
    );
    assert!(
        !only_48_plain.contains("26%-") && !only_48_plain.contains("%-48%"),
        "must not invent a missing 24h side: {only_48}"
    );
    assert_eq!(
        js["forecast_menu_24_48"][0]["text"],
        "Codex reset forecast — 24-48h consensus: 26%-48%"
    );
    assert_eq!(
        js["forecast_menu_only_48h"][0]["text"],
        "Codex reset forecast — 48h consensus: 48%"
    );
    let color_100 = js["color_100"].as_str().unwrap();
    let color_0 = js["color_0"].as_str().unwrap();
    let color_26 = js["color_26"].as_str().unwrap();
    let color_48 = js["color_48"].as_str().unwrap();
    let color_50 = js["color_50"].as_str().unwrap();
    assert_eq!(color_100, "#8ff0a4");
    assert_eq!(color_0, "#ff0000");
    let forecast_menu_markup = js["forecast_menu_markup"].as_str().unwrap();
    assert!(
        forecast_menu_markup.contains("Codex &amp; reset &lt;"),
        "menu text must be escaped before adding Pango spans: {forecast_menu_markup}"
    );
    assert!(
        forecast_menu_markup.contains(&format!(r#"<span foreground="{color_0}">0%</span>"#)),
        "0% reset forecast must use the red endpoint: {forecast_menu_markup}"
    );
    assert!(
        forecast_menu_markup.contains(&format!(r#"<span foreground="{color_50}">50%</span>"#)),
        "midpoint reset forecast must use percentColor: {forecast_menu_markup}"
    );
    assert!(
        forecast_menu_markup.contains(&format!(r#"<span foreground="{color_100}">100%</span>"#)),
        "100% reset forecast must use the green endpoint: {forecast_menu_markup}"
    );
    let forecast_100_0 = js["forecast_color_100_0"].as_str().unwrap();
    assert!(
        forecast_100_0.contains(&format!(r#"<span foreground="{color_100}">100%</span>"#)),
        "100% must use percentColor(100)={color_100}: {forecast_100_0}"
    );
    assert!(
        forecast_100_0.contains(&format!(r#"<span foreground="{color_0}">0%</span>"#)),
        "0% must use percentColor(0)={color_0}: {forecast_100_0}"
    );
    assert!(
        forecast_100_0.contains("<span foreground=\"#9a9996\">-</span>"),
        "hyphen must use NEUTRAL_COLOR: {forecast_100_0}"
    );
    let forecast_0_100 = js["forecast_color_0_100"].as_str().unwrap();
    assert!(
        forecast_0_100.contains(&format!(r#"<span foreground="{color_0}">0%</span>"#)),
        "24h 0% must use percentColor(0): {forecast_0_100}"
    );
    assert!(
        forecast_0_100.contains(&format!(r#"<span foreground="{color_100}">100%</span>"#)),
        "48h 100% must use percentColor(100): {forecast_0_100}"
    );
    assert!(
        forecast_24_48.contains(&format!(r#"<span foreground="{color_26}">26%</span>"#)),
        "24h 26% must use percentColor(26)={color_26}: {forecast_24_48}"
    );
    assert!(
        forecast_24_48.contains(&format!(r#"<span foreground="{color_48}">48%</span>"#)),
        "48h 48% must use percentColor(48)={color_48}: {forecast_24_48}"
    );
    // ClutterText drops `foreground` on the first glyph (mutter#1324). A
    // wrapping span is not enough; 24h must not start the string.
    let prefix = js["clutter_markup_prefix"].as_str().unwrap();
    assert_eq!(prefix, "\u{200A}");
    assert!(
        forecast_24_48.starts_with(prefix),
        "24h percentColor needs a glyph before the first colored span: {forecast_24_48:?}"
    );
    assert!(
        !forecast_24_48.starts_with("<span foreground="),
        "24h percentColor is ignored when it is the leading span: {forecast_24_48}"
    );
    assert!(
        forecast_24_48.contains(&format!(
            "{prefix}<span><span foreground=\"{color_26}\">26%</span>"
        )),
        "24h digit must follow the prefix so Clutter applies percentColor: {forecast_24_48:?}"
    );
    assert!(
        forecast_100_0.starts_with(prefix) && !forecast_100_0.starts_with("<span foreground="),
        "forecast markup itself must not start with a foreground span: {forecast_100_0:?}"
    );
    assert_eq!(
        js["forecast_menu_entries"],
        serde_json::json!([
            {
                "kind": "heading",
                "name": "Codex reset forecast",
                "text": "Codex reset forecast — 24h consensus: 26%",
                "url": null,
            },
            {
                "kind": "site",
                "name": "Polymarket",
                "text": "Polymarket — 23.9: 24% $1,235 · 24.9: 15% $0",
                "url": "https://polymarket.com/event/openai-resets-codex-weekly-limit-onptptpt",
            },
            {
                "kind": "site",
                "name": "codex-reset.com",
                "text": "codex-reset.com — 24h: 24% · 48h: 42%",
                "url": "https://codex-reset.com/",
            },
            {
                "kind": "site",
                "name": "Will Codex Reset",
                "text": "Will Codex Reset — 24h 20% · 48h 35%",
                "url": "https://willcodexreset.org/",
            },
            {
                "kind": "site",
                "name": "codexreset.org",
                "text": "codexreset.org — unavailable",
                "url": "https://codexreset.org/",
            },
            {
                "kind": "site",
                "name": "WhenReset",
                "text": "WhenReset — latest banked reset · Sep 22 18:23 UTC",
                "url": "https://whenreset.dev/codex",
            },
        ])
    );
    assert_eq!(js["disabled_forecast_menu_entries"], serde_json::json!([]));
    assert_eq!(
        js["forecast_menu_weekly_questions"][1]["text"],
        "Polymarket — unavailable"
    );
    assert_eq!(
        js["forecast_menu_count_market"][1]["text"],
        "Polymarket — unavailable"
    );
    assert_eq!(
        js["forecast_menu_no_open_market"][1]["text"],
        "Polymarket — no open market"
    );
    assert_eq!(
        js["forecast_menu_stale_market"][1]["text"],
        "Polymarket — 23.9: 57% · last known"
    );
    assert_eq!(js["valid_forecast_launch"], true);
    assert_eq!(
        js["reference_forecast_launches"],
        serde_json::json!([true, true])
    );
    assert_eq!(js["rejected_forecast_launch"], false);
    assert_eq!(
        js["forecast_launch_calls"],
        serde_json::json!([
            [
                "launch",
                "https://polymarket.com/event/openai-resets-codex-weekly-limit-onptptpt",
                null,
                null
            ],
            [
                "finish",
                "https://polymarket.com/event/openai-resets-codex-weekly-limit-onptptpt"
            ],
            ["launch", "https://whenreset.dev/codex", null, null],
            ["finish", "https://whenreset.dev/codex"],
            ["launch", "https://willcodexreset.org/", null, null],
            ["finish", "https://willcodexreset.org/"],
        ])
    );
    assert!(js["forecast_launch_logs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry[0] == "warn" && entry[1].as_str().unwrap().contains("Unsafe")));
    assert_eq!(js["instance_tag_first"], "#7ec8ff");
    assert_eq!(js["instance_tag_last"], "#a856e8");
    assert_eq!(js["instance_tag_only"], "#7ec8ff");
    assert_eq!(js["instance_tag_mid"], "#938ff4");
    assert!(two.contains("#7ec8ff"));
    assert!(two.contains("#a856e8"));
    assert_eq!(js["second_codex_title"], "Codex [2] trial *");
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
    let percent_markup = extract_function(&source, "forecastPercentMarkup");
    assert!(
        percent_markup.contains("percentColor("),
        "forecast digits must reuse percentColor, not a second mapper: {percent_markup}"
    );
    assert!(
        !percent_markup.contains("forecastColor") && !percent_markup.contains("GRADIENT_"),
        "forecast digits must not define a forecast-specific gradient: {percent_markup}"
    );
    let render_start = source.find("    render(status) {").unwrap();
    let render_source = &source[render_start..];
    let forecast_menu_index = render_source
        .find("this._appendForecastSection(status.codex_reset_forecast)")
        .unwrap();
    let cli_menu_index = render_source.find("const names = {").unwrap();
    assert!(forecast_menu_index < cli_menu_index);
    assert!(source.contains("forecastMenuMarkup(entry.text)"));
    assert!(source.contains("forecastMenuMarkup(`↗ ${entry.text}`)"));
    assert!(source.contains("item.connect('activate', () =>"));
    assert!(source.contains("Gio.AppInfo.launch_default_for_uri_async"));
    assert!(source.contains("item.label.add_style_class_name('acs-forecast-site-label')"));
    let stylesheet = fs::read_to_string(
        repo_root()
            .join("agentic-cli-status@local")
            .join("stylesheet.css"),
    )
    .unwrap();
    assert!(stylesheet.contains(".acs-forecast-site-label"));
    assert!(stylesheet.contains("max-width: 34em"));
    assert!(source.contains("item.label.clutter_text.set_ellipsize(Pango.EllipsizeMode.END)"));
    let og_index = source.find("['opencode_go', 'OG']").unwrap();
    let or_index = source.find("['openrouter', 'OR']").unwrap();
    let crs_index = source.find("['cursor', 'CRS']").unwrap();
    assert!(og_index < or_index);
    assert!(or_index < crs_index);
    let crs = js["cursor_segment"].as_str().unwrap();
    assert!(crs.contains(">CRS <") || crs.contains("CRS "), "{crs}");
    assert!(crs.contains("CRS"), "{crs}");
    assert!(
        crs.contains(">77<"),
        "cursor models remainder missing: {crs}"
    );
    assert!(
        crs.contains(">55<"),
        "other models remainder missing: {crs}"
    );
    assert!(crs.contains(">88<"), "grok bot remainder missing: {crs}");
    let crs_plain = regex::Regex::new(r"<[^>]+>").unwrap().replace_all(crs, "");
    assert!(
        !crs_plain.contains("Oct")
            && !crs_plain.contains("14 Oct")
            && !crs_plain.contains("12 Oct"),
        "compact CRS must not append usage-limit reset date: {crs}"
    );
    let crs_lower = crs.to_ascii_lowercase();
    assert!(!crs_lower.contains("ondemand"), "{crs}");
    assert!(!crs_lower.contains("on-demand"), "{crs}");
    assert!(!crs.contains("$999"), "{crs}");
    assert!(!crs.contains("99.9"), "{crs}");
    let hidden = js["cursor_on_demand_absent"].as_str().unwrap();
    let hidden_lower = hidden.to_ascii_lowercase();
    assert!(hidden.contains("CRS"), "{hidden}");
    assert!(!hidden_lower.contains("ondemand"), "{hidden}");
    assert!(!hidden_lower.contains("99.9"), "{hidden}");
    let extras = js["cursor_submenu_extras"].as_array().unwrap();
    let extra_blob = extras
        .iter()
        .map(|v| v.as_str().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    assert!(
        extra_blob.contains("cursor_models remaining"),
        "included extras should still list remaining: {extras:?}"
    );
    assert!(
        !extra_blob.contains("usage limit reset")
            && !extra_blob.contains("14 oct")
            && !extra_blob.contains("12 oct"),
        "CRS submenu must not include usage-limit reset: {extras:?}"
    );
    assert!(!extra_blob.contains("ondemand"), "{extras:?}");
    assert!(!source.contains("usage limit reset${"));
}

#[test]
fn credit_state_is_visible_on_the_bar_and_in_the_menu() {
    let Some(js) = agentic_results() else {
        return;
    };
    assert_eq!(js["credit_block_team"], "credits: team credits out");
    assert_eq!(js["credit_block_generic"], "credits: credits out");
    // An account with no credit facility reports a permanent zero balance;
    // that is noise, not information, so the menu stays quiet about it.
    assert_eq!(js["credit_balance_only"], "");
    assert_eq!(js["credit_zero_formatted"], "");
    assert_eq!(js["credit_zero_with_facility"], "credits: 0 left");
    assert_eq!(js["credit_nonzero_without_facility"], "credits: 12.50 left");
    assert_eq!(js["credit_unlimited"], "credits: unlimited");
    assert_eq!(js["credit_absent"], "");
    assert_eq!(js["credit_blocked_true"], true);
    assert_eq!(js["credit_blocked_false"], false);
    // The block marker rides along even though both windows have budget left,
    // because a window reset will not lift a credit block.
    let marked = js["credit_marked_segment"].as_str().unwrap();
    assert!(
        marked.ends_with("<span foreground=\"#ff0000\">$</span>"),
        "credit block not marked: {marked}"
    );
    let unmarked = js["credit_unmarked_segment"].as_str().unwrap();
    assert!(
        !unmarked.contains('$'),
        "unexpected credit mark: {unmarked}"
    );
}

#[test]
fn model_status_entries_sort_workspaces_and_monitor_rows() {
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
    let snippets = vec![
        extract_function(&source, "monitorSortValue"),
        extract_function(&source, "workspaceSortValue"),
        extract_function(&source, "compareWorkspaceEntries"),
        extract_function(&source, "groupWorkspaceEntries"),
    ];
    let js = run_gjs(
        &snippets,
        r#"
const entries = [
  {provider: {label: 'Codex'}, agent: {title: 'monitor 2 workspace 1', monitor: 1, workspace: 1}},
  {provider: {label: 'Claude'}, agent: {title: 'monitor 1 workspace 2', monitor: 0, workspace: 2}},
  {provider: {label: 'Codex'}, agent: {title: 'monitor 1 workspace 1', monitor: 0, workspace: 1}},
  {provider: {label: 'Codex'}, agent: {title: 'monitor 3 workspace 2', monitor: 2, workspace: 2}},
  {provider: {label: 'Codex'}, agent: {title: 'monitor 2 workspace 3', monitor: 1, workspace: 3}},
  {provider: {label: 'Codex'}, agent: {title: 'monitor 1 workspace 4', monitor: 0, workspace: 4}},
  {provider: {label: 'Codex'}, agent: {title: 'unknown monitor', monitor: -1, workspace: 1}},
];
const groups = groupWorkspaceEntries(entries);
print(JSON.stringify({
  workspaces: groups.map(group => group.workspace),
  titles: groups.map(group => group.entries.map(entry => entry.agent.title)),
}));
"#,
    );
    assert_eq!(js["workspaces"], serde_json::json!([1, 2, 3, 4]));
    assert_eq!(
        js["titles"],
        serde_json::json!([
            [
                "monitor 1 workspace 1",
                "monitor 2 workspace 1",
                "unknown monitor"
            ],
            ["monitor 1 workspace 2", "monitor 3 workspace 2"],
            ["monitor 2 workspace 3"],
            ["monitor 1 workspace 4"]
        ])
    );
}

#[test]
fn model_status_omits_session_worktree_mark() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    assert!(
        !source.contains("worktree_mark"),
        "agent status taskbar must not render the session worktree mark"
    );
    assert!(
        !source.contains("agentWorktreeMark"),
        "agent status taskbar must not format ·N"
    );
    assert!(
        !source.contains("providerCountText"),
        "panel count must stay the running-agent integer"
    );
}

#[test]
fn panel_running_count_is_left_of_icon() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let empty = extract_method(&source, "_renderEmpty");
    let provider = extract_method(&source, "_renderProvider");
    let orphans = extract_method(&source, "_renderPanelHarnessRates");
    assert_eq!(
        segment_add_child_classes(&empty),
        ["ams-count", "ams-icon"],
        "empty-state 0 must sit left of the placeholder icon"
    );
    let provider_classes = segment_add_child_classes(&provider);
    assert_eq!(
        &provider_classes[..2],
        ["ams-count", "ams-icon"],
        "running-agent amount must sit left of the provider icon"
    );
    assert_eq!(
        provider_classes.last().map(String::as_str),
        Some("ams-rate"),
        "token-rate chip must stay to the right of the icon"
    );
    assert_eq!(
        segment_add_child_classes(&orphans),
        ["ams-icon", "ams-rate"],
        "orphan rate chips stay icon-then-rate with no running-count"
    );
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
    {phase: 'DONE', unread_completion: true, workspace: 2, current_workspace: false},
    {phase: 'done', unread_completion: false, workspace: 2, current_workspace: false},
    {phase: 'INTERRUPTED', workspace: 5, current_workspace: true},
    {phase: 'DONE', unread_completion: true, workspace: 0, current_workspace: false}]},
  {kind: 'grok', agents: [
    {phase: 'STOPPED', workspace: 5, current_workspace: true},
    {phase: 'DONE', unread_completion: true, workspace: 5, current_workspace: true}]},
]};
"#;
    let snippets = vec![
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
    assert_eq!(labels, ["W2:1", "W5:1"]);
    assert!(source.contains("this._renderWorkspaceAttention("));
    assert!(source.contains("agent.unread_completion = unread"));
    assert!(source.contains("this._box.add_style_class_name('ams-unread-glow')"));
    assert!(!source.contains("ams-ws-attn-dim"));
    assert!(!source.contains("ams-unread-glow-dim"));
    assert!(source.contains("item.actor.add_style_class_name('ams-agent-unread')"));
    let css = fs::read_to_string(
        repo_root()
            .join("acc-model-status@local")
            .join("stylesheet.css"),
    )
    .unwrap();
    for style in ["ams-ws-attn", "ams-ws-attn-current", "ams-agent-unread"] {
        assert!(css.contains(&format!(".{style} {{")), "{style}");
    }
    for phase in 0..4 {
        assert!(css.contains(&format!(".ams-unread-flow-{phase} {{")));
    }
}

#[test]
fn finished_agent_glows_until_its_terminal_is_focused() {
    if !gjs_available() {
        eprintln!("gjs not installed; skipping");
        return;
    }
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let js = run_gjs(
        &[
            extract_function(&source, "unreadCompletionCount"),
            extract_function(&source, "workspaceAttentionCounts"),
        ],
        r#"
const states = new Map();
const agent = {session_id: 'session-1', phase: 'RUNNING', focused: false,
    workspace: 3, current_workspace: false,
    last_activity_at: '2026-09-27T10:00:00Z'};
const payload = {providers: [{agents: [agent]}]};
const count = () => unreadCompletionCount(payload, states);
const running = count();
agent.phase = 'DONE';
const finished = count();
const marked = workspaceAttentionCounts(payload).map(entry => entry.label);
const repeated = count();
agent.focused = true;
const opened = count();
const cleared = workspaceAttentionCounts(payload).map(entry => entry.label);
agent.focused = false;
const stillRead = count();
agent.last_activity_at = '2026-09-27T10:01:00Z';
const afterActivity = count();
const afterReload = unreadCompletionCount({providers: [{agents: [agent]}]},
    new Map(JSON.parse(JSON.stringify([...states]))));
agent.phase = 'RUNNING';
count();
agent.phase = 'DONE';
agent.last_activity_at = '2026-09-27T10:05:00Z';
const nextTurn = count();
agent.focused = true;
const nextTurnOpened = count();
agent.phase = 'RUNNING';
count();
agent.phase = 'DONE';
const finishedWhileFocused = count();
agent.focused = false;
const remainsRead = count();
print(JSON.stringify({running, finished, marked, repeated, opened, cleared,
    stillRead, afterActivity, afterReload,
    nextTurn, nextTurnOpened, finishedWhileFocused, remainsRead}));
"#,
    );
    assert_eq!(js["running"], 0);
    assert_eq!(js["finished"], 1);
    assert_eq!(js["marked"][0], "W3:1");
    assert_eq!(js["repeated"], 1);
    assert_eq!(js["opened"], 0);
    assert_eq!(js["cleared"].as_array().unwrap().len(), 0);
    assert_eq!(js["stillRead"], 0);
    assert_eq!(js["afterActivity"], 0);
    assert_eq!(js["afterReload"], 0);
    assert_eq!(js["nextTurn"], 1);
    assert_eq!(js["nextTurnOpened"], 0);
    assert_eq!(js["finishedWhileFocused"], 0);
    assert_eq!(js["remainsRead"], 0);

    let css = fs::read_to_string(repo_root().join("acc-model-status@local/stylesheet.css")).unwrap();
    assert!(source.contains("'notify::focus-window'"));
    assert!(source.contains("focused: match.focused"));
    assert!(source.contains("this._box.add_style_class_name('ams-unread-glow')"));
    assert!(css.contains(".ams-unread-glow {"));
    assert!(css.contains(".ams-unread-flow-0 {"));
}

#[test]
fn unread_completion_acknowledgement_survives_shell_reload() {
    if !gjs_available() {
        eprintln!("gjs not installed; skipping");
        return;
    }
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model-status-unread.json");
    let path_json = serde_json::to_string(&path.to_string_lossy().to_string()).unwrap();
    let probe = format!(
        r#"
const file = Gio.File.new_for_path({path_json});
const states = new Map([['session-1', {{phase: 'DONE', unread: false,
    completionAt: '2026-09-27T10:00:00Z'}}]]);
saveUnreadStates(file, states);
const loaded = readUnreadStates(file);
const mode = file.query_info('unix::mode', Gio.FileQueryInfoFlags.NONE, null)
    .get_attribute_uint32('unix::mode') & 0o777;
GLib.file_set_contents(file.get_path(), '{{');
const corrupt = readUnreadStates(file);
print(JSON.stringify({{loaded: [...loaded], corrupt: [...corrupt], mode}}));
"#
    );
    let js = run_gjs(
        &[
            "const Gio = imports.gi.Gio; const GLib = imports.gi.GLib;".into(),
            extract_function(&source, "saveUnreadStates"),
            extract_function(&source, "readUnreadStates"),
        ],
        &probe,
    );
    assert_eq!(js["loaded"][0][0], "session-1");
    assert_eq!(js["loaded"][0][1]["unread"], false);
    assert_eq!(js["corrupt"].as_array().unwrap().len(), 0);
    assert_eq!(js["mode"], 0o600);
}

#[test]
fn live_agent_badge_is_elapsed_clock_not_running() {
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
    let snippets = vec![
        extract_const_block(&source, "PHASE_COLORS"),
        extract_function(&source, "phaseColor"),
        extract_function(&source, "formatRunTimer"),
        extract_function(&source, "agentPhaseBadgeText"),
        extract_function(&source, "agentPhaseBadgeColor"),
    ];
    let now_ms: i64 = 1_724_000_000_000;
    let started_ms = now_ms - (12 * 60 + 34) * 1000;
    let probe = format!(
        r#"
const live = {{
    phase: 'RUNNING',
    run_active: true,
    run_started_at: new Date({started_ms}).toISOString(),
}};
const goal = {{...live, goal_active: true}};
const pausedGoal = {{...live, goal_active: false}};
const done = {{
    phase: 'DONE',
    run_active: false,
    run_started_at: new Date({started_ms}).toISOString(),
}};
const attn = {{phase: 'ATTN'}};
const queued = {{phase: 'STOPPED', queued_reset_at: new Date({now_ms} + 82_000).toISOString()}};
print(JSON.stringify({{
    live_text: agentPhaseBadgeText(live, {now_ms}),
    live_color: agentPhaseBadgeColor(live),
    goal_color: agentPhaseBadgeColor(goal),
    paused_goal_color: agentPhaseBadgeColor(pausedGoal),
    done_text: agentPhaseBadgeText(done, {now_ms}),
    done_color: agentPhaseBadgeColor(done),
    attn_text: agentPhaseBadgeText(attn, {now_ms}),
    queued_text: agentPhaseBadgeText(queued, {now_ms}),
    queued_color: agentPhaseBadgeColor(queued, {now_ms}),
    clock: formatRunTimer(754),
    hour_clock: formatRunTimer(7509),
}}));
"#
    );
    let js = run_gjs(&snippets, &probe);
    assert_eq!(js["clock"], "12:34");
    assert_eq!(js["hour_clock"], "2:05:09");
    assert_eq!(js["live_text"], "12:34");
    assert_eq!(js["live_color"], "#e6c200");
    assert_eq!(js["goal_color"], "#a855f7");
    assert_eq!(js["paused_goal_color"], "#e6c200");
    assert_ne!(js["live_text"], "RUNNING");
    assert_eq!(js["done_text"], "DONE");
    assert_eq!(js["done_color"], "#22c55e");
    assert_eq!(js["attn_text"], "ATTN");
    assert_eq!(js["queued_text"], "-1:22");
    assert_eq!(js["queued_color"], "#b5d747");
    assert!(source.contains("agentPhaseBadgeText(agent)"));
    assert!(source.contains("agentPhaseBadgeColor(agent)"));
}

#[test]
fn harness_rate_strip_formats_counts_and_na() {
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
    let snippets = vec![
        extract_function(&source, "formatCompactTokens"),
        extract_function(&source, "formatHarnessRateText"),
        extract_function(&source, "harnessHasMinuteActivity"),
        extract_function(&source, "harnessRateLines"),
        extract_function(&source, "formatHarnessPanelRateText"),
        extract_function(&source, "panelHarnessRateRows"),
    ];
    let js = run_gjs(
        &snippets,
        r#"
print(JSON.stringify({
    na: formatHarnessRateText({kind: 'grok', label: 'Grok', tokens_per_second: null}),
    compact: {
        small: formatCompactTokens(12),
        k: formatCompactTokens(130681),
        m: formatCompactTokens(6758015),
    },
    huge: formatHarnessRateText({
        label: 'Codex',
        tokens_per_second: 130681,
        tokens_per_minute: 945695,
        tokens_per_hour: 6758015,
    }),
    counts: formatHarnessRateText({
        label: 'Codex',
        tokens_per_second: 12,
        tokens_per_minute: 80,
        tokens_per_hour: 900,
    }),
    active: harnessHasMinuteActivity({tokens_per_second: 0, tokens_per_minute: 10}),
    idle: harnessHasMinuteActivity({tokens_per_second: 0, tokens_per_minute: 0, tokens_per_hour: 50}),
    missing: harnessHasMinuteActivity({tokens_per_second: null}),
    panelLive: formatHarnessPanelRateText({tokens_per_second: 12, tokens_per_minute: 80}),
    panelMinute: formatHarnessPanelRateText({tokens_per_second: 0, tokens_per_minute: 10}),
    panelIdle: formatHarnessPanelRateText({tokens_per_second: 0, tokens_per_minute: 0}),
    lines: harnessRateLines({harnessRates: [
        {label: 'Claude', tokens_per_second: 0, tokens_per_minute: 10, tokens_per_hour: 10},
        {label: 'Grok', tokens_per_second: null},
        {label: 'Qwen', tokens_per_second: 0, tokens_per_minute: 0, tokens_per_hour: 50},
        {label: 'Codex', tokens_per_second: 12, tokens_per_minute: 80, tokens_per_hour: 900},
    ]}),
    panelRows: panelHarnessRateRows({harnessRates: [
        {kind: 'claude', tokens_per_second: 0, tokens_per_minute: 10},
        {kind: 'grok', tokens_per_second: null},
        {kind: 'codex', tokens_per_second: 12, tokens_per_minute: 80},
    ]}).map(row => row.kind),
}));
"#,
    );
    assert_eq!(js["na"], "Grok n/a");
    assert_eq!(js["compact"]["small"], "12");
    assert_eq!(js["compact"]["k"], "131k");
    assert_eq!(js["compact"]["m"], "6.8M");
    assert_eq!(js["huge"], "Codex 131k/s  946k/min  6.8M/h");
    assert_eq!(js["counts"], "Codex 12/s  80/min  900/h");
    assert_eq!(js["active"], true);
    assert_eq!(js["idle"], false);
    assert_eq!(js["missing"], false);
    assert_eq!(js["panelLive"], "12/s");
    assert_eq!(js["panelMinute"], "1/s");
    assert_eq!(js["panelIdle"], "");
    let lines = js["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "Claude 0/s  10/min  10/h");
    assert_eq!(lines[1], "Codex 12/s  80/min  900/h");
    let kinds: Vec<_> = js["panelRows"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(kinds, ["claude", "codex"]);
    let rate_pos = source.find("this._renderHarnessRates(payload)").unwrap();
    let workspace_pos = source.find("groupWorkspaceEntries(workspaceEntries)").unwrap();
    assert!(rate_pos < workspace_pos, "harness rates must render above workspaces");
    assert!(source.contains("ams-harness-rates"));
    assert!(source.contains("this._renderPanelHarnessRates(payload"));
    assert!(source.contains("style_class: 'ams-rate'"));
    assert!(source.contains("Workspace ${workspace}"));
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
fn model_status_codex_instance_label_matches_taskbar_identity_colors() {
    let model = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let taskbar = fs::read_to_string(repo_root().join("agentic-cli-status@local/extension.js")).unwrap();
    for name in ["INSTANCE_TAG_FIRST_RGB", "INSTANCE_TAG_LAST_RGB"] {
        assert_eq!(extract_constant(&model, name), extract_constant(&taskbar, name));
    }
    let label = extract_function(&model, "codexInstanceLabel");
    let color = extract_function(&model, "instanceTagColor");
    let result = run_gjs(&[
        extract_constant(&model, "INSTANCE_TAG_FIRST_RGB"),
        extract_constant(&model, "INSTANCE_TAG_LAST_RGB"),
        color,
        label,
    ], "print(JSON.stringify([codexInstanceLabel({kind: 'codex', codex_instance: 1}, 3), codexInstanceLabel({kind: 'codex', codex_instance: 2}, 3), codexInstanceLabel({kind: 'codex', codex_instance: 3}, 3), codexInstanceLabel({kind: 'claude', codex_instance: 1}, 3)]))");
    assert_eq!(result, serde_json::json!([["CX1", "#7ec8ff"], ["CX2", "#938ff4"], ["CX3", "#a856e8"], null]));

    let menu = extract_method(&model, "_agentMenuItem");
    assert!(menu.find("ams-menu-provider-icon").unwrap() < menu.find("ams-instance-tag").unwrap());
}

#[test]
fn opt_in_clis_stay_off_the_bar_until_enabled() {
    let Some(js) = agentic_results() else {
        return;
    };
    // Cold start (no config file yet) must mirror the poller defaults: the
    // opt-in Claude Code, OpenRouter, and OpenCode Go segments are not drawn at
    // all.
    assert_eq!(js["cold_start_defaults"]["openrouter"], false);
    assert_eq!(js["cold_start_defaults"]["opencode_go"], false);
    assert_eq!(js["cold_start_defaults"]["cursor"], false);
    assert_eq!(js["cold_start_defaults"]["codex"], true);
    assert_eq!(js["cold_start_defaults"]["claude_code"], false);
    assert_eq!(js["cold_start_defaults"]["grok"], true);
    // An explicit opt-in brings the segment back, and an opt-out removes a
    // default-on one.
    assert_eq!(js["cold_start_opt_in"]["openrouter"], true);
    assert_eq!(js["cold_start_opt_in"]["opencode_go"], true);
    assert_eq!(js["cold_start_opt_in"]["cursor"], true);
    assert_eq!(js["cold_start_opt_in"]["claude_code"], true);
    assert_eq!(js["cold_start_opt_out"]["grok"], false);
    // A non-boolean value is not an opt-in.
    assert_eq!(js["cold_start_garbage"]["openrouter"], false);

    let source = fs::read_to_string(
        repo_root()
            .join("agentic-cli-status@local")
            .join("extension.js"),
    )
    .unwrap();
    let code = code_only(&source);
    // The waiting-for-poller branch consults the config instead of painting a
    // placeholder for every known CLI.
    assert!(code.contains("cliEnabledFromConfig(readPollerConfig(), DEFAULT_CLI_ENABLED)"));
    assert!(code.contains("openrouter: false"));
    assert!(code.contains("opencode_go: false"));
    assert!(code.contains("cursor: false"));
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
