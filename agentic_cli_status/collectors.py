#!/usr/bin/env python3
"""Headless rate-limit collectors for Codex, Claude Code, and Grok CLI.

Each collector reads data the CLI has already written locally (or exposes on a
billing/metadata endpoint) so polling never sends a prompt and never consumes
plan tokens:

- **Codex** — the TUI appends ``token_count`` events with a ``rate_limits``
  payload to ``~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl``. The collector
  parses the newest event; no Codex process is involved.
- **Claude Code** — the ``/usage`` panel is powered by the OAuth usage
  endpoint (``api.anthropic.com/api/oauth/usage``). The collector calls it
  with the credentials Claude Code itself stores in
  ``~/.claude/.credentials.json``. It is a billing/metadata endpoint, not a
  model endpoint, so it costs no tokens; calls are throttled to at most one
  per :data:`~agentic_cli_status.config.CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS`.
- **Grok** — the CLI logs a ``billing: fetched credits config`` record (with
  ``creditUsagePercent`` and the weekly period) to ``~/.grok/logs/*.log``
  every time it starts or refreshes billing. The collector parses the newest
  record.

Every collector returns the shared snapshot shape::

    {"source": str, "observed_at": float | None, "plan": str | None,
     "windows": [{"label": str, "used_percent": float,
                  "resets_at": int | None}, ...],
     "error": str | None}

``parse_pane`` provides the tmux fallback: it extracts the same shape from
captured ``/status`` / ``/usage`` TUI pane text when a headless source is
unavailable or stale.
"""

from __future__ import annotations

import datetime as _dt
import json
import re
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from .acs_logging import LOGGER
from .config import CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS

#: How many days of Codex session directories to scan backwards.
CODEX_SCAN_DAYS = 7

#: How much of the end of a large session/log file to read per tick. The
#: newest rate-limit record lives at the end, so a bounded tail keeps
#: second-scale polling cheap even for multi-megabyte files.
TAIL_READ_BYTES = 262144

CLAUDE_USAGE_URL = "https://api.anthropic.com/api/oauth/usage"
CLAUDE_OAUTH_BETA = "oauth-2025-04-20"
#: Fallback User-Agent version when ``claude --version`` is unavailable. The
#: endpoint requires a ``claude-code/<version>`` agent to stay in the normally
#: rate-limited bucket.
CLAUDE_UA_FALLBACK_VERSION = "2.0.0"


def _snapshot(
    source: str,
    observed_at: float | None = None,
    plan: str | None = None,
    windows: list[dict[str, Any]] | None = None,
    error: str | None = None,
) -> dict[str, Any]:
    """Build the shared collector snapshot dictionary."""
    return {
        "source": source,
        "observed_at": observed_at,
        "plan": plan,
        "windows": windows or [],
        "error": error,
    }


def _to_epoch(value: Any) -> int | None:
    """Normalize an epoch integer or ISO-8601 string to an epoch integer."""
    if isinstance(value, (int, float)):
        return int(value)
    if isinstance(value, str):
        try:
            return int(
                _dt.datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp()
            )
        except ValueError:
            return None
    return None


def _to_percent(value: Any) -> float | None:
    """Normalize a numeric percentage, clamping to the 0-100 range."""
    if isinstance(value, (int, float)):
        return max(0.0, min(100.0, float(value)))
    return None


def _tail_lines(path: Path, max_bytes: int = TAIL_READ_BYTES) -> list[str]:
    """Return the last complete lines of a file, reading at most ``max_bytes``.

    Args:
        path: File to read.
        max_bytes: Byte budget for the tail read.

    Raises:
        OSError: Propagated so callers keep their existing error handling.
    """
    size = path.stat().st_size
    with path.open("rb") as fh:
        if size > max_bytes:
            fh.seek(size - max_bytes)
            fh.readline()  # drop the (likely) partial first line
        data = fh.read()
    return data.decode("utf-8", errors="replace").splitlines()


# ---------------------------------------------------------------------------
# Codex: ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
# ---------------------------------------------------------------------------

def collect_codex(home: Path) -> dict[str, Any]:
    """Collect Codex rate limits from the newest session rollout file.

    Scans up to :data:`CODEX_SCAN_DAYS` of ``~/.codex/sessions`` day
    directories newest-first and returns the last ``token_count`` event's
    ``rate_limits`` payload (primary = 5h window, secondary = weekly window).

    Args:
        home: The user's home directory.
    """
    sessions = home / ".codex" / "sessions"
    if not sessions.is_dir():
        return _snapshot("codex_sessions", error="no ~/.codex/sessions directory")
    day_dirs = sorted(sessions.glob("*/*/*"), key=lambda p: p.as_posix(), reverse=True)
    for day_dir in day_dirs[:CODEX_SCAN_DAYS]:
        files = sorted(day_dir.glob("rollout-*.jsonl"), reverse=True)
        for rollout in files:
            found = _codex_last_rate_limits(rollout)
            if found is not None:
                return found
    return _snapshot("codex_sessions", error="no rate_limits events found")


def _codex_last_rate_limits(rollout: Path) -> dict[str, Any] | None:
    """Return the snapshot for the last rate_limits event in one rollout file."""
    last: dict[str, Any] | None = None
    last_ts: float | None = None
    try:
        for line in _tail_lines(rollout):
            if '"rate_limits"' not in line:
                continue
            try:
                event = json.loads(line)
            except ValueError:
                continue
            payload = event.get("payload") or {}
            limits = payload.get("rate_limits")
            if isinstance(limits, dict):
                last = limits
                last_ts = _to_epoch(event.get("timestamp"))
    except OSError as exc:
        LOGGER.error("cannot read codex rollout %s: %s", rollout, exc)
        return None
    if last is None:
        return None
    windows = []
    for key, label in (("primary", "5h"), ("secondary", "week")):
        window = last.get(key) or {}
        used = _to_percent(window.get("used_percent"))
        if used is not None:
            windows.append(
                {
                    "label": label,
                    "used_percent": used,
                    "resets_at": _to_epoch(window.get("resets_at")),
                }
            )
    if not windows:
        return None
    observed = float(last_ts) if last_ts is not None else rollout.stat().st_mtime
    return _snapshot(
        "codex_sessions",
        observed_at=observed,
        plan=last.get("plan_type"),
        windows=windows,
    )


# ---------------------------------------------------------------------------
# Claude Code: OAuth usage endpoint with Claude Code's own credentials
# ---------------------------------------------------------------------------

def claude_user_agent(home: Path) -> str:
    """Return the ``claude-code/<version>`` User-Agent the endpoint expects.

    Reads the version from Claude Code's installation metadata when possible
    (never spawns the CLI); falls back to a pinned version string.

    Args:
        home: The user's home directory.
    """
    for candidate in (
        home / ".local" / "state" / "claude" / "latest-version",
        home / ".claude" / "version",
    ):
        try:
            version = candidate.read_text(encoding="utf-8").strip()
            if re.fullmatch(r"[0-9][0-9A-Za-z_.-]*", version):
                return f"claude-code/{version}"
        except OSError:
            continue
    # Any real claude-code UA keeps us out of the anonymous 429 bucket.
    return f"claude-code/{CLAUDE_UA_FALLBACK_VERSION}"


def _claude_access_token(home: Path) -> tuple[str | None, str | None, str | None]:
    """Read the OAuth access token from Claude Code's credentials file.

    Returns:
        ``(token, subscription, error)`` — token is None with an error message
        when credentials are missing or expired.
    """
    creds_file = home / ".claude" / ".credentials.json"
    try:
        creds = json.loads(creds_file.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return None, None, f"credentials unavailable: {exc}"
    oauth = creds.get("claudeAiOauth") or {}
    token = oauth.get("accessToken")
    if not token:
        return None, None, "no accessToken in credentials"
    expires_at = oauth.get("expiresAt")
    if isinstance(expires_at, (int, float)) and expires_at / 1000 < time.time():
        # Claude Code refreshes the token itself on next launch; never refresh
        # here, just report the condition.
        return None, oauth.get("subscriptionType"), "access token expired"
    return token, oauth.get("subscriptionType"), None


def collect_claude(
    home: Path,
    last_result: dict[str, Any] | None = None,
    now: float | None = None,
    opener: Any = None,
) -> dict[str, Any]:
    """Collect Claude Code rate limits from the OAuth usage endpoint.

    Enforces a minimum interval between endpoint *attempts* (the endpoint
    429s aggressive pollers, and short poll ticks must not turn failures into
    a retry storm); within the window the previous result is returned
    unchanged. Every attempted snapshot carries ``checked_at``.

    Args:
        home: The user's home directory.
        last_result: The previous snapshot for this CLI, if any.
        now: Current epoch override (tests).
        opener: ``urllib.request.urlopen``-compatible callable (tests).
    """
    now = now if now is not None else time.time()
    if (
        last_result
        and last_result.get("source") == "claude_oauth_endpoint"
        and last_result.get("checked_at")
        and now - last_result["checked_at"] < CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS
    ):
        return last_result
    token, subscription, error = _claude_access_token(home)
    if token is None:
        snapshot = _snapshot("claude_oauth_endpoint", plan=subscription, error=error)
        snapshot["checked_at"] = now
        return snapshot
    request = urllib.request.Request(
        CLAUDE_USAGE_URL,
        headers={
            "Authorization": f"Bearer {token}",
            "anthropic-beta": CLAUDE_OAUTH_BETA,
            "User-Agent": claude_user_agent(home),
            "Content-Type": "application/json",
        },
    )
    opener = opener or urllib.request.urlopen
    snapshot: dict[str, Any]
    try:
        with opener(request, timeout=20) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        LOGGER.error("claude usage endpoint HTTP %s", exc.code)
        snapshot = _snapshot(
            "claude_oauth_endpoint", plan=subscription, error=f"HTTP {exc.code}"
        )
        snapshot["checked_at"] = now
        return snapshot
    except (urllib.error.URLError, OSError, ValueError) as exc:
        LOGGER.error("claude usage endpoint failed: %s", exc)
        snapshot = _snapshot("claude_oauth_endpoint", plan=subscription, error=str(exc))
        snapshot["checked_at"] = now
        return snapshot
    windows = _claude_windows(payload)
    if not windows:
        snapshot = _snapshot(
            "claude_oauth_endpoint",
            plan=subscription,
            error="no rate-limit windows in endpoint response",
        )
    else:
        snapshot = _snapshot(
            "claude_oauth_endpoint", observed_at=now, plan=subscription, windows=windows
        )
    snapshot["checked_at"] = now
    return snapshot


def _claude_windows(payload: dict[str, Any]) -> list[dict[str, Any]]:
    """Extract normalized windows from a usage endpoint response.

    The response formats have varied across releases (top-level
    ``five_hour``/``seven_day`` objects with ``utilization`` or
    ``used_percentage``). Newer responses also expose model-specific weekly
    limits, including Fable, in a ``limits`` array, so parsing is defensive.
    """
    labels = (("five_hour", "5h"), ("seven_day", "week"), ("seven_day_opus", "opus"))
    windows: list[dict[str, Any]] = []
    for key, label in labels:
        window = payload.get(key)
        if not isinstance(window, dict):
            continue
        used = _to_percent(
            window.get("utilization")
            if window.get("utilization") is not None
            else window.get("used_percentage")
        )
        if used is None:
            continue
        windows.append(
            {
                "label": label,
                "used_percent": used,
                "resets_at": _to_epoch(window.get("resets_at")),
            }
        )
    existing_labels = {window["label"] for window in windows}
    for limit in payload.get("limits") or []:
        if not isinstance(limit, dict) or limit.get("kind") != "weekly_scoped":
            continue
        scope = limit.get("scope") or {}
        model = scope.get("model") or {}
        display_name = model.get("display_name")
        if not isinstance(display_name, str) or not display_name.strip():
            continue
        label = display_name.strip().lower()
        used = _to_percent(limit.get("percent"))
        if used is None or label in existing_labels:
            continue
        windows.append(
            {
                "label": label,
                "used_percent": used,
                "resets_at": _to_epoch(limit.get("resets_at")),
            }
        )
        existing_labels.add(label)
    return windows


# ---------------------------------------------------------------------------
# Grok: billing records in ~/.grok/logs/*.log
# ---------------------------------------------------------------------------

def collect_grok(home: Path) -> dict[str, Any]:
    """Collect Grok credit usage from the newest CLI billing log record.

    Grok CLI logs ``billing: fetched credits config`` with
    ``creditUsagePercent`` and the weekly usage period whenever it starts, so
    the newest record mirrors the TUI ``/usage`` view.

    Args:
        home: The user's home directory.
    """
    logs_dir = home / ".grok" / "logs"
    if not logs_dir.is_dir():
        return _snapshot("grok_logs", error="no ~/.grok/logs directory")
    log_files = sorted(
        (p for p in logs_dir.iterdir() if p.is_file()),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    for log_file in log_files[:3]:
        found = _grok_last_billing(log_file)
        if found is not None:
            return found
    return _snapshot("grok_logs", error="no billing records found")


def _grok_last_billing(log_file: Path) -> dict[str, Any] | None:
    """Return the snapshot for the last billing record in one grok log file."""
    last: dict[str, Any] | None = None
    last_ts: float | None = None
    try:
        for line in _tail_lines(log_file):
            if "fetched credits config" not in line:
                continue
            try:
                record = json.loads(line)
            except ValueError:
                continue
            ctx = record.get("ctx") or {}
            if isinstance(ctx.get("config"), dict):
                last = ctx
                last_ts = _to_epoch(record.get("ts"))
    except OSError as exc:
        LOGGER.error("cannot read grok log %s: %s", log_file, exc)
        return None
    if last is None:
        return None
    billing = last["config"]
    used = _to_percent(billing.get("creditUsagePercent"))
    if used is None:
        return None
    period = billing.get("currentPeriod") or {}
    resets = _to_epoch(period.get("end") or billing.get("billingPeriodEnd"))
    observed = float(last_ts) if last_ts is not None else log_file.stat().st_mtime
    return _snapshot(
        "grok_logs",
        observed_at=observed,
        plan=last.get("subscriptionTier"),
        windows=[{"label": "week", "used_percent": used, "resets_at": resets}],
    )


# ---------------------------------------------------------------------------
# tmux pane fallback: parse rendered /status | /usage TUI text
# ---------------------------------------------------------------------------

#: Per-CLI window keywords used to find "<window label> … NN% left|used"
#: lines in the rendered TUI output. Grouped as (label, keyword regex).
_PANE_WINDOW_KEYWORDS: dict[str, list[tuple[str, re.Pattern[str]]]] = {
    "codex": [
        ("5h", re.compile(r"5h(?:our)?\s+limit", re.I)),
        ("week", re.compile(r"week(?:ly)?\s+limit", re.I)),
    ],
    "claude_code": [
        ("5h", re.compile(r"current\s+session|5-?h(?:our)?\s+limit", re.I)),
        ("week", re.compile(r"current\s+week|week(?:ly)?\s+limit", re.I)),
        ("fable", re.compile(r"(?:current\s+week[^\n]*\b)?fable(?:\s+only)?", re.I)),
    ],
    "grok": [
        ("week", re.compile(r"week(?:ly)?\s+limit|credits?|usage", re.I)),
    ],
}

_PANE_PERCENT = re.compile(r"(\d+(?:\.\d+)?)\s*%")


def _pane_window_percent(
    lines: list[str], keyword: re.Pattern[str]
) -> float | None:
    """Extract one window's used percentage from rendered pane lines.

    Finds the first line matching ``keyword`` that carries a percentage (or
    whose next line does — some TUIs wrap), and normalizes the direction:
    modern CLIs render "NN% left" (e.g. Codex "5h limit: 99% left", Grok
    "Weekly limit left: 0%"), older ones "NN% used". A line mentioning
    "left" yields ``100 - NN``.
    """
    for index, line in enumerate(lines):
        if not keyword.search(line):
            continue
        source_line = line
        match = _PANE_PERCENT.search(line)
        if not match and index + 1 < len(lines):
            source_line = lines[index + 1]
            match = _PANE_PERCENT.search(source_line)
        if not match:
            continue
        percent = _to_percent(float(match.group(1)))
        if percent is None:
            continue
        if re.search(r"\bleft\b", line + " " + source_line, re.I):
            percent = 100.0 - percent
        return percent
    return None


def parse_pane(cli_id: str, text: str, now: float | None = None) -> dict[str, Any]:
    """Parse captured tmux pane text into the shared snapshot shape.

    This is the best-effort fallback when no headless source is available:
    the pane shows the CLI's own ``/status`` / ``/usage`` rendering, whose
    wording can change between releases, so any percentage found per window
    label is accepted (with "left" percentages converted to used).

    Args:
        cli_id: One of ``codex``, ``claude_code``, ``grok``.
        text: The output of ``tmux capture-pane -p`` for that CLI's window.
        now: Current epoch override (tests).
    """
    lines = text.splitlines()
    windows: list[dict[str, Any]] = []
    for label, keyword in _PANE_WINDOW_KEYWORDS.get(cli_id, []):
        percent = _pane_window_percent(lines, keyword)
        if percent is None:
            continue
        windows.append({"label": label, "used_percent": percent, "resets_at": None})
    if not windows:
        return _snapshot("tmux_pane", error="no usage percentages found in pane")
    return _snapshot(
        "tmux_pane",
        observed_at=now if now is not None else time.time(),
        windows=windows,
    )
