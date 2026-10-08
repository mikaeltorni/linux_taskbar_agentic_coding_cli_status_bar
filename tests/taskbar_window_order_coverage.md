# Taskbar window ordering checks

Run `cargo test --test gjs taskbar_window_position_ids_order_rows`, `cargo test`, and `bash tests/test_install.sh` from the repository root. The Rust test executes extracted production functions under gjs with isolated file and Shell stubs, without GUI effects.

The corner badge contract is the 1-based, per-workspace `number` in
`outline_window_index.json`. Global 0-based `display_id` values from
`cli_window_ids.json` serve CLI selectors and are not the corner badge order.

| Repair validation | Executable check and independent expectation |
| --- | --- |
| Correct file on every refresh | `taskbar_window_position_ids_order_rows` checks production `_refresh` requests `outline_window_index.json`. |
| Two offset monitors | Same case reads badge numbers 1, 2, 10 with conflicting global display IDs 9, 8, 0; output is [1, 2, 10, null]. |
| Real Workspace 5 shape | `taskbar_monitor_separators_are_workspace_local` renders [1, 2, 3, 4, separator, 5, 6, 7, 8], keeping the four landscape windows together before the portrait stack. Workspace 6 renders [1, 2, 3, separator, 4]. |
| Refresh input order | Reader/join case repeats with reversed provider input and changed titles; badge order stays [1, 2, 10, null]. |
| Invalid badge values | Reader/join case rejects zero, negative, fractional, null, empty, boolean and missing numbers/identities. Numeric string 2 remains accepted. |
| Stale desktop and missing/corrupt file | Retained reader/join assertions reject wrong desktop joins and recover to unknown IDs without hiding rows. |

| Retained behavior | Executable check and independent expectation |
| --- | --- |
| Unknown-ID fallback | `model_status_entries_sort_workspaces_and_monitor_rows` retains workspace/monitor ordering when no badge index is available. |
| Separator styling | Render case checks muted dashed drawing, standard horizontal padding and two-pixel top/bottom padding. |
| Full-width dash painting | `taskbar_dashed_separator_paints_full_allocated_width` checks production Cairo calls for widths 200, 460 and 780: endpoints 0 to allocated width, pattern [6, 4], thickness 1 and disposed context. |

The previous global-display-ID reader is last valid at commit e4e8419, but
never matched the workspace corner-badge contract. This repair uses only the
outline index's `number`; conflicting `display_id` values are deliberately
ignored. Existing unknown-ID workspace/monitor fallback, separator styling and
full-width Cairo painting remain covered by the cumulative gjs suite.
No CLI operands or selector numbering change.
