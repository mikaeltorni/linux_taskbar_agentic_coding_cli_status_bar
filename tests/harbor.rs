//! Public standalone Harbor observer and taskbar checks.
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

/// Run the public observer with isolated artifact and output directories.
/// Parameters: root - sandbox; extra - additional CLI tokens.
/// Returns: the independently published taskbar snapshot.
fn poll(root: &Path, extra: &[&str]) -> Value {
    println!("root={root:?} extra={extra:?}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar"));
    command
        .args(["--once", "--jobs-dir"])
        .arg(root.join("jobs"))
        .arg("--state-dir")
        .arg(root.join("state"));
    if !extra.contains(&"--proc-root") {
        command.arg("--proc-root").arg(root.join("proc"));
    }
    if !extra.contains(&"--tts-command") {
        command.arg("--tts-command").arg(root.join("no-engine"));
    }
    let output = command.args(extra).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result =
        serde_json::from_str(&fs::read_to_string(root.join("state/status.json")).unwrap()).unwrap();
    println!("{result}");
    result
}

/// Save realistic Harbor job artifacts.
/// Parameters: root - sandbox; name - job name; data - Harbor result.
/// Returns: job directory.
fn job(root: &Path, name: &str, data: Value) -> std::path::PathBuf {
    println!("root={root:?} name={name} data={data}");
    let path = root.join("jobs").join(name);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("config.json"),
        json!({"job_name": name, "tasks": [{"path": "addition"}], "n_attempts": 4}).to_string(),
    )
    .unwrap();
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    println!("{path:?}");
    path
}

/// Track current jobs through the public standalone binary and tolerate partial writes.
/// Parameters: none.
/// Returns: None when all snapshot and rejection checks pass.
#[test]
fn artifact_tracking() {
    println!("parameters=none");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut data = json!({"id": "demo", "started_at": "2026-10-20T00:00:00Z", "finished_at": null,
        "n_total_trials": 4, "stats": {"n_completed_trials": 1}});
    let path = job(root, "demo", data.clone());
    let rows = poll(root, &[]);
    assert_eq!(rows["jobs"][0]["name"], "demo");
    assert_eq!(rows["jobs"][0]["status"], "running");
    data["finished_at"] = json!("2026-10-20T00:10:00Z");
    data["stats"]["n_completed_trials"] = json!(4);
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    assert_eq!(poll(root, &[])["jobs"][0]["status"], "complete");
    fs::write(path.join("result.json"), "{").unwrap();
    assert_eq!(poll(root, &[])["jobs"], json!([]));
    for args in [
        vec!["--unknown"],
        vec!["--jobs-dir"],
        vec!["--interval", "bad"],
        vec!["--interval", "0"],
        vec!["--interval", "NaN"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar"))
            .args(&args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
    }
    println!("None");
}

/// Verify the actual Harbor taskbar rendering owner with isolated GNOME actors.
/// Parameters: none.
/// Returns: None after public renderer assertions pass.
#[test]
fn taskbar_rendering() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("gjs")
        .args(["tests/harbor.js", root])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("None");
}

/// Detect externally launched Harbor jobs and retain their status across observer restarts.
/// Parameters: none.
/// Returns: None after discovery and process-disappearance assertions pass.
#[test]
fn independent_discovery() {
    println!("parameters=none");
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let data = json!({"started_at": "2026-10-20T00:00:00Z", "finished_at": null,
        "n_total_trials": 4, "stats": {"n_completed_trials": 1}});
    let path = job(root, "external", data);
    fs::rename(root.join("jobs"), root.join("elsewhere")).unwrap();
    let external = root.join("elsewhere/external");
    let proc_root = root.join("proc");
    let process = proc_root.join("123");
    fs::create_dir_all(&process).unwrap();
    symlink(root, process.join("cwd")).unwrap();
    fs::write(
        process.join("stat"),
        "123 (harbor) S 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 998",
    )
    .unwrap();
    // Python-script and native invocation; equals flags and relative output paths.
    for args in [
        vec![
            "/venv/python",
            "/bin/harbor",
            "run",
            "--jobs-dir=elsewhere",
            "--job-name=external",
        ],
        vec![
            "harbor",
            "jobs",
            "start",
            "-o",
            "elsewhere",
            "--job-name",
            "external",
        ],
    ] {
        fs::write(process.join("cmdline"), args.join("\0") + "\0").unwrap();
        let rows = poll(root, &["--proc-root", proc_root.to_str().unwrap()]);
        assert_eq!(rows["jobs"].as_array().unwrap().len(), 1, "{args:?}");
        assert_eq!(rows["jobs"][0]["id"], external.to_str().unwrap());
        assert_eq!(rows["jobs"][0]["pid"], 123);
        assert_eq!(rows["jobs"][0]["status"], "running");
    }
    // Configuration-only launch, with an arbitrary jobs directory in YAML.
    fs::write(
        root.join("run.yaml"),
        "job_name: external\njobs_dir: elsewhere\n",
    )
    .unwrap();
    fs::write(process.join("cmdline"), b"harbor\0run\0-c\0run.yaml\0").unwrap();
    assert_eq!(
        poll(root, &["--proc-root", proc_root.to_str().unwrap()])["jobs"][0]["status"],
        "running"
    );
    fs::remove_dir_all(&process).unwrap();
    assert_eq!(
        poll(root, &["--proc-root", proc_root.to_str().unwrap()])["jobs"][0]["status"],
        "interrupted"
    );
    // A shell command containing the word harbor must never register as a run.
    fs::create_dir_all(&process).unwrap();
    symlink(root, process.join("cwd")).unwrap();
    fs::write(
        process.join("cmdline"),
        b"bash\0-c\0harbor run --job-name fake\0",
    )
    .unwrap();
    let rows = poll(root, &["--proc-root", proc_root.to_str().unwrap()]);
    assert_eq!(rows["jobs"].as_array().unwrap().len(), 1);
    assert!(!path.exists());
    println!("None");
}

/// Estimate a current run using independently timed, matching historical jobs.
/// Parameters: none.
/// Returns: None after historical estimate and unknown-history checks pass.
#[test]
fn historical_estimates() {
    println!("parameters=none");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (name, finish) in [
        ("old1", "2026-10-19T00:10:00"),
        ("old2", "2026-10-19T00:20:00"),
    ] {
        let path = job(
            root,
            name,
            json!({"started_at": "2026-10-19T00:00:00", "finished_at": finish,
            "n_total_trials": 4, "stats": {"n_completed_trials": 4}}),
        );
        let mut config: Value =
            serde_json::from_str(&fs::read_to_string(path.join("config.json")).unwrap()).unwrap();
        config["n_concurrent_trials"] = json!(2);
        fs::write(path.join("config.json"), config.to_string()).unwrap();
    }
    job(
        root,
        "incomplete",
        json!({"started_at": "2026-10-19T00:00:00Z", "finished_at": "2026-10-19T00:01:00Z",
        "n_total_trials": 4, "stats": {"n_completed_trials": 1}}),
    );
    let mismatch = job(
        root,
        "different-model",
        json!({"started_at": "2026-10-19T00:00:00Z", "finished_at": "2026-10-19T00:01:00Z",
        "n_total_trials": 4, "stats": {"n_completed_trials": 4}}),
    );
    let mut other: Value =
        serde_json::from_str(&fs::read_to_string(mismatch.join("config.json")).unwrap()).unwrap();
    other["agents"] = json!([{"model_name": "different-model"}]);
    fs::write(mismatch.join("config.json"), other.to_string()).unwrap();
    let start = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
    let path = job(
        root,
        "current",
        json!({"started_at": start, "finished_at": null,
        "n_total_trials": 8, "stats": {"n_completed_trials": 1}}),
    );
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(path.join("config.json")).unwrap()).unwrap();
    config["n_concurrent_trials"] = json!(2);
    fs::write(path.join("config.json"), config.to_string()).unwrap();
    let data = poll(root, &[]);
    let current = data["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "current")
        .unwrap();
    assert_eq!(current["estimate"]["samples"], 2);
    // Prior jobs took 600 and 1200 seconds for two waves; four waves take median 1800.
    assert_eq!(current["estimate"]["total_seconds"], 1800.0);
    let remaining = current["estimate"]["remaining_seconds"].as_f64().unwrap();
    assert!((1730.0..=1740.0).contains(&remaining), "{remaining}");
    config["tasks"] = json!([{"path": "different-task"}]);
    fs::write(path.join("config.json"), config.to_string()).unwrap();
    let data = poll(root, &[]);
    let current = data["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "current")
        .unwrap();
    assert!(current["estimate"].is_null());
    println!("None");
}

/// Publish exact completed trial counts and distinguish cancellation from completion.
/// Parameters: none.
/// Returns: None after progress and terminal-state assertions pass.
#[test]
fn trial_progress() {
    println!("parameters=none");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut data = json!({"started_at": "2026-10-20T00:00:00Z", "finished_at": null,
        "n_total_trials": 4, "stats": {"n_completed_trials": 1, "n_errored_trials": 1, "n_cancelled_trials": 0}});
    let path = job(root, "progress", data.clone());
    let first = poll(root, &[]);
    assert_eq!(first["jobs"][0]["completed"], 1);
    assert_eq!(first["jobs"][0]["total"], 4);
    assert_eq!(first["jobs"][0]["fraction"], 0.25);
    data["stats"]["n_completed_trials"] = json!(3);
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    assert_eq!(poll(root, &[])["jobs"][0]["fraction"], 0.75);
    data["finished_at"] = json!(chrono::Utc::now().to_rfc3339());
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    assert_eq!(poll(root, &[])["jobs"][0]["status"], "interrupted");
    data["stats"]["n_completed_trials"] = json!(4);
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    assert_eq!(poll(root, &[])["jobs"][0]["status"], "complete");
    data["stats"]["n_cancelled_trials"] = json!(1);
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    assert_eq!(poll(root, &[])["jobs"][0]["status"], "interrupted");
    data["stats"]["n_completed_trials"] = json!(5);
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    assert!(poll(root, &[])["jobs"][0]["fraction"].is_null());
    println!("None");
}

/// Read asynchronous mock speech calls without touching the real sound device.
/// Parameters: root - isolated mock executable directory; expected - minimum line to await.
/// Returns: captured speech arguments.
fn speech_calls(root: &Path, expected: &str) -> String {
    println!("root={root:?} expected={expected}");
    let mut result = String::new();
    for _ in 0..100 {
        result = fs::read_to_string(root.join("tts-calls")).unwrap_or_default();
        if result.contains(expected) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    println!("{result}");
    result
}

/// Announce a genuine running-to-complete transition once using a distinct female voice.
/// Parameters: none.
/// Returns: None after workspace, deduplication and cancellation assertions pass.
#[test]
fn completion_speech() {
    println!("parameters=none");
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let engine = root.join("agent-tts");
    fs::write(&engine, "#!/bin/sh\nprintf '%s\\n' \"$ATTS_ANNOUNCE_WORKSPACE\" \"$@\" >> \"$(dirname \"$0\")/tts-calls\"\n").unwrap();
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
    let extra = ["--tts-command", engine.to_str().unwrap()];
    let mut data = json!({"started_at": "2026-10-20T00:00:00Z", "finished_at": null,
        "n_total_trials": 4, "stats": {"n_completed_trials": 1}});
    let path = job(root, "speech", data.clone());
    poll(root, &extra); // First observed running arms this job.
    let old_placement =
        json!({"updated_at": chrono::Utc::now().to_rfc3339(), "jobs": {path.to_str().unwrap(): 3}});
    fs::write(
        root.join("state/placements.json"),
        old_placement.to_string(),
    )
    .unwrap();
    poll(root, &extra);
    let placement = json!({"updated_at": chrono::Utc::now().to_rfc3339(),
        "jobs": {path.to_str().unwrap(): 5}});
    fs::write(root.join("state/placements.json"), placement.to_string()).unwrap();
    data["finished_at"] = json!(chrono::Utc::now().to_rfc3339());
    data["stats"]["n_completed_trials"] = json!(4);
    fs::write(path.join("result.json"), data.to_string()).unwrap();
    poll(root, &extra);
    let calls = speech_calls(root, "af_nicole");
    assert!(
        calls.contains("Harbor run on workspace 5 is done."),
        "{calls}"
    );
    assert!(calls.contains("--voice\naf_nicole"), "{calls}");
    assert!(
        calls.starts_with("0\nannounce\n--event\nfinish\n"),
        "{calls}"
    );
    poll(root, &extra); // Every call is a fresh process: restart must not replay.
    assert_eq!(speech_calls(root, "af_nicole"), calls);
    // Completed runs first encountered at startup are history, never replayed speech.
    job(root, "already-done", data.clone());
    poll(root, &extra);
    assert_eq!(speech_calls(root, "af_nicole"), calls);
    data["finished_at"] = Value::Null;
    data["stats"]["n_completed_trials"] = json!(1);
    let cancelled = job(root, "cancelled", data.clone());
    poll(root, &extra);
    data["finished_at"] = json!(chrono::Utc::now().to_rfc3339());
    fs::write(cancelled.join("result.json"), data.to_string()).unwrap();
    poll(root, &extra);
    assert_eq!(speech_calls(root, "af_nicole"), calls);
    // A stale placement must explicitly announce an unknown workspace.
    data["finished_at"] = Value::Null;
    data["stats"]["n_completed_trials"] = json!(1);
    let unknown = job(root, "unknown", data.clone());
    poll(root, &extra);
    fs::write(
        root.join("state/placements.json"),
        json!({"updated_at": "2000-01-01T00:00:00Z", "jobs": {unknown.to_str().unwrap(): 9}})
            .to_string(),
    )
    .unwrap();
    data["finished_at"] = json!(chrono::Utc::now().to_rfc3339());
    data["stats"]["n_completed_trials"] = json!(4);
    fs::write(unknown.join("result.json"), data.to_string()).unwrap();
    poll(root, &extra);
    let all = speech_calls(root, "Harbor run on an unknown workspace is done.");
    assert!(
        all.contains("Harbor run on an unknown workspace is done."),
        "{all}"
    );
    assert_eq!(all.matches("af_nicole").count(), 2);
    println!("None");
}

/// Reject concurrent observers before they can race snapshot or speech claims.
/// Parameters: none.
/// Returns: None after lock rejection and recovery checks pass.
#[test]
fn single_observer() {
    println!("parameters=none");
    use std::os::fd::AsRawFd;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("state")).unwrap();
    let held = fs::File::create(root.join("state/observer.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let output = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar"))
        .arg("--once")
        .arg("--state-dir")
        .arg(root.join("state"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("another Harbor observer"));
    assert!(!root.join("state/status.json").exists());
    drop(held);
    assert_eq!(poll(root, &[])["jobs"], json!([]));
    println!("None");
}

/// Keep concurrent autogenerated runs distinct and retire interrupted owners.
/// Parameters: none.
/// Returns: None after default launch, resume and interruption-retention assertions pass.
#[test]
fn generated_job_discovery() {
    println!("parameters=none");
    use chrono::TimeZone;
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let proc_root = root.join("proc");
    fs::create_dir_all(&proc_root).unwrap();
    fs::write(proc_root.join("stat"), "btime 1700000000\n").unwrap();
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    let mut expected = Vec::new();
    for (pid, offset) in [(901, 1000), (902, 1100)] {
        let time = chrono::Local
            .timestamp_opt(1_700_000_000 + offset, 0)
            .single()
            .unwrap();
        let name = time.format("%Y-%m-%d__%H-%M-%S").to_string();
        let path = job(
            root,
            &name,
            json!({"started_at": time.to_rfc3339(), "finished_at": null,
            "n_total_trials": 4, "stats": {"n_completed_trials": 1}}),
        );
        expected.push((path, pid));
        let process = proc_root.join(pid.to_string());
        fs::create_dir_all(&process).unwrap();
        symlink(root, process.join("cwd")).unwrap();
        fs::write(process.join("cmdline"), b"harbor\0run\0").unwrap();
        let mut fields = vec!["S".to_string(), "1".to_string()];
        fields.extend(vec!["0".to_string(); 17]);
        fields.push((offset as u64 * ticks).to_string());
        fs::write(
            process.join("stat"),
            format!("{pid} (harbor) {}", fields.join(" ")),
        )
        .unwrap();
    }
    let data = poll(root, &[]);
    for (path, pid) in &expected {
        let row = data["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == path.to_str().unwrap())
            .unwrap();
        assert_eq!(row["pid"], *pid, "{path:?}");
    }
    let resume = format!(
        "harbor\0jobs\0resume\0--job-path\0{}\0",
        expected[0].0.display()
    );
    fs::write(proc_root.join("901/cmdline"), resume).unwrap();
    let data = poll(root, &[]);
    assert_eq!(data["jobs"][0]["pid"], 901);
    fs::remove_dir_all(proc_root.join("901")).unwrap();
    let data = poll(root, &[]);
    assert_eq!(data["jobs"][0]["status"], "interrupted");
    let mut saved: Value =
        serde_json::from_str(&fs::read_to_string(root.join("state/watched.json")).unwrap())
            .unwrap();
    saved[expected[0].0.to_str().unwrap()]["last_seen_at"] = json!("2000-01-01T00:00:00Z");
    fs::write(root.join("state/watched.json"), saved.to_string()).unwrap();
    let data = poll(root, &[]);
    assert!(data["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["id"] != expected[0].0.to_str().unwrap()));
    println!("None");
}

/// Keep optional Harbor provider credentials out of observer traces and state.
/// Parameters: none.
/// Returns: None after canary exclusion and retained estimate checks pass.
#[test]
fn private_config_fields() {
    println!("parameters=none");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let path = job(
        root,
        "private",
        json!({"started_at": "2026-10-20T00:00:00Z", "finished_at": null,
        "n_total_trials": 4, "stats": {"n_completed_trials": 1}}),
    );
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(path.join("config.json")).unwrap()).unwrap();
    let canary = "harbor-credential-canary-12345";
    config["agents"] = json!([{"model_name": "demo", "kwargs": {"api_key": canary, "reasoning_effort": "low"}, "env": {"API_KEY": canary}}]);
    config["environment"] = json!({"type": "docker", "env": {"TOKEN": canary}});
    fs::write(path.join("config.json"), config.to_string()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar"))
        .arg("--once")
        .arg("--jobs-dir")
        .arg(root.join("jobs"))
        .arg("--state-dir")
        .arg(root.join("state"))
        .arg("--proc-root")
        .arg(root.join("proc"))
        .arg("--tts-command")
        .arg(root.join("no-engine"))
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(canary),
        "provider fields leaked in CLI tracing"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains(canary));
    for entry in fs::read_dir(root.join("state")).unwrap().flatten() {
        assert!(
            !fs::read_to_string(entry.path()).unwrap().contains(canary),
            "{:?}",
            entry.path()
        );
    }
    println!("None");
}
