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

/// Validate that the metadata response belongs to the requested login.
/// Parameters: result - decoded metadata response; expected_account_id - registered account identifier.
/// Returns: Ok when the account matches, otherwise an error.
fn validate_metadata_account(result: &Value, expected_account_id: &str) -> Result<(), String> {
    println!("result={result}, expected_account_id={expected_account_id}");
    let outcome = if result.get("accountId").and_then(Value::as_str) == Some(expected_account_id) {
        Ok(())
    } else {
        Err("Codex reset availability belonged to another account".into())
    };
    println!("{outcome:?}");
    outcome
}

/// Accept a metadata window only when its converted values identify a supported limit.
/// Parameters: window - decoded API window.
/// Returns: the normalized timer window, or None when incomplete or invalid.
fn validate_metadata_window(window: &Value) -> Option<Value> {
    println!("window={window}");
    let result = (|| {
        let label = match window.get("windowDurationMins")?.as_i64()? {
            300 => "5h",
            10080 => "week",
            _ => return None,
        };
        let used = window.get("usedPercent")?.as_i64()?;
        if !(0..=100).contains(&used) {
            return None;
        }
        let reset = window.get("resetsAt")?;
        if !reset.is_null() && !reset.as_i64().is_some_and(|value| value > 0) {
            return None;
        }
        Some(json!({"label": label, "used_percent": used, "resets_at": reset}))
    })();
    println!("{result:?}");
    result
}

/// Normalize the account's count and explicit timer windows from decoded metadata.
/// Parameters: result - validated account metadata response.
/// Returns: normalized metadata for publication.
fn parse_rate_limit_metadata(result: &Value) -> Value {
    println!("result={result}");
    let count = result
        .pointer("/rateLimitResetCredits/availableCount")
        .and_then(Value::as_i64)
        .filter(|count| *count >= 0);
    let limits = result.pointer("/rateLimitsByLimitId/codex").or_else(|| {
        result.get("rateLimits").filter(|limits| {
            limits
                .get("limitId")
                .and_then(Value::as_str)
                .map_or(true, |id| id == "codex")
        })
    });
    let mut windows: Vec<Value> = limits
        .into_iter()
        .flat_map(|limits| {
            ["primary", "secondary"]
                .into_iter()
                .filter_map(|key| limits.get(key).and_then(validate_metadata_window))
        })
        .collect();
    if windows.len() == 2 && windows[0]["label"] == windows[1]["label"] {
        windows.clear();
    }
    let normalized = json!({"resets_available": count, "timer_windows": windows});
    println!("{normalized}");
    normalized
}

/// Read account-scoped Codex timer data through the read-only app-server protocol.
/// Parameters: binary - Codex executable; codex_home - isolated login directory; expected_account_id - registered account identifier.
/// Returns: normalized metadata, or an error when the read or identity check fails.
pub fn read_codex_rate_limit_metadata(
    binary: &Path,
    codex_home: &Path,
    expected_account_id: &str,
) -> Result<Value, String> {
    println!(
        "binary={binary:?}, codex_home={codex_home:?}, expected_account_id={expected_account_id}"
    );
    let outcome = (|| -> Result<Value, String> {
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
        validate_metadata_account(result, expected_account_id)?;
        Ok(parse_rate_limit_metadata(result))
    })();
    println!("{outcome:?}");
    outcome
}

/// Return the remaining reset count for a verified Codex account.
/// Parameters: binary - Codex executable; codex_home - isolated login directory; expected_account_id - registered account identifier.
/// Returns: nonnegative reset count, or an error when unavailable.
pub fn read_codex_reset_count(
    binary: &Path,
    codex_home: &Path,
    expected_account_id: &str,
) -> Result<i64, String> {
    println!(
        "binary={binary:?}, codex_home={codex_home:?}, expected_account_id={expected_account_id}"
    );
    let result =
        read_codex_rate_limit_metadata(binary, codex_home, expected_account_id).and_then(|value| {
            value["resets_available"]
                .as_i64()
                .ok_or_else(|| "Codex reset availability response had no count".to_string())
        });
    println!("{result:?}");
    result
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
