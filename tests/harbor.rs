//! Public standalone Harbor observer and taskbar checks.
use std::{fs, path::Path, process::Command};
use serde_json::{json, Value};

/// Run the public observer with isolated artifact and output directories.
/// Parameters: root - sandbox; extra - additional CLI tokens.
/// Returns: the independently published taskbar snapshot.
fn poll(root: &Path, extra: &[&str]) -> Value {
    println!("root={root:?} extra={extra:?}");
    let output = Command::new(env!("CARGO_BIN_EXE_harbor-taskbar"))
        .args(["--once", "--jobs-dir"]).arg(root.join("jobs"))
        .arg("--state-dir").arg(root.join("state")).args(extra).output().unwrap();
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
