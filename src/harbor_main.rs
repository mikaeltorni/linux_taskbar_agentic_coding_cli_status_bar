//! Standalone Harbor job observer; no ACC runtime or launch hooks required.
use clap::Parser;
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}, process::ExitCode, time::Duration, collections::BTreeMap};
use std::os::unix::fs::MetadataExt;

#[derive(Parser, Debug)]
#[command(about = "Independently track Harbor jobs in the taskbar")]
struct Options {
    /// Job container directories (repeatable; default ~/jobs).
    #[arg(long = "jobs-dir")]
    jobs_dirs: Vec<PathBuf>,
    /// Snapshot directory (default $XDG_STATE_HOME/harbor-taskbar).
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// Process filesystem (default /proc; override for isolated diagnostics).
    #[arg(long, default_value = "/proc")]
    proc_root: PathBuf,
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
/// Parameters: jobs_dirs - artifact containers; state_dir - output state directory; proc_root - process filesystem.
/// Returns: filesystem success or error.
fn poll(jobs_dirs: &[PathBuf], state_dir: &Path, proc_root: &Path) -> std::io::Result<()> {
    println!("jobs_dirs={jobs_dirs:?} state_dir={state_dir:?} proc_root={proc_root:?}");
    let live = discover_processes(proc_root);
    let saved = read_object(&state_dir.join("watched.json"));
    let mut watched: BTreeMap<String, Value> = saved.as_object().map(|rows| rows.iter().map(|(k,v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
    for (path, metadata) in &live { watched.insert(path.clone(), metadata.clone()); }
    let mut paths: BTreeMap<String, Value> = watched.clone();
    for root in jobs_dirs {
        if let Ok(entries) = fs::read_dir(root) {
            for path in entries.flatten() {
                if path.path().join("config.json").is_file() {
                    paths.entry(path.path().canonicalize().unwrap_or(path.path()).to_string_lossy().into_owned()).or_insert(Value::Null);
                }
            }
        }
    }
    let mut jobs = Vec::new();
    for (path, metadata) in paths {
        if let Some(mut row) = read_job(Path::new(&path)) {
            if let Some(object) = metadata.as_object() {
                for (key, value) in object { row[key] = value.clone(); }
                if !live.contains_key(&path) && row["status"] == "running" { row["status"] = json!("interrupted"); }
            }
            jobs.push(row);
        }
    }
    write_object(&state_dir.join("watched.json"), &json!(watched))?;
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
        poll(&roots, &options.state_dir, &options.proc_root)?;
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

/// Parse a Harbor process's raw command syntax and normalize its artifact search.
/// Parameters: args - raw process argv; cwd - process launch directory.
/// Returns: artifact container and optional exact job directory, or None for unrelated argv.
fn parse_launch(args: &[String], cwd: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    println!("args_count={} cwd={cwd:?}", args.len());
    // Print no raw argv: unrelated flags can contain provider credentials.
    let base = |arg: &str| Path::new(arg).file_name().unwrap_or_default().to_string_lossy().into_owned();
    let offset = if args.first().is_some_and(|arg| base(arg) == "harbor") { 1 }
        else if args.first().is_some_and(|arg| base(arg).starts_with("python")) && args.get(1).is_some_and(|arg| base(arg) == "harbor") { 2 }
        else if args.get(1).is_some_and(|arg| arg == "-m") && args.get(2).is_some_and(|arg| arg == "harbor") { 3 }
        else { println!("None"); return None; };
    let tokens: Vec<String> = args[offset..].iter().flat_map(|arg| {
        if arg.starts_with('-') { arg.splitn(2, '=').map(str::to_owned).collect::<Vec<_>>() } else { vec![arg.clone()] }
    }).collect();
    let resume = tokens.starts_with(&["jobs".to_owned(), "resume".to_owned()]);
    if !resume && tokens.first().is_none_or(|arg| arg != "run") && !tokens.starts_with(&["jobs".to_owned(), "start".to_owned()]) {
        println!("None"); return None;
    }
    let flag = |names: &[&str]| tokens.windows(2).rev().find(|pair| names.contains(&pair[0].as_str())).map(|pair| pair[1].clone());
    let resolve = |text: &str| { let path = PathBuf::from(text); if path.is_absolute() { path } else { cwd.join(path) } };
    let result = if resume {
        flag(&["-p", "--job-path"]).map(|path| { let job = resolve(&path); (job.parent().unwrap_or(cwd).to_path_buf(), Some(job)) })
    } else {
        let config = flag(&["-c", "--config"]).and_then(|path| fs::read_to_string(resolve(&path)).ok())
            .and_then(|text| serde_yaml::from_str::<Value>(&text).ok()).unwrap_or(Value::Null);
        let root = resolve(&flag(&["-o", "--jobs-dir"]).or_else(|| config["jobs_dir"].as_str().map(str::to_owned)).unwrap_or_else(|| "jobs".into()));
        let job = flag(&["--job-name"]).or_else(|| config["job_name"].as_str().map(str::to_owned)).map(|name| root.join(name));
        Some((root, job))
    };
    println!("{result:?}");
    result
}

/// Read a process's parent chain without depending on ACC registry metadata.
/// Parameters: proc_root - process filesystem; pid - process identity.
/// Returns: process and ancestor ids, bounded against malformed cycles.
fn ancestors(proc_root: &Path, pid: u32) -> Vec<u32> {
    println!("proc_root={proc_root:?} pid={pid}");
    let mut result = Vec::new();
    let mut current = pid;
    while current > 1 && result.len() < 64 && !result.contains(&current) {
        result.push(current);
        current = fs::read_to_string(proc_root.join(current.to_string()).join("stat")).ok()
            .and_then(|text| text.rsplit_once(") ").and_then(|(_, rest)| rest.split_whitespace().nth(1)).and_then(|value| value.parse().ok())).unwrap_or(0);
    }
    println!("{result:?}");
    result
}

/// Discover user-owned native Harbor processes and their artifact directories.
/// Parameters: proc_root - process filesystem.
/// Returns: canonical job paths mapped to process/terminal metadata only.
fn discover_processes(proc_root: &Path) -> BTreeMap<String, Value> {
    println!("proc_root={proc_root:?}");
    let mut result = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(proc_root) {
        for entry in entries.flatten() {
            let process = entry.path();
            let Some(pid) = entry.file_name().to_str().and_then(|text| text.parse::<u32>().ok()) else { continue; };
            if fs::metadata(&process).ok().is_none_or(|metadata| metadata.uid() != unsafe { libc::geteuid() }) { continue; }
            let Ok(raw) = fs::read(process.join("cmdline")) else { continue; };
            let args: Vec<String> = raw.split(|byte| *byte == 0).filter(|arg| !arg.is_empty()).map(|arg| String::from_utf8_lossy(arg).into_owned()).collect();
            // Filter before parsing so other users' argument data is never traced.
            if !args.iter().take(3).any(|arg| Path::new(arg).file_name().is_some_and(|name| name == "harbor")) { continue; }
            let Ok(cwd) = fs::read_link(process.join("cwd")) else { continue; };
            let Some((root, exact)) = parse_launch(&args, &cwd) else { continue; };
            let paths = exact.map(|path| vec![path]).unwrap_or_else(|| {
                let mut candidates: Vec<_> = fs::read_dir(root).into_iter().flatten().flatten().map(|entry| entry.path())
                    .filter(|path| path.join("config.json").is_file()).collect();
                candidates.sort_by_key(|path| fs::metadata(path.join("config.json")).and_then(|m| m.modified()).ok());
                candidates.into_iter().rev().take(1).collect()
            });
            for path in paths {
                if let Ok(canonical) = path.canonicalize() {
                    result.insert(canonical.to_string_lossy().into_owned(), json!({"pid": pid, "owner_pids": ancestors(proc_root, pid), "cwd": cwd}));
                }
            }
        }
    }
    println!("{result:?}");
    result
}
