//! Standalone Harbor job observer; no ACC runtime or launch hooks required.
use clap::Parser;
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}, process::ExitCode, time::Duration};

#[derive(Parser, Debug)]
#[command(about = "Independently track Harbor jobs in the taskbar")]
struct Options {
    /// Job container directories (repeatable; default ~/jobs).
    #[arg(long = "jobs-dir")]
    jobs_dirs: Vec<PathBuf>,
    /// Snapshot directory (default $XDG_STATE_HOME/harbor-taskbar).
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// Polling delay in seconds.
    #[arg(long, default_value_t = 2.0)]
    interval: f64,
    /// Publish one snapshot and exit.
    #[arg(long)]
    once: bool,
}

/// Resolve the independent observer's default state directory.
/// Parameters: none.
/// Returns: the XDG state home plus harbor-taskbar.
fn default_state_dir() -> PathBuf {
    println!("parameters=none");
    let base = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"));
    let result = base.join("harbor-taskbar");
    println!("{result:?}");
    result
}

/// Read a JSON object while tolerating partial or absent artifact writes.
/// Parameters: path - artifact file.
/// Returns: parsed object or null when unavailable.
fn read_object(path: &Path) -> Value {
    println!("path={path:?}");
    let result = fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object).unwrap_or(Value::Null);
    println!("{result}");
    result
}

/// Read one Harbor job's current artifact status.
/// Parameters: path - Harbor job directory.
/// Returns: normalized job or None for unavailable artifacts.
fn read_job(path: &Path) -> Option<Value> {
    println!("path={path:?}");
    let config = read_object(&path.join("config.json"));
    let data = read_object(&path.join("result.json"));
    let result = if config["job_name"].is_string() && data["started_at"].is_string() && data["stats"].is_object() {
        Some(json!({"id": path.canonicalize().unwrap_or_else(|_| path.to_path_buf()), "name": config["job_name"],
            "status": if data["finished_at"].is_string() { "complete" } else { "running" },
            "started_at": data["started_at"], "finished_at": data["finished_at"]}))
    } else { None };
    println!("{result:?}");
    result
}

/// Atomically replace the independent watcher's JSON state.
/// Parameters: path - destination; data - complete JSON value.
/// Returns: filesystem success or error.
fn write_object(path: &Path, data: &Value) -> std::io::Result<()> {
    println!("path={path:?} data={data}");
    fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, data.to_string())?;
    fs::rename(temporary, path)?;
    println!("Ok(())");
    Ok(())
}

/// Accept a finite positive polling delay.
/// Parameters: interval - converted seconds.
/// Returns: accepted duration or an invalid-input error.
fn require_interval(interval: f64) -> std::io::Result<Duration> {
    println!("interval={interval}");
    let result = Duration::try_from_secs_f64(interval).ok().filter(|value| !value.is_zero())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "interval must be finite and positive"));
    println!("{result:?}");
    result
}

/// Collect current Harbor jobs and publish the taskbar snapshot.
/// Parameters: jobs_dirs - artifact containers; state_dir - output state directory.
/// Returns: filesystem success or error.
fn poll(jobs_dirs: &[PathBuf], state_dir: &Path) -> std::io::Result<()> {
    println!("jobs_dirs={jobs_dirs:?} state_dir={state_dir:?}");
    let mut jobs = Vec::new();
    for root in jobs_dirs {
        if let Ok(paths) = fs::read_dir(root) {
            for path in paths.flatten() {
                if let Some(row) = read_job(&path.path()) { jobs.push(row); }
            }
        }
    }
    jobs.sort_by_key(|row| row["id"].as_str().unwrap_or_default().to_owned());
    write_object(&state_dir.join("status.json"), &json!({"updated_at": chrono::Utc::now().to_rfc3339(), "jobs": jobs}))?;
    println!("Ok(())");
    Ok(())
}

/// Run the selected polling operation without parsing raw input again.
/// Parameters: options - parsed watcher paths, delay and execution mode.
/// Returns: polling success or error.
fn watch(options: Options) -> std::io::Result<()> {
    println!("options={options:?}");
    let delay = require_interval(options.interval)?;
    let roots = if options.jobs_dirs.is_empty() {
        vec![PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("jobs")]
    } else { options.jobs_dirs };
    loop {
        poll(&roots, &options.state_dir)?;
        if options.once { break; }
        std::thread::sleep(delay);
    }
    println!("Ok(())");
    Ok(())
}

/// Parse once and route to the independent Harbor watch operation.
/// Parameters: none.
/// Returns: success or failure process exit code.
fn main() -> ExitCode {
    println!("parameters=none");
    let result = match watch(Options::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("Harbor watcher: {error}"); ExitCode::FAILURE }
    };
    println!("{result:?}");
    result
}
