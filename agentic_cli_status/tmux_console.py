#!/usr/bin/env python3
"""Background tmux debug console for the agentic CLI status poller.

Maintains one detached tmux session (default ``agentic-cli-status``) with one
window per monitored CLI running that CLI's interactive TUI. The console
serves two purposes:

1. **Debugging** — the user can ``tmux attach -t agentic-cli-status`` at any
   time and see exactly what each CLI reports.
2. **Refresh + fallback** — when a CLI's headless data is stale (``auto``
   mode) or headless sources are disabled (``tmux`` mode), the poller sends
   that CLI's ``/usage`` rate-limit command into its window and captures the
   rendered pane text.

Only slash commands are ever typed into the panes — never a prompt — so the
console cannot consume plan tokens or start model turns.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from .acs_logging import LOGGER
from .config import SLASH_COMMANDS

#: Binary each window runs. The TUIs are launched with their default
#: interactive mode; they idle between slash commands.
CLI_COMMANDS = {
    "codex": "codex",
    "claude_code": "claude",
    "grok": "grok",
    "opencode_go": "opencode",
}


def tmux_console_cli_ids(cli_ids: list[str]) -> list[str]:
    """Return the subset of ``cli_ids`` that own a tmux debug-console window.

    Headless-only monitors (e.g. OpenRouter) are intentionally absent from
    :data:`CLI_COMMANDS` and must never be passed to :func:`ensure_console`.
    """
    return [cli_id for cli_id in cli_ids if cli_id in CLI_COMMANDS]

#: Environment exported into every console window (via ``tmux -e``) so the
#: TUIs stay silent monitoring surfaces instead of announcing fake agent
#: sessions at login: ATTS_SUPPRESS_ANNOUNCEMENTS mutes the TTS notification
#: hooks (Claude Code / Grok / Cline), and CODEX_AGENT_SKIP_TRACKER makes the
#: codex launcher run plain codex without ACC registration or its spoken
#: launch announcement.
WINDOW_ENV = (
    "ATTS_SUPPRESS_ANNOUNCEMENTS=1",
    "CODEX_AGENT_SKIP_TRACKER=1",
)

#: ``tmux new-session``/``new-window`` argument pairs for :data:`WINDOW_ENV`.
_WINDOW_ENV_ARGS = [arg for var in WINDOW_ENV for arg in ("-e", var)]

#: Install locations to try when the binary is not on PATH. The poller runs
#: as a systemd user service whose PATH lacks per-app bin dirs (notably
#: ~/.grok/bin), and a tmux window whose command cannot be resolved dies
#: instantly — so windows are always created with an absolute path.
_BINARY_FALLBACK_DIRS = (".local/bin", ".grok/bin", ".codex/bin", ".opencode/bin")


def resolve_cli_binary(cli_id: str) -> str | None:
    """Return the absolute path of a CLI's binary, or None when missing.

    Args:
        cli_id: One of the :data:`CLI_COMMANDS` keys.
    """
    binary = CLI_COMMANDS[cli_id]
    found = shutil.which(binary)
    if found:
        return found
    for rel in _BINARY_FALLBACK_DIRS:
        candidate = Path.home() / rel / binary
        if candidate.is_file():
            return str(candidate)
    return None

#: Seconds to wait after sending a slash command before capturing the pane so
#: the TUI has time to render the usage panel.
CAPTURE_DELAY_SECONDS = 4.0

#: A rendered Codex rate-limit line ("5h limit: … 99% left", "Weekly limit:
#: … 7% left"). Its presence means the async /status panel has finished
#: rendering, so the retry loop can stop re-requesting it.
_CODEX_RATE_LIMIT_LINE = re.compile(r"\blimit\b[^\n]*\d+\s*%", re.I)

#: How many times to (re)request Codex /status while its async refresh settles.
#: The first call on a cold process only answers "refresh requested; run
#: /status again shortly", so a warm render can need a couple of retries.
_CODEX_STATUS_ATTEMPTS = 3


def _tmux(args: list[str]) -> subprocess.CompletedProcess[str]:
    """Run a tmux subcommand, capturing output, never raising on failure."""
    return subprocess.run(
        ["tmux", *args], capture_output=True, text=True, timeout=30, check=False
    )


def tmux_available() -> bool:
    """Return whether the tmux binary is installed."""
    return shutil.which("tmux") is not None


def ensure_console(session: str, cli_ids: list[str]) -> bool:
    """Ensure the detached console session exists with one window per CLI.

    Windows are created only for CLIs whose binary is installed. Existing
    windows are left untouched so in-flight TUI state (e.g. an answered trust
    prompt) survives poller restarts.

    Args:
        session: The tmux session name.
        cli_ids: CLI ids that should have console windows.

    Returns:
        True when the session exists (or was created) with at least one
        window, False when tmux is unavailable or creation failed.
    """
    if not tmux_available():
        LOGGER.warning("tmux not installed; console disabled")
        return False
    have_session = _tmux(["has-session", "-t", session]).returncode == 0
    existing: set[str] = set()
    if have_session:
        listed = _tmux(["list-windows", "-t", session, "-F", "#{window_name}"])
        existing = set(listed.stdout.split())
    created_any = have_session
    for cli_id in cli_ids:
        if cli_id not in CLI_COMMANDS:
            LOGGER.debug("cli %s has no tmux console; skipping window", cli_id)
            continue
        if cli_id in existing:
            continue
        binary = resolve_cli_binary(cli_id)
        if binary is None:
            LOGGER.info("cli %s not installed; skipping console window", cli_id)
            continue
        if not have_session:
            result = _tmux(
                ["new-session", "-d", *_WINDOW_ENV_ARGS, "-s", session, "-n", cli_id, binary]
            )
            have_session = created_any = result.returncode == 0
            if not have_session:
                LOGGER.error(
                    "failed to create tmux session %s: %s", session, result.stderr.strip()
                )
                return False
        else:
            result = _tmux(
                ["new-window", "-d", *_WINDOW_ENV_ARGS, "-t", session, "-n", cli_id, binary]
            )
            if result.returncode != 0:
                LOGGER.error(
                    "failed to create console window %s: %s",
                    cli_id,
                    result.stderr.strip(),
                )
            created_any = created_any or result.returncode == 0
        LOGGER.info("created console window %s in session %s", cli_id, session)
    return created_any


def _send_slash(target: str, command: str, sleeper: Any) -> bool:
    """Type one slash command into a pane, clearing any half-typed input first.

    "Escape" backs out of transient TUI popups without side effects in all
    three CLIs; "C-u" clears the input line so a stray keystroke cannot
    concatenate with the command (e.g. "/status" + leftover -> "/statusfoo").

    Returns:
        True when every keystroke was delivered, False on the first failure.
    """
    for keys in (["Escape"], ["C-u"], [command], ["Enter"]):
        result = _tmux(["send-keys", "-t", target, *keys])
        if result.returncode != 0:
            LOGGER.warning(
                "send-keys to %s failed: %s", target, result.stderr.strip()
            )
            return False
        if keys == [command]:
            sleeper(0.3)
    return True


def _capture_pane(target: str) -> str | None:
    """Capture a pane's rendered text, or None when tmux fails."""
    captured = _tmux(["capture-pane", "-p", "-t", target])
    if captured.returncode != 0:
        LOGGER.warning("capture-pane %s failed: %s", target, captured.stderr.strip())
        return None
    return captured.stdout


def _capture_codex_status(target: str, sleeper: Any) -> str | None:
    """Request Codex /status until its rate-limit panel renders, and capture it.

    /status is authoritative for the 5h and weekly windows but is async on a
    cold process: the first call only answers "refresh requested; run /status
    again shortly". Resend up to :data:`_CODEX_STATUS_ATTEMPTS` times, stopping
    as soon as a rate-limit line appears, and return the last capture either
    way so a slow render still contributes whatever it has.
    """
    captured: str | None = None
    for _ in range(_CODEX_STATUS_ATTEMPTS):
        if not _send_slash(target, "/status", sleeper):
            return captured
        sleeper(CAPTURE_DELAY_SECONDS)
        captured = _capture_pane(target)
        if captured is None:
            return None
        if _CODEX_RATE_LIMIT_LINE.search(captured):
            break
    return captured


def refresh_and_capture(
    session: str, cli_id: str, sleeper: Any = time.sleep
) -> str | None:
    """Send the CLI's rate-limit slash command and capture its pane text.

    Args:
        session: The tmux session name.
        cli_id: The CLI window to refresh.
        sleeper: ``time.sleep``-compatible callable (tests).

    Returns:
        The captured pane text, or None when the window is missing or tmux
        fails. Only the documented rate-limit slash commands are ever sent.
    """
    target = f"{session}:{cli_id}"
    if not _send_slash(target, SLASH_COMMANDS[cli_id], sleeper):
        return None
    sleeper(CAPTURE_DELAY_SECONDS)
    text = _capture_pane(target)
    if text is None:
        return None
    if cli_id == "codex":
        # Across Codex versions /usage renders either a reset-redemption menu
        # or a daily-activity heatmap — it carries the earned-reset allowance
        # but not reliably the rate-limit windows. /status is the authoritative
        # 5h + weekly breakdown, so always capture it too and parse the union;
        # never navigate into the reset-redemption action.
        status_text = _capture_codex_status(target, sleeper)
        if status_text:
            text = f"{text.rstrip()}\n{status_text}"
    # Close the usage popup so the pane returns to its idle state.
    _tmux(["send-keys", "-t", target, "Escape"])
    return text
