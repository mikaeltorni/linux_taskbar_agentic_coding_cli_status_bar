//! Exercise taskbar input routing without opening a window or changing user state.
use std::process::Command;

/// Run public panel event assertions under GJS with isolated Shell dependencies.
/// Parameters: none.
/// Returns: None after every public interaction assertion passes.
#[test]
fn panel_interaction_events() {
    println!("parameters=none");
    let root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("gjs")
        .args(["tests/panel_interaction.js", root])
        .current_dir(root)
        .output()
        .expect("gjs is required for taskbar interaction checks");
    assert!(output.status.success(), "{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    println!("None");
}
