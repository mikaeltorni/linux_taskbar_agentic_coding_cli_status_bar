//! Run the public taskbar bar rendering probe with isolated GNOME actors.
use std::process::Command;

/// Verify actual bar widths and refresh through the extension rendering API.
/// Parameters: none.
/// Returns: None after all public rendering assertions pass.
#[test]
fn panel_progress_rendering() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("gjs")
        .args(["tests/panel_progress.js", root])
        .current_dir(root)
        .output()
        .expect("gjs is required for extension rendering checks");
    assert!(output.status.success(), "{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    println!("None");
}
