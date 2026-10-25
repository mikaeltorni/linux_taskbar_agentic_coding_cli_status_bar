//! Deployment and extension-source contracts formerly covered by pytest.
//!
//! These checks keep the installer, systemd unit, and ACC model-status
//! extension in lockstep without a Python runtime.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|exc| {
        panic!("read {rel}: {exc}");
    })
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

fn quoted_array(text: &str, name: &str) -> Vec<String> {
    let pattern = format!(r"(?s){name}=\((.*?)\)");
    let re = Regex::new(&pattern).unwrap();
    let block = re
        .captures(text)
        .unwrap_or_else(|| panic!("{name} array not found"))
        .get(1)
        .unwrap()
        .as_str();
    Regex::new(r#""([^"]+)""#)
        .unwrap()
        .captures_iter(block)
        .map(|c| c[1].to_string())
        .collect()
}

fn code_only(text: &str) -> String {
    let no_block = Regex::new(r"(?s)/\*.*?\*/").unwrap().replace_all(text, "");
    Regex::new(r"(?m)^\s*//.*$")
        .unwrap()
        .replace_all(&no_block, "")
        .into_owned()
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let name = entry.file_name();
        if name == ".git"
            || name == "target"
            || name == "dist"
            || name == ".cargo-container"
            || name == ".log"
        {
            continue;
        }
        if path.is_dir() {
            walk_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[test]
/// Keep the shipped application runtime native while allowing isolated tests and generated installer support.
/// Parameters: none. Returns: None.
fn no_python_runtime_remains() {
    println!("parameters=none");
    let root = repo_root();
    let mut files = Vec::new();
    walk_files(&root, &mut files);
    let python: Vec<_> = files
        .iter()
        .filter(|path| {
            let relative = path.strip_prefix(&root).unwrap();
            !relative.starts_with("tests")
                && !relative.starts_with("tmp")
                && !relative.starts_with(".installer-support")
                && path.extension().and_then(|extension| extension.to_str()) == Some("py")
        })
        .cloned()
        .collect();
    assert!(python.is_empty(), "Application Python files remain: {python:?}");
    println!("None");
}

#[test]
fn installer_has_no_python_runtime() {
    for rel in [
        "lib/status_setup.sh",
        "lib/acs_bin.sh",
        "install.sh",
        "systemd/agentic-cli-status.service",
        "tests/test_install.sh",
    ] {
        let text = read(rel);
        assert!(
            !text.contains("python3 -")
                && !text.contains("python3 -c")
                && !text.contains("python3 -m"),
            "{rel} still invokes python3"
        );
        assert!(!text.contains("PYTHONPATH"), "{rel} still sets PYTHONPATH");
    }
}

#[test]
fn systemd_unit_starts_the_rust_binary() {
    let unit = read("systemd/agentic-cli-status.service");
    assert!(unit.contains("ExecStart=%h/.local/bin/agentic-cli-status --daemon"));
    assert!(unit.contains("EnvironmentFile=-%h/.config/agentic-cli-status/.env"));
    assert!(unit.contains("StandardError=journal"));
    assert!(!unit.contains("python3"));
    assert!(!unit.contains("PYTHONPATH"));
}

#[test]
/// Keep native deployment and separate read-only query routing.
/// Parameters: none. Returns: None.
fn installer_deploys_the_rust_binary() {
    println!("parameters=none");
    let setup = read("lib/status_setup.sh");
    assert!(setup.contains("source \"$SCRIPT_DIR/lib/acs_bin.sh\""));
    assert!(setup.contains("acs_bin config-set"));
    assert!(setup.contains("acs_query config-get"));
    assert!(setup.contains("acs_bin config-init"));
    assert!(setup.contains("ensure_acs_bin"));
    assert!(setup.contains(".local/bin/agentic-cli-status"));
    assert!(!setup.contains("ACS_PACKAGE_FILES"));
    println!("None");
}

#[test]
fn installer_delegates_readonly_routing() {
    let text = read("install.sh");
    assert!(text.contains("isc_route_readonly_args"));
    assert!(!text.contains("--list-components|--export-selection|--detect"));
    let routing = text.split("isc_route_readonly_args").next().unwrap();
    let tail = routing.rsplit("main() {").next().unwrap();
    assert!(tail.matches("install_agentic_status_core").count() <= 1);
}

#[test]
fn master_installer_detects_every_readonly_flag() {
    let text = read("install.sh");
    assert!(text.contains("isc_route_readonly_args"));
    for flag in [
        "--list-components",
        "--list-configurable-components",
        "--list-select-configure-components",
        "--list-component-config-values",
        "--configure-component",
        "--export-selection",
        "--detect",
        "--uninstall",
    ] {
        assert!(
            text.contains(flag) || text.contains("isc_route_readonly_args"),
            "missing {flag}"
        );
    }
}

#[test]
fn model_status_copy_list_matches_extension_files() {
    let setup = read("lib/status_setup.sh");
    let listed = quoted_array(&setup, "ACS_MODEL_STATUS_FILES");
    let ext = repo_root().join("acc-model-status@local");
    let mut on_disk = Vec::new();
    walk_files(&ext, &mut on_disk);
    let mut rel: Vec<String> = on_disk
        .iter()
        .map(|p| {
            p.strip_prefix(&ext)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rel.sort();
    let mut listed = listed;
    listed.sort();
    assert_eq!(listed, rel);
}

#[test]
fn model_status_icons_are_symbolic_svgs() {
    let setup = read("lib/status_setup.sh");
    let listed = quoted_array(&setup, "ACS_MODEL_STATUS_FILES");
    let icons: Vec<_> = listed
        .into_iter()
        .filter(|name| name.starts_with("icons/"))
        .collect();
    let expected = [
        "icons/acc-provider-api-symbolic.svg",
        "icons/acc-provider-claude-symbolic.svg",
        "icons/acc-provider-cline-symbolic.svg",
        "icons/acc-provider-codex-symbolic.svg",
        "icons/acc-provider-generic-symbolic.svg",
        "icons/acc-provider-grok-symbolic.svg",
        "icons/acc-provider-nvidia-symbolic.svg",
        "icons/acc-provider-opencode-symbolic.svg",
        "icons/acc-provider-openrouter-symbolic.svg",
        "icons/acc-provider-qwen-symbolic.svg",
    ];
    assert_eq!(icons, expected);
    let icon_re = Regex::new(r"^icons/acc-provider-[a-z0-9]+-symbolic\.svg$").unwrap();
    for icon in &icons {
        assert!(icon_re.is_match(icon), "{icon}");
        let svg = read(&format!("acc-model-status@local/{icon}"));
        assert!(svg.contains("<svg"), "{icon}");
    }
}

#[test]
fn model_status_component_is_optional() {
    let components = read("installer/components.sh");
    let re = Regex::new(r#""(model_status_bar\|[^"]+)""#).unwrap();
    let entry = re
        .captures(&components)
        .unwrap_or_else(|| panic!("model_status_bar entry missing"))
        .get(1)
        .unwrap()
        .as_str();
    let fields: Vec<_> = entry.split('|').collect();
    assert_eq!(fields[0], "model_status_bar");
    assert_eq!(fields[2], "off");
    assert_eq!(fields[6], "Taskbar extensions");
    let setup = read("lib/status_setup.sh");
    for func in [fields[3], fields[4], fields[5]] {
        assert!(setup.contains(&format!("{func}() {{")), "missing {func}");
    }
}

#[test]
fn claude_code_monitor_is_opt_in_without_removing_install_hooks() {
    let components = read("installer/components.sh");
    let re = Regex::new(r#""(claude_code_monitor\|[^"]+)""#).unwrap();
    let entry = re
        .captures(&components)
        .unwrap_or_else(|| panic!("claude_code_monitor entry missing"))
        .get(1)
        .unwrap()
        .as_str();
    let fields: Vec<_> = entry.split('|').collect();
    assert_eq!(fields[0], "claude_code_monitor");
    assert_eq!(fields[2], "off");

    let setup = read("lib/status_setup.sh");
    for function in [
        "acs_install_claude_code",
        "acs_detect_claude_code",
        "acs_uninstall_claude_code",
        "acs_configure_claude_code",
        "acs_claude_code_status",
    ] {
        assert!(
            setup.contains(&format!("{function}()")),
            "missing {function}"
        );
    }

    let config: Value = serde_json::from_str(&read("installation_configs/default.json")).unwrap();
    assert_eq!(config["components"]["claude_code_monitor"]["on"], 0);
}

#[test]
fn model_status_extension_contract() {
    let js = read("acc-model-status@local/extension.js");
    let code = code_only(&js);
    assert!(code.contains("model_status_bar.json"));
    assert!(code.contains("CODEX_AGENT_TRACKER_STATE_DIR"));
    for forbidden in ["GLib.spawn", "Soup", "subprocess"] {
        assert!(!code.contains(forbidden), "{forbidden}");
    }
    assert!(code.contains("Gio.Subprocess.new"));
    assert!(code.contains("this._focusAgent(noteId)"));
    for required in [
        "global.workspace_manager.get_active_workspace()",
        "global.get_window_actors()",
        "win.get_workspace()",
        "win.get_monitor()",
        "win.located_on_workspace(activeWorkspace)",
        "win.get_title()",
        "scopePayloadToWindows(payload, windows)",
        "monitor: match.monitor",
        "workspace: match.workspace",
        "current_workspace: match.current",
        "live: agents.length",
        "const running = agents.filter(agent =>",
        "const attention = agents.filter(agent =>",
        "providers.push({...provider, agents, live: agents.length, running, attention})",
        "groupWorkspaceEntries(workspaceEntries)",
        "orderedGroups.sort((left, right) => workspaceSortValue(left.workspace) -",
        "for (const entry of group.entries)",
        "Workspace ${workspace}",
        "current ? ' — current' : ''",
        "new PopupMenu.PopupMenuSection()",
        "workspaceSection.addMenuItem(heading)",
        "workspaceSection.addMenuItem(this._agentMenuItem",
        "heading.actor.add_style_class_name('ams-current-workspace-block')",
        "this._renderHarnessRates(payload)",
        "function formatCompactTokens(value)",
        "function formatHarnessRateText(row)",
        "function harnessHasMinuteActivity(row)",
        "function harnessRateLines(payload)",
        "function formatHarnessPanelRateText(row)",
        "function panelHarnessRateRows(payload)",
        "this._renderPanelHarnessRates(payload",
        "style_class: 'ams-rate'",
        "Harness token rates",
        "parsed.harness_rates",
        "this._shellSignals",
        "this._windowSignals",
    ] {
        assert!(code.contains(required), "{required}");
    }
    assert!(
        !code
            .contains("workspaceSection.actor.add_style_class_name('ams-current-workspace-block')"),
        "current-workspace highlight must not cover agent rows or instance tags"
    );
    for signal in [
        "active-workspace-changed",
        "window-created",
        "workspace-changed",
        "position-changed",
        "notify::title",
        "unmanaged",
    ] {
        assert!(code.contains(&format!("'{signal}'")), "{signal}");
    }
    let install = read("install.sh");
    assert!(install.contains("MODEL_STATUS_UUID=\"acc-model-status@local\""));
    assert!(install.contains("MODEL_STATUS_SRC=\"$SCRIPT_DIR/acc-model-status@local\""));
    let setup = read("lib/status_setup.sh");
    let core = Regex::new(r"install_agentic_status_core\(\) \{(?s)(.*?)\n\}")
        .unwrap()
        .captures(&setup)
        .unwrap();
    assert!(!core.get(1).unwrap().as_str().contains("MODEL_STATUS"));
}

#[test]
fn model_status_menu_hotkey_is_registered_and_released() {
    let js = code_only(&read("acc-model-status@local/extension.js"));
    for required in [
        "this.getSettings()",
        "Main.wm.addKeybinding(",
        "Shell.ActionMode.ALL",
        "this._button.menu.toggle()",
        "Main.wm.removeKeybinding('menu-accelerator')",
        "menu-accelerator",
        "org.gnome.desktop.wm.keybindings",
        "const SWITCH_GROUP_KEY = 'switch-group';",
        "set_strv(SWITCH_GROUP_KEY",
    ] {
        assert!(js.contains(required), "{required}");
    }
}

#[test]
fn panel_running_count_is_left_of_icon() {
    let js = read("acc-model-status@local/extension.js");
    let empty = extract_method(&js, "_renderEmpty");
    let provider = extract_method(&js, "_renderProvider");
    let orphans = extract_method(&js, "_renderPanelHarnessRates");
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
fn model_status_metadata_and_styles() {
    let metadata: Value =
        serde_json::from_str(&read("acc-model-status@local/metadata.json")).unwrap();
    assert_eq!(metadata["uuid"], "acc-model-status@local");
    assert_eq!(metadata["version"], 15);
    let settings_schema = "org.gnome.shell.extensions.acc-model-status";
    assert_eq!(metadata["settings-schema"], settings_schema);
    let schema = read(
        "acc-model-status@local/schemas/org.gnome.shell.extensions.acc-model-status.gschema.xml",
    );
    assert!(schema.contains(&format!("id=\"{settings_schema}\"")));
    let versions = metadata["shell-version"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();
    assert!(versions.contains(&"48"), "{versions:?}");
    let css = read("acc-model-status@local/stylesheet.css");
    let js = read("acc-model-status@local/extension.js");
    for style in [
        "ams-box",
        "ams-segment",
        "ams-count",
        "ams-active",
        "ams-idle",
        "ams-attention",
        "ams-empty",
        "ams-stale",
        "ams-harness-rates",
        "ams-harness-rate",
        "ams-rate",
    ] {
        assert!(css.contains(&format!(".{style}")), "{style}");
        assert!(js.contains(style), "{style}");
    }
    let current = Regex::new(r"(?s)\.ams-current-workspace-block\s*\{(.*?)\}")
        .unwrap()
        .captures(&css)
        .unwrap();
    assert!(Regex::new(r"background-color:\s*rgba\([^;]+,\s*0\.35\)")
        .unwrap()
        .is_match(&current[1]));
}

/// Verify the shared palette and menu row presentation.
/// Parameters: none.
/// Returns: None.
#[test]
fn model_status_phase_palette_and_rows() {
    println!("parameters=none");
    let js = read("acc-model-status@local/extension.js");
    let palette_block = Regex::new(r"const PHASE_COLORS = \{(?s)(.*?)\n\};")
        .unwrap()
        .captures(&js)
        .unwrap();
    let pairs: Vec<(String, String)> = Regex::new(r"(\w+):\s*'#([0-9a-f]{6})'")
        .unwrap()
        .captures_iter(&palette_block[1])
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    let expected = [
        ("RUNNING", "e6c200"),
        ("ATTN", "ffb454"),
        ("DONE", "22c55e"),
        ("DRAFT", "84c32f"),
        ("STOPPED", "ef4444"),
        ("INTERRUPTED", "ef4444"),
        ("TRANSFERRED", "c45c5c"),
        ("UNKNOWN", "7f8ea3"),
    ];
    for (phase, color) in expected {
        assert!(
            pairs.iter().any(|(p, c)| p == phase && c == color),
            "{phase}"
        );
    }
    let row = Regex::new(
        r"    _agentMenuItem\(agent, provider, codexInstanceCount, track = true\) \{(?s)(.*?)\n    \}",
    )
    .unwrap()
    .captures(&js)
    .unwrap();
    let body = &row[1];
    assert!(body.contains("agent.project_tag"));
    assert!(body.contains("ams-project-tag"));
    assert!(!body.contains("`W${workspace}`"));
    assert!(!body.contains("ams-workspace"));
    assert!(body.find("ams-project-tag").unwrap() < body.find("ams-phase").unwrap());
    assert!(body.contains("gicon: this._icon(provider.icon)"));
    assert!(body.contains("ams-menu-provider-icon"));
    assert!(body.contains("codexInstanceLabel(agent, codexInstanceCount)"));
    assert!(!body.contains("text: `${provider.label"));
    assert!(!body.contains("ams-current-workspace-block"));
    assert!(body.contains("agentPhaseBadgeText(agent)"));
    assert!(body.contains("agentPhaseBadgeColor(agent)"));
    assert!(!body.contains("text: phase"));
    println!("None");
}

#[test]
/// Check every panel timer is released during teardown.
/// Parameters: none.
/// Returns: None after teardown contract assertions pass.
fn model_status_destroy_releases_timers() {
    println!("parameters=none");
    let js = read("acc-model-status@local/extension.js");
    let destroy = Regex::new(r"    destroy\(\) \{(?s)(.*?)\n    \}")
        .unwrap()
        .captures(&js)
        .unwrap();
    let body = &destroy[1];
    assert_eq!(body.matches("GLib.source_remove").count(), 4);
    for timer in ["_panelPulseId", "_refreshId", "_pollId", "_runClockId"] {
        assert!(body.contains(&format!("GLib.source_remove(this.{timer})")));
    }
    assert!(body.contains("this._monitor.cancel()"));
    assert!(body.contains("object.disconnect(id)"));
    assert!(body.contains("win.disconnect(id)"));
    assert!(body.contains("this._windowSignals.clear()"));
    assert!(body.contains("super.destroy()"));
    println!("None");
}

#[test]
fn crate_layout_is_the_poller() {
    assert!(repo_root().join("src/main.rs").is_file());
    assert!(repo_root().join("src/poller.rs").is_file());
    assert!(repo_root().join("Cargo.toml").is_file());
    assert!(repo_root().join("Cargo.lock").is_file());
    assert!(repo_root()
        .join("scripts/build_agentic_cli_status.sh")
        .is_file());
}

fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (message.len() as u64).saturating_mul(8);
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn codex_provider_mark_is_a_blossom() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let svg = read("acc-model-status@local/icons/acc-provider-codex-symbolic.svg");
    assert!(svg.contains("<svg"), "shipped codex icon");
    assert!(svg.contains("viewBox=\"0 0 16 16\""));
    assert!(svg.contains("fill=\"#2e3436\""));
    assert_eq!(svg.matches("fill=").count(), 1);
    assert!(!svg.contains("gradient"));
    assert!(!svg.contains("<image"));
    assert!(!svg.contains("M8 1.1 14 4.5 14 11.5 8 14.9 2 11.5 2 4.5"));
    let path = Regex::new(r#"d="([^"]+)""#)
        .unwrap()
        .captures(&svg)
        .unwrap()
        .get(1)
        .unwrap()
        .as_str();
    assert!(
        path.contains('C') || path.contains('A') || path.contains('Q'),
        "blossom path needs curves"
    );

    let unchanged = [
        (
            "acc-provider-api-symbolic.svg",
            "9e30e7bda5e9feeeeff5cdacf44a4d19ae77530e341fb936f5e101caab9162a4",
        ),
        (
            "acc-provider-claude-symbolic.svg",
            "047ed1dbf92a2952804c918299a37f996ee5be88a2bd4b66a3c3d90ae27e8e32",
        ),
        (
            "acc-provider-cline-symbolic.svg",
            "983bdb9a146aca0f7ccb1b5a54c1570eac9fb7aed8effd047608e69c2dd1b1af",
        ),
        (
            "acc-provider-generic-symbolic.svg",
            "7e03c49edc8f2093036ce4a46e468a21f7bf3654b72c8b18ee7d888fa515c83c",
        ),
        (
            "acc-provider-grok-symbolic.svg",
            "25f3b14b3574f3d7e7c893407d1486e8d09ab24edfe02b2a92961eaef5c3d680",
        ),
        (
            "acc-provider-nvidia-symbolic.svg",
            "1f80e7eb796407caac79cd7534cdcbcdf0bf3451269c286101d09e902f01aef6",
        ),
        (
            "acc-provider-opencode-symbolic.svg",
            "9ed9e4d853c448289817bceacb517ee4f28296b2c458dba2924f37a50a3503b1",
        ),
        (
            "acc-provider-openrouter-symbolic.svg",
            "3d8dae7fc46e1f37fc09f31cb23dbe621c4906058f4052223310c320f8018087",
        ),
        (
            "acc-provider-qwen-symbolic.svg",
            "7ae51a1d7fb6b67ebb31abd2eb4c2df2cea4faeffbc8a5b654a9edcf623fdcf9",
        ),
    ];
    for (name, expected) in unchanged {
        let bytes = fs::read(repo_root().join("acc-model-status@local/icons").join(name)).unwrap();
        assert_eq!(sha256_hex(&bytes), expected, "{name} changed");
    }
}
