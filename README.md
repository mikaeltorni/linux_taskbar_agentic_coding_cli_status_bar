# Linux Taskbar Agentic Coding CLI Status Bar

GNOME taskbar indicator that displays the current rate-limit / usage status of
the three major agentic coding CLIs — **Codex**, **Claude Code**, and **Grok
CLI** — polled in the background **without consuming plan limits**. Cline is
deliberately not monitored: it has no `/usage`-style rate-limit surface yet.

The top bar shows one compact colored segment per CLI with the percentage of
each limit window **left** (remaining, matching the CLIs' own "limit left"
phrasing): green when more than 40% is left, yellow at 16–40%, red at 15% or
less, dimmed with a trailing `?` when the data is stale — e.g.
`CX 43·57%  CC 88·60·38%  GK 12%`. Claude's third value is its active
model-scoped weekly limit (currently Fable) when `/usage` exposes one. Clicking the indicator opens a details menu
with each window's remaining budget, plan, and reset time.

## How It Works

Two cooperating parts, connected only by a JSON file:

1. **Poller** (`agentic_cli_status` Python package, systemd user service
   `agentic-cli-status.service`) collects each enabled CLI's rate limits on a
   configurable interval — expressed **in seconds**, default every 5 — and
   atomically writes `~/.local/state/agentic-cli-status/status.json`. Slow
   sources protect themselves independently of the tick rate: the Claude
   endpoint is called at most every 180 s and tmux refreshes run at most once
   per minute per CLI.
2. **GNOME Shell extension** (`agentic-cli-status@local`) watches that file
   with a file monitor plus a fallback timer paced at the same configured
   polling rate (`poll_interval_seconds` is echoed into `status.json`), so
   the indicator refreshes at the installed rate even if a monitor event is
   missed. It never talks to a CLI or the network itself.

### Data sources (headless first — zero token cost)

| CLI | Headless source | tmux slash command |
|---|---|---|
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` `token_count` events (`rate_limits.primary/secondary`: 5h + weekly `used_percent`, `resets_at`, plan) | `/status` |
| Claude Code | OAuth usage endpoint (`api.anthropic.com/api/oauth/usage`) called with Claude Code's own stored credentials; returns the same 5h, weekly, and model-scoped weekly windows (including Fable) as `/usage`. Self-throttled to ≥ 180 s regardless of polling rate | `/usage` |
| Grok CLI | `billing: fetched credits config` records in `~/.grok/logs/*.log` (`creditUsagePercent` + weekly period + tier) | `/usage` |

None of these send a prompt or start a model turn, so **monitoring never
consumes plan limits**: the Codex and Grok sources are files the CLIs already
write, and the Claude endpoint is the billing/metadata endpoint that powers
`/usage` itself.

### tmux debug console

The poller maintains a detached tmux session (default `agentic-cli-status`)
with one window per monitored CLI running that CLI's TUI. Attach any time to
inspect what the CLIs report:

```bash
tmux attach -t agentic-cli-status    # detach again with Ctrl+B D
```

In `auto` source mode the console doubles as a refresher: when a CLI's
headless data is older than its staleness threshold, the poller types the
CLI's rate-limit slash command (see table above) into its window, captures the
rendered pane as a fallback reading, and stores the capture under
`~/.local/state/agentic-cli-status/panes/<cli>.txt` for debugging. Only slash
commands are ever typed — never a prompt.

Console windows are created with `ATTS_SUPPRESS_ANNOUNCEMENTS=1` and
`CODEX_AGENT_SKIP_TRACKER=1` in their environment so the idle TUIs stay
silent monitoring surfaces: their lifecycle hooks never speak TTS
announcements and the codex launcher skips agent-tracker registration.

## Installation

```bash
bash install.sh                   # interactive component menu (TTY), else defaults
bash install.sh --default         # core + every default-on component, no prompts
bash install.sh --all             # core + every component, no prompts
bash install.sh --select a,b,c    # core + only the listed component ids
bash install.sh --list-components # machine-readable component list (no deploy)
```

No sudo needed: everything installs under `$HOME`. tmux itself is provided by
`linux_installations_setup` in the master chain. The core deploy (poller
package, extension, default config) always runs; the components are:

| Component | Default | What it controls |
|---|---|---|
| `codex_monitor` | on | Codex monitoring; reconfigure source mode + staleness |
| `claude_code_monitor` | on | Claude Code monitoring; same options |
| `grok_monitor` | on | Grok CLI monitoring; same options |
| `polling_rate` | on | How often limits are checked — **always in seconds** (5–3600, default 5) |
| `tmux_console` | on | The background tmux debug console / refresher |
| `startup_service` | on | Poller starts at login (`systemd --user`) and keeps running |

Every CLI exposes the same reconfigurable option set (via the menu's edit
action or `bash install.sh --configure-component <id>`):

- **source** — `auto` (headless first, tmux pane fallback when stale),
  `headless` (never touch tmux), or `tmux` (pane capture only).
- **stale_after_seconds** — age after which headless data counts as stale.

All options live in `~/.config/agentic-cli-status/config.json`; the poller
re-reads it every tick, so changes apply without a service restart.

## Usage

```bash
systemctl --user status agentic-cli-status.service   # poller health
python3 -m agentic_cli_status.poller --show          # print current status.json
PYTHONPATH=~/.local/share/agentic-cli-status \
  python3 -m agentic_cli_status.poller --once        # force one poll tick
journalctl --user -u agentic-cli-status.service      # poller logs (installed)
```

Repository checkouts log to `.log/` instead of the journal; the GNOME
extension logs to the Shell journal
(`journalctl --user -o cat /usr/bin/gnome-shell`, prefix
`[agentic-cli-status]`).

## Architecture

```
agentic_cli_status/          Python poller package (deployed to ~/.local/share/agentic-cli-status)
  acs_logging.py             Centralized environment-aware logging (repo .log/ vs journald)
  config.py                  Config schema/defaults/validation (~/.config/agentic-cli-status/config.json)
  collectors.py              Headless collectors (codex/claude_code/grok) + tmux pane parser
  tmux_console.py            Debug console: session/window management, slash-command refresh
  poller.py                  Aggregating tick loop; writes status.json atomically
agentic-cli-status@local/    GNOME Shell extension (file-monitor + poll-rate-paced timer + renderer only)
installer/components.sh      Component manifest (shared installer framework)
lib/status_setup.sh          Install functions (core deploy, per-CLI config, service, tmux)
systemd/agentic-cli-status.service  User unit template (WantedBy=default.target)
tests/                       pytest (collectors/config/poller/deploy contract) + installer test
```

## Testing

```bash
python3 -m pytest tests/     # Python unit tests
bash tests/test_install.sh   # sandboxed installer behavior test (no sudo, no desktop changes)
```

## Troubleshooting

- **Indicator shows `CX –`** — the poller has not produced data yet, or that
  CLI has never run: start the CLI once (or wait one poll tick), and check
  `systemctl --user status agentic-cli-status.service`.
- **Claude segment stuck / `HTTP 429`** — the OAuth usage endpoint throttles
  aggressive polling; the collector already enforces ≥ 180 s. Persistent 401 /
  "expired" errors mean Claude Code needs a fresh login.
- **Stale `?` markers** — the CLI has not written new local data and the tmux
  refresh could not parse the pane; attach to the tmux session and check the
  CLI's window (trust prompts on first launch must be answered once).

## Disclaimer

This software is provided under the MIT License on an **“as is”** basis, without warranties of any kind. To the maximum extent permitted by applicable law, the authors and copyright holders shall not be liable for any claims, damages, losses, or other liability arising from the use of this software.

You are solely responsible for determining whether this software is suitable, safe, lawful, and appropriate for your intended use. Unless explicitly stated otherwise, this project is general-purpose software and is not designed, tested, certified, or approved for safety-critical, medical, automotive, aviation, industrial-control, life-support, cybersecurity-critical, financial-critical, or other high-risk use cases.

The authors and copyright holders make no guarantees regarding security, reliability, availability, correctness, compliance, non-infringement, or fitness for any particular purpose.

This notice is intended to clarify the nature of the project and does not impose additional restrictions beyond the MIT License.
