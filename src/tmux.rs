//! Background tmux debug console for the agentic CLI status poller.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::config::slash_command;
use crate::logging;

pub const CAPTURE_DELAY_SECONDS: f64 = 4.0;
pub const CODEX_STATUS_ATTEMPTS: usize = 3;

pub const CLI_COMMANDS: &[(&str, &str)] = &[
    ("codex", "codex"),
    ("claude_code", "claude"),
    ("grok", "grok"),
    ("opencode_go", "opencode"),
];

const WINDOW_ENV: &[&str] = &[
    "ATTS_SUPPRESS_ANNOUNCEMENTS=1",
    "CODEX_AGENT_SKIP_TRACKER=1",
];

const BINARY_FALLBACK_DIRS: &[&str] = &[".local/bin", ".grok/bin", ".codex/bin", ".opencode/bin"];

fn cli_binary_name(cli_id: &str) -> Option<&'static str> {
    CLI_COMMANDS
        .iter()
        .find(|(id, _)| *id == cli_id)
        .map(|(_, bin)| *bin)
}

/// Return the subset of `cli_ids` that own a tmux debug-console window.
pub fn tmux_console_cli_ids(cli_ids: &[String]) -> Vec<String> {
    cli_ids
        .iter()
        .filter(|id| cli_binary_name(id).is_some())
        .cloned()
        .collect()
}

/// Trait wrapping tmux subprocess calls so tests can inject a fake.
pub trait TmuxRunner: Send + Sync {
    fn run(&self, args: &[String]) -> (i32, String, String);
}

/// Production tmux runner.
pub struct RealTmux;

impl TmuxRunner for RealTmux {
    fn run(&self, args: &[String]) -> (i32, String, String) {
        let output = Command::new("tmux").args(args).output();
        match output {
            Ok(Output {
                status,
                stdout,
                stderr,
            }) => (
                status.code().unwrap_or(1),
                String::from_utf8_lossy(&stdout).into_owned(),
                String::from_utf8_lossy(&stderr).into_owned(),
            ),
            Err(exc) => (1, String::new(), exc.to_string()),
        }
    }
}

fn which(binary: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = Path::new(dir).join(binary);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Return whether the tmux binary is installed.
pub fn tmux_available() -> bool {
    which("tmux").is_some()
}

/// Return the absolute path of a CLI's binary, or None when missing.
pub fn resolve_cli_binary(cli_id: &str, home: &Path) -> Option<PathBuf> {
    let binary = cli_binary_name(cli_id)?;
    if let Some(found) = which(binary) {
        return Some(found);
    }
    for rel in BINARY_FALLBACK_DIRS {
        let candidate = home.join(rel).join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn window_env_args() -> Vec<String> {
    let mut args = Vec::new();
    for var in WINDOW_ENV {
        args.push("-e".into());
        args.push((*var).into());
    }
    args
}

/// Ensure the detached console session exists with one window per CLI.
pub fn ensure_console(
    session: &str,
    cli_ids: &[String],
    home: &Path,
    tmux: &dyn TmuxRunner,
    available: bool,
) -> bool {
    if !available {
        logging::warn("tmux not installed; console disabled");
        return false;
    }
    let has = tmux.run(&["has-session".into(), "-t".into(), session.into()]);
    let mut have_session = has.0 == 0;
    let mut existing = std::collections::HashSet::new();
    if have_session {
        let listed = tmux.run(&[
            "list-windows".into(),
            "-t".into(),
            session.into(),
            "-F".into(),
            "#{window_name}".into(),
        ]);
        existing = listed.1.split_whitespace().map(str::to_string).collect();
    }
    let mut created_any = have_session;
    for cli_id in cli_ids {
        if cli_binary_name(cli_id).is_none() {
            logging::debug(format!("cli {cli_id} has no tmux console; skipping window"));
            continue;
        }
        if existing.contains(cli_id) {
            continue;
        }
        let Some(binary) = resolve_cli_binary(cli_id, home) else {
            logging::info(format!(
                "cli {cli_id} not installed; skipping console window"
            ));
            continue;
        };
        let mut env_args = window_env_args();
        if !have_session {
            let mut args = vec!["new-session".into(), "-d".into()];
            args.append(&mut env_args);
            args.extend([
                "-s".into(),
                session.into(),
                "-n".into(),
                cli_id.clone(),
                binary.to_string_lossy().into_owned(),
            ]);
            let result = tmux.run(&args);
            have_session = result.0 == 0;
            created_any = have_session;
            if !have_session {
                logging::error(format!(
                    "failed to create tmux session {session}: {}",
                    result.2.trim()
                ));
                return false;
            }
        } else {
            let mut args = vec!["new-window".into(), "-d".into()];
            args.append(&mut env_args);
            args.extend([
                "-t".into(),
                session.into(),
                "-n".into(),
                cli_id.clone(),
                binary.to_string_lossy().into_owned(),
            ]);
            let result = tmux.run(&args);
            if result.0 != 0 {
                logging::error(format!(
                    "failed to create console window {cli_id}: {}",
                    result.2.trim()
                ));
            }
            created_any = created_any || result.0 == 0;
        }
        logging::info(format!(
            "created console window {cli_id} in session {session}"
        ));
    }
    created_any
}

fn send_slash(tmux: &dyn TmuxRunner, target: &str, command: &str, sleeper: &dyn Fn(f64)) -> bool {
    for keys in [
        vec!["Escape".into()],
        vec!["C-u".into()],
        vec![command.to_string()],
        vec!["Enter".into()],
    ] {
        let mut args = vec!["send-keys".into(), "-t".into(), target.into()];
        args.extend(keys.clone());
        let result = tmux.run(&args);
        if result.0 != 0 {
            logging::warn(format!("send-keys to {target} failed: {}", result.2.trim()));
            return false;
        }
        if keys.len() == 1 && keys[0] == command {
            sleeper(0.3);
        }
    }
    true
}

fn capture_pane(tmux: &dyn TmuxRunner, target: &str) -> Option<String> {
    let captured = tmux.run(&[
        "capture-pane".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
    ]);
    if captured.0 != 0 {
        logging::warn(format!(
            "capture-pane {target} failed: {}",
            captured.2.trim()
        ));
        return None;
    }
    Some(captured.1)
}

fn rate_limit_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\blimit\b[^\n]*\d+\s*%").unwrap())
}

fn capture_codex_status(
    tmux: &dyn TmuxRunner,
    target: &str,
    sleeper: &dyn Fn(f64),
) -> Option<String> {
    let mut captured = None;
    for _ in 0..CODEX_STATUS_ATTEMPTS {
        if !send_slash(tmux, target, "/status", sleeper) {
            return captured;
        }
        sleeper(CAPTURE_DELAY_SECONDS);
        captured = capture_pane(tmux, target);
        let text = captured.as_deref()?;
        if rate_limit_line().is_match(text) {
            break;
        }
    }
    captured
}

/// Send the CLI's rate-limit slash command and capture its pane text.
pub fn refresh_and_capture(
    session: &str,
    cli_id: &str,
    sleeper: &dyn Fn(f64),
    tmux: &dyn TmuxRunner,
) -> Option<String> {
    let target = format!("{session}:{cli_id}");
    let command = slash_command(cli_id)?;
    if !send_slash(tmux, &target, command, sleeper) {
        return None;
    }
    sleeper(CAPTURE_DELAY_SECONDS);
    let mut text = capture_pane(tmux, &target)?;
    if cli_id == "codex" {
        if let Some(status_text) = capture_codex_status(tmux, &target, sleeper) {
            text = format!("{}\n{status_text}", text.trim_end());
        }
    }
    let _ = tmux.run(&["send-keys".into(), "-t".into(), target, "Escape".into()]);
    Some(text)
}

/// No-op sleeper for tests.
pub fn noop_sleep(_: f64) {}

/// Real sleeper.
pub fn real_sleep(seconds: f64) {
    if seconds > 0.0 {
        std::thread::sleep(Duration::from_secs_f64(seconds));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Sandbox;
    use std::sync::Mutex;

    struct FakeTmux {
        calls: Mutex<Vec<Vec<String>>>,
        has_session: bool,
        captures: Mutex<Vec<String>>,
    }

    impl TmuxRunner for FakeTmux {
        fn run(&self, args: &[String]) -> (i32, String, String) {
            self.calls.lock().unwrap().push(args.to_vec());
            if args.first().map(String::as_str) == Some("has-session") {
                return (
                    if self.has_session { 0 } else { 1 },
                    String::new(),
                    String::new(),
                );
            }
            if args.first().map(String::as_str) == Some("list-windows") {
                return (0, "codex\n".into(), String::new());
            }
            if args.first().map(String::as_str) == Some("capture-pane") {
                let mut caps = self.captures.lock().unwrap();
                let stdout = if caps.is_empty() {
                    String::new()
                } else {
                    caps.remove(0)
                };
                return (0, stdout, String::new());
            }
            (0, String::new(), String::new())
        }
    }

    #[test]
    fn codex_monitor_uses_usage_command() {
        assert_eq!(slash_command("codex"), Some("/usage"));
    }

    #[test]
    fn opencode_go_monitor_uses_ogc_usage_command() {
        assert_eq!(slash_command("opencode_go"), Some("/ogc-usage"));
    }

    #[test]
    fn openrouter_enabled_does_not_keyerror() {
        let sandbox = Sandbox::new();
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(Vec::new()),
        };
        let ids = vec!["codex".into(), "openrouter".into()];
        assert!(ensure_console("acs", &ids, &sandbox.home, &tmux, true));
        assert_eq!(
            tmux_console_cli_ids(&["codex".into(), "openrouter".into(), "grok".into()]),
            vec!["codex".to_string(), "grok".to_string()]
        );
    }

    #[test]
    fn new_session_and_window_set_suppression_env() {
        let sandbox = Sandbox::new();
        let bin = sandbox.home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["claude", "codex"] {
            std::fs::write(bin.join(name), "#!/bin/sh\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(bin.join(name)).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(bin.join(name), p).unwrap();
        }
        let original_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", format!("{}:/usr/bin", bin.display()));
        }
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: false,
            captures: Mutex::new(Vec::new()),
        };
        assert!(ensure_console(
            "acs",
            &["claude_code".into(), "codex".into()],
            &sandbox.home,
            &tmux,
            true,
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        let creations: Vec<_> = calls
            .iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("new-session")
                    || c.first().map(String::as_str) == Some("new-window")
            })
            .cloned()
            .collect();
        assert_eq!(creations[0][0], "new-session");
        assert_eq!(creations[1][0], "new-window");
        for args in &creations {
            let joined = args.join(" ");
            assert!(joined.contains("-e ATTS_SUPPRESS_ANNOUNCEMENTS=1"));
            assert!(joined.contains("-e CODEX_AGENT_SKIP_TRACKER=1"));
        }
        unsafe {
            match original_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn falls_back_to_known_install_dirs() {
        let sandbox = Sandbox::new();
        let grok = sandbox.home.join(".grok").join("bin").join("grok");
        std::fs::create_dir_all(grok.parent().unwrap()).unwrap();
        std::fs::write(&grok, "#!/bin/sh\n").unwrap();
        let original_path = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "") };
        assert_eq!(resolve_cli_binary("grok", &sandbox.home), Some(grok));
        unsafe {
            match original_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn missing_binary_returns_none() {
        let sandbox = Sandbox::new();
        let original_path = std::env::var_os("PATH");
        unsafe { std::env::set_var("PATH", "") };
        assert!(resolve_cli_binary("claude_code", &sandbox.home).is_none());
        unsafe {
            match original_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn codex_combines_usage_allowance_with_status_breakdown() {
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(vec![
                "Usage\n› 1. Show usage\n  2. Redeem usage limit reset\nYou have 1 usage limit reset available.\n".into(),
                "5h limit: 99% left\nWeekly limit: 9% left\n".into(),
            ]),
        };
        let text = refresh_and_capture("acs", "codex", &noop_sleep, &tmux).unwrap();
        assert!(text.contains("1 usage limit reset available"));
        assert!(text.contains("Weekly limit: 9% left"));
        let calls = tmux.calls.lock().unwrap().clone();
        let send_keys: Vec<_> = calls
            .into_iter()
            .filter(|c| c.first().map(String::as_str) == Some("send-keys"))
            .collect();
        let expected: Vec<String> = ["send-keys", "-t", "acs:codex", "/status"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert!(send_keys.iter().any(|c| c == &expected));
    }

    #[test]
    fn codex_retries_status_until_rate_limits_render() {
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(vec![
                "Usage\n› 1. Show usage\nYou have 2 usage limit resets available.\n".into(),
                "Refresh requested; run /status again shortly.\n".into(),
                "5h limit: 99% left (resets 20:12)\nWeekly limit: 9% left\n".into(),
            ]),
        };
        let text = refresh_and_capture("acs", "codex", &noop_sleep, &tmux).unwrap();
        assert!(text.contains("2 usage limit resets available"));
        assert!(text.contains("5h limit: 99% left"));
        let calls = tmux.calls.lock().unwrap().clone();
        let expected: Vec<String> = ["send-keys", "-t", "acs:codex", "/status"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let status_sends = calls.iter().filter(|c| *c == &expected).count();
        assert_eq!(status_sends, 2);
    }
}
