//! Read Codex reset-credit availability through its read-only local app-server protocol.
//!
//! The CLI handles its own authentication. This module sends only
//! `account/rateLimits/read`; it never requests a reset redemption.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

const RESET_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Return the remaining reset count for `codex_home`, validating the backend
/// account against the ID in that home's auth file.
pub fn read_codex_reset_count(
    binary: &Path,
    codex_home: &Path,
    expected_account_id: &str,
) -> Result<i64, String> {
    let mut child = Command::new(binary)
        .args(["app-server", "--stdio"])
        .env("CODEX_HOME", codex_home)
        .env("CODEX_AGENT_SKIP_TRACKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot start Codex metadata reader: {error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Codex metadata reader has no stdin".into());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Codex metadata reader has no stdout".into());
    };
    let requests = [
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "agentic-cli-status", "version": "0.1.0"}}}),
        json!({"method": "initialized"}),
        json!({"id": 2, "method": "account/rateLimits/read", "params": {"excludeResetCreditDetails": true}}),
    ];
    for request in requests {
        if let Err(error) = writeln!(stdin, "{request}") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("cannot request Codex reset availability: {error}"));
        }
    }
    if let Err(error) = stdin.flush() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("cannot flush Codex metadata request: {error}"));
    }

    let (sender, receiver) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id") == Some(&json!(2)) {
                let _ = sender.send(message);
                break;
            }
        }
    });
    let reply = receiver.recv_timeout(RESET_READ_TIMEOUT);
    let _ = child.kill();
    let _ = child.wait();
    let reply = reply.map_err(|_| "Codex reset availability read timed out".to_string())?;
    let _ = reader.join();
    let result = reply
        .get("result")
        .ok_or("Codex reset availability response had no result")?;
    if result.get("accountId").and_then(Value::as_str) != Some(expected_account_id) {
        return Err("Codex reset availability belonged to another account".into());
    }
    let count = result
        .pointer("/rateLimitResetCredits/availableCount")
        .and_then(Value::as_i64)
        .ok_or("Codex reset availability response had no count")?;
    if count < 0 {
        return Err("Codex reset availability response had a negative count".into());
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_matching_accounts_reset_count() {
        let sandbox = tempfile::TempDir::new().unwrap();
        let binary = sandbox.path().join("codex");
        std::fs::write(
            &binary,
            "#!/bin/sh\nread -r initialize\nread -r initialized\nread -r request\nprintf '%s\\n' '{\"id\":2,\"result\":{\"accountId\":\"account-one\",\"rateLimitResetCredits\":{\"availableCount\":0}}}'\n",
        ).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).unwrap();
        assert_eq!(
            read_codex_reset_count(&binary, sandbox.path(), "account-one"),
            Ok(0)
        );
        assert!(read_codex_reset_count(&binary, sandbox.path(), "account-three").is_err());
    }
}
