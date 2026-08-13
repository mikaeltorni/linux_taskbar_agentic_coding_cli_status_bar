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
    for start in starts {
        let mut current = start;
        for _ in 0..10 {
            if current.join(".git").exists() {
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
            return;
        }
    }
    eprint!("{line}");
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

/// Log a debug/trace detail. Avoid in hot poll ticks except on decisions.
pub fn debug(message: impl AsRef<str>) {
    write_line("DEBUG", message.as_ref());
}
