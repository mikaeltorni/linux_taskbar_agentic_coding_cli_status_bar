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
fn no_python_runtime_remains() {
    let mut files = Vec::new();
    walk_files(&repo_root(), &mut files);
    let python: Vec<_> = files
        .iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("py"))
        .cloned()
        .collect();
    assert!(python.is_empty(), "Python files remain: {python:?}");
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
fn installer_deploys_the_rust_binary() {
    let setup = read("lib/status_setup.sh");
    assert!(setup.contains("source \"$SCRIPT_DIR/lib/acs_bin.sh\""));
    assert!(setup.contains("acs_bin config-set"));
    assert!(setup.contains("acs_bin config-get"));
    assert!(setup.contains("acs_bin config-init"));
    assert!(setup.contains("ensure_acs_bin"));
    assert!(setup.contains(".local/bin/agentic-cli-status"));
    assert!(!setup.contains("ACS_PACKAGE_FILES"));
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
    for forbidden in ["GLib.spawn", "Gio.Subprocess", "Soup", "subprocess"] {
        assert!(!code.contains(forbidden), "{forbidden}");
    }
    for required in [
        "global.workspace_manager.get_active_workspace()",
        "global.get_window_actors()",
        "win.get_workspace()",
        "win.located_on_workspace(activeWorkspace)",
        "win.get_title()",
        "scopePayloadToWindows(payload, windows)",
        "workspace: match.workspace",
        "current_workspace: match.current",
        "live: agents.length",
        "const running = agents.filter(agent =>",
        "const attention = agents.filter(agent =>",
        "providers.push({...provider, agents, live: agents.length, running, attention})",
        "workspaceEntries.sort(compareWorkspaceEntries)",
        "Workspace ${workspace}",
        "current ? ' — current' : ''",
        "new PopupMenu.PopupMenuSection()",
        "workspaceSection.addMenuItem(heading)",
        "workspaceSection.addMenuItem(this._agentMenuItem",
        "workspaceSection.actor.add_style_class_name('ams-current-workspace-block')",
        "this._shellSignals",
        "this._windowSignals",
    ] {
        assert!(code.contains(required), "{required}");
    }
    for signal in [
        "active-workspace-changed",
        "window-created",
        "workspace-changed",
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
fn model_status_metadata_and_styles() {
    let metadata: Value =
        serde_json::from_str(&read("acc-model-status@local/metadata.json")).unwrap();
    assert_eq!(metadata["uuid"], "acc-model-status@local");
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

#[test]
fn model_status_phase_palette_and_rows() {
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
        ("INTERRUPTED", "c084fc"),
        ("TRANSFERRED", "c45c5c"),
        ("UNKNOWN", "7f8ea3"),
    ];
    for (phase, color) in expected {
        assert!(
            pairs.iter().any(|(p, c)| p == phase && c == color),
            "{phase}"
        );
    }
    let row = Regex::new(r"    _agentMenuItem\(agent, provider\) \{(?s)(.*?)\n    \}")
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
    assert!(!body.contains("text: `${provider.label"));
    assert!(!body.contains("ams-current-workspace-block"));
    assert!(body.contains("phaseColor(phase)"));
}

#[test]
fn model_status_destroy_releases_timers() {
    let js = read("acc-model-status@local/extension.js");
    let destroy = Regex::new(r"    destroy\(\) \{(?s)(.*?)\n    \}")
        .unwrap()
        .captures(&js)
        .unwrap();
    let body = &destroy[1];
    assert_eq!(body.matches("GLib.source_remove").count(), 3);
    assert!(body.contains("this._monitor.cancel()"));
    assert!(body.contains("object.disconnect(id)"));
    assert!(body.contains("win.disconnect(id)"));
    assert!(body.contains("this._windowSignals.clear()"));
    assert!(body.contains("super.destroy()"));
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
