//! Mistral's monthly Vibe Code allowance, using the native metadata RPC.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::collectors::to_epoch;
use crate::snapshot::{snapshot, Window};

const FETCH_INTERVAL: f64 = 300.0;
const SOURCE: &str = "vibe_account_read";

/// Wait for one RPC result while ignoring events and unrelated responses.
/// Parameters: messages - incoming responses; id - request ID; deadline - shared RPC timeout.
/// Returns: the matched result, if present.
fn response(messages: &Receiver<String>, id: u64, deadline: Instant) -> Option<Value> {
    println!("messages={messages:p}, id={id}, deadline={deadline:?}");
    let result = loop {
        let Ok(line) = messages.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        else {
            break None;
        };
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            if value["id"].as_u64() == Some(id) {
                break value.get("result").cloned();
            }
        }
        if Instant::now() >= deadline {
            break None;
        }
    };
    // The wire response can contain account information. Report only success.
    println!("response available={}", result.is_some());
    result
}

/// Run the native app server's account read without starting a turn.
/// Parameters: home - Vibe user's home directory.
/// Returns: the monthly allowance and pay-as-you-go setting, if visible.
fn read_allowance(home: &Path) -> Option<(Window, bool)> {
    println!("home={home:?}");
    let result = (|| {
        let preferred = home.join(".local/bin/vibe-app-server");
        let mut child = Command::new(if preferred.is_file() {
            preferred.as_os_str()
        } else {
            std::ffi::OsStr::new("vibe-app-server")
        })
        .current_dir(std::env::temp_dir())
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
        let mut input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let (send, messages) = mpsc::sync_channel(32);
        let _reader = thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let result = (|| {
            let mut send = |method: &str, params: Value, id: Option<u64>| {
                let mut request = json!({"jsonrpc": "2.0", "method": method, "params": params});
                if let Some(id) = id {
                    request["id"] = json!(id);
                }
                writeln!(input, "{request}").ok()
            };
            send(
                "initialize",
                json!({"clientInfo": {"name": "agentic-cli-status", "version": env!("CARGO_PKG_VERSION")}}),
                Some(1),
            )?;
            response(&messages, 1, deadline)?;
            send("initialized", json!({}), None)?;
            send(
                "session/start",
                json!({"kind": "ephemeral", "historyLimit": 1}),
                Some(2),
            )?;
            let state = response(&messages, 2, deadline)?;
            let id = state.pointer("/state/session/id")?.as_str()?;
            send("account/read", json!({"sessionId": id}), Some(3))?;
            let result = response(&messages, 3, deadline)?;
            let budget = result.pointer("/account/budget")?;
            let used_percent = budget["usagePercentage"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0)?;
            Some((
                Window {
                    label: "month".into(),
                    used_percent,
                    resets_at: to_epoch(&budget["resetAt"]),
                },
                budget["paygEnabled"].as_bool().unwrap_or(false),
            ))
        })();
        drop(input);
        drop(messages);
        // EOF lets Vibe discard its ephemeral session before shutdown.
        let shutdown = Instant::now() + Duration::from_millis(500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < shutdown => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        result
    })();
    println!("{result:?}");
    result
}

/// Publish the Vibe quota without spending model usage.
/// Parameters: home - Vibe user's home; last - previous poll result; now - current epoch seconds.
/// Returns: shared CLI snapshot with the monthly window or a missing-data reason.
pub fn collect_vibe(home: &Path, last: Option<&Value>, now: f64) -> Value {
    println!("home={home:?}, has_last={}, now={now}", last.is_some());
    let reading = last
        .filter(|value| {
            value["fetch_at"].as_f64().is_some_and(|at| {
                now >= at
                    && now - at < FETCH_INTERVAL
                    && !value["windows"][0]["resets_at"]
                        .as_i64()
                        .is_some_and(|reset| at < reset as f64 && reset as f64 <= now)
            })
        })
        .cloned()
        .unwrap_or_else(|| {
            let mut result = match read_allowance(home) {
                Some((window, payg)) => {
                    let mut result = snapshot(SOURCE, Some(now), None, vec![window], None);
                    result["payg_enabled"] = json!(payg);
                    result
                }
                None => snapshot(
                    SOURCE,
                    None,
                    None,
                    vec![],
                    Some("Vibe monthly allowance unavailable".into()),
                ),
            };
            result["fetch_at"] = json!(now);
            result
        });
    println!("{reading}");
    reading
}
