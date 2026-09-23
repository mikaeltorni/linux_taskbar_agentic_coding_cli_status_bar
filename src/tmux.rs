//! Background tmux debug console for the agentic CLI status poller.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use regex::Regex;

use crate::collectors::codex_home_authenticated;
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

/// ACC environment variable naming the Codex account a launch belongs to.
const ACC_CODEX_INSTANCE_ENV: &str = "ACC_CODEX_INSTANCE";

/// Seconds a freshly started console pane is left alone before it is judged.
///
/// A pane younger than this is still walking its launcher exec chain, so its
/// environment is not yet the login the window will run.
const CONSOLE_SETTLE_SECONDS: f64 = 45.0;

const BINARY_FALLBACK_DIRS: &[&str] = &[".local/bin", ".grok/bin", ".codex/bin", ".opencode/bin"];

/// Codex debug windows recreated since the last poll, by window name.
///
/// Each ACC account owns its own console window, so the pending set names the
/// windows whose next poll must capture `/status` instead of trusting the pane
/// text left behind by the login that window used to run.
static CODEX_CONSOLE_REFRESH: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn codex_console_refresh() -> &'static Mutex<HashSet<String>> {
    CODEX_CONSOLE_REFRESH.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Return and clear the Codex console windows that need an immediate `/status`.
pub fn take_codex_console_refresh() -> HashSet<String> {
    let mut pending = codex_console_refresh()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    std::mem::take(&mut *pending)
}

fn mark_codex_console_refresh(window: &str) {
    codex_console_refresh()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(window.to_string());
}

/// One window the console session should end up with.
struct DesiredWindow {
    /// tmux window name.
    window: String,
    /// CLI the window runs.
    cli_id: String,
    /// `CODEX_HOME` for Codex windows tied to an ACC account.
    codex_home: Option<PathBuf>,
    /// ACC registry instance id for Codex windows.
    instance_id: Option<i64>,
}

/// One ACC Codex account's debug console window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexConsole {
    /// tmux window name, unique per account.
    pub window: String,
    /// `CODEX_HOME` the window's `codex` process runs with.
    pub home: PathBuf,
    /// ACC registry instance id, exported so the launcher keeps this account.
    pub instance_id: Option<i64>,
}

/// Return the console window name that belongs to an ACC Codex instance.
///
/// Instance 1 keeps the historical `codex` window so single-account installs
/// are untouched; every extra login gets `codex-<id>`. Names are derived from
/// the instance id rather than the selection, so switching accounts never
/// rebuilds a window and both logins stay monitored at the same time.
pub fn codex_console_window(instance_id: i64) -> String {
    if instance_id <= 1 {
        "codex".to_string()
    } else {
        format!("codex-{instance_id}")
    }
}

/// Return whether a tmux window name belongs to a Codex debug console.
fn is_codex_window(window: &str) -> bool {
    window == "codex"
        || window
            .strip_prefix("codex-")
            .is_some_and(|id| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_digit()))
}

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

    /// Read `CODEX_HOME` from a pane process. `new-window -e` puts the
    /// variable on the child, not on `tmux show-environment`.
    fn process_codex_home(&self, pid: &str) -> Option<String> {
        read_proc_environ_var(pid, "CODEX_HOME")
    }

    /// Seconds since a pane process started, or None when the pid is gone.
    fn process_age_seconds(&self, pid: &str) -> Option<f64> {
        read_proc_age_seconds(pid)
    }

    /// Nul-separated `/proc/<pid>/cmdline` of a pane process.
    fn process_cmdline(&self, pid: &str) -> Option<String> {
        read_proc_cmdline(pid)
    }
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
    // The user-managed CLI is updated by the setup repository. systemd's PATH
    // can still prefer an obsolete global npm install with a different /usage.
    if cli_id == "codex" {
        let managed = home.join(".local/share/linux_codex_claude_code_setup/npm/bin/codex");
        if managed.is_file() {
            return Some(managed);
        }
    }
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

/// Newer Codex builds need this mode when the optional shared daemon package
/// is absent. Probe the installed binary so older builds keep their CLI syntax.
fn codex_supports_no_daemon(binary: &Path) -> bool {
    match Command::new(binary).arg("--help").output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).contains("--no-daemon")
        }
        Ok(_) => false,
        Err(error) => {
            logging::warn(format!("cannot probe Codex console options: {error}"));
            false
        }
    }
}

/// Build the `-e VAR=value` arguments a console window is created with.
///
/// The `codex` launcher hands the run to ACC, which re-exports `CODEX_HOME`
/// for the *selected* account.  Passing `ACC_CODEX_INSTANCE` pins the account
/// the launcher resolves, so a console keeps the login it was created for
/// instead of drifting to the selection and being torn down every tick.
fn window_env_args(
    cli_id: &str,
    codex_home: Option<&Path>,
    instance_id: Option<i64>,
) -> Vec<String> {
    let mut args = Vec::new();
    for var in WINDOW_ENV {
        args.push("-e".into());
        args.push((*var).into());
    }
    if cli_id == "codex" {
        if let Some(home) = codex_home {
            args.push("-e".into());
            args.push(format!("CODEX_HOME={}", home.display()));
        }
        if let Some(id) = instance_id {
            args.push("-e".into());
            args.push(format!("{ACC_CODEX_INSTANCE_ENV}={id}"));
        }
    }
    args
}

/// Parse `/proc/<pid>/environ` for `key`. Returns None when the pid is
/// gone or the variable is missing.
fn read_proc_environ_var(pid: &str, key: &str) -> Option<String> {
    if !pid.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let bytes = fs::read(format!("/proc/{pid}/environ")).ok()?;
    let prefix = format!("{key}=");
    bytes
        .split(|b| *b == 0)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .find_map(|entry| entry.strip_prefix(&prefix).map(str::to_string))
}

/// Return how long a process has been running, from `/proc/<pid>`.
///
/// The kernel stamps the directory with the process start time, so its mtime
/// dates the pane without parsing clock ticks.
///
/// Args:
///     pid: Process id as printed by tmux.
///
/// Returns:
///     Age in seconds, or None when the pid is gone or unreadable.
fn read_proc_age_seconds(pid: &str) -> Option<f64> {
    if !pid.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let started = fs::metadata(format!("/proc/{pid}")).ok()?.modified().ok()?;
    SystemTime::now()
        .duration_since(started)
        .ok()
        .map(|age| age.as_secs_f64())
}

/// Return the `CODEX_HOME` the tmux debug console is currently logged into.
///
/// Read straight from the console process environment, so the reading is the
/// account identity itself rather than a guess from usage numbers.
///
/// Args:
///     tmux: Runner used to query the console session.
///     session: Detached console session name.
///     window: Console window name, one per ACC account.
///
/// Returns:
///     The console pane's `CODEX_HOME`, or None when the window or the
///     variable is missing.
pub fn console_codex_home(tmux: &dyn TmuxRunner, session: &str, window: &str) -> Option<String> {
    let pid = console_pane_pid(tmux, session, window)?;
    tmux.process_codex_home(&pid)
}

/// Return the pid of a console window's first pane, or None when it is gone.
fn console_pane_pid(tmux: &dyn TmuxRunner, session: &str, window: &str) -> Option<String> {
    let result = tmux.run(&[
        "list-panes".into(),
        "-t".into(),
        format!("{session}:{window}"),
        "-F".into(),
        "#{pane_pid}".into(),
    ]);
    if result.0 != 0 {
        return None;
    }
    let pid = result.1.trim().lines().next()?.trim();
    if pid.is_empty() {
        return None;
    }
    Some(pid.to_string())
}

fn homes_match(left: &str, right: &Path) -> bool {
    Path::new(left) == right
}

/// Return whether a pane cmdline is already running `binary`.
///
/// Args are nul-separated as in `/proc/<pid>/cmdline`. A missing cmdline is
/// treated as unknown (not a mismatch) so tests that do not stub it keep
/// existing windows.
fn cmdline_runs_binary(cmdline: &str, binary: &Path) -> bool {
    let wanted = binary.to_string_lossy();
    cmdline
        .split('\0')
        .any(|arg| !arg.is_empty() && (Path::new(arg) == binary || arg == wanted.as_ref()))
}

fn read_proc_cmdline(pid: &str) -> Option<String> {
    if !pid.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Kill a Codex debug window when it is logged into a different ACC instance
/// or still running an older `codex` binary than the poller would launch.
fn recreate_codex_window_if_stale(
    session: &str,
    existing: &mut HashSet<String>,
    tmux: &dyn TmuxRunner,
    window: &str,
    desired_home: &Path,
    desired_binary: Option<&Path>,
) {
    if !existing.contains(window) {
        return;
    }
    let Some(pid) = console_pane_pid(tmux, session, window) else {
        return;
    };
    let current = tmux.process_codex_home(&pid);
    let home_ok = current
        .as_deref()
        .is_some_and(|home| homes_match(home, desired_home));
    let binary_ok = match (desired_binary, tmux.process_cmdline(&pid)) {
        (Some(binary), Some(cmd)) => cmdline_runs_binary(&cmd, binary),
        _ => true,
    };
    if home_ok && binary_ok {
        if console_started_before_auth(tmux, &pid, desired_home) {
            logging::info(format!(
                "recreating Codex debug console {window}; login finished after the pane started"
            ));
            if !kill_console_window(session, tmux, window) {
                return;
            }
            existing.remove(window);
            mark_codex_console_refresh(window);
        }
        return;
    }
    // The launcher execs through wrapper processes before the CLI settles on
    // its final environment, so a young pane can still be reading the pre-exec
    // value.  Rebuilding it then would restart that wait every tick and the
    // account would never live long enough to answer `/status`.
    if tmux
        .process_age_seconds(&pid)
        .is_some_and(|age| age < CONSOLE_SETTLE_SECONDS)
    {
        logging::debug(format!(
            "Codex debug console {window} is still starting; leaving it to settle"
        ));
        return;
    }
    if !binary_ok {
        logging::info(format!(
            "recreating Codex debug console {window} with {} (pane still on an older binary)",
            desired_binary
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "codex".into())
        ));
    } else {
        logging::info(format!(
            "recreating Codex debug console {window} with CODEX_HOME={} (was {})",
            desired_home.display(),
            current.as_deref().unwrap_or("unset")
        ));
    }
    if !kill_console_window(session, tmux, window) {
        return;
    }
    existing.remove(window);
    mark_codex_console_refresh(window);
}

/// Kill one console window, reporting whether tmux accepted the request.
fn kill_console_window(session: &str, tmux: &dyn TmuxRunner, window: &str) -> bool {
    let killed = tmux.run(&[
        "kill-window".into(),
        "-t".into(),
        format!("{session}:{window}"),
    ]);
    if killed.0 != 0 {
        logging::warn(format!(
            "could not restart Codex debug console {window}: {}",
            killed.2.trim()
        ));
        return false;
    }
    true
}

/// Kill Codex console windows whose ACC account is no longer registered.
///
/// ACC can retire a login while the poller runs. Leaving its window behind
/// would keep a `codex` process alive for an account nothing reads.
fn drop_unregistered_codex_windows(
    session: &str,
    existing: &mut HashSet<String>,
    tmux: &dyn TmuxRunner,
    desired: &HashSet<String>,
) {
    let orphans: Vec<String> = existing
        .iter()
        .filter(|window| is_codex_window(window) && !desired.contains(*window))
        .cloned()
        .collect();
    for window in orphans {
        logging::info(format!(
            "closing Codex debug console {window}; its ACC account is no longer registered"
        ));
        if kill_console_window(session, tmux, &window) {
            existing.remove(&window);
        }
    }
}

/// Log a missing CLI binary once per process so the five-second poll loop
/// cannot flood journald.
fn note_missing_cli(cli_id: &str) {
    static MISSING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = MISSING
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(cli_id.to_string()) {
        logging::info(format!(
            "cli {cli_id} not installed; skipping console window"
        ));
    }
}

/// Log once per window that a Codex console is waiting on ChatGPT login.
fn note_waiting_for_auth(window: &str, home: &Path) {
    static WAITING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let mut seen = WAITING
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if seen.insert(window.to_string()) {
        logging::info(format!(
            "skipping Codex debug console {window} until {}/auth.json exists",
            home.display()
        ));
    }
}

/// Seconds since ``auth.json`` was written, when the file exists.
fn auth_json_age_seconds(home: &Path) -> Option<f64> {
    let modified = fs::metadata(home.join("auth.json")).ok()?.modified().ok()?;
    SystemTime::now()
        .duration_since(modified)
        .ok()
        .map(|age| age.as_secs_f64())
}

/// Return whether a console process started before this home finished login.
///
/// The status-bar ``codex`` TUI opens a browser login when ``auth.json`` is
/// missing. After ACC writes that file the original process stays in the
/// OAuth retry loop, so the pane must be rebuilt onto the authenticated TUI.
fn console_started_before_auth(tmux: &dyn TmuxRunner, pid: &str, home: &Path) -> bool {
    let Some(age) = tmux.process_age_seconds(pid) else {
        return false;
    };
    let Some(auth_age) = auth_json_age_seconds(home) else {
        return false;
    };
    auth_age + 1.0 < age
}

/// Ensure the detached console session exists with one window per console.
///
/// `codex_consoles` lists every ACC Codex account and the window that keeps it
/// logged in. Each account owns a window of its own, so `/status` reports both
/// logins on every tick instead of only the selected one. A window still
/// running a different `CODEX_HOME` — after ACC re-registers an account — is
/// recreated, and windows for retired accounts are closed.
///
/// Homes without ``auth.json`` are not started: a bare ``codex`` TUI there
/// opens a ChatGPT login browser every few seconds. After login lands, a
/// console that started beforehand is rebuilt so that loop dies.
pub fn ensure_console(
    session: &str,
    cli_ids: &[String],
    home: &Path,
    tmux: &dyn TmuxRunner,
    available: bool,
    codex_consoles: &[CodexConsole],
) -> bool {
    if !available {
        logging::warn("tmux not installed; console disabled");
        return false;
    }
    let has = tmux.run(&["has-session".into(), "-t".into(), session.into()]);
    let mut have_session = has.0 == 0;
    let mut existing = HashSet::new();
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
    let wants_codex = cli_ids.iter().any(|id| id == "codex");
    // Every window the session should end up with: one per non-Codex CLI plus
    // one per registered Codex account.
    let mut desired: Vec<DesiredWindow> = Vec::new();
    for cli_id in cli_ids {
        if cli_id == "codex" {
            continue;
        }
        desired.push(DesiredWindow {
            window: cli_id.clone(),
            cli_id: cli_id.clone(),
            codex_home: None,
            instance_id: None,
        });
    }
    if wants_codex {
        if codex_consoles.is_empty() {
            // No ACC registry: the single historical window on the CLI's own
            // default `CODEX_HOME`, but only after ChatGPT login has finished.
            let default_home = home.join(".codex");
            if codex_home_authenticated(&default_home) {
                desired.push(DesiredWindow {
                    window: codex_console_window(1),
                    cli_id: "codex".to_string(),
                    codex_home: None,
                    instance_id: None,
                });
            } else {
                note_waiting_for_auth(&codex_console_window(1), &default_home);
            }
        }
        for console in codex_consoles {
            if !codex_home_authenticated(&console.home) {
                note_waiting_for_auth(&console.window, &console.home);
                continue;
            }
            desired.push(DesiredWindow {
                window: console.window.clone(),
                cli_id: "codex".to_string(),
                codex_home: Some(console.home.clone()),
                instance_id: console.instance_id,
            });
        }
    }
    if wants_codex {
        let codex_windows: HashSet<String> = desired
            .iter()
            .filter(|entry| entry.cli_id == "codex")
            .map(|entry| entry.window.clone())
            .collect();
        let desired_binary = resolve_cli_binary("codex", home);
        for entry in &desired {
            if let Some(codex_home) = &entry.codex_home {
                recreate_codex_window_if_stale(
                    session,
                    &mut existing,
                    tmux,
                    &entry.window,
                    codex_home,
                    desired_binary.as_deref(),
                );
            }
        }
        drop_unregistered_codex_windows(session, &mut existing, tmux, &codex_windows);
    }
    let mut created_any = have_session;
    for DesiredWindow {
        window,
        cli_id,
        codex_home,
        instance_id,
    } in &desired
    {
        if cli_binary_name(cli_id).is_none() {
            logging::debug(format!("cli {cli_id} has no tmux console; skipping window"));
            continue;
        }
        if existing.contains(window) {
            continue;
        }
        let Some(binary) = resolve_cli_binary(cli_id, home) else {
            note_missing_cli(cli_id);
            continue;
        };
        let mut command = vec![binary.to_string_lossy().into_owned()];
        if cli_id == "codex" && codex_supports_no_daemon(&binary) {
            command.push("--no-daemon".into());
        }
        let mut env_args = window_env_args(cli_id, codex_home.as_deref(), *instance_id);
        if !have_session {
            let mut args = vec!["new-session".into(), "-d".into()];
            args.append(&mut env_args);
            args.extend(["-s".into(), session.into(), "-n".into(), window.clone()]);
            args.extend(command.clone());
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
            args.extend(["-t".into(), session.into(), "-n".into(), window.clone()]);
            args.extend(command);
            let result = tmux.run(&args);
            if result.0 != 0 {
                logging::error(format!(
                    "failed to create console window {window}: {}",
                    result.2.trim()
                ));
            }
            created_any = created_any || result.0 == 0;
        }
        logging::info(format!(
            "created console window {window} in session {session}"
        ));
    }
    created_any
}

fn send_slash(tmux: &dyn TmuxRunner, target: &str, command: &str, sleeper: &dyn Fn(f64)) -> bool {
    for keys in [
        vec!["Escape".into()],
        vec!["Escape".into()],
        vec!["C-u".into()],
        vec!["-l".into(), command.into()],
        vec!["Enter".into()],
    ] {
        let mut args = vec!["send-keys".into(), "-t".into(), target.into()];
        args.extend(keys.clone());
        let result = tmux.run(&args);
        if result.0 != 0 {
            logging::warn(format!("send-keys to {target} failed: {}", result.2.trim()));
            return false;
        }
        if keys.first().map(String::as_str) == Some("Escape") {
            sleeper(0.1);
        } else if keys.first().map(String::as_str) == Some("-l") {
            // Let the TUI finish its paste burst; an immediate Enter can be
            // grouped into the paste and inserted as another composer newline.
            sleeper(0.3);
        }
    }
    true
}

/// Open the Codex `/usage` menu that prints the reset count.
///
/// Current Codex exposes the allowance in the bare menu. A literal newline is
/// composer input, not Enter: submit a key event instead. Never select redeem.
fn send_codex_usage_menu(tmux: &dyn TmuxRunner, target: &str, sleeper: &dyn Fn(f64)) -> bool {
    logging::debug(format!(
        "codex {target}: submitting bare /usage menu, not /usage daily"
    ));
    send_slash(tmux, target, "/usage", sleeper)
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
///
/// `window` names the console to drive; it differs from `cli_id` for the extra
/// Codex accounts, which each own a `codex-<id>` window running their own
/// `CODEX_HOME`.
pub fn refresh_and_capture(
    session: &str,
    cli_id: &str,
    window: &str,
    sleeper: &dyn Fn(f64),
    tmux: &dyn TmuxRunner,
) -> Option<String> {
    let target = format!("{session}:{window}");
    let command = slash_command(cli_id)?;
    let sent = if cli_id == "codex" {
        send_codex_usage_menu(tmux, &target, sleeper)
    } else {
        send_slash(tmux, &target, command, sleeper)
    };
    if !sent {
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
    use crate::test_support::{write_codex_auth, Sandbox};
    use std::sync::Mutex;

    struct FakeTmux {
        calls: Mutex<Vec<Vec<String>>>,
        has_session: bool,
        captures: Mutex<Vec<String>>,
        codex_home_env: Option<String>,
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
            if args.first().map(String::as_str) == Some("list-panes") {
                return (0, "1\n".into(), String::new());
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

        fn process_codex_home(&self, _pid: &str) -> Option<String> {
            self.codex_home_env.clone()
        }

        fn process_age_seconds(&self, _pid: &str) -> Option<f64> {
            None
        }

        fn process_cmdline(&self, _pid: &str) -> Option<String> {
            None
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
            codex_home_env: None,
        };
        let ids = vec!["codex".into(), "openrouter".into()];
        assert!(ensure_console("acs", &ids, &sandbox.home, &tmux, true, &[],));
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
        write_codex_auth(&sandbox.home.join(".codex"));
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: false,
            captures: Mutex::new(Vec::new()),
            codex_home_env: None,
        };
        assert!(ensure_console(
            "acs",
            &["claude_code".into(), "codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[],
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
    fn prefers_managed_codex_over_service_path() {
        let sandbox = Sandbox::new();
        let managed = sandbox
            .home
            .join(".local/share/linux_codex_claude_code_setup/npm/bin/codex");
        std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
        std::fs::write(&managed, "#!/bin/sh\n").unwrap();
        assert_eq!(resolve_cli_binary("codex", &sandbox.home), Some(managed));
    }

    #[test]
    fn codex_console_uses_no_daemon_when_installed_cli_supports_it() {
        let sandbox = Sandbox::new();
        let bin = sandbox
            .home
            .join(".local/share/linux_codex_claude_code_setup/npm/bin/codex");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(&bin, "#!/bin/sh\nprintf '%s\\n' '  --no-daemon'\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&bin, permissions).unwrap();
        let codex_home = sandbox.home.join(".codex-account-2");
        write_codex_auth(&codex_home);
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(Vec::new()),
            codex_home_env: None,
        };
        ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex-2".into(),
                home: codex_home,
                instance_id: Some(2),
            }],
        );
        let calls = tmux.calls.lock().unwrap();
        let created = calls
            .iter()
            .find(|args| args.first().map(String::as_str) == Some("new-window"))
            .unwrap();
        assert_eq!(
            created[created.len() - 2..],
            [bin.display().to_string(), "--no-daemon".into()]
        );
    }

    #[test]
    fn slash_commands_submit_with_enter_not_literal_newline() {
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(Vec::new()),
            codex_home_env: None,
        };
        let delays = std::cell::RefCell::new(Vec::new());
        assert!(send_codex_usage_menu(&tmux, "acs:codex", &|seconds| {
            delays.borrow_mut().push(seconds);
        }));
        assert_eq!(*delays.borrow(), vec![0.1, 0.1, 0.3]);
        assert!(send_slash(&tmux, "acs:codex", "/status", &noop_sleep));
        let calls = tmux.calls.lock().unwrap();
        for command in ["/usage", "/status"] {
            let index = calls
                .iter()
                .position(|call| call.last().map(String::as_str) == Some(command))
                .unwrap();
            assert_eq!(calls[index + 1].last().unwrap(), "Enter");
        }
        assert!(!calls.iter().flatten().any(|arg| arg.contains('\n')));
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

    fn sent_key_payloads(calls: &[Vec<String>]) -> Vec<String> {
        calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("send-keys"))
            .filter_map(|c| c.get(3).cloned())
            .collect()
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
            codex_home_env: None,
        };
        let text = refresh_and_capture("acs", "codex", "codex", &noop_sleep, &tmux).unwrap();
        assert!(text.contains("1 usage limit reset available"));
        assert!(text.contains("Weekly limit: 9% left"));
        let calls = tmux.calls.lock().unwrap().clone();
        let send_keys: Vec<_> = calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("send-keys"))
            .cloned()
            .collect();
        let expected: Vec<String> = ["send-keys", "-t", "acs:codex", "-l", "/status"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert!(send_keys.iter().any(|c| c == &expected));
        let payloads = sent_key_payloads(&calls);
        assert!(
            !payloads.iter().any(|k| k.contains("daily")),
            "typed /usage must not become /usage daily: {payloads:?}"
        );
        let usage_send = send_keys
            .iter()
            .find(|c| c.iter().any(|k| k.contains("/usage")))
            .expect("/usage must be typed");
        assert_eq!(
            usage_send,
            &["send-keys", "-t", "acs:codex", "-l", "/usage"]
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>(),
            "submit bare /usage with a separate Enter key: {send_keys:?}"
        );
        assert!(
            !usage_send
                .iter()
                .any(|k| k == "2" || k == "Down" || k == "BSpace"),
            "must not enter the redeem menu action: {usage_send:?}"
        );
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
            codex_home_env: None,
        };
        let text = refresh_and_capture("acs", "codex", "codex", &noop_sleep, &tmux).unwrap();
        assert!(text.contains("2 usage limit resets available"));
        assert!(text.contains("5h limit: 99% left"));
        let payloads = sent_key_payloads(&tmux.calls.lock().unwrap());
        assert!(
            !payloads.iter().any(|k| k.contains("daily")),
            "typed /usage must not become /usage daily: {payloads:?}"
        );
        let calls = tmux.calls.lock().unwrap().clone();
        let expected: Vec<String> = ["send-keys", "-t", "acs:codex", "-l", "/status"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let status_sends = calls.iter().filter(|c| *c == &expected).count();
        assert_eq!(status_sends, 2);
    }

    #[test]
    fn codex_window_sets_selected_codex_home() {
        let sandbox = Sandbox::new();
        let bin = sandbox.home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(bin.join("codex")).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(bin.join("codex"), p).unwrap();
        let original_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", format!("{}:/usr/bin", bin.display()));
        }
        let extra = sandbox.home.join(".codex-account-2");
        write_codex_auth(&extra);
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: false,
            captures: Mutex::new(Vec::new()),
            codex_home_env: None,
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex".into(),
                home: extra.clone(),
                instance_id: Some(1),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        let creation = calls
            .iter()
            .find(|c| c.first().map(String::as_str) == Some("new-session"))
            .unwrap();
        assert!(creation
            .iter()
            .any(|arg| arg == &format!("CODEX_HOME={}", extra.display())));
        unsafe {
            match original_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    /// tmux runner that reports the windows it was told exist and answers
    /// `CODEX_HOME` per window, the way two separate account consoles do.
    struct MultiWindowTmux {
        calls: Mutex<Vec<Vec<String>>>,
        windows: Vec<String>,
        homes: std::collections::HashMap<String, String>,
        /// Age every pane reports; None keeps the panes old enough to judge.
        pane_age_seconds: Option<f64>,
        /// Optional `/proc/<pid>/cmdline` per window pid (window name).
        cmdlines: std::collections::HashMap<String, String>,
    }

    impl TmuxRunner for MultiWindowTmux {
        fn run(&self, args: &[String]) -> (i32, String, String) {
            self.calls.lock().unwrap().push(args.to_vec());
            match args.first().map(String::as_str) {
                Some("has-session") => (0, String::new(), String::new()),
                Some("list-windows") => {
                    (0, format!("{}\n", self.windows.join("\n")), String::new())
                }
                // The pane "pid" is the window name so the fake can answer a
                // different CODEX_HOME per console.
                Some("list-panes") => {
                    let target = args.get(2).cloned().unwrap_or_default();
                    let window = target.split(':').next_back().unwrap_or("").to_string();
                    (0, format!("{window}\n"), String::new())
                }
                _ => (0, String::new(), String::new()),
            }
        }

        fn process_codex_home(&self, pid: &str) -> Option<String> {
            self.homes.get(pid).cloned()
        }

        fn process_age_seconds(&self, _pid: &str) -> Option<f64> {
            self.pane_age_seconds
        }

        fn process_cmdline(&self, pid: &str) -> Option<String> {
            self.cmdlines.get(pid).cloned()
        }
    }

    fn install_fake_codex(home: &Path) -> Option<std::ffi::OsString> {
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut mode = std::fs::metadata(bin.join("codex")).unwrap().permissions();
        mode.set_mode(0o755);
        std::fs::set_permissions(bin.join("codex"), mode).unwrap();
        let original = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", format!("{}:/usr/bin", bin.display()));
        }
        original
    }

    fn restore_path(original: Option<std::ffi::OsString>) {
        unsafe {
            match original {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn instance_two_gets_its_own_console_window() {
        assert_eq!(codex_console_window(1), "codex");
        assert_eq!(codex_console_window(2), "codex-2");
        assert_eq!(codex_console_window(3), "codex-3");
    }

    #[test]
    fn every_registered_account_gets_a_console_of_its_own() {
        // The reboot case: neither login has a rollout newer than the boot, so
        // both need their own /status console to report a 5h limit.
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let first = sandbox.home.join(".codex");
        let second = sandbox.home.join(".codex-account-2");
        write_codex_auth(&first);
        write_codex_auth(&second);
        let tmux = MultiWindowTmux {
            pane_age_seconds: None,
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex".into()],
            homes: std::collections::HashMap::from([(
                "codex".to_string(),
                first.display().to_string(),
            )]),
            cmdlines: std::collections::HashMap::new(),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[
                CodexConsole {
                    window: "codex".into(),
                    home: first.clone(),
                    instance_id: Some(1),
                },
                CodexConsole {
                    window: "codex-2".into(),
                    home: second.clone(),
                    instance_id: Some(2),
                },
            ],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        let created: Vec<&Vec<String>> = calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("new-window"))
            .collect();
        assert_eq!(created.len(), 1, "only the missing account is created");
        let joined = created[0].join(" ");
        assert!(joined.contains("-n codex-2"), "{joined}");
        assert!(
            joined.contains(&format!("CODEX_HOME={}", second.display())),
            "{joined}"
        );
        assert!(
            !calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("kill-window")),
            "a console already on its own account is left running"
        );
        // The launcher re-exports CODEX_HOME for the selected account, so the
        // window has to name the account it belongs to or it comes back logged
        // into the other one and is torn down on the next tick.
        assert!(joined.contains("ACC_CODEX_INSTANCE=2"), "{joined}");
        restore_path(original_path);
    }

    #[test]
    fn a_starting_console_is_left_to_settle_instead_of_recreated() {
        // A pane that is still walking its launcher exec chain reports the
        // pre-exec CODEX_HOME. Killing it then restarts that wait every tick,
        // so the account never lives long enough to answer /status.
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let second = sandbox.home.join(".codex-account-2");
        write_codex_auth(&second);
        let tmux = MultiWindowTmux {
            pane_age_seconds: Some(3.0),
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex-2".into()],
            homes: std::collections::HashMap::from([(
                "codex-2".to_string(),
                sandbox.home.join(".codex").display().to_string(),
            )]),
            cmdlines: std::collections::HashMap::new(),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex-2".into(),
                home: second,
                instance_id: Some(2),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        assert!(
            !calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("kill-window")),
            "a console younger than the settle window is left alone"
        );
        restore_path(original_path);
    }

    #[test]
    fn a_settled_console_on_the_wrong_account_is_recreated() {
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let second = sandbox.home.join(".codex-account-2");
        write_codex_auth(&second);
        let tmux = MultiWindowTmux {
            pane_age_seconds: Some(CONSOLE_SETTLE_SECONDS + 1.0),
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex-2".into()],
            homes: std::collections::HashMap::from([(
                "codex-2".to_string(),
                sandbox.home.join(".codex").display().to_string(),
            )]),
            cmdlines: std::collections::HashMap::new(),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex-2".into(),
                home: second,
                instance_id: Some(2),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        let killed: Vec<&Vec<String>> = calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("kill-window"))
            .collect();
        assert_eq!(killed.len(), 1, "the stale login is rebuilt once settled");
        assert_eq!(killed[0][2], "acs:codex-2");
        restore_path(original_path);
    }

    #[test]
    fn a_console_on_an_old_codex_binary_is_recreated() {
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let first = sandbox.home.join(".codex");
        write_codex_auth(&first);
        let wanted = sandbox.home.join("bin").join("codex");
        let tmux = MultiWindowTmux {
            pane_age_seconds: Some(CONSOLE_SETTLE_SECONDS + 1.0),
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex".into()],
            homes: std::collections::HashMap::from([(
                "codex".to_string(),
                first.display().to_string(),
            )]),
            cmdlines: std::collections::HashMap::from([(
                "codex".to_string(),
                "node\0/usr/local/bin/codex".into(),
            )]),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex".into(),
                home: first,
                instance_id: Some(1),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("kill-window")),
            "a console still running /usr/local/bin/codex is rebuilt onto {}",
            wanted.display()
        );
        restore_path(original_path);
    }

    #[test]
    fn console_of_an_unregistered_account_is_closed() {
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let first = sandbox.home.join(".codex");
        write_codex_auth(&first);
        let tmux = MultiWindowTmux {
            pane_age_seconds: None,
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex".into(), "codex-3".into()],
            homes: std::collections::HashMap::from([
                ("codex".to_string(), first.display().to_string()),
                (
                    "codex-3".to_string(),
                    sandbox.home.join(".codex-account-3").display().to_string(),
                ),
            ]),
            cmdlines: std::collections::HashMap::new(),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex".into(),
                home: first,
                instance_id: Some(1),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        let killed: Vec<&Vec<String>> = calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("kill-window"))
            .collect();
        assert_eq!(killed.len(), 1);
        assert_eq!(killed[0][2], "acs:codex-3");
        restore_path(original_path);
    }

    #[test]
    fn stale_codex_console_is_recreated_for_the_selected_home() {
        let sandbox = Sandbox::new();
        let bin = sandbox.home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(bin.join("codex")).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(bin.join("codex"), p).unwrap();
        let original_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", format!("{}:/usr/bin", bin.display()));
        }
        let extra = sandbox.home.join(".codex-account-2");
        write_codex_auth(&extra);
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(Vec::new()),
            codex_home_env: Some(sandbox.home.join(".codex").display().to_string()),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex".into(),
                home: extra.clone(),
                instance_id: Some(1),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        assert!(calls
            .iter()
            .any(|c| c.first().map(String::as_str) == Some("kill-window")));
        let creation = calls
            .iter()
            .find(|c| c.first().map(String::as_str) == Some("new-window"))
            .unwrap();
        assert!(creation
            .iter()
            .any(|arg| arg == &format!("CODEX_HOME={}", extra.display())));
        unsafe {
            match original_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn matching_pane_codex_home_does_not_recreate_console() {
        let sandbox = Sandbox::new();
        let bin = sandbox.home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex"), "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(bin.join("codex")).unwrap().permissions();
        p.set_mode(0o755);
        std::fs::set_permissions(bin.join("codex"), p).unwrap();
        let original_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("PATH", format!("{}:/usr/bin", bin.display()));
        }
        let extra = sandbox.home.join(".codex-account-2");
        write_codex_auth(&extra);
        let tmux = FakeTmux {
            calls: Mutex::new(Vec::new()),
            has_session: true,
            captures: Mutex::new(Vec::new()),
            codex_home_env: Some(extra.display().to_string()),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex".into(),
                home: extra.clone(),
                instance_id: Some(1),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        assert!(calls
            .iter()
            .any(|c| c.first().map(String::as_str) == Some("list-panes")));
        assert!(calls
            .iter()
            .all(|c| c.first().map(String::as_str) != Some("kill-window")));
        unsafe {
            match original_path {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
        }
    }

    #[test]
    fn unauthenticated_account_does_not_start_a_console() {
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let extra = sandbox.home.join(".codex-account-3");
        std::fs::create_dir_all(&extra).unwrap();
        let tmux = MultiWindowTmux {
            pane_age_seconds: None,
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex".into()],
            homes: std::collections::HashMap::from([(
                "codex".to_string(),
                sandbox.home.join(".codex").display().to_string(),
            )]),
            cmdlines: std::collections::HashMap::new(),
        };
        write_codex_auth(&sandbox.home.join(".codex"));
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[
                CodexConsole {
                    window: "codex".into(),
                    home: sandbox.home.join(".codex"),
                    instance_id: Some(1),
                },
                CodexConsole {
                    window: "codex-3".into(),
                    home: extra,
                    instance_id: Some(3),
                },
            ],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        assert!(
            !calls.iter().any(|c| {
                c.first().map(String::as_str) == Some("new-window")
                    && c.join(" ").contains("-n codex-3")
            }),
            "an account still waiting on login must not start a codex TUI"
        );
        restore_path(original_path);
    }

    #[test]
    fn unauthenticated_console_is_closed() {
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let extra = sandbox.home.join(".codex-account-3");
        std::fs::create_dir_all(&extra).unwrap();
        write_codex_auth(&sandbox.home.join(".codex"));
        let tmux = MultiWindowTmux {
            pane_age_seconds: None,
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex".into(), "codex-3".into()],
            homes: std::collections::HashMap::from([
                (
                    "codex".to_string(),
                    sandbox.home.join(".codex").display().to_string(),
                ),
                ("codex-3".to_string(), extra.display().to_string()),
            ]),
            cmdlines: std::collections::HashMap::new(),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[
                CodexConsole {
                    window: "codex".into(),
                    home: sandbox.home.join(".codex"),
                    instance_id: Some(1),
                },
                CodexConsole {
                    window: "codex-3".into(),
                    home: extra,
                    instance_id: Some(3),
                },
            ],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        let killed: Vec<&Vec<String>> = calls
            .iter()
            .filter(|c| c.first().map(String::as_str) == Some("kill-window"))
            .collect();
        assert_eq!(killed.len(), 1);
        assert_eq!(killed[0][2], "acs:codex-3");
        restore_path(original_path);
    }

    #[test]
    fn console_started_before_auth_is_recreated() {
        let sandbox = Sandbox::new();
        let original_path = install_fake_codex(&sandbox.home);
        let extra = sandbox.home.join(".codex-account-3");
        write_codex_auth(&extra);
        let tmux = MultiWindowTmux {
            pane_age_seconds: Some(120.0),
            calls: Mutex::new(Vec::new()),
            windows: vec!["codex-3".into()],
            homes: std::collections::HashMap::from([(
                "codex-3".to_string(),
                extra.display().to_string(),
            )]),
            cmdlines: std::collections::HashMap::new(),
        };
        assert!(ensure_console(
            "acs",
            &["codex".into()],
            &sandbox.home,
            &tmux,
            true,
            &[CodexConsole {
                window: "codex-3".into(),
                home: extra,
                instance_id: Some(3),
            }],
        ));
        let calls = tmux.calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|c| c.first().map(String::as_str) == Some("kill-window")),
            "a login-loop pane that started before auth.json must be rebuilt"
        );
        restore_path(original_path);
    }

    #[test]
    fn reads_codex_home_from_proc_environ() {
        let pid = std::process::id().to_string();
        let path = read_proc_environ_var(&pid, "PATH");
        assert!(
            path.as_deref().is_some_and(|value| !value.is_empty()),
            "current process should expose PATH in /proc/pid/environ"
        );
        assert!(read_proc_environ_var("not-a-pid", "PATH").is_none());
    }
}
