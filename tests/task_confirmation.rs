//! Public taskbar completion interaction without live desktop effects.
use std::process::Command;

/// Verify the extension's click callbacks and confirmation actors.
/// Parameters: none.
/// Returns: None after behavioral assertions pass.
#[test]
fn task_confirmation() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("gjs").args(["tests/task_confirmation.js", root])
        .current_dir(root).output().expect("gjs is required");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    println!("None");
}

/// Exercise completion with GIO's real subprocess and async output pipes.
/// Parameters: none.
/// Returns: None after success and rejection both preserve selected-session routing.
#[test]
fn task_completion_native_subprocess() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("gjs").args(["tests/task_completion_subprocess.js", root])
        .current_dir(root).output().expect("gjs is required");
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    println!("None");
}
