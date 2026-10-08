# Grok default coverage

Feature: `remove grok from the status task bar as default and for now.`

| Rule | Executable case | Public call and fixture | Independent expected observation |
| --- | --- | --- | --- |
| Fresh install leaves Grok disabled by default | `tests/test_install.sh`, default install block | Run `install.sh` against a fresh sandbox home | `clis.grok.enabled` is `false`; Codex remains `true`, Cursor remains opt-in `false` |
| Grok remains manually selectable | `tests/test_install.sh`, Grok opt-in block | Run `install.sh --select grok_monitor` after the default install | `clis.grok.enabled` is `true`; Codex remains `true` |
| A later default install preserves an explicit Grok choice | `tests/test_install.sh`, idempotence block | Run `install.sh` after explicit selection | `clis.grok.enabled` remains `true` |
| Uninstall disables Grok without disabling Codex | `tests/test_install.sh`, uninstall block | Run `install.sh --uninstall grok_monitor` | Grok is `false`; Codex remains `true`; Claude Code remains `false` |
| Default component listing shows Grok as opt-in | `tests/test_install.sh`, side-effect-free listing block | Run `install.sh --list-components` in a fresh sandbox home | `grok_monitor` is listed with default `off`, without deploying files |
| A missing poller config uses the new default | `src/config.rs`, `defaults_when_missing` | Load config in a fresh `Sandbox` | Only Codex is enabled by default; Grok and Cursor are disabled |
| Grok collection still works when explicitly enabled | `src/poller.rs`, `aggregates_enabled_clis_and_writes_status` | Save a config with `clis.grok.enabled=true`, then call `poll_once` using seeded Grok and Codex data | Grok retains its expected usage window and Codex remains enabled |
| The taskbar cold-start fallback matches poller defaults, while explicit selection still shows Grok | `tests/gjs.rs`, `opt_in_clis_stay_off_the_bar_until_enabled` | Evaluate `agentic-cli-status@local/extension.js` with no status file and with explicit enabled config | Cold-start Grok is `false`; explicit Grok, Codex, and Cursor settings are honored |

No command syntax, numeric conversion, or operation-domain rule changes in this feature. Existing installer checks continue to cover invalid polling interval rejection and retained Cursor/Codex state.
