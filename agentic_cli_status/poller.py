#!/usr/bin/env python3
"""Polling daemon that aggregates agentic CLI rate limits into a status file.

On every tick the poller re-reads the config (so installer reconfiguration
applies live), collects each enabled CLI's rate limits, and atomically writes
the merged snapshot to ``$XDG_STATE_HOME/agentic-cli-status/status.json``.
Local Codex/Grok files are scanned directly; remote metadata calls and tmux
slash-command refreshes run on background workers so they cannot delay the
shared status cadence. The GNOME Shell extension watches the status file and
renders every taskbar segment from that one snapshot.

Usage:
    python3 -m agentic_cli_status.poller --once     # single tick, then exit
    python3 -m agentic_cli_status.poller --daemon   # loop at the configured rate
    python3 -m agentic_cli_status.poller --show     # pretty-print the status file
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import signal
import sys
import threading
import time
from pathlib import Path
from typing import Any, Callable

from .acs_logging import LOGGER
from .collectors import (
    _carry_last_reading,
    collect_claude,
    collect_codex,
    collect_grok,
    collect_openrouter,
    collect_opencode_go,
    parse_pane,
)
from .config import CLI_IDS, SLASH_COMMANDS, load_config, state_dir
from .tmux_console import ensure_console, refresh_and_capture, tmux_console_cli_ids

STATUS_FILE_NAME = "status.json"
PANE_DIR_NAME = "panes"

#: Minimum seconds between tmux slash-command refreshes per CLI, so
#: second-scale poll ticks cannot spam the console windows.
TMUX_REFRESH_MIN_INTERVAL_SECONDS = 60

TmuxRefreshRequest = Callable[[str, str], bool]


class TmuxRefreshWorker:
    """Run slow tmux slash-command captures outside the five-second poll tick.

    Requests are deduplicated per ``(session, cli_id)`` while queued or
    running. The single daemon thread intentionally serializes interaction
    with the shared tmux session, then atomically stores each capture for the
    next fast poll tick to consume.

    Args:
        home: Home directory whose state tree receives pane captures.
        refresh_fn: Capture callable override used by tests.
    """

    def __init__(
        self,
        home: Path,
        *,
        refresh_fn: Callable[[str, str], str | None] = refresh_and_capture,
    ) -> None:
        self._home = home
        self._refresh_fn = refresh_fn
        self._queue: queue.Queue[tuple[str, str] | None] = queue.Queue()
        self._pending: set[tuple[str, str]] = set()
        self._lock = threading.Lock()
        self._thread = threading.Thread(
            target=self._run,
            name="agentic-cli-status-tmux-refresh",
            daemon=True,
        )
        self._started = False

    def start(self) -> None:
        """Start the background capture thread once."""
        if self._started:
            return
        self._started = True
        self._thread.start()
        LOGGER.info("background tmux refresh worker started")

    def request(self, session: str, cli_id: str) -> bool:
        """Queue one capture without waiting.

        Args:
            session: tmux session containing the CLI window.
            cli_id: CLI window identifier.

        Returns:
            ``True`` when queued, or ``False`` when the same capture is
            already queued/running or the worker is not active.
        """
        key = (session, cli_id)
        with self._lock:
            if not self._started or key in self._pending:
                return False
            self._pending.add(key)
        self._queue.put(key)
        LOGGER.debug("queued background tmux refresh for %s", cli_id)
        return True

    def close(self) -> None:
        """Request worker shutdown without delaying service termination."""
        if not self._started:
            return
        with self._lock:
            self._started = False
        self._queue.put(None)
        self._thread.join(timeout=1.0)
        if self._thread.is_alive():
            LOGGER.info("background tmux refresh still exiting asynchronously")
        else:
            LOGGER.info("background tmux refresh worker stopped")

    def _run(self) -> None:
        """Process queued captures serially until the shutdown sentinel."""
        while True:
            request = self._queue.get()
            if request is None:
                self._queue.task_done()
                return
            session, cli_id = request
            try:
                pane_text = self._refresh_fn(session, cli_id)
                if pane_text is not None:
                    _save_pane_capture(self._home, cli_id, pane_text)
            except Exception as exc:  # noqa: BLE001 - worker must survive a CLI
                LOGGER.exception(
                    "background tmux refresh failed for %s: %s", cli_id, exc
                )
            finally:
                with self._lock:
                    self._pending.discard(request)
                self._queue.task_done()

# Headless collector dispatch. Each entry receives the CLI's config dict so
# per-CLI knobs (like Claude's endpoint interval) reach the collector.
_HEADLESS_COLLECTORS = {
    "codex": lambda home, last, cli_config, now: collect_codex(home, now=now),
    "claude_code": lambda home, last, cli_config, now: collect_claude(
        home,
        last,
        now=now,
        min_interval=cli_config.get("endpoint_min_interval_seconds"),
    ),
    "grok": lambda home, last, cli_config, now: collect_grok(home),
    "opencode_go": lambda home, last, cli_config, now: collect_opencode_go(
        home,
        last,
        now=now,
        min_interval=cli_config.get("fetch_min_interval_seconds"),
    ),
    "openrouter": lambda home, last, cli_config, now: collect_openrouter(
        home,
        last,
        now=now,
        min_interval=cli_config.get("fetch_min_interval_seconds"),
        model_slug=cli_config.get("model_slug"),
        daily_limit=cli_config.get("daily_limit"),
    ),
}

# Local Codex/Grok file reads stay in the fast tick. These HTTP-backed
# collectors run independently so a network timeout cannot freeze every bar.
REMOTE_HEADLESS_CLI_IDS = ("claude_code", "opencode_go", "openrouter")


class HeadlessRefreshWorker:
    """Run remote headless collectors without blocking the shared poll tick.

    One daemon thread per remote CLI lets Claude, OpenCode Go, and OpenRouter
    progress independently. Requests are deduplicated while running, and the
    main poll loop consumes completed snapshots on its next five-second tick.

    Args:
        collect_fn: Optional collector dispatcher override used by tests.
    """

    def __init__(
        self,
        *,
        collect_fn: Callable[
            [str, Path, dict[str, Any] | None, dict[str, Any], float],
            dict[str, Any],
        ]
        | None = None,
    ) -> None:
        self._collect_fn = collect_fn or self._collect
        self._queue: queue.Queue[
            tuple[str, Path, dict[str, Any] | None, dict[str, Any], float] | None
        ] = queue.Queue()
        self._pending: set[str] = set()
        self._results: dict[str, dict[str, Any]] = {}
        self._lock = threading.Lock()
        self._threads = [
            threading.Thread(
                target=self._run,
                name=f"agentic-cli-status-remote-{index}",
                daemon=True,
            )
            for index in range(len(REMOTE_HEADLESS_CLI_IDS))
        ]
        self._started = False

    @staticmethod
    def _collect(
        cli_id: str,
        home: Path,
        last: dict[str, Any] | None,
        cli_config: dict[str, Any],
        now: float,
    ) -> dict[str, Any]:
        """Dispatch one configured remote collector."""
        return _HEADLESS_COLLECTORS[cli_id](home, last, cli_config, now)

    def start(self) -> None:
        """Start all remote collector threads once."""
        if self._started:
            return
        self._started = True
        for thread in self._threads:
            thread.start()
        LOGGER.info("background remote collector workers started")

    def request(
        self,
        cli_id: str,
        home: Path,
        last: dict[str, Any] | None,
        cli_config: dict[str, Any],
        now: float,
    ) -> bool:
        """Queue one remote collection without waiting.

        Returns ``False`` when that CLI already has a request in flight or the
        worker is shutting down.
        """
        with self._lock:
            if not self._started or cli_id in self._pending:
                return False
            self._pending.add(cli_id)
        self._queue.put((cli_id, home, last, dict(cli_config), now))
        return True

    def take_result(self, cli_id: str) -> dict[str, Any] | None:
        """Return and remove the latest completed snapshot for ``cli_id``."""
        with self._lock:
            return self._results.pop(cli_id, None)

    def has_result(self, cli_id: str) -> bool:
        """Return whether ``cli_id`` has a completed snapshot ready."""
        with self._lock:
            return cli_id in self._results

    def close(self) -> None:
        """Request shutdown without waiting on long network timeouts."""
        if not self._started:
            return
        with self._lock:
            self._started = False
        for _thread in self._threads:
            self._queue.put(None)
        for thread in self._threads:
            thread.join(timeout=1.0)
        if any(thread.is_alive() for thread in self._threads):
            LOGGER.info("background remote collectors still exiting asynchronously")
        else:
            LOGGER.info("background remote collector workers stopped")

    def _run(self) -> None:
        """Collect queued remote snapshots until a shutdown sentinel."""
        while True:
            request = self._queue.get()
            if request is None:
                self._queue.task_done()
                return
            cli_id, home, last, cli_config, now = request
            try:
                result = self._collect_fn(cli_id, home, last, cli_config, now)
                with self._lock:
                    self._results[cli_id] = result
            except Exception as exc:  # noqa: BLE001 - worker must survive a CLI
                LOGGER.exception(
                    "background headless collection failed for %s: %s", cli_id, exc
                )
            finally:
                with self._lock:
                    self._pending.discard(cli_id)
                self._queue.task_done()


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
    for key in ("resets_available", "resets_checked_at"):
        if key in last:
            carried[key] = last[key]
    if not carried.get("plan"):
        carried["plan"] = last.get("plan")
    return carried


def _carry_codex_reset_metadata(
    snapshot: dict[str, Any], last: dict[str, Any] | None
) -> dict[str, Any]:
    """Retain the last authoritative Codex /usage reset allowance.

    Codex rollout events contain rate-limit windows but not the separately
    earned manual-reset allowance. Preserve the latest tmux /usage result
    across ordinary headless ticks until the next periodic /usage refresh
    explicitly updates it.

    Args:
        snapshot: Current headless or pane snapshot.
        last: Previous poll snapshot, if any.
    """
    if not last or "resets_available" in snapshot:
        return snapshot
    carried = dict(snapshot)
    for key in ("resets_available", "resets_checked_at"):
        if key in last:
            carried[key] = last[key]
    return carried


def _carry_window_resets(
    snapshot: dict[str, Any], *fallbacks: dict[str, Any] | None
) -> dict[str, Any]:
    """Fill each window's missing reset epoch from a fallback reading.

    A window's reset time only changes when it actually rolls over, but not
    every source renders it: the Grok /usage pane shows a bare
    ``Weekly limit left: 0%`` with no reset clock, and some Codex captures drop
    a weekly reset that wrapped onto a continuation line. When the reading
    about to be published names a window but leaves its ``resets_at`` empty,
    inherit the most recent epoch seen for that label — this tick's headless
    read first, then the previous poll — so an exhausted window keeps showing
    its reset date instead of momentarily collapsing to a bare "0". A source
    that reports its own reset always wins, and a carried epoch that has since
    passed replenishes through the extension's normal roll-over handling.

    Args:
        snapshot: The reading whose windows are about to be published.
        fallbacks: Prior readings to source reset epochs from, most trusted
            first.
    """
    windows = snapshot.get("windows")
    if not windows:
        return snapshot
    known: dict[str, Any] = {}
    for source in fallbacks:
        for window in (source or {}).get("windows", []):
            label = window.get("label")
            reset = window.get("resets_at")
            if label is not None and reset is not None and label not in known:
                known[label] = reset
    filled: list[dict[str, Any]] = []
    changed = False
    for window in windows:
        if window.get("resets_at") is None and known.get(window.get("label")) is not None:
            window = {**window, "resets_at": known[window["label"]]}
            changed = True
        filled.append(window)
    if not changed:
        return snapshot
    return {**snapshot, "windows": filled}


def _load_pane_capture(home: Path, cli_id: str) -> tuple[str | None, float | None]:
    """Return saved pane text and its mtime epoch, if any."""
    pane = state_dir(home) / PANE_DIR_NAME / f"{cli_id}.txt"
    try:
        stat = pane.stat()
        return pane.read_text(encoding="utf-8"), stat.st_mtime
    except OSError:
        return None, None


def resolve_grok_snapshot(
    billing: dict[str, Any],
    *,
    live_text: str | None,
    saved_text: str | None,
    saved_at: float | None,
    last: dict[str, Any] | None,
    source_mode: str,
    tmux_enabled: bool,
    now: float,
    stale_after: int,
    tmux_throttled: bool,
) -> dict[str, Any]:
    """Merge Grok billing and console readings with a fixed priority.

    The billing log is Grok's authoritative live source: every billing fetch
    is logged with ``creditUsagePercent`` (and balance-exhaustion 402 errors
    are detected — see :func:`collect_grok`). The tmux ``/usage`` pane is
    only a fallback: its persistent ``Weekly limit left`` border freezes
    between refreshes, and current Grok builds do not submit ``/usage`` via
    ``send-keys`` the way older ones did, so the border can lag the billing
    log for hours and leave the bar stuck on a stale percentage (e.g. 5% left
    while the log already shows the balance exhausted).

    Priority (first match wins):

    1. ``headless`` source, tmux disabled, or billing has windows → billing
       (authoritative; ignores live, saved, and prior console readings).
    2. Live tmux text parses to windows → console (``observed_at`` = ``now``).
    3. Saved pane fresh (mtime within ``stale_after``) → console
       (``observed_at`` = file mtime).
    4. Throttled tick + last ``tmux_pane`` still fresh → last console.
    5. Billing fallback (``resets_at`` may be filled from billing period ends
       after merge when the winning reading omitted it).
    """
    plan = billing.get("plan")

    if source_mode == "headless" or not tmux_enabled or billing.get("windows"):
        return dict(billing)

    def _from_pane(text: str, observed_at: float) -> dict[str, Any] | None:
        pane = parse_pane("grok", text, now=now)
        if not pane.get("windows"):
            return None
        merged = dict(pane)
        merged["plan"] = plan or pane.get("plan")
        merged["observed_at"] = observed_at
        return merged

    if live_text is not None:
        live_snap = _from_pane(live_text, now)
        if live_snap is not None:
            return live_snap

    if (
        saved_text is not None
        and saved_at is not None
        and (now - saved_at) <= stale_after
    ):
        saved_snap = _from_pane(saved_text, saved_at)
        if saved_snap is not None:
            return saved_snap

    if (
        tmux_throttled
        and last
        and last.get("source") == "tmux_pane"
        and last.get("windows")
        and not _is_stale(last, stale_after, now)
    ):
        carried = dict(billing)
        for key in ("source", "observed_at", "windows", "error"):
            if key in last:
                carried[key] = last[key]
        if not carried.get("plan"):
            carried["plan"] = last.get("plan") or plan
        return carried

    return dict(billing)


def _collect_grok_cli(
    cli_config: dict[str, Any],
    tmux_config: dict[str, Any],
    home: Path,
    last: dict[str, Any] | None,
    now: float,
    tmux_refresh_request: TmuxRefreshRequest | None = None,
) -> dict[str, Any]:
    """Collect Grok via billing logs plus optional tmux console resolution.

    Args:
        cli_config: Grok source/staleness configuration.
        tmux_config: Shared tmux enable/session configuration.
        home: User home containing logs and saved pane captures.
        last: Previous Grok snapshot.
        now: Current poll epoch.
        tmux_refresh_request: Optional non-blocking background refresh enqueue
            function. When absent, capture remains synchronous for ``--once``
            compatibility and focused tests.
    """
    source_mode = cli_config["source"]
    stale_after = cli_config["stale_after_seconds"]
    tmux_enabled = tmux_config.get("enabled", True)
    billing = collect_grok(home)
    last_tmux_attempt = (last or {}).get("tmux_attempt_at")
    tmux_wanted = (
        tmux_enabled
        and source_mode in ("auto", "tmux")
        and (
            source_mode == "tmux"
            or _is_stale(billing, stale_after, now)
            or source_mode == "auto"
        )
        and (
            last_tmux_attempt is None
            or now - last_tmux_attempt >= TMUX_REFRESH_MIN_INTERVAL_SECONDS
        )
    )
    live_text: str | None = None
    if tmux_wanted:
        if tmux_refresh_request is None:
            last_tmux_attempt = now
            live_text = refresh_and_capture(tmux_config["session"], "grok")
        elif tmux_refresh_request(tmux_config["session"], "grok"):
            last_tmux_attempt = now
    saved_text, saved_at = (None, None)
    if source_mode in ("auto", "tmux") and tmux_enabled:
        saved_text, saved_at = _load_pane_capture(home, "grok")
    snapshot = resolve_grok_snapshot(
        billing,
        live_text=live_text,
        saved_text=saved_text,
        saved_at=saved_at,
        last=last,
        source_mode=source_mode,
        tmux_enabled=tmux_enabled,
        now=now,
        stale_after=stale_after,
        tmux_throttled=not tmux_wanted,
    )
    snapshot = _carry_window_resets(snapshot, billing)
    # A failed billing read or unparseable tmux capture must not blank GK to
    # "–" when the previous tick had a good reading (see _carry_last_reading).
    snapshot = _carry_last_reading(snapshot, last)
    if (
        live_text is not None
        and parse_pane("grok", live_text, now=now).get("windows")
    ):
        _save_pane_capture(home, "grok", live_text)
    if last_tmux_attempt is not None:
        snapshot["tmux_attempt_at"] = last_tmux_attempt
    snapshot["stale"] = _is_stale(snapshot, stale_after, now)
    return snapshot


def _merge_cli_pane(
    snapshot: dict[str, Any],
    *,
    cli_id: str,
    pane_text: str,
    pane_observed_at: float,
    source_mode: str,
    stale_after: int,
    last: dict[str, Any] | None,
    now: float,
) -> dict[str, Any]:
    """Merge one newly captured generic CLI pane into a headless snapshot.

    Args:
        snapshot: Current headless/carried snapshot.
        cli_id: CLI whose pane was captured.
        pane_text: Captured pane contents.
        pane_observed_at: Capture epoch (live tick or saved-file mtime).
        source_mode: Configured ``auto``/``headless``/``tmux`` mode.
        stale_after: Seconds after which the headless reading is stale.
        last: Previous published snapshot for metadata transition logging.
        now: Current poll epoch used for staleness evaluation.

    Returns:
        Snapshot with eligible pane windows and Codex reset metadata merged.

    A newly captured Codex ``/status`` report is allowed to replace a
    non-stale rollout snapshot when it was observed at least as recently.
    Codex can reset or reconcile account limits without appending a matching
    ``token_count`` event to the rollout selected by the headless scan, so
    treating the pane as a metadata-only source leaves the bar stuck on the
    pre-reset percentages.
    """
    pane_snapshot = parse_pane(cli_id, pane_text, now=pane_observed_at)
    pane_has_reset_metadata = "resets_available" in pane_snapshot
    pane_is_newer = pane_observed_at >= (snapshot.get("observed_at") or 0)
    prior_reset_metadata = {
        key: snapshot[key]
        for key in ("resets_available", "resets_checked_at")
        if key in snapshot
    }
    use_pane = (
        source_mode == "tmux"
        or not snapshot.get("windows")
        or _is_stale(snapshot, stale_after, now)
        or (
            cli_id == "codex"
            and source_mode == "auto"
            and pane_is_newer
        )
    )
    if pane_snapshot.get("windows") and use_pane:
        if (
            cli_id == "codex"
            and source_mode == "auto"
            and snapshot.get("windows")
            and pane_is_newer
        ):
            LOGGER.info(
                "codex pane reading superseded rollout snapshot: %s -> %s",
                snapshot.get("windows"),
                pane_snapshot.get("windows"),
            )
        pane_snapshot["plan"] = snapshot.get("plan") or pane_snapshot.get("plan")
        pane_snapshot["observed_at"] = pane_observed_at
        if cli_id == "codex" and "resets_available" not in pane_snapshot:
            # A heatmap-style /usage capture may omit the allowance even
            # though the previous /usage response established it.
            pane_snapshot.update(prior_reset_metadata)
        snapshot = pane_snapshot
    if cli_id == "codex" and pane_has_reset_metadata:
        previous_count = (last or {}).get("resets_available", 0)
        current_count = pane_snapshot["resets_available"]
        if current_count > 0:
            snapshot["resets_available"] = current_count
        else:
            snapshot.pop("resets_available", None)
        snapshot["resets_checked_at"] = pane_observed_at
        if previous_count != current_count:
            LOGGER.info(
                "codex reset allowance changed from %s to %s",
                previous_count,
                current_count,
            )
    snapshot["pane_checked_at"] = pane_observed_at
    return snapshot


def _collect_cli(
    cli_id: str,
    cli_config: dict[str, Any],
    tmux_config: dict[str, Any],
    home: Path,
    last: dict[str, Any] | None,
    now: float,
    tmux_refresh_request: TmuxRefreshRequest | None = None,
    headless_snapshot: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Collect one CLI's snapshot honoring its configured source mode.

    ``headless``/``auto`` try the local-data collector first; ``auto`` and
    ``tmux`` fall back to (or rely on) a slash-command refresh in the tmux
    console window followed by a pane capture. A newly captured Codex pane
    report supersedes a rollout snapshot observed at the same or an older
    time because it includes post-reset server state. Whichever reading is
    newest — this tick's or the previous one's — wins (see
    :func:`_carry_fresher_reading`).
    """
    if cli_id == "grok":
        return _collect_grok_cli(
            cli_config,
            tmux_config,
            home,
            last,
            now,
            tmux_refresh_request,
        )

    source_mode = cli_config["source"]
    stale_after = cli_config["stale_after_seconds"]
    snapshot: dict[str, Any] = {"windows": []}
    if source_mode in ("auto", "headless"):
        snapshot = (
            headless_snapshot
            if headless_snapshot is not None
            else _HEADLESS_COLLECTORS[cli_id](home, last, cli_config, now)
        )
    snapshot = _carry_fresher_reading(snapshot, last)
    if cli_id == "codex":
        snapshot = _carry_codex_reset_metadata(snapshot, last)
    # Remember this tick's headless reset epochs so they survive a pane
    # replacement below (Codex weekly resets can wrap onto continuation lines).
    headless_snapshot = snapshot
    last_tmux_attempt = (last or {}).get("tmux_attempt_at")
    tmux_wanted = (
        cli_id in SLASH_COMMANDS
        and tmux_config.get("enabled", True)
        and source_mode in ("auto", "tmux")
        and (
            source_mode == "tmux"
            or _is_stale(snapshot, stale_after, now)
            # Reset availability is absent from rollout files, so Codex auto
            # mode must refresh that read-only /usage metadata periodically
            # even while its headless rate-limit windows are fresh.
            or (cli_id == "codex" and source_mode == "auto")
        )
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
    pane_text: str | None = None
    pane_observed_at: float | None = None
    if tmux_wanted:
        if tmux_refresh_request is None:
            last_tmux_attempt = now
            pane_text = refresh_and_capture(tmux_config["session"], cli_id)
            pane_observed_at = now
            if pane_text is not None:
                _save_pane_capture(home, cli_id, pane_text)
        elif tmux_refresh_request(tmux_config["session"], cli_id):
            last_tmux_attempt = now
    if (
        tmux_refresh_request is not None
        and tmux_config.get("enabled", True)
        and source_mode in ("auto", "tmux")
    ):
        saved_text, saved_at = _load_pane_capture(home, cli_id)
        if (
            saved_text is not None
            and saved_at is not None
            and saved_at > (last or {}).get("pane_checked_at", 0)
            and now - saved_at <= stale_after
        ):
            pane_text = saved_text
            pane_observed_at = saved_at
    if pane_text is not None and pane_observed_at is not None:
        snapshot = _merge_cli_pane(
            snapshot,
            cli_id=cli_id,
            pane_text=pane_text,
            pane_observed_at=pane_observed_at,
            source_mode=source_mode,
            stale_after=stale_after,
            last=last,
            now=now,
        )
    snapshot = _carry_window_resets(snapshot, headless_snapshot, last)
    snapshot.update(endpoint_marks)
    if last_tmux_attempt is not None:
        # Carry the throttle timestamp across ticks (and pane replacements).
        snapshot["tmux_attempt_at"] = last_tmux_attempt
    if last and "pane_checked_at" in last and "pane_checked_at" not in snapshot:
        snapshot["pane_checked_at"] = last["pane_checked_at"]
    snapshot["stale"] = _is_stale(snapshot, stale_after, now)
    return snapshot


def _save_pane_capture(home: Path, cli_id: str, text: str) -> None:
    """Atomically persist the last captured pane text (best-effort)."""
    try:
        pane_dir = state_dir(home) / PANE_DIR_NAME
        pane_dir.mkdir(parents=True, exist_ok=True)
        target = pane_dir / f"{cli_id}.txt"
        temporary = pane_dir / f".{cli_id}.txt.tmp"
        temporary.write_text(text, encoding="utf-8")
        os.replace(temporary, target)
    except OSError as exc:
        LOGGER.warning("cannot save pane capture for %s: %s", cli_id, exc)


def poll_once(
    home: Path | None = None,
    previous: dict[str, Any] | None = None,
    now: float | None = None,
    tmux_refresh_request: TmuxRefreshRequest | None = None,
    headless_refresh_worker: HeadlessRefreshWorker | None = None,
) -> dict[str, Any]:
    """Run one poll tick and write the aggregated status file.

    Args:
        home: Home directory override (tests).
        previous: The previous aggregated status (for endpoint throttling).
        now: Current epoch override (tests).
        tmux_refresh_request: Optional non-blocking background tmux enqueue
            function used by daemon mode. ``None`` preserves synchronous
            behavior for the one-shot command and focused callers.
        headless_refresh_worker: Optional daemon-mode worker for HTTP-backed
            collectors. Completed results are published and replacement work
            is queued without delaying this tick.

    Returns:
        The aggregated status dictionary that was written.
    """
    home = home or Path.home()
    now = now if now is not None else time.time()
    config = load_config()
    enabled = [cli for cli in CLI_IDS if config["clis"][cli]["enabled"]]
    console_clis = tmux_console_cli_ids(enabled)
    if config["tmux"].get("enabled", True) and console_clis:
        ensure_console(config["tmux"]["session"], console_clis)
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
        last_cli = previous_clis.get(cli_id)
        headless_snapshot = None
        if (
            headless_refresh_worker is not None
            and cli_id in REMOTE_HEADLESS_CLI_IDS
            and cli_config["source"] in ("auto", "headless")
        ):
            headless_snapshot = (
                headless_refresh_worker.take_result(cli_id)
                or last_cli
                or {"windows": []}
            )
            headless_refresh_worker.request(
                cli_id,
                home,
                headless_snapshot,
                cli_config,
                now,
            )
        snapshot = _collect_cli(
            cli_id,
            cli_config,
            config["tmux"],
            home,
            last_cli,
            now,
            tmux_refresh_request,
            headless_snapshot,
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


def _load_previous_status(home: Path) -> dict[str, Any] | None:
    """Load the last status snapshot so restart-only metadata survives.

    Args:
        home: Home directory containing the poller's state directory.

    Returns:
        The prior aggregate snapshot, or ``None`` when it is absent or invalid.
    """
    target = status_path(home)
    try:
        previous = json.loads(target.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None
    except (OSError, ValueError) as exc:
        LOGGER.warning("cannot reload previous status from %s: %s", target, exc)
        return None
    if not isinstance(previous, dict) or not isinstance(previous.get("clis"), dict):
        LOGGER.warning("ignoring invalid previous status shape in %s", target)
        return None
    LOGGER.info("reloaded previous status from %s", target)
    return previous


def _next_tick_delay(started_at: float, interval: int, now: float) -> float:
    """Return seconds remaining until a start-to-start polling deadline.

    Args:
        started_at: Monotonic time when the current tick began.
        interval: Configured polling interval in seconds.
        now: Current monotonic time after the tick finished.
    """
    return max(0.0, started_at + interval - now)


def run_daemon(home: Path | None = None) -> None:
    """Poll forever at the configured rate until SIGTERM/SIGINT.

    The config is re-read every tick, so polling-rate and per-CLI option
    changes made by the installer apply without a service restart. Slow tmux
    and remote HTTP refreshes are queued on background workers; the foreground
    loop scans local sources and writes status on start-to-start deadlines.
    """
    LOGGER.info("agentic-cli-status poller starting (pid %d)", os.getpid())
    stop = {"requested": False}

    def _handle_stop(signum: int, _frame: Any) -> None:
        LOGGER.info("received signal %d; stopping", signum)
        stop["requested"] = True

    signal.signal(signal.SIGTERM, _handle_stop)
    signal.signal(signal.SIGINT, _handle_stop)
    resolved_home = home or Path.home()
    previous = _load_previous_status(resolved_home)
    tmux_worker = TmuxRefreshWorker(resolved_home)
    headless_worker = HeadlessRefreshWorker()
    tmux_worker.start()
    headless_worker.start()
    overrun = False
    try:
        while not stop["requested"]:
            tick_started = time.monotonic()
            try:
                previous = poll_once(
                    home=home,
                    previous=previous,
                    tmux_refresh_request=tmux_worker.request,
                    headless_refresh_worker=headless_worker,
                )
            except Exception as exc:  # noqa: BLE001 - daemon must survive any tick
                LOGGER.exception("poll tick failed: %s", exc)
            interval = (previous or {}).get("poll_interval_seconds") or load_config()[
                "poll_interval_seconds"
            ]
            delay = _next_tick_delay(tick_started, interval, time.monotonic())
            if delay == 0 and not overrun:
                LOGGER.warning("poll tick exceeded configured %ss cadence", interval)
                overrun = True
            elif delay > 0 and overrun:
                LOGGER.info("poll tick cadence recovered to %ss", interval)
                overrun = False
            deadline = time.monotonic() + delay
            while not stop["requested"]:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                time.sleep(min(1.0, remaining))
    finally:
        tmux_worker.close()
        headless_worker.close()
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
