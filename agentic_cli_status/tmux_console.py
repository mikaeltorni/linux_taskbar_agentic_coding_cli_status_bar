#!/usr/bin/env python3
"""Background tmux debug console for the agentic CLI status poller.

Maintains one detached tmux session (default ``agentic-cli-status``) with one
window per monitored CLI running that CLI's interactive TUI. The console
serves two purposes:

1. **Debugging** — the user can ``tmux attach -t agentic-cli-status`` at any
   time and see exactly what each CLI reports.
2. **Refresh + fallback** — when a CLI's headless data is stale (``auto``
   mode) or headless sources are disabled (``tmux`` mode), the poller sends
   that CLI's rate-limit slash command (``/status`` for Codex, ``/usage`` for
   Claude Code and Grok) into its window and captures the rendered pane text.

Only slash commands are ever typed into the panes — never a prompt — so the
console cannot consume plan tokens or start model turns.
"""

from __future__ import annotations

import shutil
import subprocess
import time
from typing import Any

from .acs_logging import LOGGER
from .config import SLASH_COMMANDS

#: Command each window runs. The TUIs are launched with their default
#: interactive mode; they idle between slash commands.
CLI_COMMANDS = {
    "codex": "codex",
    "claude_code": "claude",
    "grok": "grok",
}

#: Seconds to wait after sending a slash command before capturing the pane so
#: the TUI has time to render the usage panel.
CAPTURE_DELAY_SECONDS = 4.0


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
        binary = CLI_COMMANDS[cli_id].split()[0]
        if cli_id in existing:
            continue
        if shutil.which(binary) is None:
            LOGGER.info("cli %s not installed; skipping console window", cli_id)
            continue
        if not have_session:
            result = _tmux(
                ["new-session", "-d", "-s", session, "-n", cli_id, CLI_COMMANDS[cli_id]]
            )
            have_session = created_any = result.returncode == 0
            if not have_session:
                LOGGER.error(
                    "failed to create tmux session %s: %s", session, result.stderr.strip()
                )
                return False
        else:
            result = _tmux(
                ["new-window", "-d", "-t", session, "-n", cli_id, CLI_COMMANDS[cli_id]]
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
        fails. Only the documented slash command plus Enter is ever sent.
    """
    target = f"{session}:{cli_id}"
    command = SLASH_COMMANDS[cli_id]
    # Clear any half-typed input, then type the slash command. "Escape" backs
    # out of transient TUI popups without side effects in all three CLIs.
    for keys in (["Escape"], ["C-u"], [command], ["Enter"]):
        result = _tmux(["send-keys", "-t", target, *keys])
        if result.returncode != 0:
            LOGGER.warning(
                "send-keys to %s failed: %s", target, result.stderr.strip()
            )
            return None
        if keys == [command]:
            sleeper(0.3)
    sleeper(CAPTURE_DELAY_SECONDS)
    captured = _tmux(["capture-pane", "-p", "-t", target])
    if captured.returncode != 0:
        LOGGER.warning("capture-pane %s failed: %s", target, captured.stderr.strip())
        return None
    # Close the usage popup so the pane returns to its idle state.
    _tmux(["send-keys", "-t", target, "Escape"])
    return captured.stdout
