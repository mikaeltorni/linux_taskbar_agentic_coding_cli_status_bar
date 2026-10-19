//! Public standalone Harbor observer and taskbar checks.
use std::{fs, path::Path, process::Command};
use serde_json::{json, Value};

/// Run the public observer with isolated artifact and output directories.
/// Parameters: root - sandbox; extra - additional CLI tokens.
/// Returns: the independently published taskbar snapshot.
fn poll(root: &Path, extra: &[&str]) -> Value {
    println!("root={root:?} extra={extra:?}");
    let mut command = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar"));
    command.args(["--once", "--jobs-dir"]).arg(root.join("jobs"))
        .arg("--state-dir").arg(root.join("state"));
    if !extra.contains(&"--proc-root") { command.arg("--proc-root").arg(root.join("proc")); }
    let output = command.args(extra).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let result = serde_json::from_str(&fs::read_to_string(root.join("state/status.json")).unwrap()).unwrap();
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
    fs::write(path.join("config.json"), json!({"job_name": name, "tasks": [{"path": "addition"}], "n_attempts": 4}).to_string()).unwrap();
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
    for args in [vec!["--unknown"], vec!["--jobs-dir"], vec!["--interval", "bad"], vec!["--interval", "0"], vec!["--interval", "NaN"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar")).args(&args).output().unwrap();
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
    let output = Command::new("gjs").args(["tests/harbor.js", root]).current_dir(root).output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
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
    fs::write(process.join("stat"), "123 (harbor) S 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 998").unwrap();
    // Python-script and native invocation; equals flags and relative output paths.
    for args in [
        vec!["/venv/python", "/bin/harbor", "run", "--jobs-dir=elsewhere", "--job-name=external"],
        vec!["harbor", "jobs", "start", "-o", "elsewhere", "--job-name", "external"],
    ] {
        fs::write(process.join("cmdline"), args.join("\0") + "\0").unwrap();
        let rows = poll(root, &["--proc-root", proc_root.to_str().unwrap()]);
        assert_eq!(rows["jobs"].as_array().unwrap().len(), 1, "{args:?}");
        assert_eq!(rows["jobs"][0]["id"], external.to_str().unwrap());
        assert_eq!(rows["jobs"][0]["pid"], 123);
        assert_eq!(rows["jobs"][0]["status"], "running");
    }
    // Configuration-only launch, with an arbitrary jobs directory in YAML.
    fs::write(root.join("run.yaml"), "job_name: external\njobs_dir: elsewhere\n").unwrap();
    fs::write(process.join("cmdline"), b"harbor\0run\0-c\0run.yaml\0").unwrap();
    assert_eq!(poll(root, &["--proc-root", proc_root.to_str().unwrap()])["jobs"][0]["status"], "running");
    fs::remove_dir_all(&process).unwrap();
    assert_eq!(poll(root, &["--proc-root", proc_root.to_str().unwrap()])["jobs"][0]["status"], "interrupted");
    // A shell command containing the word harbor must never register as a run.
    fs::create_dir_all(&process).unwrap();
    symlink(root, process.join("cwd")).unwrap();
    fs::write(process.join("cmdline"), b"bash\0-c\0harbor run --job-name fake\0").unwrap();
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
    for (name, finish) in [("old1", "2026-10-19T00:10:00"), ("old2", "2026-10-19T00:20:00")] {
        let path = job(root, name, json!({"started_at": "2026-10-19T00:00:00", "finished_at": finish,
            "n_total_trials": 4, "stats": {"n_completed_trials": 4}}));
        let mut config: Value = serde_json::from_str(&fs::read_to_string(path.join("config.json")).unwrap()).unwrap();
        config["n_concurrent_trials"] = json!(2);
        fs::write(path.join("config.json"), config.to_string()).unwrap();
    }
    job(root, "incomplete", json!({"started_at": "2026-10-19T00:00:00Z", "finished_at": "2026-10-19T00:01:00Z",
        "n_total_trials": 4, "stats": {"n_completed_trials": 1}}));
    let mismatch = job(root, "different-model", json!({"started_at": "2026-10-19T00:00:00Z", "finished_at": "2026-10-19T00:01:00Z",
        "n_total_trials": 4, "stats": {"n_completed_trials": 4}}));
    let mut other: Value = serde_json::from_str(&fs::read_to_string(mismatch.join("config.json")).unwrap()).unwrap();
    other["agents"] = json!([{"model_name": "different-model"}]);
    fs::write(mismatch.join("config.json"), other.to_string()).unwrap();
    let start = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
    let path = job(root, "current", json!({"started_at": start, "finished_at": null,
        "n_total_trials": 8, "stats": {"n_completed_trials": 1}}));
    let mut config: Value = serde_json::from_str(&fs::read_to_string(path.join("config.json")).unwrap()).unwrap();
    config["n_concurrent_trials"] = json!(2);
    fs::write(path.join("config.json"), config.to_string()).unwrap();
    let data = poll(root, &[]);
    let current = data["jobs"].as_array().unwrap().iter().find(|row| row["name"] == "current").unwrap();
    assert_eq!(current["estimate"]["samples"], 2);
    // Prior jobs took 600 and 1200 seconds for two waves; four waves take median 1800.
    assert_eq!(current["estimate"]["total_seconds"], 1800.0);
    let remaining = current["estimate"]["remaining_seconds"].as_f64().unwrap();
    assert!((1730.0..=1740.0).contains(&remaining), "{remaining}");
    config["tasks"] = json!([{"path": "different-task"}]);
    fs::write(path.join("config.json"), config.to_string()).unwrap();
    let data = poll(root, &[]);
    let current = data["jobs"].as_array().unwrap().iter().find(|row| row["name"] == "current").unwrap();
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
