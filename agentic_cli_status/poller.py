#!/usr/bin/env python3
"""Polling daemon that aggregates agentic CLI rate limits into a status file.

On every tick the poller re-reads the config (so installer reconfiguration
applies live), collects each enabled CLI's rate limits headlessly, optionally
refreshes stale data through the tmux debug console, and atomically writes the
merged snapshot to ``$XDG_STATE_HOME/agentic-cli-status/status.json``. The
GNOME Shell extension watches that file and renders the taskbar indicator.

Usage:
    python3 -m agentic_cli_status.poller --once     # single tick, then exit
    python3 -m agentic_cli_status.poller --daemon   # loop at the configured rate
    python3 -m agentic_cli_status.poller --show     # pretty-print the status file
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import time
from pathlib import Path
from typing import Any

from .acs_logging import LOGGER
from .collectors import collect_claude, collect_codex, collect_grok, parse_pane
from .config import CLI_IDS, load_config, state_dir
from .tmux_console import ensure_console, refresh_and_capture

STATUS_FILE_NAME = "status.json"
PANE_DIR_NAME = "panes"

#: Minimum seconds between tmux slash-command refreshes per CLI, so
#: second-scale poll ticks cannot spam the console windows.
TMUX_REFRESH_MIN_INTERVAL_SECONDS = 60

# Headless collector dispatch. Each entry receives the CLI's config dict so
# per-CLI knobs (like Claude's endpoint interval) reach the collector.
_HEADLESS_COLLECTORS = {
    "codex": lambda home, last, cli_config: collect_codex(home),
    "claude_code": lambda home, last, cli_config: collect_claude(
        home, last, min_interval=cli_config.get("endpoint_min_interval_seconds")
    ),
    "grok": lambda home, last, cli_config: collect_grok(home),
}


def status_path(home: Path | None = None) -> Path:
    """Return the aggregated status file path."""
    return state_dir(home) / STATUS_FILE_NAME


def _is_stale(snapshot: dict[str, Any], stale_after: int, now: float) -> bool:
    """Return whether a snapshot is missing, failed, or older than the limit."""
    observed = snapshot.get("observed_at")
    if not snapshot.get("windows") or observed is None:
        return True
    return (now - observed) > stale_after


def _carry_fresher_reading(
    snapshot: dict[str, Any], last: dict[str, Any] | None
) -> dict[str, Any]:
    """Keep the previous tick's reading when it is newer than this one.

    Codex only appends ``rate_limits`` events when a prompt actually runs, so
    after a tmux ``/usage`` refresh the captured pane reading stays newer
    than anything the headless collector can produce until real usage
    resumes. Without this carry the pane reading would be shown for a single
    tick and then replaced by the older headless data again, flapping the
    indicator once per refresh. It also keeps the last pane reading visible
    between throttled refreshes in ``tmux`` source mode.
    """
    if not last or not last.get("windows"):
        return snapshot
    if (last.get("observed_at") or 0) <= (snapshot.get("observed_at") or 0):
        return snapshot
    carried = dict(snapshot)
    for key in ("source", "observed_at", "windows"):
        carried[key] = last.get(key)
    if "resets_available" in last:
        carried["resets_available"] = last["resets_available"]
    if not carried.get("plan"):
        carried["plan"] = last.get("plan")
    return carried


def _collect_cli(
    cli_id: str,
    cli_config: dict[str, Any],
    tmux_config: dict[str, Any],
    home: Path,
    last: dict[str, Any] | None,
    now: float,
) -> dict[str, Any]:
    """Collect one CLI's snapshot honoring its configured source mode.

    ``headless``/``auto`` try the local-data collector first; ``auto`` and
    ``tmux`` fall back to (or rely on) a slash-command refresh in the tmux
    console window followed by a pane capture. Whichever reading is newest —
    this tick's or the previous one's — wins (see
    :func:`_carry_fresher_reading`).
    """
    source_mode = cli_config["source"]
    stale_after = cli_config["stale_after_seconds"]
    snapshot: dict[str, Any] = {"windows": []}
    if source_mode in ("auto", "headless"):
        snapshot = _HEADLESS_COLLECTORS[cli_id](home, last, cli_config)
    snapshot = _carry_fresher_reading(snapshot, last)
    last_tmux_attempt = (last or {}).get("tmux_attempt_at")
    tmux_wanted = (
        tmux_config.get("enabled", True)
        and source_mode in ("auto", "tmux")
        and (source_mode == "tmux" or _is_stale(snapshot, stale_after, now))
        and (
            last_tmux_attempt is None
            or now - last_tmux_attempt >= TMUX_REFRESH_MIN_INTERVAL_SECONDS
        )
    )
    # Endpoint throttle bookkeeping (Claude's checked_at / retry_after_until)
    # must survive a pane replacement below, or the next tick would attempt
    # the endpoint again on every poll tick and 429-lock the account.
    endpoint_marks = {
        key: snapshot[key]
        for key in ("checked_at", "retry_after_until")
        if key in snapshot
    }
    if tmux_wanted:
        last_tmux_attempt = now
        pane_text = refresh_and_capture(tmux_config["session"], cli_id)
        if pane_text is not None:
            _save_pane_capture(home, cli_id, pane_text)
            pane_snapshot = parse_pane(cli_id, pane_text, now=now)
            if pane_snapshot.get("windows"):
                # In auto mode the pane refresh also nudges the CLI to update
                # its local data, so the next tick's headless read is fresh.
                if _is_stale(snapshot, stale_after, now):
                    # The pane rendering carries no plan name; keep the one
                    # the headless source reported.
                    pane_snapshot["plan"] = snapshot.get("plan")
                    snapshot = pane_snapshot
            elif not snapshot.get("windows"):
                snapshot = pane_snapshot
    snapshot.update(endpoint_marks)
    if last_tmux_attempt is not None:
        # Carry the throttle timestamp across ticks (and pane replacements).
        snapshot["tmux_attempt_at"] = last_tmux_attempt
    snapshot["stale"] = _is_stale(snapshot, stale_after, now)
    return snapshot


def _save_pane_capture(home: Path, cli_id: str, text: str) -> None:
    """Persist the last captured pane text for debugging (best-effort)."""
    try:
        pane_dir = state_dir(home) / PANE_DIR_NAME
        pane_dir.mkdir(parents=True, exist_ok=True)
        (pane_dir / f"{cli_id}.txt").write_text(text, encoding="utf-8")
    except OSError as exc:
        LOGGER.warning("cannot save pane capture for %s: %s", cli_id, exc)


def poll_once(
    home: Path | None = None,
    previous: dict[str, Any] | None = None,
    now: float | None = None,
) -> dict[str, Any]:
    """Run one poll tick and write the aggregated status file.

    Args:
        home: Home directory override (tests).
        previous: The previous aggregated status (for endpoint throttling).
        now: Current epoch override (tests).

    Returns:
        The aggregated status dictionary that was written.
    """
    home = home or Path.home()
    now = now if now is not None else time.time()
    config = load_config()
    enabled = [cli for cli in CLI_IDS if config["clis"][cli]["enabled"]]
    if config["tmux"].get("enabled", True) and enabled:
        ensure_console(config["tmux"]["session"], enabled)
    previous_clis = (previous or {}).get("clis", {})
    status: dict[str, Any] = {
        "generated_at": now,
        "poll_interval_seconds": config["poll_interval_seconds"],
        "clis": {},
    }
    for cli_id in CLI_IDS:
        cli_config = config["clis"][cli_id]
        if not cli_config["enabled"]:
            status["clis"][cli_id] = {"enabled": False}
            continue
        snapshot = _collect_cli(
            cli_id, cli_config, config["tmux"], home, previous_clis.get(cli_id), now
        )
        snapshot["enabled"] = True
        status["clis"][cli_id] = snapshot
        # Warn only on error transitions: a throttled snapshot carries its
        # error across many second-scale ticks and must not spam the journal.
        if snapshot.get("error") and snapshot["error"] != (
            previous_clis.get(cli_id) or {}
        ).get("error"):
            LOGGER.warning("cli %s: %s", cli_id, snapshot["error"])
    _write_status(status, home)
    return status


def _write_status(status: dict[str, Any], home: Path) -> None:
    """Atomically write the aggregated status file."""
    target = status_path(home)
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_suffix(".tmp")
    tmp.write_text(json.dumps(status, indent=2) + "\n", encoding="utf-8")
    os.replace(tmp, target)
    LOGGER.debug("status written to %s", target)


def run_daemon(home: Path | None = None) -> None:
    """Poll forever at the configured rate until SIGTERM/SIGINT.

    The config is re-read every tick, so polling-rate and per-CLI option
    changes made by the installer apply without a service restart.
    """
    LOGGER.info("agentic-cli-status poller starting (pid %d)", os.getpid())
    stop = {"requested": False}

    def _handle_stop(signum: int, _frame: Any) -> None:
        LOGGER.info("received signal %d; stopping", signum)
        stop["requested"] = True

    signal.signal(signal.SIGTERM, _handle_stop)
    signal.signal(signal.SIGINT, _handle_stop)
    previous: dict[str, Any] | None = None
    while not stop["requested"]:
        try:
            previous = poll_once(home=home, previous=previous)
        except Exception as exc:  # noqa: BLE001 - daemon must survive any tick
            LOGGER.exception("poll tick failed: %s", exc)
        interval = (previous or {}).get("poll_interval_seconds") or load_config()[
            "poll_interval_seconds"
        ]
        deadline = time.time() + interval
        while not stop["requested"] and time.time() < deadline:
            time.sleep(1)
    LOGGER.info("agentic-cli-status poller stopped")


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for the poller.

    Args:
        argv: Argument list override (tests).
    """
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--once", action="store_true", help="run one tick and exit")
    mode.add_argument("--daemon", action="store_true", help="poll at the configured rate")
    mode.add_argument("--show", action="store_true", help="print the current status file")
    args = parser.parse_args(argv)
    if args.show:
        target = status_path()
        try:
            print(target.read_text(encoding="utf-8"), end="")
        except OSError as exc:
            print(f"no status available at {target}: {exc}", file=sys.stderr)
            return 1
        return 0
    if args.once:
        poll_once()
        return 0
    run_daemon()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
