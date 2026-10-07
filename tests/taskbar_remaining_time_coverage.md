# Taskbar maximum remaining-time coverage

Runner: `cargo test --test gjs panel_remaining` (gjs required). Final cumulative
runner: `cargo test`, then `bash tests/test_install.sh`.
Tests use fresh snapshots and actor stubs; no live files, AI requests or GUI calls.

| Public observation | Fixture and independent expected result | Saved assertion |
| --- | --- | --- |
| Maximum across providers and workspaces | Active 120s, 540s, 240s; expect ~9m left, never sum ~15m | panel_remaining_uses_maximum_active_historical_estimate |
| Work time, not waiting or finished estimates | Inactive RUNNING, WAITING, DONE carry 9999s; ignore all | panel_remaining_uses_maximum_active_historical_estimate |
| Numeric validity | Strings, NaN, Infinity, negative and zero are unavailable; learning rather than fake zero | panel_remaining_uses_maximum_active_historical_estimate |
| Missing/partial publisher fields | Null payload/providers, null provider/agent, absent agents, estimate status complete; no throw, no false duration | panel_remaining_uses_maximum_active_historical_estimate |
| Formatting boundaries | 1s, 60s, 61s, 3600s, 3601s become ~1m, ~1m, ~2m, ~1h, ~1h 1m | panel_remaining_uses_maximum_active_historical_estimate |
| Learning/overrun fallback | All learning: learning; all overrun: past estimate; no active agents: hidden | panel_remaining_uses_maximum_active_historical_estimate |
| Trailing layout and unscoped maximum | Scope removes 540s agent but panel still ~9m after every instance/rate/attention chip | panel_remaining_refreshes_at_far_right_without_replacing_menu |
| Open-menu update | 540s becomes 240s; same actor and click target, ~4m left; stale style applied and removed | panel_remaining_refreshes_at_far_right_without_replacing_menu |
| Absence/recovery | Missing snapshot hides existing label while menu is open; restored snapshot refreshes same actor; closed-menu rebuild creates final actor | panel_remaining_refreshes_at_far_right_without_replacing_menu |
| Diagnostics | New functions print parameters and every normal return; test capture keeps JSON assertions independent of diagnostics | Both panel_remaining tests |

Command operand validation is inapplicable: no new command/parser; the interface
is the published ACC payload and its rendered panel label. Malformed fields are
read-only and cannot mutate ACC history or notes. Existing menu identity checks
remain in the cumulative runner.
