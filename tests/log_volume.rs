//! Production status updates must not flood the system journal.

/// Execute the real extension helpers with isolated actors and an observed print sink.
/// Parameters: none.
/// Returns: None.
#[test]
fn routine_updates_do_not_log_payloads_or_describe_actors() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    let output = std::process::Command::new("gjs")
        .args(["tests/log_volume.js", root])
        .current_dir(root)
        .output()
        .expect("gjs is required for extension regression checks");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
    println!("None");
}
