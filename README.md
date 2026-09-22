# Linux Taskbar Agentic Coding CLI Status Bar

GNOME taskbar indicator that displays the current rate-limit / usage status of
the major agentic coding CLIs — **Codex**, **Claude Code**, and **Grok CLI**,
plus **OpenRouter**, **OpenCode Go**, and **Cursor** spending (all three
opt-in, off by default, and their segments stay hidden until enabled) — polled
in the background **without consuming plan limits**. Cline is
deliberately not monitored: it has no `/usage`-style rate-limit surface yet.

The top bar shows one compact colored segment per CLI with the percentage of
each limit window **left** (remaining, matching the CLIs' own "limit left"
phrasing). Every percentage is colored individually on a smooth full-RGB
gradient: green at 100% left, fading to orange at 1% left, and snapping to
pure red at 0%. An exhausted weekly limit shows only its red local reset time
(`GK Thu 09:04`), hiding the irrelevant 5h value. If only a 5h limit is
exhausted, its red reset countdown replaces zero and the weekly values remain
visible (`CC 4h59m-41-14`); below one hour the countdown omits zero hours
(`CC 55m-41-14`). An exhausted model-scoped weekly limit stays inline as its
red reset clock (`CC 55m-41-Sun 17:00`). Once a window's reset moment passes it
has rolled over — with no fresh usage yet recorded it reads as fully available
(`100`) again rather than a stuck `0m`, so an exhausted window flips back on its
own the instant its countdown reaches zero. Reset clocks are rounded to the
nearest minute to avoid endpoint timestamp jitter. Otherwise the CLI tag takes the
color of its 5h window (Grok: its weekly window), dashes and separators stay
neutral gray, and stale data is dimmed with a trailing `?` — e.g.
`CX 43·57  CC 88·60·38  GK 12  OG 100·17·8  OR 58  CRS 77·55·88`.
When Agent Command Center has extra ChatGPT logins (`acc ca instance create`),
the Codex segment lists every account as `CX1`, `CX2`, … with a gray pipe:
`CX1 43·57 | CX2 12`. The `CXN` tags use a light-blue→purple identity
gradient (first account blue, last account purple); remaining-percent
digits keep the usual green→red usage colors. A trial/free plan is just another instance; its month
window is independent of the paid account's 5h/week windows. The click menu
lists `Codex [1]` and `Codex [2] trial *` as separate sections, with `*` on the
selected account.
Claude's third value is its active model-scoped weekly limit (currently
Fable) when `/usage` exposes one. Clicking the indicator opens a details menu
with each window's remaining budget, plan, and reset time.
Codex's manual-reset allowance remains visible from the last explicit
`/usage` report when newer Codex versions replace that report with an activity
heatmap that does not expose reset metadata.
The monitor prefers the Codex installed by `linux_codex_claude_code_setup`
over older system copies, submits slash commands with an Enter key, and treats
the menu's “No usage limit resets available” as zero, replacing a prior count.
When concurrent Codex sessions record several distinct rolling windows, the
poller uses the most depleted active window and ignores exhausted candidates
after their reset time, so an old pre-reset snapshot cannot mask current usage.

## How It Works

Two cooperating parts, connected only by a JSON file:

1. **Poller** (`agentic-cli-status` Rust binary, systemd user service
   `agentic-cli-status.service`) collects each enabled CLI's rate limits on a
   configurable interval — expressed **in seconds**, default every 5 — and
   atomically writes `~/.local/state/agentic-cli-status/status.json`. Slow
   sources run on background workers and protect themselves independently of
   the tick rate: the Claude endpoint is called at most every 180 s and tmux
   refreshes run at most once per minute per CLI. A slow endpoint or slash
   command therefore cannot pause the common five-second status-file cadence.
2. **GNOME Shell extension** (`agentic-cli-status@local`) watches that file
   with a file monitor plus a fallback timer paced at the same configured
   polling rate (`poll_interval_seconds` is echoed into `status.json`), so
   the indicator refreshes at the installed rate even if a monitor event is
   missed. It never talks to a CLI or the network itself.

The shared cadence defaults to 5 seconds. Configure it interactively through
the installer's `polling_rate` row, or non-interactively with
`ACS_POLL_INTERVAL_SECONDS=10 bash install.sh --select polling_rate`. The same
value drives every CLI scan, each status-file write, and the extension fallback
timer; slower billing/network refreshes retain their independent safety limits.

`polling_rate` is a settings row rather than an installed artifact, so it has no
detect or uninstall step of its own: deselecting it simply means "leave the
cadence alone", and the stored value lives in the poller's config, which the
poller's own components remove. To go back to the default, set it explicitly:
`ACS_POLL_INTERVAL_SECONDS=5 bash install.sh --select polling_rate`.

### Data sources (headless first — zero token cost)

| CLI | Headless source | tmux slash command |
|---|---|---|
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` plus every extra home listed in Agent Command Center's `~/.local/state/codex-agent-tracker/codex-instances.json` (each `CODEX_HOME` independently; trial/free plans are just another instance). `token_count` events (`rate_limits.primary/secondary` labeled from `window_minutes`: Plus 5h + weekly, free/inactive short windows ~monthly; `used_percent`, `resets_at`, plan). The debug-console Codex window is started with the last `caN`/`catN` `CODEX_HOME`. Stale-login detection reads that variable from the pane process (`/proc/<pid>/environ`), not `tmux show-environment`, which never sees `new-window -e`. When that process home is the selected login, `/status` updates that account even if used-percent jumped (for example remaining 0). Otherwise `/status` still applies only to the instance whose usage matches the pane | Bare `/usage` for available resets (bare `/usage` followed by an Enter key; the redeem entry is never selected), followed by `/status` for 5h/weekly and/or monthly limits. Debug consoles still running an older `codex` binary are recreated onto the resolved one. Each account keeps its own count across heatmap/`/status`-only ticks |
| Claude Code | OAuth usage endpoint (`api.anthropic.com/api/oauth/usage`) called with Claude Code's own stored credentials; returns the same 5h, weekly, and model-scoped weekly windows (including Fable) as `/usage`. Self-throttled to a configurable interval (60–3600 s, default 180) regardless of polling rate; 429 `Retry-After` lockouts are honored and the last good reading is kept on errors. An expired access token is refreshed from the stored refresh token — under an advisory lock, re-reading the credentials once held and writing them back atomically — even while Claude Code is running, because sessions launched with a long-lived `CLAUDE_CODE_OAUTH_TOKEN` never renew `~/.claude/.credentials.json` themselves | `/usage` |
| Grok CLI | `billing: fetched credits config` records in `~/.grok/logs/unified.jsonl` (historically `*.log`) — `creditUsagePercent` + weekly period + tier. Post-reset records omit `creditUsagePercent` (no usage yet) and are read as 0% used. The billing log is authoritative over the tmux `/usage` pane (its `Weekly limit left` border freezes between refreshes); a newer `balance exhausted` 402 error record with no later billing fetch reports 0% left | `/usage` |
| OpenCode Go | Headless scrape of the workspace `/go` page on `opencode.ai` (same embedded `rollingUsage`/`weeklyUsage`/`monthlyUsage` JSON as the [`opencode-go-usage`](https://npmx.dev/package/opencode-go-usage) plugin). Credentials from `OPENCODE_GO_WORKSPACE_ID`/`OPENCODE_GO_AUTH_COOKIE` or `~/.config/opencode/opencode-go-usage.json`. Self-throttled to ≥300 s | `/ogc-usage` |
| OpenRouter | Management API `POST /api/v1/analytics/query` (read-only; not chat/completions). Daily window for the configured free-model slug (default `stealth/ox-alpha`, limit 1000). Key from `OPENROUTER_MANAGEMENT_API_KEY` or `~/.config/agentic-cli-status/.env`. Self-throttled to ≥60 s | — |
| Cursor | Headless scrape of the spending dashboard's own usage endpoints (`/api/usage-summary` + `/api/dashboard/get-sand-usage-status`). The HTML `/dashboard/spending` URL 307-loops through a web bootstrap and is not fetched. Session cookie from `CRS_AUTH_COOKIE` or `~/.config/agentic-cli-status/cursor-spending.json` (percent-decoded). Included Pro pools only: Cursor Models, Other Models, Grok Bot, usage-limit reset. On-demand is never collected. Self-throttled independently of the bar tick (`CRS_FETCH_INTERVAL_SECONDS` or config, default 30 s), including after errors | — |

None of these send a prompt or start a model turn, so **monitoring never
consumes plan limits**: the Codex and Grok sources are files the CLIs already
write, the Claude endpoint is the billing/metadata endpoint that powers
`/usage` itself, OpenCode Go only reads the same quota page the plugin
scrapes, and Cursor only GETs the spending dashboard with a session cookie.

### tmux debug console

The poller maintains a detached tmux session (default `agentic-cli-status`)
with one window per monitored CLI running that CLI's TUI. Codex windows
wait until that account's `auth.json` exists, and a pane that started
before login is rebuilt afterwards, so the TUI cannot keep opening a
ChatGPT browser loop. Attach any time to inspect what the CLIs report:

```bash
tmux attach -t agentic-cli-status    # detach again with Ctrl+B D
```

In `auto` source mode the console doubles as a refresher: when a CLI's
headless data is older than its staleness threshold, the poller types the
CLI's rate-limit slash command (see table above) into its window, captures the
rendered pane as a fallback reading, and stores the capture under
`~/.local/state/agentic-cli-status/panes/<cli>.txt` for debugging. Codex is a
special case: a newly captured `/status` report replaces an equally new or
older rollout percentage snapshot because Codex can reset or reconcile limits
without appending a matching local `token_count` event. Only slash commands
are ever typed — never a prompt.

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

No sudo needed: everything installs under `$HOME`. Building the poller needs
a Rust toolchain (`cargo` via rustup, or Docker/Podman with `rust:1-bookworm`).
tmux itself is provided by `linux_installations_setup` in the master chain.
The core deploy (poller binary to `~/.local/bin/agentic-cli-status`, extension,
default config) always runs; keep `~/.local/bin` on `PATH`. The components are:

| Component | Default | What it controls |
|---|---|---|
| `codex_monitor` | on | Codex monitoring; reconfigure source mode + staleness |
| `claude_code_monitor` | **off (opt-in)** | Claude Code monitoring; same options |
| `grok_monitor` | on | Grok CLI monitoring; same options |
| `opencode_go_monitor` | **off (opt-in)** | OpenCode Go monitoring; registers `opencode-go-usage` in `~/.config/opencode/opencode.json` and walks through workspace ID + auth cookie setup (paste, edit `~/.config/opencode/opencode-go-usage.json`, or read from a file) |
| `openrouter_monitor` | **off (opt-in)** | OpenRouter free-model daily usage; enabling it adds the OR segment and walks through the management API key + monitored model slug. While off, no OR segment is shown and no analytics query is made |
| `cursor_monitor` | **off (opt-in)** | Cursor spending-page scrape (not a CLI); enabling it adds the CRS segment. Single-command setup: `CRS_AUTH_COOKIE=… [CRS_FETCH_INTERVAL_SECONDS=30] crs setup` |
| `polling_rate` | on | How often limits are checked — **always in seconds** (5–3600, default 5) |
| `tmux_console` | on | The background tmux debug console / refresher |
| `startup_service` | on | Poller starts at login (`systemd --user`) and keeps running |
| `model_status_bar` | **off (opt-in)** | Separate ACC model status bar extension (provider icons + running counts) |

The Codex reset forecast is part of the default Codex monitoring experience:
a clean install enables its public-site consensus automatically, and disabling
the Codex monitor also suppresses forecast fetching and display. The popup
starts with the 24-48h consensus (`26%-48%` when both horizons exist) followed
by one named row per forecast site, including every horizon that site publishes. The arrow button left of each
site opens its public page in the desktop's default browser; Polymarket points
directly to the newest active Codex-reset event returned by its public API. Each
open daily Polymarket expiry shows its cumulative trading volume in dollars
beside the Yes probability when the API publishes it.

Every CLI exposes the same reconfigurable option set (via the menu's edit
action or `bash install.sh --configure-component <id>`):

- **source** — `auto` (headless first, tmux pane fallback when stale),
  `headless` (never touch tmux), or `tmux` (pane capture only).
- **stale_after_seconds** — age after which headless data counts as stale.

**Claude Code is opt-in.** A default `bash install.sh` run leaves its `CC`
segment hidden. Enable it on demand with:

```bash
bash install.sh --select claude_code_monitor     # enable + configure it
bash install.sh --uninstall claude_code_monitor  # turn it back off
```

**OpenCode Go is opt-in.** A default `bash install.sh` run leaves it disabled:
no `opencode.ai` scraping, no plugin registration, and no OpenCode segment in
the taskbar. Enable it on demand with:

```bash
bash install.sh --select opencode_go_monitor     # enable + configure it
bash install.sh --uninstall opencode_go_monitor  # turn it back off
```

**OpenCode Go credentials** (for `opencode_go_monitor`): in the installer menu,
highlight **OpenCode Go usage** and press **→** (right arrow) to configure
credentials, or run `bash install.sh --configure-component opencode_go_monitor`.
The menu line shows `creds missing (press →)` until a workspace ID and auth
cookie are saved. To find the cookie: log in at [opencode.ai](https://opencode.ai),
open DevTools (**F12**), go to **Application** → **Cookies** →
`https://opencode.ai`, and copy the **Value** of the **`auth`** row (starts with
`Fe26.2…`). It is not in the page HTML — searching Elements/Styles for `fe26`
will not find it.

**Cursor spending is opt-in.** Cursor is not a coding CLI; CRS scrapes
[cursor.com/dashboard/spending](https://cursor.com/dashboard/spending) for
included Pro pools (Cursor Models, Other Models, Grok Bot) plus the usage
limit reset. On-demand usage is never collected. A default install leaves CRS
hidden. Enable it with one command (no TTY):

```bash
CRS_AUTH_COOKIE='WorkosCursorSessionToken-value' \
  CRS_FETCH_INTERVAL_SECONDS=30 \
  crs setup
```

`CRS_FETCH_INTERVAL_SECONDS` (default 30, range 5–3600) is independent of the
bar's 5-second poll tick. The cookie is the **WorkosCursorSessionToken** value
from DevTools → Application → Cookies → `cursor.com` while logged in on the
spending page. Equivalent installer path:
`CRS_AUTH_COOKIE=… bash install.sh --select cursor_monitor`.

`claude_code_monitor` additionally exposes **endpoint_min_interval_seconds**
(60–3600, default 180) — how often the OAuth usage endpoint may be checked.
The endpoint enforces its own small call budget (HTTP 429 with a multi-minute
`Retry-After` lockout when polled aggressively — measured 2026-07-03), so the
Claude reading cannot follow second-scale polling rates; on endpoint errors
the poller honors `Retry-After` and keeps showing the last good reading.

All options live in `~/.config/agentic-cli-status/config.json`; the poller
re-reads it every tick, so changes apply without a service restart.

### Optional: Agent Command Center model status bar

A second, independent taskbar extension (`acc-model-status@local`) shows one
icon per agent provider with the number of Agent Command Center agents of that
provider that are **currently running in open terminal windows**, plus a menu
listing those windows across every GNOME workspace. The menu is grouped in
workspace-number order; each row starts with the compact project label from
Phrase Automation's project catalog (for example `INS`), followed by the
harness's symbolic icon instead of a repeated provider name. The active
workspace's heading plus all of its rows share one continuous translucent teal
block. Switching workspaces, moving a terminal, opening or closing one, and
changing its ACC-managed title updates the bar immediately. Detached tmux
sessions remain in ACC but are not shown until their terminal window is opened.
The extension is opt-in and unrelated to the rate-limit indicator or the CLI
status line:

```bash
bash install.sh --select model_status_bar     # install just this component
bash install.sh --uninstall model_status_bar  # remove it again
```

It has no poller of its own: it renders
`~/.local/state/codex-agent-tracker/model_status_bar.json`, which the Agent
Command Center monitor publishes (honoring `CODEX_AGENT_TRACKER_STATE_DIR`).
It joins those agent rows to GNOME's live window titles and actual workspace
metadata; no subprocess or tmux polling is involved. Segments are green while
agents run, yellow when an agent needs input, gray when idle, and dimmed when
the payload stops being refreshed.

## Usage

```bash
systemctl --user status agentic-cli-status.service   # poller health
agentic-cli-status --show                            # print current status.json
agentic-cli-status --once                            # force one poll tick
journalctl --user -u agentic-cli-status.service      # poller logs (installed)
```

Repository checkouts log to `.log/` instead of the journal; the GNOME
extension logs to the Shell journal
(`journalctl --user -o cat /usr/bin/gnome-shell`, prefix
`[agentic-cli-status]`).

## Architecture

```
src/                         Rust poller crate (installed to ~/.local/bin/agentic-cli-status)
  logging.rs                 Centralized environment-aware logging (repo .log/ vs journald)
  config.rs                  Config schema/defaults/validation (~/.config/agentic-cli-status/config.json)
  http.rs                    HTTP opener used by Claude, OpenRouter, OpenCode Go, and Cursor collectors
  snapshot.rs                Shared status-window JSON shape
  collectors/                Headless collectors (codex/claude_code/grok/opencode_go/openrouter/cursor) + tmux pane parser
  tmux.rs                    Debug console: session/window management, slash-command refresh
  poller.rs                  Aggregating tick loop; writes status.json atomically
  installer_ops.rs           Installer config/credential helpers (config-get/set, OpenCode, OpenRouter)
  openrouter_client.rs       OpenRouter analytics query + management-key lookup
scripts/build_agentic_cli_status.sh  Release build into dist/agentic-cli-status
lib/acs_bin.sh               Installer helper that builds and invokes the poller CLI
agentic-cli-status@local/    GNOME Shell extension (file-monitor + poll-rate-paced timer + renderer only)
acc-model-status@local/      Optional ACC model status bar extension (provider icons + running counts)
  icons/                     Symbolic provider marks (Claude, Codex, Grok, Qwen, … + generic fallback)
installer/components.sh      Component manifest (shared installer framework)
lib/status_setup.sh          Install functions (core deploy, per-CLI config, service, tmux)
systemd/agentic-cli-status.service  User unit template (WantedBy=default.target)
tests/                       cargo tests (collectors/config/poller/gjs/deploy) + installer test
```

## Testing

```bash
cargo test                   # Rust unit and integration tests
bash tests/test_install.sh   # sandboxed installer behavior test (no sudo, no desktop changes)
bash scripts/check.sh        # fmt, clippy, tests, shell syntax, installer test
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

Team Codex accounts without a credit facility use the same usage percentages
and reset times as personal accounts. Their premium-family
`workspace_member_credits_depleted` event does not add a dollar marker or a
"team credits out" message; accounts reporting an actual credit facility retain
credit-depletion reporting.
