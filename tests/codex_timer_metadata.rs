//! Read-only Codex metadata retains each account's timer observations.

use agentic_cli_status::collectors::codex_reset_credits::read_codex_rate_limit_metadata;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

/// Read explicit idle and rolling windows from the account metadata protocol.
/// Parameters: none.
/// Returns: None.
#[test]
fn metadata_reports_account_scoped_windows_without_a_prompt() {
    println!("parameters=none");
    let sandbox = tempfile::tempdir().unwrap();
    let binary = sandbox.path().join("codex");
    std::fs::write(&binary, concat!(
        "#!/bin/sh\nread -r init\nread -r ready\nread -r request\n",
        "printf '%s\\n' '{\"id\":2,\"result\":{\"accountId\":\"account-one\",",
        "\"rateLimitResetCredits\":{\"availableCount\":1},",
        "\"rateLimits\":{\"primary\":{\"windowDurationMins\":300,\"usedPercent\":0,\"resetsAt\":null},",
        "\"secondary\":{\"windowDurationMins\":10080,\"usedPercent\":24,\"resetsAt\":2000}}}}'\n"
    )).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let metadata = read_codex_rate_limit_metadata(&binary, sandbox.path(), "account-one").unwrap();
    assert_eq!(metadata["resets_available"], json!(1));
    assert_eq!(
        metadata["timer_windows"],
        json!([
            {"label":"5h", "used_percent":0, "resets_at":null},
            {"label":"week", "used_percent":24, "resets_at":2000}
        ])
    );
    assert!(read_codex_rate_limit_metadata(&binary, sandbox.path(), "another-account").is_err());
    println!("None");
}

/// Reject metadata from another quota bucket and incomplete timer fields.
/// Parameters: none.
/// Returns: None.
#[test]
fn metadata_ignores_other_buckets_and_missing_reset_fields() {
    println!("parameters=none");
    let sandbox = tempfile::tempdir().unwrap();
    let binary = sandbox.path().join("codex");
    std::fs::write(&binary, concat!(
        "#!/bin/sh\nread -r init\nread -r ready\nread -r request\n",
        "printf '%s\\n' '{\"id\":2,\"result\":{\"accountId\":\"one\",",
        "\"rateLimits\":{\"limitId\":\"premium\",\"primary\":{\"windowDurationMins\":300,\"usedPercent\":0,\"resetsAt\":null}},",
        "\"rateLimitsByLimitId\":{\"codex\":{\"primary\":{\"windowDurationMins\":300,\"usedPercent\":0},",
        "\"secondary\":{\"windowDurationMins\":10080,\"usedPercent\":2,\"resetsAt\":3000}}}}}'\n"
    )).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let metadata = read_codex_rate_limit_metadata(&binary, sandbox.path(), "one").unwrap();
    assert!(metadata["resets_available"].is_null());
    assert_eq!(
        metadata["timer_windows"],
        json!([
            {"label":"week", "used_percent":2, "resets_at":3000}
        ])
    );
    println!("None");
}
