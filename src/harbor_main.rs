//! Standalone Harbor job observer; no ACC runtime or launch hooks required.
use clap::Parser;
use chrono::TimeZone;
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
/// Returns: normalized job and parsed config/result, or None for unavailable artifacts.
fn read_job(path: &Path) -> Option<(Value, Value, Value)> {
    println!("path={path:?}");
    let config = read_object(&path.join("config.json"));
    let data = read_object(&path.join("result.json"));
    let result = if config["job_name"].is_string() && data["started_at"].is_string() && data["stats"].is_object() {
        let counts = trial_counts(&data);
        Some((json!({"id": path.canonicalize().unwrap_or_else(|_| path.to_path_buf()), "name": config["job_name"],
            "status": job_status(&data, counts),
            "completed": counts.map(|(completed,_)| completed), "total": counts.map(|(_,total)| total),
            "fraction": counts.map(|(completed,total)| completed as f64 / total as f64),
            "started_at": data["started_at"], "finished_at": data["finished_at"]}), config, data))
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
    let mut history_roots = jobs_dirs.to_vec();
    for metadata in live.values() {
        if let Some(cwd) = metadata["cwd"].as_str() { history_roots.push(PathBuf::from(cwd)); }
    }
    let history = load_history(&history_roots, state_dir)?;
    let mut jobs = Vec::new();
    for (path, metadata) in paths {
        if let Some((mut row, config, data)) = read_job(Path::new(&path)) {
            if let Some(object) = metadata.as_object() {
                for (key, value) in object { row[key] = value.clone(); }
                if !live.contains_key(&path) && row["status"] == "running" { row["status"] = json!("interrupted"); }
            }
            row["estimate"] = estimate_job(&path, &row, &config, &data, &history);
            let finished_age = timestamp(&row["finished_at"]).map(|at| chrono::Utc::now().timestamp() as f64 - at);
            if finished_age.is_none_or(|age| age < 300.0) { jobs.push(row); }
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
                    result.insert(canonical.to_string_lossy().into_owned(), json!({"pid": pid, "owner_pids": terminal_owners(proc_root, pid, &ancestors(proc_root, pid)), "cwd": cwd}));
                }
            }
        }
    }
    println!("{result:?}");
    result
}

/// Convert Harbor's offset-aware or local-naive timestamps into Unix seconds.
/// Parameters: value - timestamp JSON string.
/// Returns: Unix seconds or None for invalid or ambiguous local times.
fn timestamp(value: &Value) -> Option<f64> {
    println!("value={value}");
    let result = value.as_str().and_then(|text| {
        chrono::DateTime::parse_from_rfc3339(text).ok().map(|date| date.timestamp_millis() as f64 / 1000.0)
            .or_else(|| chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()
                .and_then(|date| chrono::Local.from_local_datetime(&date).single())
                .map(|date| date.timestamp_millis() as f64 / 1000.0))
    });
    println!("{result:?}");
    result
}

/// Match comparable task sets, agent models/efforts and environment types.
/// Parameters: config - parsed Harbor job configuration.
/// Returns: stable workload identity, excluding private provider configuration.
fn workload_signature(config: &Value) -> Value {
    println!("config_keys={:?}", config.as_object().map(|value| value.keys().collect::<Vec<_>>()));
    let mut tasks: Vec<_> = config["tasks"].as_array().into_iter().flatten()
        .filter_map(|task| task["path"].as_str().and_then(|path| Path::new(path).file_name()).map(|name| name.to_string_lossy().into_owned())).collect();
    tasks.sort();
    let agents: Vec<_> = config["agents"].as_array().into_iter().flatten().map(|agent| json!({
        "model": agent["model_name"], "name": agent["name"], "import": agent["import_path"],
        "effort": agent["kwargs"]["reasoning_effort"]})).collect();
    let datasets: Vec<_> = config["datasets"].as_array().into_iter().flatten().map(|dataset| json!({
        "name": dataset["name"], "version": dataset["version"], "path": dataset["path"]})).collect();
    let result = json!({"tasks": tasks, "datasets": datasets, "agents": agents, "environment": config["environment"]["type"]});
    println!("{result}");
    result
}

/// Calculate scheduler waves from the configured trial capacity.
/// Parameters: config - parsed job configuration; data - Harbor result.
/// Returns: number of occupied concurrency waves, or None without a trial total.
fn concurrency_waves(config: &Value, data: &Value) -> Option<f64> {
    println!("concurrency={} total={}", config["n_concurrent_trials"], data["n_total_trials"]);
    let result = data["n_total_trials"].as_u64().filter(|total| *total > 0).map(|total| {
        let capacity = config["n_concurrent_trials"].as_u64().unwrap_or(1).max(1);
        total.div_ceil(capacity) as f64
    });
    println!("{result:?}");
    result
}

/// Accept only fully completed runs with usable positive wall-clock duration.
/// Parameters: config - parsed configuration; data - Harbor job result.
/// Returns: seconds per occupied concurrency wave, or None for incomplete history.
fn completed_sample(config: &Value, data: &Value) -> Option<f64> {
    println!("job_name={} started={} finished={}", config["job_name"], data["started_at"], data["finished_at"]);
    let result = (|| {
        if job_status(data, trial_counts(data)) != "complete" { return None; }
        let duration = timestamp(&data["finished_at"])? - timestamp(&data["started_at"])?;
        if duration <= 0.0 { return None; }
        Some(duration / concurrency_waves(config, data)?)
    })();
    println!("{result:?}");
    result
}

/// Scan a bounded local directory tree for genuine completed Harbor jobs.
/// Parameters: root - current directory; depth - remaining directory levels; budget - remaining visits; samples - collected history.
/// Returns: None after extending the sample list.
fn scan_history(root: &Path, depth: u8, budget: &mut usize, samples: &mut Vec<Value>) {
    println!("root={root:?} depth={depth} budget={budget} samples={samples:?}");
    if *budget == 0 { println!("None"); return; }
    *budget -= 1;
    if root.join("config.json").is_file() && root.join("result.json").is_file() {
        if let Some((_, config, data)) = read_job(root) {
            if let Some(seconds) = completed_sample(&config, &data) {
                samples.push(json!({"path": root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
                    "signature": workload_signature(&config), "seconds_per_wave": seconds, "finished_at": data["finished_at"]}));
            }
        }
    } else if depth > 0 {
        let mut directories: Vec<_> = fs::read_dir(root).into_iter().flatten().flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()) && !entry.file_name().to_string_lossy().starts_with('.')
                && !["node_modules", "target", "venv", "__pycache__"].contains(&entry.file_name().to_string_lossy().as_ref()))
            .map(|entry| entry.path()).collect();
        directories.sort_by_key(|path| std::cmp::Reverse(fs::metadata(path).and_then(|m| m.modified()).ok()));
        for directory in directories { scan_history(&directory, depth - 1, budget, samples); if *budget == 0 { break; } }
    }
    println!("None");
}

/// Cache a bounded historical scan outside the Shell rendering process.
/// Parameters: roots - known artifact containers and Harbor launch directories; state_dir - observer state.
/// Returns: cached completed-run sample list, or filesystem error.
fn load_history(roots: &[PathBuf], state_dir: &Path) -> std::io::Result<Vec<Value>> {
    println!("roots={roots:?} state_dir={state_dir:?}");
    let saved = read_object(&state_dir.join("history.json"));
    let now = chrono::Utc::now().timestamp() as f64;
    let mut samples = saved["samples"].as_array().cloned().unwrap_or_default();
    if timestamp(&saved["updated_at"]).is_none_or(|at| now - at > 60.0) {
        samples.clear();
        let mut budget = 20000;
        for root in roots { scan_history(root, 5, &mut budget, &mut samples); }
        samples.sort_by(|a,b| b["finished_at"].as_str().cmp(&a["finished_at"].as_str()));
        let mut seen = std::collections::BTreeSet::new();
        samples.retain(|sample| seen.insert(sample["path"].to_string()));
        samples.truncate(1000);
        write_object(&state_dir.join("history.json"), &json!({"updated_at": chrono::Utc::now().to_rfc3339(), "samples": samples}))?;
    }
    println!("{samples:?}");
    Ok(samples)
}

/// Forecast a running job using the median of comparable completed runs.
/// Parameters: path - current job identity; row - normalized job state; config - parsed configuration; data - parsed result; history - completed samples.
/// Returns: total/remaining seconds and sample count, or null when no estimate is supported.
fn estimate_job(path: &str, row: &Value, config: &Value, data: &Value, history: &[Value]) -> Value {
    println!("path={path} row={row} job_name={} total={} history_count={}", config["job_name"], data["n_total_trials"], history.len());
    let signature = workload_signature(config);
    let mut samples: Vec<f64> = history.iter().filter(|sample| sample["path"] != path && sample["signature"] == signature)
        .take(20).filter_map(|sample| sample["seconds_per_wave"].as_f64()).filter(|seconds| seconds.is_finite() && *seconds > 0.0).collect();
    samples.sort_by(f64::total_cmp);
    let result = if row["status"] == "running" && !samples.is_empty() {
        match (concurrency_waves(config, data), timestamp(&data["started_at"])) {
            (Some(waves), Some(start)) => {
                let middle = samples.len() / 2;
                let median = if samples.len() % 2 == 0 { (samples[middle - 1] + samples[middle]) / 2.0 } else { samples[middle] };
                let total = median * waves;
                let elapsed = (chrono::Utc::now().timestamp_millis() as f64 / 1000.0 - start).max(0.0);
                json!({"total_seconds": total, "remaining_seconds": (total - elapsed).max(0.0), "samples": samples.len()})
            }
            _ => Value::Null,
        }
    } else { Value::Null };
    println!("{result}");
    result
}

/// Accept consistent nonzero Harbor trial counts, including legacy result names.
/// Parameters: data - parsed Harbor result.
/// Returns: completed/total counts or None for unavailable or inconsistent progress.
fn trial_counts(data: &Value) -> Option<(u64, u64)> {
    println!("data={data}");
    let result = (|| {
        let total = data["n_total_trials"].as_u64()?;
        let completed = data["stats"]["n_completed_trials"].as_u64().or_else(|| data["stats"]["n_trials"].as_u64())?;
        if total == 0 || completed > total { return None; }
        Some((completed, total))
    })();
    println!("{result:?}");
    result
}

/// Classify finalization without presenting cancelled or incomplete runs as done.
/// Parameters: data - parsed result; counts - validated completed/total pair.
/// Returns: running, complete or interrupted.
fn job_status(data: &Value, counts: Option<(u64, u64)>) -> &'static str {
    println!("data={data} counts={counts:?}");
    let result = if !data["finished_at"].is_string() { "running" }
        else if counts.is_some_and(|(done,total)| done == total) && data["stats"]["n_cancelled_trials"].as_u64().unwrap_or(0) == 0 { "complete" }
        else { "interrupted" };
    println!("{result}");
    result
}

/// Resolve attached tmux clients rather than a pane's stale original window.
/// Parameters: proc_root - process filesystem; pid - Harbor pid; parents - direct process ancestors.
/// Returns: terminal/client ancestry, or direct ancestry outside tmux; empty for detached tmux.
fn terminal_owners(proc_root: &Path, pid: u32, parents: &[u32]) -> Vec<u32> {
    println!("proc_root={proc_root:?} pid={pid} parents={parents:?}");
    let socket = fs::read(proc_root.join(pid.to_string()).join("environ")).ok().and_then(|bytes| {
        bytes.split(|byte| *byte == 0).find_map(|part| part.strip_prefix(b"TMUX=")
            .and_then(|value| String::from_utf8(value.to_vec()).ok())).map(|value| value.split(',').next().unwrap_or_default().to_owned())
    });
    let result = if let Some(socket) = socket {
        let query = |args: &[&str]| std::process::Command::new("timeout").args(["1", "tmux", "-S", &socket]).args(args)
            .output().ok().filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
        let panes = query(&["list-panes", "-a", "-F", "#{pane_pid}\t#{session_id}"]);
        let session = panes.lines().filter_map(|line| line.split_once('\t'))
            .find(|(pane,_)| pane.parse::<u32>().is_ok_and(|pid| parents.contains(&pid))).map(|(_,session)| session.to_owned());
        let clients = query(&["list-clients", "-F", "#{session_id}\t#{client_pid}"]);
        clients.lines().filter_map(|line| line.split_once('\t')).filter(|(candidate,_)| session.as_deref() == Some(candidate))
            .filter_map(|(_,pid)| pid.parse::<u32>().ok()).flat_map(|pid| ancestors(proc_root, pid)).collect()
    } else { parents.to_vec() };
    println!("{result:?}");
    result
}
