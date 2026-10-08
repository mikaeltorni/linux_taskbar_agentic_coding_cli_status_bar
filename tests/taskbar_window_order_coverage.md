# Taskbar window ordering checks

Run `cargo test --test gjs taskbar_window_position_ids_order_rows`, `cargo test`, and `bash tests/test_install.sh` from the repository root. The Rust test executes extracted production functions under gjs with isolated file and Shell stubs, without GUI effects.

| Behavior | Executable check and independent expectation |
| --- | --- |
| Position IDs beat session/provider/title order | Joined note IDs have display IDs 0, 2, 10 across opposing monitors 1, 2, 0; group rows have IDs [0, 2, 10, null], despite monitor/provider/title/session order. Unknown IDs remain last even on the first monitor. |
| Zero ID and numeric strings | Published 0 and string 2 normalize to integer keys. |
| Missing/corrupt index | Reader returns empty Map for missing, malformed, non-array windows. |
| Invalid index rows | Negative, fractional, null display IDs and absent identity are ignored. |
| Unmatched rows / stale workspace | Unknown number sorts last; index from another desktop does not join. |
| Retained workspace grouping / unknown-ID fallback | Existing cargo/gjs workspace sort case remains active. |

| Monitor separators | taskbar_monitor_separators_are_workspace_local renders workspace 1 as heading, ID 0, full-width drawn dashes, ID 2, full-width drawn dashes, ID 10; workspace 2 as heading, ID 11, full-width drawn dashes, ID 12; single/unknown monitor workspaces get no internal line. |
| Separator styling | Same test checks the dedicated separator class has an muted grey drawing, standard horizontal padding and two-pixel top/bottom padding. |

| Full-width dash painting | taskbar_dashed_separator_paints_full_allocated_width checks production Cairo calls for widths 200, 460 and 780: endpoints 0 to allocated width, pattern [6, 4], thickness 1 and disposed context. |

Follow-up contract replaces monitor-primary ordering for known corner IDs. The
last valid prior rule is commit 1b160a8. Numeric ID now precedes monitor;
workspace headings remain ordered and monitor separators mark each transition.
No CLI operands change; existing index validation and missing/corrupt index
assertions remain in the same public file-reader/join case.
