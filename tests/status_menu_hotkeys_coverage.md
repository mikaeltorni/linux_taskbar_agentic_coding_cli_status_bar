# Status menu workspace-hotkey coverage

Runner: `cargo test --test gjs status_menu_hotkeys_remain_available_when_open -- --exact`.

## Coverage inventory

The click menu is a GNOME Shell `PopupMenu`, not a command parser, so there are
no operand, conversion, domain-rejection, or unchanged-state cases. The shared
menu manager normally opens popups with `Shell.ActionMode.POPUP`; this feature
must temporarily include `NORMAL` so regular Shell bindings still run, then
restore the manager's original mode after closing.

| Behavior | Saved assertion | Expected result |
| --- | --- | --- |
| Open the status menu from the existing popup mode | `tests/gjs.rs::status_menu_hotkeys_remain_available_when_open` | The active menu grab includes both `POPUP` and `NORMAL`. |
| Close the status menu | `tests/gjs.rs::status_menu_hotkeys_remain_available_when_open` | The original manager action mode is restored exactly. |
| Install the action-mode callback before panel registration wires the manager listener | `tests/gjs.rs::status_menu_hotkeys_remain_available_when_open` | The callback source appears in the button initializer before `addToStatusArea`, so the manager sees the updated mode during its open handler. |
| User-visible workspace switching | Manual session check after deployment | The configured workspace shortcut switches workspace while the status menu remains open; clicking a menu row still works. |

Baseline regression: failed because `setStatusMenuActionMode` was absent. The
focused regression passes after the helper and menu callback were added. The
retained open-menu refresh probe now exercises both open and closed action modes
while preserving stable click targets.

Verification: `cargo test` passes (231 library tests, one cold-start probe, 18
deploy tests, and 29 GJS tests); `bash tests/test_install.sh` passes all 92
assertions. The user-visible workspace shortcut check remains pending until the
merged extension is deployed and reloaded in the active X11 session.
