# Task menu progress coverage

Run from project root: `cargo test` and `bash tests/test_install.sh`.
GJS drawing checks use stub actors and fresh cases; they do not access the desktop.

| Input / behavior | Executable check and expected result |
| --- | --- |
| Complete phase, active code action, pending and skipped blocks | model_status_segmented_task_background: seven bounded colored blocks, wider phase gaps |
| Narrow/zero width and invalid segments | Same check: bounded disjoint rectangles; invalid arrays return empty |
| Per-row background, resizing, update and click identity | Existing model_status_task_progress_layer_updates_in_place and model_status_open_menu_progress_refresh_keeps_actor_identity plus new segmented actor case |
| Missing/nonfinite/string fraction and confirmed finish | model_status_task_progress_validates_payload: hidden progress; DONE glow retains priority |
| Legacy payload fraction | Existing fill tests retain their independently expected widths |

No command parsers change; numeric conversion and operand validation are inapplicable.

| Compact active-phase counts before timer | model_status_compact_workflow_count_precedes_timer: `[1/4] 2:05`, next tick `[1/4] 2:06`, zero `[0/4]`, full `[4/4]`; no phase name |
| Bad/missing count operands, confirmed completion | Same GJS case: negative, fractional, zero-total, excessive, string, missing and NaN counts omit brackets; confirmed/unread DONE omits progress |
| Queued reset timer and waiting state | Same case: `[1/4] -1:00` and `[1/4] WAITING`; existing plain clocks and colors retained |
