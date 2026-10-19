//! Run the public taskbar bar rendering probe with isolated GNOME actors.
use std::process::Command;

/// Verify public bar rendering and real native GObject initialization.
/// Parameters: none.
/// Returns: None after all public rendering assertions pass.
#[test]
fn panel_progress_rendering() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    for script in ["tests/panel_progress.js", "tests/native_panel_init.js"] {
        let output = Command::new("gjs")
            .args([script, root])
            .current_dir(root)
            .output()
            .expect("gjs is required for extension rendering checks");
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("None");
}
