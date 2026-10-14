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
selected account. A Codex pane's separate `Luna Reserve` weekly pool is kept
out of the normal weekly value; when it has budget left, only its remaining
percentage appears next to the account that reported it, while the click menu
always lists the reserve under that account as `Luna Reserve`.
Claude's third value is its active model-scoped weekly limit (currently
Fable) when `/usage` exposes one. Clicking the indicator opens a details menu
with each window's remaining budget, plan, and reset time.
Codex's manual-reset allowance is refreshed independently for each account
through the installed CLI's read-only `account/rateLimits/read` metadata request
every minute. The menu shows zero explicitly; a failed read hides an old count
until the next retry. This does not redeem a reset or send a prompt.
The monitor prefers the Codex installed by `linux_codex_claude_code_setup`
over older system copies and submits slash commands with an Enter key.
When concurrent Codex sessions record several distinct rolling windows, the
poller uses the most depleted active window and ignores exhausted candidates
after their reset time, so an old pre-reset snapshot cannot mask current usage.
Across account homes, shared hard-linked rollouts are ignored; the newest
observed account-owned rollout wins even if an older file was touched later.
The poller keeps a newer console reading for each account until fresh usage
arrives. Codex debug consoles use `--no-daemon` when the installed CLI supports
it, so an incomplete optional background-server package cannot stop `/status`.

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
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` plus every extra home listed in Agent Command Center's `~/.local/state/codex-agent-tracker/codex-instances.json` (each `CODEX_HOME` independently; trial/free plans are just another instance). `token_count` events (`rate_limits.primary/secondary` labeled from `window_minutes`: Plus 5h + weekly, free/inactive short windows ~monthly; `used_percent`, `resets_at`, plan). The debug-console Codex window is started with the last `caN`/`catN` `CODEX_HOME`. Stale-login detection reads that variable from the pane process (`/proc/<pid>/environ`), not `tmux show-environment`, which never sees `new-window -e`. When that process home is the selected login, `/status` updates that account even if used-percent jumped (for example remaining 0). Otherwise `/status` still applies only to the instance whose usage matches the pane | Read-only `account/rateLimits/read` for each account’s current reset count; bare `/usage` and `/status` still refresh visible 5h/weekly and/or monthly limits, and the redeem entry is never selected. Debug consoles still running an older `codex` binary are recreated onto the resolved one. The count is rechecked every minute and zero is displayed explicitly |
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
ChatGPT browser loop. Each Codex pane is captured before any key is sent.
A pane that is still blank, or that shows the ChatGPT sign-in or "finish
signing in via your browser" screen, gets no keys at all. An Enter there
would open a browser tab, and the next Escape would cancel that login's
localhost callback. That happens when an expired token still leaves an
`auth.json` behind. The first time an account's pane shows that screen, the
poller runs `~/.local/bin/acc ca instance login N` once. Agent Command Center
then opens that account's sign-in console, and pressing Enter there starts
the browser login. After sign-in rewrites `auth.json`, the pane is rebuilt
and polling resumes. Attach any time to inspect what the CLIs report:

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

The public setup entrypoint is `bash install.sh`. It accepts these commands:

```bash
bash install.sh                                  # interactive menu (TTY), else defaults
bash install.sh --config NAME                    # load installation_configs/NAME
bash install.sh --default                        # install default-on components
bash install.sh --all                            # install every component
bash install.sh --select a,b,c                   # install selected component ids
bash install.sh --reconfigure a,b,c              # reapply selected component config
bash install.sh --uninstall a,b,c                # uninstall selected components
bash install.sh --auth                           # allow authenticated Git operations
bash install.sh --list-components                # list component ids and defaults
bash install.sh --list-configurable-components   # list components with detail screens
bash install.sh --list-select-configure-components # list components configured on select
bash install.sh --list-component-config-values   # list editable configuration values
bash install.sh --configure-component ID         # open a component's detail screen
bash install.sh --detect                         # show installed component state
bash install.sh --export-selection               # export resolved selection as JSON
bash install.sh --help                            # show installer help
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
| `grok_monitor` | **off (opt-in)** | Grok CLI monitoring; same options. Grok is off by default for now and can be enabled with `bash install.sh --select grok_monitor` |
| `opencode_go_monitor` | **off (opt-in)** | OpenCode Go monitoring; registers `opencode-go-usage` in `~/.config/opencode/opencode.json` and walks through workspace ID + auth cookie setup (paste, edit `~/.config/opencode/opencode-go-usage.json`, or read from a file) |
| `openrouter_monitor` | **off (opt-in)** | OpenRouter free-model daily usage; enabling it adds the OR segment and walks through the management API key + monitored model slug. While off, no OR segment is shown and no analytics query is made |
| `cursor_monitor` | **off (opt-in)** | Cursor spending-page scrape (not a CLI); enabling it adds the CRS segment. Single-command setup: `CRS_AUTH_COOKIE=… [CRS_FETCH_INTERVAL_SECONDS=30] crs setup` |
| `polling_rate` | on | How often limits are checked — **always in seconds** (5–3600, default 5) |
| `tmux_console` | on | The background tmux debug console / refresher |
| `startup_service` | on | Poller starts at login (`systemd --user`) and keeps running |
| `model_status_bar` | **off (opt-in)** | Separate ACC model status bar extension (provider icons + running counts) |

The Codex reset forecast is part of the default Codex monitoring experience:
a clean install enables its public sources automatically, and disabling
the Codex monitor also suppresses forecast fetching and display. The panel
shows available reset consensus percentages immediately before Codex usage
counters (`28%-49% | CX1 …` when both horizons are available). If only one
horizon has a value, the panel shows that value; stale consensus carries a
trailing `?`. The forecast stays visible without opening the popup and is
hidden when there is no valid consensus. The popup has a plain
`Codex reset forecast` heading followed by one named row per source. Forecast
rows show every published horizon; WhenReset links to reset history without
adding a percentage to the consensus.
The 24h/48h percentages come from Codex Reset Monitor (`codexreset.org`).
Clicking or pressing Enter on a source row opens its public page in the
desktop's default browser. Each source can be disabled with its
`codex_reset_forecast.sites.<id>.enabled` setting.

Polymarket, `codex_reset` (codex-reset.com), and `will_codex_reset` (Will Codex
Reset) are retired sources: they are no longer fetched or shown. Old enable
settings and cached rows for them are ignored automatically. Only
`codex_reset_monitor` and `whenreset` remain in the source catalog.

### Public Codex reset sources

These sites follow discretionary, broadly announced Codex resets. They cannot
show when **your** 5-hour or weekly allowance will reset; use Codex `/status`
or [OpenAI's usage guidance](https://help.openai.com/en/articles/11369540-using-codex-with-your-chatgpt-plan) for your account. OpenAI says occasional one-time resets are not guaranteed.

| Site | What it publishes | Available surface |
| --- | --- | --- |
| [Codex Reset Monitor](https://codexreset.org/) | Verified timeline and 24h/48h estimates | Public HTML page |
| [WhenReset](https://whenreset.dev/codex) | Latest direct or banked reset, with original posts and history | Public Codex RSS feed; shown as historical context |

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

A bell always appears at the far left of the ACC status indicator, including
when no agent windows are open. Click it to mute or unmute announcements across
all ACC instances: it runs the installed `agent-command-center silent toggle`,
the same shared control as `acc silent on` / `acc silent off`. A slashed bell
means muted; a regular bell means enabled. Hovering or focusing the bell
highlights its clickable area. The bell is 20px with a minimum 28px-wide click
target. The indicator reads ACC's published
`announcements_muted` setting and updates even while its menu is open. ACC's
per-invocation `ATTS_SILENT` / legacy `ACC_SILENT` environment overrides still
apply.

Both taskbar extensions keep routine rendering silent: they do not dump agent
payloads, forecasts, widget objects, or function entry/return traces into the
system journal. Startup and actionable error reports remain available. Progress
widgets defer geometry queries until attached to the Shell stage, including
construction and teardown, to avoid off-stage theme warnings.

The log-volume regression runs the real JavaScript helpers against isolated
actors and observed output sinks, checking 500 status/forecast calls plus
attached/detached allocation behavior:

```bash
gjs tests/log_volume.js .
cargo test --test log_volume
```

The existing `model_status_bar` installer component deploys the corrected source
on a clean installation. System-wide retention and recovery commands belong to
`linux_configuration_setup`'s `log_retention` component; see that repository's
README when recovering an already oversized `/var/log/syslog`.

A second, independent taskbar extension (`acc-model-status@local`) shows the
number of Agent Command Center agents of each provider that are **currently
running in open terminal windows** to the left of that provider's icon. The
panel prints a compact tok/s figure to the right of the icon for each active
harness (`12/s` or `131k/s`, or the last-minute average when the 1-second
window is 0). Each open agent also gets a **50px progress bar** beside those
counts and rates. Its fill shows ACC's actual workflow completion percentage,
with a continuous red-to-green gradient for ordinary and goal runs: red at
0%, green at 100%, and intermediate colors based on measured completion.
Active goal bars retain this gradient. Goal identity appears in the purple
runtime above the bar. Focused bars keep their existing teal outline. A
rate-limited agent uses the Notes RATE pink/purple (`#f472b6`), also used by
its menu badge. A DRAFT agent uses lime green (`#84c32f`), matching its menu
badge, for both its track and measured fill. At 0% it shows only the faint
lime track. These state colors preserve fill widths, track/fill alpha,
and the existing opacity animation. Confirmed
finished agents show a full bar; unavailable workflow progress shows an empty
neutral track. The progress fills contain no percentage text.

Progress bars are grouped by ascending workspace number, with agents ordered
by their terminal window numbers within each group. Larger round grey dots
separate workspace groups across the full taskbar height. A 13.5px workspace
label (`W2`, for example) sits inline immediately before that group's first
bar, after the provider/token summary for the first group. Label colors follow
a fixed blue-to-violet gradient from W1 (`#60a5fa`) to W8 (`#c084fc`), with each
intermediate workspace interpolated on the same scale. Higher workspace
numbers keep the W8 color; unknown workspaces show neutral `W?`. While a turn is actively running, its bar
smoothly pulses from 100% opacity to 30% and back every three seconds, matching
the terminal indicator. For active goals, only the progress fill and track pulse;
the runtime text and teal focus edges stay steady. Stale goal bars still use the usual dimmed appearance. A finished, unopened instance keeps full opacity while
its green fill smoothly turns white and back over the same three-second cycle.
Ready bars stay at least as bright as their original green rather than fading.
Other idle bars stay steady; stale snapshots stay dimmed.
Status refreshes reuse the bars so they remain visible throughout the pulse.
Updates continue while the drop-down is open. The focused terminal's bar gets
a subtle **one-pixel teal outline** (`#00ffe6`, matching the CLI selection cue).
The outline sits one pixel outside the unchanged 50×8 fill, using a 52×10
drawing area so it never covers progress pixels. Bars within each workspace
have a six-pixel gap between their drawing areas. At each monitor change
within a workspace, a thin teal vertical line spans the centered half of the
taskbar height, with
six pixels of space on each side (twice the normal spacing, plus the one-pixel
line). Windows on the same monitor keep their normal gap. Monitor moves update
these boundaries automatically without changing terminal-number order. The outline follows window
focus; other terminals on the same workspace stay unoutlined.
Hover an individual agent bar to show its colored task detail row in a separate
box anchored above that bar: project tag, provider icon, Codex instance, phase
or run clock, task title, branch, and any remaining estimate. The box stays in
place as the pointer moves within the bar, disappears on leave, and is hidden
while the right-click details menu is open. Left-click still focuses that exact
agent terminal.

**Ctrl+left-click** an instance bar, or its task row in the right-click details
menu, to ask **“Close this task and mark Done? Right-click to confirm”**. The
confirmation shows the colored task details above the selected instance. From
the details menu, the menu closes before the confirmation appears.
**Right-click** the same bar or the confirmation to finish that task and close
its CLI window. **Escape** or a click outside cancels. Ctrl+left-click itself
never completes or closes a task.

This uses Phrase Automation's existing focused-task completion service: a
private timestamped notes backup, the move to Done, and cleanup of the exact
registered ACC tmux session and its Kitty window. The installed `pa` command
must support `mark-in-progress-done --close-session SESSION_ID`. The taskbar
passes only the selected registered session ID; Phrase Automation resolves and
validates the note association. The panel's display note ID is not used as a
completion ownership check. Sessions without
saved note-link fields use Phrase Automation's existing recovery matcher: the
full launch prompt, tagged project and saved task title must identify one
in-progress task. Missing or ambiguous ownership is rejected with a desktop
error showing the native failure reason; other tasks stay open. Ordinary
left-click continues to focus the selected terminal.

The public render probe runs with
`cargo test --test panel_progress --test panel_interaction --test task_confirmation` (or
`gjs tests/panel_progress.js .`, `gjs tests/panel_interaction.js .`, and
`gjs tests/task_hover.js .`, `gjs tests/task_confirmation.js .`, and
`gjs tests/task_completion_subprocess.js .`) using
isolated actors without changing the desktop or user metadata. These checks
cover per-instance clicks and hover, right-click details, retained clocks,
and the existing menu shortcut.

The drop-down
starts with per-harness tok/s, tok/min, and tok/hour for those same active
harnesses (aggregated by harness, not by numbered instance such as `ca1`/`ca2`;
idle 0 tok/s rows and missing counters are omitted) and then lists those
windows across every GNOME workspace. When a visible agent used
two or more distinct linked Git worktrees, the panel count and that agent's
menu origin show `·N` next to the worktree identity. Each workspace appears in
one contiguous section, ordered by ascending workspace number; rows within
each section are sorted numerically by the window IDs
shown in the terminal’s top-left corner. These are read from ACC’s
`outline_window_index.json` (`number`, starting at 1 in each workspace), the
same index used by the window-focus hotkeys. The outline helper numbers one
monitor at a time in physical layout order, then rows from left to right on
that monitor. The menu follows those published numbers on every refresh;
account and conversation IDs do not determine task order. ACC's separate
`cli_window_ids.json` contains global CLI selector IDs and does not set this
menu's order.
Rows without a current published window ID follow numbered rows. A full-width grey dashed
line, aligned with the standard separators and with minimal vertical spacing,
marks each monitor change within that ID order; single-monitor workspaces
have no internal separator. Each row starts with the compact project
label from Phrase Automation's project catalog (for example `INS`), followed by
the harness's symbolic icon instead of a repeated provider name. Codex rows
place their `CX1`, `CX2`, … account name immediately to the right
of that icon, using the same blue-to-purple identity color as the main
rate-limit indicator. Other live ACC kinds place `GK1`, `CC1`, `QA1`, `OA1`,
`NA1`, `OC1`, `CL1`, or `AP1` in that same spot even when only one of that
kind is open. Only the
active workspace heading gets a translucent teal background. Agent rows keep
their text and CX instance colors, with measured task progress behind the text. A finished agent that
has not been opened since its turn ended pulses from green toward white on its
individual bar, with no green glow around the panel and no `W<n>:<count>` workspace labels
on the panel. The expanded menu retains a steady green unread row highlight
and workspace completion counts. These counts include only finished, unopened
turns; waiting, stopped, and already opened turns do not appear in them.
Focusing that agent's CLI window stops its pulse and clears its menu highlights; a turn that finishes while its window is focused
starts acknowledged. The extension keeps acknowledgements in
`~/.local/state/codex-agent-tracker/model-status-unread.json` so a Shell reload
does not revive an opened completion. Each new turn can raise a new highlight.
ACC's published `ready_completed_at` identifies a confirmed finish;
`last_worked_at` remains work history. Temporary RUNNING states, older payload
redraws, and history updates cannot rearm an opened completion. When saved
state is missing or first migrated, historical finishes start acknowledged;
only a confirmed finish within 30 seconds can create an initial unread mark.
Saved unread turns remain unread across reloads until selected. Legacy ACC
payloads lacking the confirmed field still use phase transitions.

Each agent's bar has small timing labels centered above and below its fill.
The upper label shows runtime as `HH:MM` (hours and whole minutes, without
seconds) in 13.5px text, 1.5 times its original size. While the agent runs,
it uses the current task's start or ACC's published active-duration measurement
when the start is unavailable. Regular runtime text is yellow (`#e6c200`);
active goal runtime text is purple (`#a855f7`). These running colors stay steady
while the progress fill pulses. An instance that is rate-limited and still
waiting to start uses that same upper slot for the launch countdown, prefixed
with `-` (for example `-02:45`). A partial minute stays visible as `-00:01`.
The countdown is yellow-green (`#b5d747`), or red (`#ef4444`) when the session
is interrupted. Finished work does not show this countdown.

When a task finishes, its timer freezes at the final duration and flashes
green to white with the completed bar on the same three-second cycle. It stays
visible while that session is unread, until opening or focusing its terminal
acknowledges the completion. Acknowledgement hides the timer and stops its
flash; switching away does not bring it back. Other unread sessions retain
their own timers. Starting another task shows a fresh running clock. Stopped,
interrupted and other unfinished idle sessions have no timer.

The taskbar retains the running start across completion and workspace moves.
After an extension reload, an unread completion can use ACC's published
completed active-duration measurement when its start is unavailable. Unknown
or invalid timing is omitted. Stale completed timers stay dim and stop flashing,
matching their bars.

The lower label shows that running agent's own remaining estimate in 13.5px
monospace text, 1.5 times its original size. It keeps only the duration, such
as `~3m` or `~1h30m`; hour/minute components are compact and any text after the
duration unit is omitted. It disappears when the agent stops running, when a
launch countdown is showing, or when no estimate is available. A rate-limited
wait does not put time left under the bar. The far-right aggregate time estimate is removed. Timing labels
stay readable while the progress fill pulses, update in place with the menu
open, and dim when the snapshot is stale; stale runtimes stop ticking.
All timings come from ACC's local `model_status_bar.json` data, with no AI
requests or writes to history or notes. Install/update with
`bash install.sh --select model_status_bar`. On Wayland, newly installed
extension code takes effect after logout/login.

Each task with an ACC workflow plan has colored background segments matching
its terminal workflow bar: blue, purple, teal and amber for completed phases,
gold for the active action, and charcoal for pending work. Wider gaps separate
phases; concrete microsteps have individual blocks. ACC publishes the measured
`workflow.segments` with completed/total actions and fraction
from that session's `tmp/workflow/<session-id>.md` in
`~/.local/state/codex-agent-tracker/model_status_bar.json`. Skipped actions are
excluded; legacy phase-only plans count each enabled phase once. Missing plans
show no progress fill. The background follows the row's width, preserves its
focus/click behavior, and updates in place while the menu is open. At confirmed
completion, it disappears in the same refresh that applies the green glow.
The status badge shows `[1/4] 2:05`: completed/total actionable steps in the
current phase, in brackets immediately before the timer, without a phase name.
Legacy plans without microsteps use completed/enabled phases. Missing or invalid
counts omit the brackets, and confirmed completion removes the count. The live
clock and count update together while the menu remains open.
Focusing a completed task removes its unread glow without bringing the old
progress fill back; a new running turn can show progress again.

Switching workspaces, moving a terminal, opening or closing one, and changing
its ACC-managed title updates the bar immediately. Detached tmux
sessions remain in ACC but are not listed in the menu until their terminal
window is opened; active detached agents still contribute to the remaining-time
maximum.
The extension is opt-in and unrelated to the rate-limit indicator or the CLI
status line:

Left-click an individual agent segment, including its runtime or remaining-time
label, to jump to that agent's terminal. Hover highlights only that instance's
portion of the bar. Provider icons, workspace labels and gaps stay neutral.
The highlight stays steady as the pointer moves within an instance and status
data refreshes. Task progress, timers and menu details continue updating in
place; provider summary chips refresh after the pointer leaves the instance.
Right-click anywhere on the panel indicator, or press **Alt+§**
(`Alt+Above_Tab`), to open the detailed agent status menu. While this extension is enabled, it temporarily takes that
accelerator from GNOME's window-group switcher and restores the existing
shortcut list when disabled or uninstalled. Every displayed live agent with a
window identity is clickable, including running and already-opened agents;
selecting one focuses its matching terminal and
moves the pointer to that window through ACC's shared selector,
`acc-window-select --note-id NOTE_ID`. Segments without a linked note identity
remain inactive.
Green highlighting marks finished-but-unopened work.
Super+§ still cycles only through unopened completions.
Configured GNOME Shell shortcuts, including workspace switching, continue
to work while the menu is open. Its rows and panel layout remain a stable
snapshot so background status updates cannot move or replace a row mid-click.
Existing run clocks, task progress, completion glows and the panel remaining-time
label update in place. Changes to menu membership and ordering appear
immediately after dismissal.
Selecting a row closes the popup before launching the selector, releasing its
keyboard grab before the terminal receives focus.

```bash
bash install.sh --select model_status_bar     # install just this component
bash install.sh --uninstall model_status_bar  # remove it again
```

The Codex mark copied by that install is
`acc-model-status@local/icons/acc-provider-codex-symbolic.svg` (panel name
`acc-provider-codex-symbolic`). It is a 16×16 symbolic ChatGPT blossom: six
curved segments in six-fold symmetry around an open center, with a single
recolorable `#2e3436` fill.

It has no poller of its own: it renders
`~/.local/state/codex-agent-tracker/model_status_bar.json`, which the Agent
Command Center monitor publishes (honoring `CODEX_AGENT_TRACKER_STATE_DIR`).
It joins those agent rows to GNOME's live window titles and actual workspace
metadata; it only launches the shared selector when an agent row is clicked,
and never polls tmux. Segments are green while
agents run, yellow when an agent needs input, gray when idle, and dimmed when
the payload stops being refreshed.
In the Agent Command Center menu, a Codex row queued for auto-continue shows
ACC's negative reset countdown (such as `-1:22`) in yellow-green. The menu
updates the clock locally from `queued_reset_at`, including for terminals with
no Phrase Automation note. The same wait shows `-HH:MM` above that instance's
taskbar bar. `RATE` is only for an instance still waiting to start. A finished
plan, a completion timestamp, or a visible completion footer is marked `DONE`
instead, including while the account remains rate-limited.

An ACC conversation currently marked `INTERRUPTED` uses red (`#ef4444`) for
its taskbar progress track, fill and menu badge, including focused windows and
windows without a workflow plan. Goal and retry metadata cannot recolor this
state. ACC owns interruption detection: quota warnings and unsent text leave it
interrupted until a newer task-start hook or resumed assistant output proves
recovery. The installed model status extension reads this shared status locally.

An actively pursued `/goal` colors a live agent's elapsed timer purple
(`#a855f7`) in the ACC menu. ACC publishes the goal flag with the row;
ordinary runs keep their yellow timer, and queued reset clocks keep their
yellow-green countdown.

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
