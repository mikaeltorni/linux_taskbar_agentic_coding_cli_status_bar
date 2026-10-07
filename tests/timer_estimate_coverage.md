# Task timer estimate coverage

| Interface | Case | Independent expectation |
| --- | --- | --- |
| Published status consumer | Running task with ~9m left | Elapsed timer and ~9m left displayed; snapshot unchanged |
| Finished/absent/malformed estimate | Completed status or no label | Existing display retained; no stale forecast |
| Learning and overrun | Published labels | Learning/past estimate shown beside timed tasks |
| Reset timer | Future reset + task estimate | Reset countdown preserved; estimate identifies remaining work |
| Refresh | New snapshot on open menu / AGE update | Same task actor updates estimate without changing click target |
| Notes and terminal timer formatting | Existing checks | Elapsed clock, goal color, idle age, behind counts retained |

No new commands or operand parsers. Existing entrypoints and argument checks remain owned by their current suites. All consumers read ACC metadata; no notes/history writes. Public runners: PA `.venv/bin/python -m pytest tests/test_timer_estimates.py tests/test_agent_run_timer.py tests/test_published_status.py`; ACC `.venv/bin/python -m pytest tests/test_timer_estimates.py tests/test_kitty_outline.py tests/test_agent_activity_age.py tests/test_acc_model_status_bar.py tests/test_task_time_estimation.py`; taskbar `cargo test` and `bash tests/test_install.sh`.

Saved cases: Python `test_published_estimate_reaches_all_notes_badges_without_writes` (PA), `test_terminal_estimate_round_trip_and_console_label` (ACC), invalid/complete parametrizations in both, PA `test_idle_hides_estimate_and_reset_keeps_work_forecast`, ACC `test_reset_forecast_and_malformed_protocol_leave_rendered_state` checks rendered state immediately after each malformed payload, taskbar `model_status_estimate_is_right_of_summary_and_refreshes_in_place` verifies real row actor order, alignment, refresh identity and all estimate states.

Observed: ACC 462 cumulative checks pass; PA 226 monitor/display checks plus rendered Notes regression pass, new mutable suite repeats twice in one process. Broader PA suite has an unrelated pre-existing `test_unpushed_overview_hotkey_reuses_parallel_prefetch` expectation (↑2 vs local-only 2), reproduced on unchanged live master. Taskbar cargo tests and 92 installer assertions pass; clippy passes. `cargo fmt --all -- --check` already fails on unchanged live master for previous gjs test formatting.

Origin capability: gjs `model_status_estimate_is_right_of_summary_and_refreshes_in_place` now expects `(feat/example)` with no project name while tag `ACC` and rightmost estimate remain.
