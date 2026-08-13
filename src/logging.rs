//! Centralized logging for the agentic CLI status poller.
//!
//! Every module routes diagnostics through this file. The sink is
//! environment-aware:
//!
//! - `ACS_LOG_DIR` forces a file sink in that directory (tests and developers).
//! - A repository checkout (a `.git` ancestor of the running binary or cwd)
//!   logs to the repo-root `.log/` directory, which is gitignored.
//! - An installed copy (`~/.local/bin/agentic-cli-status`) logs to stderr so
//!   the systemd user service journal is the canonical sink.
//!
//! Logging is best-effort and never aborts the program.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_LOG_FILE: &str = "agentic-cli-status.log";

static LOG_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
static SINK_READY: Mutex<bool> = Mutex::new(false);

/// Resolve the directory for the file sink, or `None` for stderr (journald).
///
/// A checkout is a directory that contains both `.git` and `Cargo.toml`, so an
/// unrelated git repo at `$HOME` cannot steal the installed daemon's logs.
fn log_dir() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var("ACS_LOG_DIR") {
        if !override_dir.is_empty() {
            return Some(PathBuf::from(override_dir));
        }
    }
    let mut starts = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            starts.push(parent.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    crate_log_dir(&starts)
}

/// Walk `starts` looking for this crate's checkout (`.git` + `Cargo.toml`).
fn crate_log_dir(starts: &[PathBuf]) -> Option<PathBuf> {
    for start in starts {
        let mut current = start.clone();
        for _ in 0..10 {
            if current.join(".git").exists() && current.join("Cargo.toml").exists() {
                return Some(current.join(".log"));
            }
            if !current.pop() {
                break;
            }
        }
    }
    None
}

fn init_sink() {
    let mut ready = SINK_READY.lock().unwrap_or_else(|e| e.into_inner());
    if *ready {
        return;
    }
    *ready = true;
    if let Some(dir) = log_dir() {
        if fs::create_dir_all(&dir).is_ok() {
            let mut guard = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(dir.join(DEFAULT_LOG_FILE));
        }
    }
}

fn write_line(level: &str, message: &str) {
    init_sink();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{ts} {level} [agentic-cli-status] {message}\n");
    let path = {
        let guard = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    };
    if let Some(path) = path {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
            return;
        }
    }
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.flush();
}

/// Log an informational action-path message.
pub fn info(message: impl AsRef<str>) {
    write_line("INFO", message.as_ref());
}

/// Log a warning that does not abort the caller.
pub fn warn(message: impl AsRef<str>) {
    write_line("WARN", message.as_ref());
}

/// Log an error condition (caller still decides how to recover).
pub fn error(message: impl AsRef<str>) {
    write_line("ERROR", message.as_ref());
}

/// Log a debug/trace detail. File sink only — journald would record every
/// five-second poll tick.
pub fn debug(message: impl AsRef<str>) {
    init_sink();
    let has_file = {
        let guard = LOG_FILE.lock().unwrap_or_else(|e| e.into_inner());
        guard.is_some()
    };
    if !has_file {
        return;
    }
    write_line("DEBUG", message.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn home_git_repo_without_cargo_toml_is_not_a_checkout() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(home.join(".git")).unwrap();
        let bin = home.join(".local").join("bin");
        fs::create_dir_all(&bin).unwrap();
        assert_eq!(crate_log_dir(&[bin, home]), None);
    }

    #[test]
    fn crate_checkout_logs_to_dot_log() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        assert_eq!(crate_log_dir(&[src]), Some(root.join(".log")));
    }
}
