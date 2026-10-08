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
        let Some(quote) = after
            .chars()
            .next()
            .filter(|c| matches!(c, '\'' | '"' | '`'))
        else {
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

/// Check retained workspace and monitor ordering.
/// Parameters: none.
/// Returns: None after successful assertions.
#[test]
fn model_status_entries_sort_workspaces_and_monitor_rows() {
    println!("parameters=none");
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
        extract_function(&source, "windowDisplaySortValue"),
        extract_function(&source, "compareWorkspaceEntries"),
        extract_function(&source, "groupWorkspaceEntries"),
    ];
    let js = run_gjs(
        &snippets,
        r#"
const emit = print; print = value => {};
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
emit(JSON.stringify({
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
    println!("None");
}

#[test]
fn model_status_menu_shortcut_binding_is_reserved_and_restored() {
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
        extract_constant(&source, "AGENT_STATUS_MENU_ACCELERATORS"),
        extract_function(&source, "agentStatusMenuShortcutBindings"),
        extract_function(&source, "withoutAgentStatusMenuShortcuts"),
        extract_function(&source, "restoreAgentStatusMenuShortcuts"),
        extract_function(&source, "readMenuShortcutBackup"),
        extract_function(&source, "saveMenuShortcutBackup"),
        extract_function(&source, "deleteMenuShortcutBackup"),
    ];
    let js = run_gjs(
        &snippets,
        r#"
const original = ['<Alt>Above_Tab', '<Alt>F2', '<Alt>section', '<Alt>grave'];
const reserved = agentStatusMenuShortcutBindings(original);
const cleared = withoutAgentStatusMenuShortcuts(original);
const restored = restoreAgentStatusMenuShortcuts([...cleared, '<Super>F12'], reserved);
const {Gio, GLib} = imports.gi;
const parent = GLib.build_filenamev([GLib.get_tmp_dir(), `acc-menu-shortcut-${GLib.get_monotonic_time()}`]);
const file = Gio.File.new_for_path(GLib.build_filenamev([parent, 'backup.json']));
const backupSaved = saveMenuShortcutBackup(file, reserved);
const backupRead = readMenuShortcutBackup(file);
deleteMenuShortcutBackup(file);
const backupDeleted = !file.query_exists(null);
GLib.rmdir(parent);
print(JSON.stringify({reserved, cleared, restored, backupSaved, backupRead, backupDeleted}));
"#,
    );
    assert_eq!(
        js["reserved"],
        serde_json::json!(["<Alt>Above_Tab", "<Alt>section", "<Alt>grave"])
    );
    assert_eq!(js["cleared"], serde_json::json!(["<Alt>F2"]));
    assert_eq!(
        js["restored"],
        serde_json::json!([
            "<Alt>F2",
            "<Super>F12",
            "<Alt>Above_Tab",
            "<Alt>section",
            "<Alt>grave"
        ])
    );
    assert_eq!(js["backupSaved"], true);
    assert_eq!(js["backupRead"], js["reserved"]);
    assert_eq!(js["backupDeleted"], true);
}

#[test]
fn model_status_omits_session_worktree_mark() {
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
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
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
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
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let js = run_gjs(
        &[
            extract_function(&source, "unreadCompletionCount"),
            extract_function(&source, "workspaceAttentionCounts"),
        ],
        r#"
const states = new Map();
const agent = {session_id: 'session-1', phase: 'RUNNING', focused: false,
    workspace: 3, current_workspace: false,
    last_activity_at: '2026-09-27T10:00:00Z',
    last_worked_at: '2026-09-27T10:00:00Z'};
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
agent.last_worked_at = '2026-09-27T10:03:00Z';
const missedRunning = count();
agent.focused = true;
count();
agent.focused = false;
agent.phase = 'RUNNING';
count();
agent.phase = 'DONE';
agent.last_worked_at = '2026-09-27T10:05:00Z';
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
    stillRead, afterActivity, afterReload, missedRunning,
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
    assert_eq!(js["missedRunning"], 1);
    assert_eq!(js["nextTurn"], 1);
    assert_eq!(js["nextTurnOpened"], 0);
    assert_eq!(js["finishedWhileFocused"], 0);
    assert_eq!(js["remainsRead"], 0);

    let css =
        fs::read_to_string(repo_root().join("acc-model-status@local/stylesheet.css")).unwrap();
    assert!(source.contains("'notify::focus-window'"));
    assert!(source.contains("focused: match.focused"));
    assert!(source.contains("this._box.add_style_class_name('ams-unread-glow')"));
    assert!(css.contains(".ams-unread-glow {"));
    assert!(css.contains(".ams-unread-flow-0 {"));
}

#[test]
fn confirmed_completions_do_not_rearm_after_redraw_or_state_loss() {
    if !gjs_available() {
        eprintln!("gjs not installed; skipping");
        return;
    }
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let js = run_gjs(
        &[extract_function(&source, "unreadCompletionCount")],
        r#"
const now = Date.parse('2026-09-29T12:00:00Z');
const old = '2026-09-25T10:00:00Z';
const recent = '2026-09-29T11:59:55Z';
const agent = {session_id: 'session-1', phase: 'DONE', focused: false,
    last_worked_at: old, ready_completed_at: ''};
const payload = {providers: [{agents: [agent]}]};
let states = new Map([['session-1', {phase: 'DONE', unread: true, completedAt: old}]]);
const count = () => unreadCompletionCount(payload, states, now);
const unconfirmed = count();
agent.ready_completed_at = old;
const historical = count();
states = new Map();
const lostStateOld = count();
agent.ready_completed_at = recent;
const fresh = count();
const repeated = count();
states = new Map(JSON.parse(JSON.stringify([...states])));
const reloadUnread = unreadCompletionCount(payload, states, now + 300000);
agent.focused = true;
const opened = count();
agent.focused = false;
agent.phase = 'RUNNING';
agent.ready_completed_at = '';
count();
states = new Map(JSON.parse(JSON.stringify([...states])));
agent.phase = 'DONE';
agent.ready_completed_at = recent;
agent.last_worked_at = '2026-09-29T12:00:00Z';
const redraw = count();
agent.last_worked_at = '2026-09-29T12:01:00Z';
const changedHistory = count();
agent.ready_completed_at = old;
const olderPayload = count();
agent.ready_completed_at = recent;
const restoredPayload = count();
agent.ready_completed_at = '2026-09-29T12:00:00Z';
const newCompletion = count();
agent.focused = true;
count();
agent.ready_completed_at = '2026-09-29T12:00:01Z';
const finishedFocused = count();
agent.focused = false;
const noReplay = count();
agent.ready_completed_at = 'invalid';
const invalid = count();
agent.ready_completed_at = '2026-09-30T12:00:00Z';
states = new Map();
const future = count();
print(JSON.stringify({unconfirmed, historical, lostStateOld, fresh, repeated,
    reloadUnread, opened, redraw, changedHistory, olderPayload, restoredPayload, newCompletion,
    finishedFocused, noReplay, invalid, future}));
"#,
    );
    for field in ["unconfirmed", "historical", "lostStateOld", "opened", "redraw",
        "changedHistory", "olderPayload", "restoredPayload", "finishedFocused", "noReplay", "invalid", "future"] {
        assert_eq!(js[field], 0, "{field}");
    }
    for field in ["fresh", "repeated", "reloadUnread", "newCompletion"] {
        assert_eq!(js[field], 1, "{field}");
    }
}

#[test]
fn unread_completion_acknowledgement_survives_shell_reload() {
    if !gjs_available() {
        eprintln!("gjs not installed; skipping");
        return;
    }
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
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
    let kinds: Vec<_> = js["panelRows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["claude", "codex"]);
    let rate_pos = source.find("this._renderHarnessRates(payload)").unwrap();
    let workspace_pos = source
        .find("groupWorkspaceEntries(workspaceEntries)")
        .unwrap();
    assert!(
        rate_pos < workspace_pos,
        "harness rates must render above workspaces"
    );
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
    for forbidden in ["GLib.spawn", "Soup", "subprocess"] {
        assert!(!code.contains(forbidden), "{forbidden}");
    }
    assert!(code.contains("Gio.Subprocess.new"));
    assert!(code.contains("this._focusAgent(noteId)"));
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
    let model =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let taskbar =
        fs::read_to_string(repo_root().join("agentic-cli-status@local/extension.js")).unwrap();
    for name in ["INSTANCE_TAG_FIRST_RGB", "INSTANCE_TAG_LAST_RGB"] {
        assert_eq!(
            extract_constant(&model, name),
            extract_constant(&taskbar, name)
        );
    }
    let label = extract_function(&model, "codexInstanceLabel");
    let color = extract_function(&model, "instanceTagColor");
    let result = run_gjs(&[
        extract_constant(&model, "INSTANCE_TAG_FIRST_RGB"),
        extract_constant(&model, "INSTANCE_TAG_LAST_RGB"),
        color,
        label,
    ], "print(JSON.stringify([codexInstanceLabel({kind: 'codex', codex_instance: 1}, 3), codexInstanceLabel({kind: 'codex', codex_instance: 2}, 3), codexInstanceLabel({kind: 'codex', codex_instance: 3}, 3), codexInstanceLabel({kind: 'grok'}, 3), codexInstanceLabel({kind: 'claude', codex_instance: 1}, 3), codexInstanceLabel({kind: 'qwen'}, 3), codexInstanceLabel({kind: 'openrouter'}, 3), codexInstanceLabel({kind: 'nvidia'}, 3), codexInstanceLabel({kind: 'opencode'}, 3), codexInstanceLabel({kind: 'cline'}, 3), codexInstanceLabel({kind: 'api'}, 3)]))");
    assert_eq!(
        result,
        serde_json::json!([
            ["CX1", "#7ec8ff"],
            ["CX2", "#938ff4"],
            ["CX3", "#a856e8"],
            ["GK1", "#7ec8ff"],
            ["CC1", "#7ec8ff"],
            ["QA1", "#7ec8ff"],
            ["OA1", "#7ec8ff"],
            ["NA1", "#7ec8ff"],
            ["OC1", "#7ec8ff"],
            ["CL1", "#7ec8ff"],
            ["AP1", "#7ec8ff"]
        ])
    );

    let menu = extract_method(&model, "_agentMenuItem");
    assert!(menu.find("ams-menu-provider-icon").unwrap() < menu.find("ams-instance-tag").unwrap());
}

/// Preserve menu click targets while recurring refreshes update panel estimates.
/// Parameters: none.
/// Returns: None; asserts stable menu and clock actors through open/close cycles.
#[test]
fn model_status_menu_keeps_click_targets_until_closed() {
    println!("parameters=none");
    let model =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let methods = ["_init", "_refresh", "_queueRefresh"]
        .map(|name| extract_method(&model, name))
        .join("\n");
    let stubs = format!(
        "{}\n{}",
        extract_function(&model, "setStatusMenuActionMode"),
        r#"
const emit = print; print = value => {};
const Shell = {ActionMode: {NORMAL: 1, POPUP: 2}};
const Main = {panel: {menuManager: {_grabParams: {actionMode: Shell.ActionMode.POPUP}}}};
const activeTaskEstimates = payload => [];
const longestTaskEstimate = estimates => ({status: 'inactive'});
const formatPanelTaskEstimate = estimate => '';
let reads = 0;
let removals = 0;
const callbacks = [];
const GLib = {PRIORITY_DEFAULT: 0, SOURCE_REMOVE: false,
    timeout_add(_priority, _delay, callback) { callbacks.push(callback); return callbacks.length; }};
const St = {BoxLayout: class {destroy_all_children() {}}};
const UNREAD_FILENAME = 'unread.json';
function payloadFile() { return {get_parent() { return {get_child() { return {}; }}; }}; }
function readUnreadStates() { return new Map(); }
function readPayload() { reads++; return null; }
function log() {}
function panelHarnessRateRows() { return []; }
class Base {
    _init() {
        this.menu = {isOpen: false, handlers: {}, rows: [{}],
            connect(name, callback) { this.handlers[name] = callback; },
            removeAll() { removals++; this.rows = []; }};
    }
    add_child() {}
    _startWatching() {}
    _startWindowWatching() {}
    _openWindowDescriptors() { return []; }
    _renderEmpty() { this.menu.rows = [{}]; }
    _syncGlowTimer() {}
    _updateTaskRows() {}
    _syncRunClockTimer() {}
}
"#
    );
    let stubs =
        format!("{stubs}\nBase.prototype._updatePanelTaskEstimate = (text, stale) => {{}};");
    let button = format!("class Button extends Base {{ {methods} }}");
    let result = run_gjs(
        &[stubs, button],
        r#"
const button = new Button();
button._init({dir: {get_child() { return {}; }}});
const snapshots = [];
for (let cycle = 0; cycle < 2; cycle++) {
    button.menu.isOpen = true;
    button.menu.handlers['open-state-changed']?.(button.menu, true);
    const target = button.menu.rows[0];
    const clocks = button._runClockActors;
    for (let tick = 0; tick < 5; tick++) {
        button._queueRefresh();
        callbacks.shift()();
        button._refresh();
    }
    snapshots.push({reads, removals, sameTarget: target === button.menu.rows[0],
        sameClocks: clocks === button._runClockActors,
        actionMode: Main.panel.menuManager._grabParams.actionMode});
    button.menu.isOpen = false;
    button.menu.handlers['open-state-changed']?.(button.menu, false);
    const queued = callbacks.length;
    callbacks.shift()?.();
    snapshots.push({reads, removals, queued,
        actionMode: Main.panel.menuManager._grabParams.actionMode});
}
emit(JSON.stringify(snapshots));
"#,
    );
    assert_eq!(
        result,
        serde_json::json!([
            {"reads": 11, "removals": 1, "sameTarget": true, "sameClocks": true, "actionMode": 3},
            {"reads": 12, "removals": 2, "queued": 1, "actionMode": 2},
            {"reads": 22, "removals": 2, "sameTarget": true, "sameClocks": true, "actionMode": 3},
            {"reads": 23, "removals": 3, "queued": 1, "actionMode": 2},
        ])
    );
    println!("None");
}

#[test]
fn model_status_menu_rows_focus_the_exact_agent_through_acc() {
    let model =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let menu = extract_method(&model, "_agentMenuItem");
    let item = format!("const menuBuilder = {{ {} }};", menu);
    let stubs = r#"
const focused = [];
const PopupMenu = {
    PopupBaseMenuItem: class extends Actor {
        constructor(options) { super(options); this.options = options; this.actor = this; }
    },
};
St.BoxLayout = Actor;
St.Icon = Actor;
St.Label = Actor;
Clutter.ActorAlign = {CENTER: 0};
function codexInstanceLabel() { return null; }
function agentPhaseBadgeText() { return 'DONE'; }
function agentPhaseBadgeColor() { return '#fff'; }
"#
    .to_string();
    let extension = format!(
        "{}\nconst extension = Object.assign({{_runClockActors: [], _taskRows: new Map(), _icon() {{ return {{}}; }}, _focusAgent(noteId) {{ focused.push(noteId); }} }}, menuBuilder);",
        item
    );
    let result = run_gjs(
        &[task_progress_stubs(), stubs, extract_function(&model, "taskProgressValue"),
            extract_function(&model, "taskProgressRectangles"), extract_function(&model, "taskEstimateText"), extract_function(&model, "updateTaskProgress"), extract_function(&model, "buildTaskProgressLayer"), extension],
        r#"
const unread = extension._agentMenuItem({note_id: 'note-unread', unread_completion: true}, {}, 0);
const read = extension._agentMenuItem({note_id: 'note-read', unread_completion: false}, {}, 0);
const running = extension._agentMenuItem({note_id: 'note-running', phase: 'RUNNING', unread_completion: false}, {}, 0);
const waiting = extension._agentMenuItem({note_id: 'note-waiting', phase: 'WAITING', unread_completion: false}, {}, 0);
const stopped = extension._agentMenuItem({note_id: 'note-stopped', phase: 'STOPPED', unread_completion: false}, {}, 0);
const missing = extension._agentMenuItem({unread_completion: true}, {}, 0);
const rows = [unread, read, running, waiting, stopped, missing];
for (const row of rows) row.handlers.activate?.();
print(JSON.stringify({
    options: rows.map(row => row.options),
    highlighted: rows.map(row => row.styles.includes('ams-agent-unread')),
    focused,
}));
"#,
    );
    assert_eq!(
        result,
        serde_json::json!({
            "options": [
                {"reactive": true, "can_focus": true},
                {"reactive": true, "can_focus": true},
                {"reactive": true, "can_focus": true},
                {"reactive": true, "can_focus": true},
                {"reactive": true, "can_focus": true},
                {"reactive": false, "can_focus": false},
            ],
            "highlighted": [true, false, false, false, false, true],
            "focused": ["note-unread", "note-read", "note-running", "note-waiting", "note-stopped"],
        })
    );

    let focus = extract_method(&model, "_focusAgent");
    let gio_stub = r#"
const launched = [];
const ordering = [];
const menu = {close() { ordering.push('close'); }};
const GLib = {
    build_filenamev(parts) { return parts.join('/'); },
    get_home_dir() { return '/home/test'; },
};
const Gio = {
    SubprocessFlags: {NONE: 0},
    Subprocess: {
        new(argv, flags) {
            ordering.push('launch');
            launched.push({argv: [...argv], flags});
            return {
                wait_check_async(_cancellable, callback) { callback(this, {}); },
                wait_check_finish() { return true; },
            };
        },
    },
};
function logError(_error, _message) {}
    "#
    .to_string();
    let extension = format!("const extension = {{ menu, {} }};", focus);
    let result = run_gjs(
        &[gio_stub, extension],
        "extension._focusAgent('note-123'); extension._focusAgent('  '); print(JSON.stringify({launched, ordering}));",
    );
    assert_eq!(
        result,
        serde_json::json!({"launched": [{
            "argv": ["/home/test/.local/bin/acc-window-select", "--note-id", "note-123"],
            "flags": 0
        }], "ordering": ["close", "launch"]})
    );
}

#[test]
/// Keep opt-in CLI taskbar segments hidden until selected.
///
/// Parameters: none.
/// Returns: None.
fn opt_in_clis_stay_off_the_bar_until_enabled() {
    println!("parameters=none");
    let Some(js) = agentic_results() else {
        println!("()");
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
    assert_eq!(js["cold_start_defaults"]["grok"], false);
    // An explicit opt-in brings the segment back, and an opt-out removes a
    // default-on one.
    assert_eq!(js["cold_start_opt_in"]["openrouter"], true);
    assert_eq!(js["cold_start_opt_in"]["opencode_go"], true);
    assert_eq!(js["cold_start_opt_in"]["cursor"], true);
    assert_eq!(js["cold_start_opt_in"]["claude_code"], true);
    assert_eq!(js["cold_start_opt_in"]["grok"], true);
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
    println!("()");
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

#[test]
fn model_status_task_progress_validates_payload() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let js = run_gjs(&[extract_function(&source, "taskProgressValue")], r#"
const inputs = [
    {}, {workflow: null}, {workflow: {fraction: NaN}}, {workflow: {fraction: '0.5'}},
    {workflow: {fraction: -1}}, {workflow: {fraction: 2}}, {workflow: {fraction: 0}},
    {workflow: {fraction: .25}}, {workflow: {fraction: .5}, unread_completion: true},
    {workflow: {fraction: .5}, phase: 'DONE', ready_completed_at: '2026-10-07T01:00:00Z'},
    {workflow: {fraction: .5}, phase: 'DONE', ready_completed_at: ''}];
print(JSON.stringify(inputs.map(taskProgressValue)));
"#);
    assert_eq!(js, serde_json::json!([null, null, null, null, 0, 1, 0, 0.25, null, null, 0.5]));
}

fn task_progress_stubs() -> String {
    r#"
class Actor {
    constructor(options = {}) {
        Object.assign(this, options); this.children = []; this.handlers = {};
        this.styles = []; this.visible = true; this.width = 400; this.height = 32;
    }
    add_child(child) { this.children.push(child); }
    connect(name, callback) { this.handlers[name] = callback; }
    get_width() { return this.width; }
    get_height() { return this.height; }
    set_size(width, height) { this.width = width; this.height = height; }
    set_position(x, y) { this.x = x; this.y = y; }
    set_style(style) { this.style = style; }
    destroy() { this.destroyed = true; }
    add_style_class_name(style) { if (!this.styles.includes(style)) this.styles.push(style); }
    remove_style_class_name(style) { this.styles = this.styles.filter(s => s !== style); }
}
const St = {Widget: Actor};
const Clutter = {BinLayout: class {}, FixedLayout: class {}};
function agentPhaseBadgeText(agent) { return agent.phase || ''; }
function agentPhaseBadgeColor() { return '#fff'; }
"#.to_string()
}

#[test]
fn model_status_task_progress_layer_updates_in_place() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let snippets = vec![task_progress_stubs(), extract_function(&source, "taskProgressValue"),
        extract_function(&source, "taskProgressRectangles"), extract_function(&source, "taskEstimateText"), extract_function(&source, "updateTaskProgress"), extract_function(&source, "buildTaskProgressLayer")];
    let js = run_gjs(&snippets, r#"
const item = new Actor(); item.actor = item;
const text = new Actor(), badge = new Actor();
const agent = {phase: 'RUNNING', title: 'First task', workflow: {fraction: .25, label: 'Write code 1 out of 4'}};
const widgets = buildTaskProgressLayer(item, text, badge, agent);
const otherItem = new Actor(); otherItem.actor = otherItem;
const other = buildTaskProgressLayer(otherItem, new Actor(), new Actor(),
    {phase: 'RUNNING', workflow: {fraction: .75}});
const before = {behind: item.children[0].children[0] === widgets.track &&
    item.children[0].children[1] === text,
    firstWidth: widgets.fill.width, otherWidth: other.fill.width,
    caption: item.accessible_name};
widgets.track.width = 200;
widgets.track.handlers['notify::allocation']();
const resized = widgets.fill.width;
updateTaskProgress(widgets, {phase: 'DONE', unread_completion: true, ready_completed_at: 'finish', workflow: {fraction: 1}});
const done = {hidden: !widgets.track.visible, glow: item.styles.includes('ams-agent-unread')};
updateTaskProgress(widgets, {phase: 'DONE', unread_completion: false, ready_completed_at: 'finish', workflow: {fraction: 1}});
const read = {hidden: !widgets.track.visible, glow: item.styles.includes('ams-agent-unread')};
updateTaskProgress(widgets, {phase: 'RUNNING', workflow: {fraction: .5}});
const resumed = {shown: widgets.track.visible, width: widgets.fill.width, glow: item.styles.includes('ams-agent-unread')};
updateTaskProgress(widgets, {phase: 'RUNNING'});
print(JSON.stringify({before, resized, done, read, resumed, missingHidden: !widgets.track.visible}));
"#);
    assert_eq!(js, serde_json::json!({
        "before": {"behind": true, "firstWidth": 100, "otherWidth": 300, "caption": "First task · Write code 1 out of 4"},
        "resized": 50,
        "done": {"hidden": true, "glow": true}, "read": {"hidden": true, "glow": false},
        "resumed": {"shown": true, "width": 100, "glow": false}, "missingHidden": true
    }));
}

#[test]
fn model_status_open_menu_progress_refresh_keeps_actor_identity() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let method = extract_method(&source, "_updateTaskRows");
    let snippets = vec![task_progress_stubs(), extract_function(&source, "taskProgressValue"),
        extract_function(&source, "taskProgressRectangles"), extract_function(&source, "taskEstimateText"), extract_function(&source, "updateTaskProgress"), extract_function(&source, "buildTaskProgressLayer"),
        format!("const updater = {{ {method} }};")];
    let js = run_gjs(&snippets, r#"
const item = new Actor(); item.actor = item;
const row = new Actor(), badge = new Actor();
const widgets = buildTaskProgressLayer(item, row, badge, {phase: 'RUNNING', workflow: {fraction: .25}});
const button = Object.assign({_taskRows: new Map([['session', widgets]])}, updater);
const update = agent => button._updateTaskRows({providers: [{agents: [{session_id: 'session', ...agent}]}]});
update({phase: 'RUNNING', workflow: {fraction: .75}});
const width = widgets.fill.width;
update({phase: 'DONE', unread_completion: true, ready_completed_at: 'finish'});
print(JSON.stringify({width, hidden: !widgets.track.visible, glow: item.styles.includes('ams-agent-unread'),
    sameItem: widgets.item === item, sameText: item.children[0].children[1] === row,
    badge: badge.text}));
"#);
    assert_eq!(js, serde_json::json!({"width":300,"hidden":true,"glow":true,"sameItem":true,"sameText":true,"badge":"DONE"}));
}


#[test]
fn model_status_segmented_task_background() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let js = run_gjs(&[extract_function(&source, "taskProgressRectangles")], r#"
const agent = {workflow: {segments: [
    {phase: 'Plan', status: 'complete'}, {phase: 'Establish worktree', status: 'complete'},
    {phase: 'Write code', status: 'complete'}, {phase: 'Write code', status: 'complete'},
    {phase: 'Write code', status: 'in_progress'}, {phase: 'Write code', status: 'pending'},
    {phase: 'Write documentation', status: 'pending'}]}};
const rects = taskProgressRectangles(agent, 280);
const gaps = rects.slice(1).map((r, i) => r.x - rects[i].x - rects[i].width);
const bounded = [0, 1, 20, 120, 280].every(width => {
    const rs = taskProgressRectangles(agent, width);
    return rs.every((r, i) => r.x >= 0 && r.width > 0 && r.x + r.width <= width &&
        (!i || rs[i - 1].x + rs[i - 1].width <= r.x));
});
const skipped = taskProgressRectangles({workflow: {segments: [{phase: 'Plan', status: 'skipped'}]}}, 1)[0].color;
const many = taskProgressRectangles({workflow: {segments: Array.from({length: 100}, () => ({phase: 'Write code', status: 'pending'}))}}, 20);
const invalid = [{}, {workflow: {segments: 'bad'}},
    {workflow: {segments: [null]}}, {workflow: {segments: [{phase: 'unknown', status: 'complete'}]}},
    {workflow: {segments: [{phase: 'Plan', status: 'bad'}]}}];
print(JSON.stringify({colors: rects.map(r => r.color), gaps, bounded,
    edge: rects[6].x + rects[6].width, skipped, many: many.length, invalid: invalid.map(a => taskProgressRectangles(a, 280))}));
"#);
    assert_eq!(js, serde_json::json!({"colors": ["92, 168, 255", "174, 129, 255", "55, 211, 190", "55, 211, 190", "255, 198, 79", "48, 52, 64", "48, 52, 64"],
        "gaps": [7,7,2,2,2,7], "bounded": true, "edge": 280, "skipped": "72, 77, 89", "many": 20, "invalid": [[],[],[],[],[]]}));
}


#[test]
fn model_status_segmented_actors_resize_and_retain_click_target() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let snippets = vec![task_progress_stubs(), extract_function(&source, "taskProgressValue"),
        extract_function(&source, "taskProgressRectangles"), extract_function(&source, "taskEstimateText"), extract_function(&source, "updateTaskProgress"),
        extract_function(&source, "buildTaskProgressLayer")];
    let js = run_gjs(&snippets, r#"
const item = new Actor(); item.actor = item;
const row = new Actor(), badge = new Actor();
const agent = {phase: 'RUNNING', workflow: {fraction: .5, segments: [
    {phase: 'Plan', status: 'complete'}, {phase: 'Write code', status: 'in_progress'}]}};
const widgets = buildTaskProgressLayer(item, row, badge, agent);
const first = widgets.blocks[0];
const initial = {count: widgets.blocks.length, fillHidden: !widgets.fill.visible,
    firstColor: first.style, activeColor: widgets.blocks[1].style,
    behind: item.children[0].children[0] === widgets.track && item.children[0].children[1] === row,
    inert: widgets.blocks.every(b => b.reactive === false)};
widgets.track.width = 100; widgets.track.height = 24;
widgets.track.handlers['notify::allocation']();
const resize = {edge: widgets.blocks[1].x + widgets.blocks[1].width, height: first.height,
    sameBlock: first === widgets.blocks[0]};
agent.workflow.segments[1].status = 'complete'; updateTaskProgress(widgets, agent);
const changed = widgets.blocks[1].style;
agent.workflow.segments.pop(); updateTaskProgress(widgets, agent);
const shrunk = widgets.blocks.length;
updateTaskProgress(widgets, {phase: 'DONE', ready_completed_at: 'finish', unread_completion: true});
print(JSON.stringify({initial, resize, changed, shrunk,
    done: {hidden: !widgets.track.visible, glow: item.styles.includes('ams-agent-unread')},
    sameItem: widgets.item === item}));
"#);
    assert_eq!(js, serde_json::json!({"initial": {"count": 2, "fillHidden": true,
        "firstColor": "background-color: rgba(92, 168, 255, 0.24);", "activeColor": "background-color: rgba(255, 198, 79, 0.24);", "behind": true, "inert": true},
        "resize": {"edge": 100, "height": 24, "sameBlock": true},
        "changed": "background-color: rgba(55, 211, 190, 0.24);", "shrunk": 1,
        "done": {"hidden": true, "glow": true}, "sameItem": true}));
}


#[test]
fn model_status_compact_workflow_count_precedes_timer() {
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let snippets = [extract_function(&source, "formatRunTimer"), extract_function(&source, "agentPhaseBadgeText")];
    let js = run_gjs(&snippets, r#"
const now = Date.parse('2026-10-07T12:02:05Z');
const agent = {phase: 'RUNNING', run_active: true, run_started_at: '2026-10-07T12:00:00Z',
    workflow: {phase_completed: 1, phase_total: 4, label: 'Establish worktree'}};
const text = agentPhaseBadgeText(agent, now);
const tick = agentPhaseBadgeText(agent, now + 1000);
const invalid = [{}, {phase_completed: -1, phase_total: 4}, {phase_completed: 1.5, phase_total: 4},
    {phase_completed: 1, phase_total: 0}, {phase_completed: 5, phase_total: 4},
    {phase_completed: '1', phase_total: 4}, {phase_completed: 1, phase_total: NaN}];
const bad = invalid.map(workflow => agentPhaseBadgeText({...agent, workflow}, now));
const zero = agentPhaseBadgeText({...agent, workflow: {phase_completed: 0, phase_total: 4}}, now);
const full = agentPhaseBadgeText({...agent, workflow: {phase_completed: 4, phase_total: 4}}, now);
const queued = agentPhaseBadgeText({...agent, phase: 'WAITING', queued_reset_at: '2026-10-07T12:03:05Z'}, now);
const waiting = agentPhaseBadgeText({...agent, phase: 'WAITING', run_active: false}, now);
const done = agentPhaseBadgeText({...agent, phase: 'DONE', ready_completed_at: 'finished'}, now);
const unread = agentPhaseBadgeText({...agent, phase: 'DONE', unread_completion: true}, now);
print(JSON.stringify({text, tick, bad, zero, full, queued, waiting, done, unread}));
"#);
    assert_eq!(js, serde_json::json!({"text": "[1/4] 2:05", "tick": "[1/4] 2:06",
        "bad": ["2:05", "2:05", "2:05", "2:05", "2:05", "2:05", "2:05"],
        "zero": "[0/4] 2:05", "full": "[4/4] 2:05", "queued": "[1/4] -1:00",
        "waiting": "[1/4] WAITING", "done": "DONE", "unread": "DONE"}));
}

#[test]
fn model_status_estimate_is_right_of_summary_and_refreshes_in_place() {
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let menu = format!(
        "const builder = {{ {} }};",
        extract_method(&source, "_agentMenuItem")
    );
    let stubs = r#"
const PopupMenu = {PopupBaseMenuItem: class extends Actor {
    constructor(options) { super(options); this.actor = this; }
}};
St.BoxLayout = Actor; St.Icon = Actor; St.Label = Actor;
Clutter.ActorAlign = {CENTER: 0, END: 1};
function codexInstanceLabel() { return null; }
"#
    .to_string();
    let snippets = vec![
        task_progress_stubs(),
        stubs,
        extract_function(&source, "taskEstimateText"),
        extract_function(&source, "taskProgressValue"),
        extract_function(&source, "taskProgressRectangles"),
        extract_function(&source, "updateTaskProgress"),
        extract_function(&source, "buildTaskProgressLayer"),
        menu,
    ];
    let js = run_gjs(
        &snippets,
        r#"
const extension = Object.assign({_runClockActors: [], _taskRows: new Map(), _icon() { return {}; }}, builder);
const agent = {session_id: 'task', note_id: 'note', project_tag: 'ACC', project: 'agent_command_center',
    title: 'AI summary', branch: 'feat/example', phase: 'RUNNING', run_active: true,
    task_time_estimate: {status: 'estimated', label: '~9m left'}};
const item = extension._agentMenuItem(agent, {}, 0);
const widgets = extension._taskRows.get('task');
const row = item.children[0].children[1];
const initial = {last: row.children.at(-1).text, title: row.children.find(c => c.style_class === 'ams-agent-title').text,
    origin: row.children.find(c => c.style_class === 'ams-agent-origin').text,
    tag: row.children[0].text, aligned: row.children.at(-1).x_align === 1, expands: row.children.at(-1).x_expand};
const actor = widgets.estimateLabel;
updateTaskProgress(widgets, {...agent, task_time_estimate: {status: 'estimated', label: '~4m left'}});
const refreshed = actor.text;
updateTaskProgress(widgets, {...agent, phase: 'DONE', run_active: false});
const idle = actor.text;
const states = [null, [], {label: 123}, {status: 'complete', label: '~9m left'},
    {status: 'learning', label: 'learning time estimates'}, {status: 'overrun', label: 'past estimate'}]
    .map(task_time_estimate => taskEstimateText({...agent, task_time_estimate}));
const waiting = taskEstimateText({...agent, phase: 'STOPPED', run_active: false,
    queued_reset_at: '2099-01-01T00:00:00Z'});
print(JSON.stringify({initial, refreshed, idle, states, waiting, same: actor === widgets.estimateLabel && item === widgets.item}));
"#,
    );
    assert_eq!(
        js,
        serde_json::json!({"initial": {"last": "~9m left", "title": "AI summary", "tag": "ACC", "origin": "(feat/example)", "aligned": true, "expands": true},
        "refreshed": "~4m left", "idle": "", "states": ["", "", "", "", "learning time estimates", "past estimate"],
        "waiting": "~9m left", "same": true})
    );
}

/// Check maximum selection, active-only filtering and duration boundaries.
/// Parameters: none.
/// Returns: None; asserts independently expected rendered labels.
#[test]
fn panel_remaining_uses_maximum_active_historical_estimate() {
    println!("parameters=none");
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let snippets = vec![
        extract_function(&source, "activeTaskEstimates"),
        extract_function(&source, "longestTaskEstimate"),
        extract_function(&source, "formatPanelTaskEstimate"),
        "const emit = print; const traces = []; print = value => traces.push(value);".into(),
    ];
    let result = run_gjs(
        &snippets,
        r#"
const agent = (seconds, extra = {}) => ({phase: 'RUNNING', run_active: true,
    task_time_estimate: {status: 'estimated', remaining_seconds: seconds}, ...extra});
const label = payload => formatPanelTaskEstimate(longestTaskEstimate(activeTaskEstimates(payload)));
const payload = {providers: [
    {agents: [agent(120), agent(9999, {phase: 'DONE'}), agent(9999, {run_active: false})]},
    {agents: [agent(540, {workspace: 8}), agent(240), agent(9999, {phase: 'WAITING'})]}]};
const invalid = ['120', NaN, Infinity, -1, 0, null].map(seconds =>
    label({providers: [{agents: [agent(seconds)]}]}));
const malformed = [null, {}, {providers: null}, {providers: [null, {agents: 'bad'}]},
    {providers: [{agents: [null, {}, agent(20, {task_time_estimate: {status: 'complete'}})]}]}];
const learning = agent(null, {task_time_estimate: {status: 'learning'}});
const overrun = agent(null, {task_time_estimate: {status: 'overrun'}});
emit(JSON.stringify({maximum: label(payload), invalid, malformed: malformed.map(label),
    bounds: [1, 60, 61, 3600, 3601].map(seconds => label({providers: [{agents: [agent(seconds)]}]})),
    learning: label({providers: [{agents: [learning]}]}),
    overrun: label({providers: [{agents: [overrun]}]}),
    mixed: label({providers: [{agents: [learning, overrun, agent(540)]}]}),
    empty: label({providers: [{agents: []}]}), traced: traces.length > 20}));
"#,
    );
    assert_eq!(
        result,
        serde_json::json!({
            "maximum": "~9m left", "invalid": ["learning", "learning", "learning", "learning", "learning", "learning"],
            "malformed": ["", "", "", "", "learning"],
            "bounds": ["~1m left", "~1m left", "~2m left", "~1h left", "~1h 1m left"],
            "learning": "learning", "overrun": "past estimate", "mixed": "~9m left", "empty": "", "traced": true
        })
    );
    println!("None");
}

/// Check the trailing panel actor through open-menu refresh and recovery.
/// Parameters: none.
/// Returns: None; asserts maximum text, stable click targets and stale styling.
/// Check panel refresh with a local numbering file path.
/// Parameters: none.
/// Returns: None after successful assertions.
#[test]
fn panel_remaining_refreshes_at_far_right_without_replacing_menu() {
    println!("parameters=none");
    println!("parameters=none");
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let snippets = vec![
        extract_function(&source, "activeTaskEstimates"),
        extract_function(&source, "longestTaskEstimate"),
        extract_function(&source, "formatPanelTaskEstimate"),
        format!("const methods = {{ {}, {} }};", extract_method(&source, "_refresh"),
            extract_method(&source, "_updatePanelTaskEstimate")),
        r#"
const emit = print; print = value => {};
let published = {updatedAt: 'fresh', providers: [
    {kind: 'codex', agents: [{phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'estimated', remaining_seconds: 120}}]},
    {kind: 'claude', agents: [{phase: 'RUNNING', run_active: true, task_time_estimate: {status: 'estimated', remaining_seconds: 540}}]}]};
const readPayload = file => published;
const readWindowDisplayIds = file => new Map();
const withWindowDisplayIds = (payload, ids) => payload;
const scopePayloadToWindows = (payload, windows) => ({...payload, providers: payload.providers.slice(0, 1)});
const unreadCompletionCount = (payload, states) => 0;
const saveUnreadStates = (file, states) => {};
const log = message => {};
const ageSeconds = stamp => stamp === 'stale' ? 999 : 0;
const STALE_SECONDS = 30;
const workspaceAttentionCounts = payload => [];
const panelHarnessRateRows = payload => [];
const actor = options => ({...options, styles: [],
    add_style_class_name: name => {}, remove_style_class_name: name => {}});
const St = {Label: function(options) {
    // Create the label stub. Parameters: this - stub receiver; options - actor properties. Returns: label actor.
    print(`this=${this}, options=${JSON.stringify(options)}`);
    const label = actor(options);
    label.add_style_class_name = name => label.styles.push(name);
    label.remove_style_class_name = name => {label.styles = label.styles.filter(s => s !== name);};
    print(JSON.stringify(label));
    return label;
}};
const Clutter = {ActorAlign: {CENTER: 0, END: 1}};
"#.into(),
    ];
    let result = run_gjs(
        &snippets,
        r#"
const box = {children: [], add_child: child => box.children.push(child),
    destroy_all_children: () => {box.children = [];}};
const menu = {isOpen: false, target: {}, removeAll: () => {menu.target = {};}};
const chip = name => box.add_child({name});
const button = Object.assign({_file: {get_parent: () => ({get_child: name => ({})})}, _box: box, menu, _taskRows: new Map(), _unreadStates: new Map(), _unreadCount: 0,
    _openWindowDescriptors: () => [], _renderHarnessRates: payload => {},
    _renderProvider: (provider, stale, payload) => chip(provider.kind),
    _renderPanelHarnessRates: (payload, stale, kinds) => chip('rate'),
    _renderWorkspaceAttention: (attention, stale) => chip('attention'),
    _renderMenu: (payload, stale, attention) => {}, _renderEmpty: payload => chip('empty'),
    _syncGlowTimer: () => {}, _syncRunClockTimer: () => {}, _updateTaskRows: payload => {}}, methods);
button._refresh();
const estimate = box.children[box.children.length - 1];
const initial = {text: estimate.text, preceding: box.children.slice(0, -1).map(a => a.name),
    style: estimate.style_class, visible: estimate.visible};
menu.isOpen = true;
const target = menu.target;
published.providers[1].agents[0].task_time_estimate.remaining_seconds = 240;
published.updatedAt = 'stale'; button._refresh();
const open = {text: estimate.text, sameActor: box.children[box.children.length - 1] === estimate,
    sameTarget: menu.target === target, stale: estimate.styles.includes('ams-stale')};
published = null; button._refresh();
const missing = {hidden: !estimate.visible, sameTarget: menu.target === target};
published = {updatedAt: 'fresh', providers: [{kind: 'codex', agents: [{phase: 'RUNNING', run_active: true,
    task_time_estimate: {status: 'estimated', remaining_seconds: 61}}]}]};
button._refresh();
const recovered = {text: estimate.text, visible: estimate.visible, stale: estimate.styles.includes('ams-stale')};
menu.isOpen = false; button._refresh();
const rebuilt = box.children[box.children.length - 1];
const closed = {text: rebuilt.text, newActor: rebuilt !== estimate};
published.providers = []; button._refresh();
const empty = box.children[box.children.length - 1];
emit(JSON.stringify({initial, open, missing, recovered, closed, emptyHidden: !empty.visible}));
"#,
    );
    assert_eq!(
        result,
        serde_json::json!({
            "initial": {"text": "~9m left", "preceding": ["codex", "rate", "attention"], "style": "ams-panel-estimate", "visible": true},
            "open": {"text": "~4m left", "sameActor": true, "sameTarget": true, "stale": true},
            "missing": {"hidden": true, "sameTarget": true},
            "recovered": {"text": "~2m left", "visible": true, "stale": false},
            "closed": {"text": "~2m left", "newActor": true}, "emptyHidden": true
        })
    );
    println!("None");
    println!("None");
}

/// Check per-workspace badge numbers precede global CLI IDs and monitor indexes.
/// Parameters: none.
/// Returns: None; assertions fail on incorrect ordering or invalid index acceptance.
#[test]
fn taskbar_window_position_ids_order_rows() {
    println!("parameters=none");
    assert!(
        gjs_available(),
        "gjs is required for taskbar window ordering checks"
    );
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let names = [
        "readWindowDisplayIds",
        "windowDisplaySortValue",
        "withWindowDisplayIds",
        "monitorSortValue",
        "workspaceSortValue",
        "compareWorkspaceEntries",
        "groupWorkspaceEntries",
    ];
    let snippets: Vec<String> = names
        .iter()
        .map(|name| extract_function(&source, name))
        .collect();
    let result = run_gjs(
        &snippets,
        r#"
const emit = print; print = value => {};
const TextDecoder = class {
 /** Decode headless file content. Parameters: value - saved JSON text. Returns: the same text. */
 decode(value) {print(`value=${value}`); print(value); return value;}
};
const file = {load_contents: () => [true, JSON.stringify({windows: [
 {note_id: 'note-10', number: 10, display_id: 0, desktop: 0},
 {note_id: 'note-1', number: 1, display_id: 9, desktop: 0},
 {note_id: 'note-2', number: '2', display_id: 8, desktop: 0},
 {note_id: 'negative', number: -1}, {note_id: 'fraction', number: 1.5},
 {note_id: 'zero', number: 0}, {note_id: 'null', number: null},
 {note_id: 'empty', number: ''}, {note_id: 'boolean', number: true},
 {note_id: 'missing'}, {number: 4}
]})]};
const ids = readWindowDisplayIds(file);
if (ids.size !== 3) throw Error('invalid index rows accepted');
const entries = [
 {provider: {label: 'A'}, agent: {title: 'A', session_id: 'note-10', monitor: 0, workspace: 1}},
 {provider: {label: 'Z'}, agent: {title: 'Z', note_id: 'note-1', session_id: 'different-runtime-id', monitor: 1, workspace: 1}},
 {provider: {label: 'B'}, agent: {title: 'B', session_id: 'note-2', monitor: 2, workspace: 1}},
 {provider: {label: 'A'}, agent: {title: 'unknown', session_id: 'unmapped', monitor: 0, workspace: 1}}
];
const payload = {providers: entries.map(entry => ({...entry.provider, agents: [entry.agent]}))};
const joined = withWindowDisplayIds(payload, ids);
const rows = joined.providers.map(provider => ({provider, agent: provider.agents[0]}));
const sorted = groupWorkspaceEntries(rows)[0].entries.map(entry => entry.agent.window_display_id);
if (JSON.stringify(sorted) !== '[1,2,10,null]') throw Error('wrong badge order: ' + JSON.stringify(sorted));
if ('window_display_id' in payload.providers[0].agents[0]) throw Error('input mutated');
payload.providers.reverse();
for (const provider of payload.providers) provider.agents[0].title = 'Changed title';
const refreshed = withWindowDisplayIds(payload, ids);
const refreshedRows = refreshed.providers.map(provider => ({provider, agent: provider.agents[0]}));
const refreshedOrder = groupWorkspaceEntries(refreshedRows)[0].entries.map(entry => entry.agent.window_display_id);
if (JSON.stringify(refreshedOrder) !== '[1,2,10,null]') throw Error('refresh reordered badges');
const moved = withWindowDisplayIds({providers: [{agents: [{session_id: 'note-1', workspace: 2}]}]}, ids);
if (moved.providers[0].agents[0].window_display_id !== null) throw Error('stale desktop joined');
for (const input of ['{', '{}', '{"windows":null}']) {
 if (readWindowDisplayIds({load_contents: () => [true, input]}).size) throw Error('invalid file accepted');
}
if (readWindowDisplayIds({load_contents: () => {throw Error('missing');}}).size) throw Error('missing file accepted');
emit(JSON.stringify({passed: true}));
"#,
    );
    assert_eq!(result["passed"], true);
    let refresh = extract_method(&source, "_refresh");
    assert!(refresh.contains("get_child('outline_window_index.json')"));
    assert!(!refresh.contains("cli_window_ids.json"));
    println!("None");
}

/// Check rendered monitor boundaries independently for every workspace.
/// Parameters: none.
/// Returns: None after assertions on rows, separators, and muted line styling.
#[test]
fn taskbar_monitor_separators_are_workspace_local() {
    println!("parameters=none");
    assert!(gjs_available(), "gjs is required for taskbar render checks");
    let source =
        fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let names = [
        "monitorSortValue",
        "workspaceSortValue",
        "windowDisplaySortValue",
        "compareWorkspaceEntries",
        "groupWorkspaceEntries",
    ];
    let mut snippets: Vec<String> = names
        .iter()
        .map(|name| extract_function(&source, name))
        .collect();
    snippets.push(extract_function(&source, "paintMonitorSeparator"));
    snippets.push(extract_function(&source, "buildMonitorSeparator"));
    snippets.push(format!(
        "const button = {{ {} }};",
        extract_method(&source, "_renderMenu")
    ));
    let result = run_gjs(
        &snippets,
        r#"
const emit = print; print = value => {};
let removedRules = 0; const drawings = [];
/** Build a headless menu actor. Parameters: text - heading, or null. Returns: a menu item. */
function MenuItem(text) {
 print(`text=${text}`);
 const result = {text, children: [], actor: {add_style_class_name: name => {}},
   label: {add_style_class_name: name => {}}, get_children: () => [],
   add_child: child => {result.drawing = child; drawings.push(child);}, addMenuItem: item => result.children.push(item)};
 print(result); return result;
}
/** Build a headless separator. Parameters: none. Returns: a separator menu item. */
function Separator() {
 print('parameters=none');
 const result = new MenuItem('separator');
 result.get_children = () => [{has_style_class_name: name => name === 'popup-separator-menu-item-separator',
   destroy: () => {removedRules++;}}];
 print(result); return result;
}
const Clutter = {ActorAlign: {CENTER: 0}};
const St = {DrawingArea: class {
 /** Create a headless area. Parameters: props - drawing properties. Returns: a new area. */
 constructor(props) {print(`props=${props}`); Object.assign(this, props); print(undefined);}
 /** Capture repaint hookup. Parameters: signal - signal name; callback - paint handler. Returns: undefined. */
 connect(signal, callback) {print(`signal=${signal} callback=${callback}`); this.callback = callback; print(undefined);}
}};
const PopupMenu = {PopupMenuItem: MenuItem, PopupMenuSection: MenuItem, PopupSeparatorMenuItem: Separator};
button.menu = new MenuItem('root');
button._agentMenuItem = (agent, provider, count) => new MenuItem('id-' + agent.window_display_id);
const agents = [
 {workspace: 5, monitor: 2, window_display_id: 8},
 {workspace: 5, monitor: 2, window_display_id: 5},
 {workspace: 5, monitor: 2, window_display_id: 7},
 {workspace: 5, monitor: 2, window_display_id: 6},
 {workspace: 5, monitor: 0, window_display_id: 4},
 {workspace: 5, monitor: 0, window_display_id: 2},
 {workspace: 5, monitor: 0, window_display_id: 3},
 {workspace: 5, monitor: 0, window_display_id: 1},
 {workspace: 6, monitor: 2, window_display_id: 4},
 {workspace: 6, monitor: 0, window_display_id: 3},
 {workspace: 6, monitor: 0, window_display_id: 1},
 {workspace: 6, monitor: 0, window_display_id: 2},
 {workspace: 3, monitor: 0, window_display_id: 13},
 {workspace: 4, monitor: null, window_display_id: null},
 {workspace: 4, monitor: -1, window_display_id: null},
];
button._renderMenu({providers: [{label: 'Codex', agents}], totalRunning: 0, totalLive: 15}, false, []);
const groups = button.menu.children.filter(item => item.children.length).map(item => item.children.map(row => row.text));
emit(JSON.stringify({groups, removedRules, expands: drawings.map(area => area.x_expand),
 paints: drawings.every(area => area.callback === paintMonitorSeparator)}));
"#,
    );
    assert_eq!(
        result["groups"],
        serde_json::json!([
            ["Workspace 3", "id-13"],
            ["Workspace 4", "id-null", "id-null"],
            [
                "Workspace 5",
                "id-1",
                "id-2",
                "id-3",
                "id-4",
                "separator",
                "id-5",
                "id-6",
                "id-7",
                "id-8"
            ],
            ["Workspace 6", "id-1", "id-2", "id-3", "separator", "id-4"]
        ])
    );
    assert_eq!(result["removedRules"], 2);
    assert_eq!(result["expands"], serde_json::json!([true, true]));
    assert_eq!(result["paints"], true);
    let css =
        fs::read_to_string(repo_root().join("acc-model-status@local/stylesheet.css")).unwrap();
    assert!(css.contains(".ams-monitor-separator"));
    assert!(css.contains("background-color: transparent;"));
    assert!(css.contains("padding-top: 2px;"));
    assert!(css.contains("padding-bottom: 2px;"));
    assert!(!css.contains(".ams-monitor-separator-label"));
    println!("None");
}

/// Verify the status menu lets normal Shell shortcuts through while open.
///
/// Parameters: none.
/// Returns: None.
#[test]
fn status_menu_hotkeys_remain_available_when_open() {
    println!("parameters=none");
    let source = fs::read_to_string(
        repo_root()
            .join("acc-model-status@local")
            .join("extension.js"),
    )
    .unwrap();
    let helper = extract_function(&source, "setStatusMenuActionMode");
    let result = run_gjs(&[helper], r#"
const traces = [];
const print = (...values) => traces.push(values);
const Shell = {ActionMode: {NORMAL: 1, POPUP: 2}};
const manager = {_grabParams: {actionMode: Shell.ActionMode.POPUP}};
setStatusMenuActionMode(manager, true, Shell.ActionMode.POPUP);
const openMode = manager._grabParams.actionMode;
setStatusMenuActionMode(manager, false, Shell.ActionMode.POPUP);
globalThis.print(JSON.stringify({openMode, closedMode: manager._grabParams.actionMode, traces}));
"#);
    assert_eq!(result["openMode"], 3);
    assert_eq!(result["closedMode"], 2);
    assert!(source.contains(
        "setStatusMenuActionMode(menuManager, isOpen, this._statusMenuActionMode)"
    ));
    assert!(source.find("this.menu.connect('open-state-changed'").unwrap()
        < source.find("Main.panel.addToStatusArea('acc-model-status'").unwrap());
    println!("{:?}", ());
}

/// Check the actual paint callback spans arbitrary allocated widths with gaps.
/// Parameters: none.
/// Returns: None after width, dash pattern, colour and context disposal assertions.
#[test]
fn taskbar_dashed_separator_paints_full_allocated_width() {
    println!("parameters=none");
    let source = fs::read_to_string(repo_root().join("acc-model-status@local/extension.js")).unwrap();
    let result = run_gjs(&[extract_function(&source, "paintMonitorSeparator")], r#"
const emit = print; print = value => {};
const samples = [];
for (const width of [200, 460, 780]) {
 const calls = [];
 const context = {
  setSourceRGBA: (...args) => calls.push(['color', ...args]),
  setLineWidth: value => calls.push(['thickness', value]),
  setDash: (pattern, offset) => calls.push(['dash', pattern, offset]),
  moveTo: (x, y) => calls.push(['start', x, y]),
  lineTo: (x, y) => calls.push(['end', x, y]),
  stroke: () => calls.push(['stroke']), $dispose: () => calls.push(['dispose'])
 };
 paintMonitorSeparator({get_surface_size: () => [width, 1], get_context: () => context});
 samples.push(calls);
}
emit(JSON.stringify(samples));
"#);
    for (sample, width) in result.as_array().unwrap().iter().zip([200, 460, 780]) {
        assert_eq!(sample, &serde_json::json!([
            ["color", 0.63, 0.63, 0.63, 0.55], ["thickness", 1], ["dash", [6, 4], 0],
            ["start", 0, 0.5], ["end", width, 0.5], ["stroke"], ["dispose"]
        ]));
    }
    println!("None");
}
