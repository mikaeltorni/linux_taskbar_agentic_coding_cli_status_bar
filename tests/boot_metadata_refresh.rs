//! Startup refresh reads metadata without driving slow console slash commands.
use std::process::Command;

/// Accept the bounded metadata refresh form and reject it outside a single tick.
/// Parameters: none.
/// Returns: None.
#[test]
fn metadata_refresh_requires_once() {
    println!("parameters=none");
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config/agentic-cli-status");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.json"), r#"{"tmux":{"enabled":false},"codex_reset_forecast":{"enabled":false},"clis":{"codex":{"enabled":false}}}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentic-cli-status"))
        .args(["--once","--no-tmux-refresh"]).env("HOME", root.path())
        .env("XDG_CONFIG_HOME",root.path().join("config"))
        .env("XDG_STATE_HOME",root.path().join("state")).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    let invalid = Command::new(env!("CARGO_BIN_EXE_agentic-cli-status"))
        .arg("--no-tmux-refresh").output().unwrap();
    assert!(!invalid.status.success());
    println!("None");
}
