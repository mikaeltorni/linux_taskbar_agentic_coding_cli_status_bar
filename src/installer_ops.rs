//! Installer-facing JSON helpers that replace the former Python snippets.
//!
//! These keep stdout machine-readable for `acs_config_get` / credential
//! probes and never print secrets.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::{json, Value};

use crate::config::{
    config_path, format_config_value, get_dotted, load_config, save_config, set_dotted,
};
use crate::logging;
use crate::openrouter_client::management_api_key;

/// Seed the default config when the file is missing.
pub fn config_init() -> i32 {
    let path = config_path(None);
    if !path.exists() {
        if let Err(exc) = save_config(&load_config(None), None) {
            logging::error(format!("config-init failed: {exc}"));
            eprintln!("config-init failed: {exc}");
            return 1;
        }
        logging::info(format!("seeded default config at {}", path.display()));
    }
    0
}

/// Print one dotted config key's effective value (installer contract).
pub fn config_get(key_path: &str) -> i32 {
    let config = load_config(None);
    match get_dotted(&config, key_path) {
        Some(value) => {
            print!("{}", format_config_value(&value));
            0
        }
        None => {
            eprintln!("missing config key {key_path}");
            1
        }
    }
}

/// Set one dotted config key from a JSON value and persist.
pub fn config_set(key_path: &str, value_json: &str) -> i32 {
    let parsed: Value = match serde_json::from_str(value_json) {
        Ok(v) => v,
        Err(exc) => {
            eprintln!("invalid JSON value: {exc}");
            return 1;
        }
    };
    logging::info(format!("config set {key_path}={value_json}"));
    let mut config = load_config(None);
    set_dotted(&mut config, key_path, parsed);
    if let Err(exc) = save_config(&config, None) {
        eprintln!("config-set failed: {exc}");
        return 1;
    }
    0
}

/// Exit 0 when a management API key is present.
pub fn has_openrouter_key(home: Option<&Path>) -> i32 {
    if management_api_key(home).is_some() {
        0
    } else {
        1
    }
}

/// Add `opencode-go-usage` to opencode.json plugin list.
pub fn register_opencode_plugin(path: &Path) -> i32 {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut config = if path.exists() {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    if !config.is_object() {
        config = json!({});
    }
    let plugins = config
        .as_object_mut()
        .unwrap()
        .entry("plugin")
        .or_insert_with(|| json!([]));
    if let Some(list) = plugins.as_array_mut() {
        let already = list.iter().any(|p| p.as_str() == Some("opencode-go-usage"));
        if !already {
            list.push(json!("opencode-go-usage"));
        }
    }
    let mut text = serde_json::to_string_pretty(&config).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    if let Err(exc) = fs::write(path, text) {
        eprintln!("register-opencode-plugin failed: {exc}");
        return 1;
    }
    logging::info(format!(
        "registered opencode-go-usage in {}",
        path.display()
    ));
    0
}

/// Write workspace + cookie into the shared OpenCode Go config.
pub fn write_opencode_creds(path: &Path, workspace: &str, cookie: &str) -> i32 {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut existing = if path.exists() {
        fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    if !existing.is_object() {
        existing = json!({});
    }
    existing["workspaceId"] = json!(workspace);
    existing["authCookie"] = json!(cookie);
    if existing.get("refreshIntervalMinutes").is_none() {
        existing["refreshIntervalMinutes"] = json!(5);
    }
    if existing.get("showAtSessionStart").is_none() {
        existing["showAtSessionStart"] = json!(true);
    }
    let mut text = serde_json::to_string_pretty(&existing).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    if let Err(exc) = fs::write(path, text) {
        eprintln!("write-opencode-creds failed: {exc}");
        return 1;
    }
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    logging::info(format!(
        "wrote OpenCode Go credentials to {}",
        path.display()
    ));
    0
}

fn opencode_creds_ok(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let workspace = data
        .get("workspaceId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let cookie = data.get("authCookie").and_then(Value::as_str).unwrap_or("");
    workspace.starts_with("wrk_")
        && cookie != "PASTE_YOUR_AUTH_COOKIE_HERE"
        && !cookie.is_empty()
        && cookie.len() >= 20
}

/// Exit 0 when workspace + auth cookie are set.
pub fn opencode_creds_valid(path: &Path) -> i32 {
    if opencode_creds_ok(path) {
        0
    } else {
        1
    }
}

/// Print the stored workspace ID, if any.
pub fn opencode_workspace_id(path: &Path) -> i32 {
    let Ok(text) = fs::read_to_string(path) else {
        return 0;
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    if let Some(id) = data.get("workspaceId").and_then(Value::as_str) {
        print!("{id}");
    }
    0
}
