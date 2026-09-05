//! Configuration for the agentic CLI status poller.
//!
//! The single JSON config file lives at
//! `${XDG_CONFIG_HOME:-~/.config}/agentic-cli-status/config.json` and is owned
//! by the installer components. The poller re-reads it on every tick so
//! reconfiguration never requires a restart.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::logging;

pub const CONFIG_DIR_NAME: &str = "agentic-cli-status";
pub const CONFIG_FILE_NAME: &str = "config.json";

/// CLI ids in display order. Cline is intentionally absent.
pub const CLI_IDS: &[&str] = &["codex", "claude_code", "grok", "opencode_go", "openrouter"];

pub const DEFAULT_POLL_INTERVAL_SECONDS: i64 = 5;
pub const MIN_POLL_INTERVAL_SECONDS: i64 = 5;
pub const MAX_POLL_INTERVAL_SECONDS: i64 = 3600;
pub const MIN_STALE_AFTER_SECONDS: i64 = 60;

pub const CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS: i64 = 180;
pub const MIN_CLAUDE_ENDPOINT_INTERVAL_SECONDS: i64 = 60;

pub const OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS: i64 = 300;
pub const MIN_OPENCODE_GO_FETCH_INTERVAL_SECONDS: i64 = 300;

pub const OPENROUTER_FETCH_MIN_INTERVAL_SECONDS: i64 = 60;
pub const MIN_OPENROUTER_FETCH_INTERVAL_SECONDS: i64 = 60;
pub const OPENROUTER_DAILY_LIMIT_DEFAULT: i64 = 1000;
pub const MIN_OPENROUTER_DAILY_LIMIT: i64 = 1;
pub const MAX_OPENROUTER_DAILY_LIMIT: i64 = 100_000;
pub const OPENROUTER_MODEL_SLUG_DEFAULT: &str = "stealth/ox-alpha";

const VALID_SOURCES: &[&str] = &["auto", "headless", "tmux"];

/// Slash command each CLI's TUI uses to display rate limits/usage.
pub fn slash_command(cli_id: &str) -> Option<&'static str> {
    match cli_id {
        "codex" | "claude_code" | "grok" => Some("/usage"),
        "opencode_go" => Some("/ogc-usage"),
        _ => None,
    }
}

/// Return the default configuration object.
pub fn default_config() -> Value {
    json!({
        "poll_interval_seconds": DEFAULT_POLL_INTERVAL_SECONDS,
        "tmux": {
            "enabled": true,
            "session": "agentic-cli-status",
        },
        "clis": {
            "codex": {
                "enabled": true,
                "source": "auto",
                "stale_after_seconds": 1800,
            },
            "claude_code": {
                "enabled": true,
                "source": "auto",
                "stale_after_seconds": 1800,
                "endpoint_min_interval_seconds": CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS,
            },
            "grok": {
                "enabled": true,
                "source": "auto",
                "stale_after_seconds": 1800,
            },
            "opencode_go": {
                "enabled": false,
                "source": "auto",
                "stale_after_seconds": 1800,
                "fetch_min_interval_seconds": OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS,
            },
            // Opt-in: OpenRouter needs a management API key and a chosen
            // model slug, so the OR segment stays off (and hidden) until the
            // user enables it through the installer or config.
            "openrouter": {
                "enabled": false,
                "source": "headless",
                "stale_after_seconds": 1800,
                "fetch_min_interval_seconds": OPENROUTER_FETCH_MIN_INTERVAL_SECONDS,
                "model_slug": OPENROUTER_MODEL_SLUG_DEFAULT,
                "daily_limit": OPENROUTER_DAILY_LIMIT_DEFAULT,
            },
        },
    })
}

/// Return the config file path, honoring `XDG_CONFIG_HOME`.
///
/// `home` is used when `XDG_CONFIG_HOME` is unset (tests pass an override).
pub fn config_path(home: Option<&Path>) -> PathBuf {
    if let Ok(base) = std::env::var("XDG_CONFIG_HOME") {
        if !base.is_empty() {
            return PathBuf::from(base)
                .join(CONFIG_DIR_NAME)
                .join(CONFIG_FILE_NAME);
        }
    }
    let root = home
        .map(Path::to_path_buf)
        .unwrap_or_else(home_dir)
        .join(".config");
    root.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME)
}

/// Return the state directory, honoring `XDG_STATE_HOME`.
pub fn state_dir(home: Option<&Path>) -> PathBuf {
    if let Ok(base) = std::env::var("XDG_STATE_HOME") {
        if !base.is_empty() {
            return PathBuf::from(base).join(CONFIG_DIR_NAME);
        }
    }
    let root = home
        .map(Path::to_path_buf)
        .unwrap_or_else(home_dir)
        .join(".local")
        .join("state");
    root.join(CONFIG_DIR_NAME)
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn merge_defaults(base: &Value, override_val: &Value) -> Value {
    match (base, override_val) {
        (Value::Object(base_map), Value::Object(over_map)) => {
            let mut merged = base_map.clone();
            for (key, value) in over_map {
                if let Some(existing) = merged.get(key) {
                    if existing.is_object() && value.is_object() {
                        merged.insert(key.clone(), merge_defaults(existing, value));
                        continue;
                    }
                }
                merged.insert(key.clone(), value.clone());
            }
            Value::Object(merged)
        }
        (_, over) => over.clone(),
    }
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| u as i64)),
        _ => None,
    }
}

fn cli_object<'a>(config: &'a mut Value, cli_id: &str) -> &'a mut Map<String, Value> {
    config
        .get_mut("clis")
        .and_then(Value::as_object_mut)
        .expect("defaults always have clis")
        .entry(cli_id)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("cli entry is an object")
}

/// Load the config file merged over defaults, validating key ranges.
pub fn load_config(path: Option<&Path>) -> Value {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_path(None));
    let raw = match fs::read_to_string(&target) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => Value::Object(map),
            Ok(_) => {
                logging::error(format!(
                    "unreadable config {} (config root is not an object); using defaults",
                    target.display()
                ));
                json!({})
            }
            Err(exc) => {
                logging::error(format!(
                    "unreadable config {} ({exc}); using defaults",
                    target.display()
                ));
                json!({})
            }
        },
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => {
            logging::info(format!(
                "config not found at {}; using defaults",
                target.display()
            ));
            json!({})
        }
        Err(exc) => {
            logging::error(format!(
                "unreadable config {} ({exc}); using defaults",
                target.display()
            ));
            json!({})
        }
    };
    let mut config = merge_defaults(&default_config(), &raw);

    let interval = config
        .get("poll_interval_seconds")
        .cloned()
        .unwrap_or(Value::Null);
    if as_i64(&interval)
        .map(|v| !(MIN_POLL_INTERVAL_SECONDS..=MAX_POLL_INTERVAL_SECONDS).contains(&v))
        .unwrap_or(true)
    {
        logging::warn(format!(
            "poll_interval_seconds {interval:?} out of range [{MIN_POLL_INTERVAL_SECONDS}, {MAX_POLL_INTERVAL_SECONDS}]; using default {DEFAULT_POLL_INTERVAL_SECONDS}"
        ));
        config["poll_interval_seconds"] = json!(DEFAULT_POLL_INTERVAL_SECONDS);
    }

    let defaults = default_config();
    for cli_id in CLI_IDS {
        let default_cli = defaults["clis"][cli_id].clone();
        {
            let cli = cli_object(&mut config, cli_id);
            if *cli_id == "openrouter" {
                if cli.get("source").and_then(Value::as_str) != Some("headless") {
                    logging::warn(format!(
                        "cli {cli_id} only supports headless collection; forcing headless"
                    ));
                    cli.insert("source".into(), json!("headless"));
                }
            } else if !cli
                .get("source")
                .and_then(Value::as_str)
                .map(|s| VALID_SOURCES.contains(&s))
                .unwrap_or(false)
            {
                let fallback = default_cli["source"].clone();
                logging::warn(format!(
                    "cli {cli_id} has invalid source {:?}; using {fallback}",
                    cli.get("source")
                ));
                cli.insert("source".into(), fallback);
            }
            let stale = cli
                .get("stale_after_seconds")
                .cloned()
                .unwrap_or(Value::Null);
            if as_i64(&stale)
                .map(|v| v < MIN_STALE_AFTER_SECONDS)
                .unwrap_or(true)
            {
                cli.insert(
                    "stale_after_seconds".into(),
                    default_cli["stale_after_seconds"].clone(),
                );
            }
            let enabled = cli
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| default_cli["enabled"].as_bool().unwrap_or(true));
            cli.insert("enabled".into(), json!(enabled));
        }
    }

    {
        let claude = cli_object(&mut config, "claude_code");
        let endpoint_interval = claude
            .get("endpoint_min_interval_seconds")
            .cloned()
            .unwrap_or(Value::Null);
        if as_i64(&endpoint_interval)
            .map(|v| {
                !(MIN_CLAUDE_ENDPOINT_INTERVAL_SECONDS..=MAX_POLL_INTERVAL_SECONDS).contains(&v)
            })
            .unwrap_or(true)
        {
            logging::warn(format!(
                "claude endpoint_min_interval_seconds {endpoint_interval:?} out of range [{MIN_CLAUDE_ENDPOINT_INTERVAL_SECONDS}, {MAX_POLL_INTERVAL_SECONDS}]; using default {CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS}"
            ));
            claude.insert(
                "endpoint_min_interval_seconds".into(),
                json!(CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS),
            );
        }
    }
    {
        let opencode = cli_object(&mut config, "opencode_go");
        let fetch_interval = opencode
            .get("fetch_min_interval_seconds")
            .cloned()
            .unwrap_or(Value::Null);
        if as_i64(&fetch_interval)
            .map(|v| {
                !(MIN_OPENCODE_GO_FETCH_INTERVAL_SECONDS..=MAX_POLL_INTERVAL_SECONDS).contains(&v)
            })
            .unwrap_or(true)
        {
            logging::warn(format!(
                "opencode_go fetch_min_interval_seconds {fetch_interval:?} out of range [{MIN_OPENCODE_GO_FETCH_INTERVAL_SECONDS}, {MAX_POLL_INTERVAL_SECONDS}]; using default {OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS}"
            ));
            opencode.insert(
                "fetch_min_interval_seconds".into(),
                json!(OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS),
            );
        }
    }
    {
        let openrouter = cli_object(&mut config, "openrouter");
        let or_fetch = openrouter
            .get("fetch_min_interval_seconds")
            .cloned()
            .unwrap_or(Value::Null);
        if as_i64(&or_fetch)
            .map(|v| {
                !(MIN_OPENROUTER_FETCH_INTERVAL_SECONDS..=MAX_POLL_INTERVAL_SECONDS).contains(&v)
            })
            .unwrap_or(true)
        {
            logging::warn(format!(
                "openrouter fetch_min_interval_seconds {or_fetch:?} out of range [{MIN_OPENROUTER_FETCH_INTERVAL_SECONDS}, {MAX_POLL_INTERVAL_SECONDS}]; using default {OPENROUTER_FETCH_MIN_INTERVAL_SECONDS}"
            ));
            openrouter.insert(
                "fetch_min_interval_seconds".into(),
                json!(OPENROUTER_FETCH_MIN_INTERVAL_SECONDS),
            );
        }
        let daily_limit = openrouter
            .get("daily_limit")
            .cloned()
            .unwrap_or(Value::Null);
        if as_i64(&daily_limit)
            .map(|v| !(MIN_OPENROUTER_DAILY_LIMIT..=MAX_OPENROUTER_DAILY_LIMIT).contains(&v))
            .unwrap_or(true)
        {
            logging::warn(format!(
                "openrouter daily_limit {daily_limit:?} out of range [{MIN_OPENROUTER_DAILY_LIMIT}, {MAX_OPENROUTER_DAILY_LIMIT}]; using default {OPENROUTER_DAILY_LIMIT_DEFAULT}"
            ));
            openrouter.insert("daily_limit".into(), json!(OPENROUTER_DAILY_LIMIT_DEFAULT));
        }
        let model_slug = openrouter
            .get("model_slug")
            .and_then(Value::as_str)
            .unwrap_or("");
        if model_slug.trim().is_empty() {
            openrouter.insert("model_slug".into(), json!(OPENROUTER_MODEL_SLUG_DEFAULT));
        } else {
            let trimmed = model_slug.trim().to_string();
            openrouter.insert("model_slug".into(), json!(trimmed));
        }
    }
    config
}

/// Atomically write `config` as pretty JSON to the config file.
pub fn save_config(config: &Value, path: Option<&Path>) -> std::io::Result<()> {
    let target = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_path(None));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("tmp");
    let mut text = serde_json::to_string_pretty(config).unwrap_or_else(|_| "{}".into());
    text.push('\n');
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &target)?;
    logging::info(format!("config saved to {}", target.display()));
    Ok(())
}

/// Get a dotted key (e.g. `clis.codex.enabled`) from a config object.
pub fn get_dotted(config: &Value, key_path: &str) -> Option<Value> {
    let mut node = config;
    for key in key_path.split('.') {
        node = node.get(key)?;
    }
    Some(node.clone())
}

/// Set a dotted key, creating intermediate objects as needed.
pub fn set_dotted(config: &mut Value, key_path: &str, value: Value) {
    let keys: Vec<&str> = key_path.split('.').collect();
    let mut node = config;
    for key in &keys[..keys.len() - 1] {
        if !node.get(*key).map(Value::is_object).unwrap_or(false) {
            node[*key] = json!({});
        }
        node = &mut node[*key];
    }
    node[keys[keys.len() - 1]] = value;
}

/// Print a config value the way the installer Python helper used to.
pub fn format_config_value(value: &Value) -> String {
    match value {
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Sandbox;

    #[test]
    fn defaults_when_missing() {
        let _sandbox = Sandbox::new();
        let cfg = load_config(None);
        assert_eq!(cfg["poll_interval_seconds"], json!(5));
        let default_on = ["codex", "claude_code", "grok"];
        for cli in CLI_IDS {
            let enabled = cfg["clis"][cli]["enabled"].as_bool().unwrap();
            assert_eq!(enabled, default_on.contains(cli), "{cli}");
            let expected_source = if *cli == "openrouter" {
                "headless"
            } else {
                "auto"
            };
            assert_eq!(cfg["clis"][cli]["source"], json!(expected_source));
        }
    }

    #[test]
    fn opencode_go_is_disabled_by_default() {
        let _sandbox = Sandbox::new();
        assert_eq!(
            load_config(None)["clis"]["opencode_go"]["enabled"],
            json!(false)
        );
    }

    #[test]
    fn openrouter_is_disabled_by_default() {
        let _sandbox = Sandbox::new();
        assert_eq!(
            load_config(None)["clis"]["openrouter"]["enabled"],
            json!(false)
        );
    }

    #[test]
    fn openrouter_opt_in_is_honored() {
        let _sandbox = Sandbox::new();
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"clis": {"openrouter": {"enabled": true}}}"#).unwrap();
        assert_eq!(
            load_config(None)["clis"]["openrouter"]["enabled"],
            json!(true)
        );
    }

    #[test]
    fn cline_is_not_monitored() {
        assert!(!CLI_IDS.contains(&"cline"));
    }

    #[test]
    fn partial_config_merges_over_defaults() {
        let sandbox = Sandbox::new();
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"poll_interval_seconds": 120, "clis": {"grok": {"enabled": false}}}"#,
        )
        .unwrap();
        let cfg = load_config(None);
        assert_eq!(cfg["poll_interval_seconds"], json!(120));
        assert_eq!(cfg["clis"]["grok"]["enabled"], json!(false));
        assert_eq!(cfg["clis"]["grok"]["source"], json!("auto"));
        assert_eq!(cfg["clis"]["codex"]["enabled"], json!(true));
        drop(sandbox);
    }

    #[test]
    fn out_of_range_interval_falls_back() {
        let _sandbox = Sandbox::new();
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"poll_interval_seconds": 2}"#).unwrap();
        assert_eq!(load_config(None)["poll_interval_seconds"], json!(5));
    }

    #[test]
    fn opencode_go_fetch_interval_defaults_and_floor() {
        let _sandbox = Sandbox::new();
        assert_eq!(
            load_config(None)["clis"]["opencode_go"]["fetch_min_interval_seconds"],
            json!(300)
        );
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"clis": {"opencode_go": {"fetch_min_interval_seconds": 60}}}"#,
        )
        .unwrap();
        assert_eq!(
            load_config(None)["clis"]["opencode_go"]["fetch_min_interval_seconds"],
            json!(300)
        );
    }

    #[test]
    fn openrouter_defaults_and_validation() {
        let _sandbox = Sandbox::new();
        let openrouter = &load_config(None)["clis"]["openrouter"];
        assert_eq!(openrouter["source"], json!("headless"));
        assert_eq!(
            openrouter["model_slug"],
            json!(OPENROUTER_MODEL_SLUG_DEFAULT)
        );
        assert_eq!(openrouter["daily_limit"], json!(1000));
        assert_eq!(openrouter["fetch_min_interval_seconds"], json!(60));
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"clis": {"openrouter": {"fetch_min_interval_seconds": 30, "daily_limit": 0, "model_slug": "  "}}}"#,
        )
        .unwrap();
        let fixed = &load_config(None)["clis"]["openrouter"];
        assert_eq!(fixed["fetch_min_interval_seconds"], json!(60));
        assert_eq!(fixed["daily_limit"], json!(1000));
        assert_eq!(fixed["model_slug"], json!(OPENROUTER_MODEL_SLUG_DEFAULT));
    }

    #[test]
    fn claude_endpoint_interval_defaults_and_floor() {
        let _sandbox = Sandbox::new();
        assert_eq!(
            load_config(None)["clis"]["claude_code"]["endpoint_min_interval_seconds"],
            json!(180)
        );
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"clis": {"claude_code": {"endpoint_min_interval_seconds": 5}}}"#,
        )
        .unwrap();
        assert_eq!(
            load_config(None)["clis"]["claude_code"]["endpoint_min_interval_seconds"],
            json!(180)
        );
        fs::write(
            &path,
            r#"{"clis": {"claude_code": {"endpoint_min_interval_seconds": 60}}}"#,
        )
        .unwrap();
        assert_eq!(
            load_config(None)["clis"]["claude_code"]["endpoint_min_interval_seconds"],
            json!(60)
        );
    }

    #[test]
    fn openrouter_source_always_headless() {
        let _sandbox = Sandbox::new();
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"clis": {"openrouter": {"source": "auto"}}}"#).unwrap();
        assert_eq!(
            load_config(None)["clis"]["openrouter"]["source"],
            json!("headless")
        );
    }

    #[test]
    fn invalid_source_falls_back() {
        let _sandbox = Sandbox::new();
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"clis": {"codex": {"source": "carrier-pigeon"}}}"#,
        )
        .unwrap();
        assert_eq!(load_config(None)["clis"]["codex"]["source"], json!("auto"));
    }

    #[test]
    fn corrupt_file_yields_defaults() {
        let _sandbox = Sandbox::new();
        let path = config_path(None);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{nope").unwrap();
        assert_eq!(load_config(None)["poll_interval_seconds"], json!(5));
    }

    #[test]
    fn round_trip() {
        let _sandbox = Sandbox::new();
        let mut cfg = load_config(None);
        cfg["poll_interval_seconds"] = json!(240);
        cfg["clis"]["claude_code"]["source"] = json!("headless");
        save_config(&cfg, None).unwrap();
        let loaded = load_config(None);
        assert_eq!(loaded["poll_interval_seconds"], json!(240));
        assert_eq!(loaded["clis"]["claude_code"]["source"], json!("headless"));
    }

    #[test]
    fn write_is_atomic() {
        let _sandbox = Sandbox::new();
        save_config(&load_config(None), None).unwrap();
        let leftovers: Vec<_> = fs::read_dir(config_path(None).parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
