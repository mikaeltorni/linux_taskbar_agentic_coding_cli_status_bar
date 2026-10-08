# Taskbar window ordering checks

Run `cargo test --test gjs taskbar_window_position_ids_order_rows`, `cargo test`, and `bash tests/test_install.sh` from the repository root. The Rust test executes extracted production functions under gjs with isolated file and Shell stubs, without GUI effects.

| Behavior | Executable check and independent expectation |
| --- | --- |
| Position IDs beat session/provider/title order | Joined note IDs have display IDs 0, 2, 10; group rows have IDs [0, 2, 10], despite opposite titles/providers/session identifiers. |
| Zero ID and numeric strings | Published 0 and string 2 normalize to integer keys. |
| Missing/corrupt index | Reader returns empty Map for missing, malformed, non-array windows. |
| Invalid index rows | Negative, fractional, null display IDs and absent identity are ignored. |
| Unmatched rows / stale workspace | Unknown number sorts last; index from another desktop does not join. |
| Retained monitor/workspace grouping | Existing cargo/gjs workspace sort case remains active. |

| Monitor separators | taskbar_monitor_separators_are_workspace_local renders workspace 1 as heading, ID 0, literal grey dashes, ID 2, ID 10; workspace 2 as heading, ID 11, literal grey dashes, ID 12; single/unknown monitor workspaces get no internal line. |
| Separator styling | Same test checks the dedicated separator class has an explicit muted grey eight-pixel text and zero vertical padding. |
