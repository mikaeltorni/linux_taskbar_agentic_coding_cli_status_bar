# Cursor and Codex instance coverage

Feature: `cursor and codex instances will stay`

| Rule | Executable case | Public call and fixture | Independent expected observation |
| --- | --- | --- | --- |
| Enabled Codex and Cursor stay available with Grok off | `tests/gjs.rs`, `opt_in_clis_stay_off_the_bar_until_enabled` via `tests/extension_color_probe.js` | Resolve explicit `{codex: true, cursor: true, grok: false}` config, render two Codex accounts and Cursor included-usage windows | Codex and Cursor remain enabled, Grok is disabled, markup retains `CX1`, `CX2`, and `CRS` with both Cursor balances |
| Installer preserves existing Cursor and Codex settings after Grok is disabled | `tests/test_install.sh`, Cursor setup and default reinstall block | Enable Cursor through the mocked `crs setup`, uninstall Grok, then run the default installer | `clis.codex.enabled` and `clis.cursor.enabled` remain `true`; `clis.grok.enabled` remains `false` |

No application change is needed if both public checks pass. The default Codex and Cursor feature policies remain separate: Codex is enabled by default, while Cursor stays opt-in and preserves an existing enabled choice.
