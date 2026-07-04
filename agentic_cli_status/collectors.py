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
#: Temporary Codex allowance when neither rollout data nor /usage states it.
DEFAULT_CODEX_RESETS_AVAILABLE = 1


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
    snapshot = _snapshot(
        "codex_sessions",
        observed_at=observed,
        plan=last.get("plan_type"),
        windows=windows,
    )
    snapshot["resets_available"] = DEFAULT_CODEX_RESETS_AVAILABLE
    return snapshot


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


def _carry_last_reading(
    snapshot: dict[str, Any], last_result: dict[str, Any] | None
) -> dict[str, Any]:
    """Fill an empty error snapshot with the previous good reading.

    A failed endpoint attempt must never blank the indicator (that would also
    wrongly trigger the stale/tmux fallback), so the last known windows, their
    observation time, and the plan are carried forward alongside the error.
    """
    if last_result and last_result.get("windows") and not snapshot["windows"]:
        snapshot["windows"] = last_result["windows"]
        snapshot["observed_at"] = last_result.get("observed_at")
        snapshot["plan"] = snapshot["plan"] or last_result.get("plan")
    return snapshot


def _retry_after_seconds(exc: urllib.error.HTTPError, fallback: float) -> float:
    """Parse a Retry-After header (integer seconds) with a safe fallback."""
    value = (exc.headers.get("Retry-After") or "").strip()
    if value.isdigit():
        return float(value)
    return fallback


def collect_claude(
    home: Path,
    last_result: dict[str, Any] | None = None,
    now: float | None = None,
    opener: Any = None,
    min_interval: float | None = None,
) -> dict[str, Any]:
    """Collect Claude Code rate limits from the OAuth usage endpoint.

    Enforces a minimum interval between endpoint *attempts* (the endpoint
    grants only a small call budget and 429-locks aggressive pollers for
    minutes, so short poll ticks must not turn failures into a retry storm);
    within the window the previous result is returned unchanged. A 429's
    Retry-After lockout is honored on top of the interval, and failed
    attempts keep the last good reading (see :func:`_carry_last_reading`).
    Every attempted snapshot carries ``checked_at``.

    Args:
        home: The user's home directory.
        last_result: The previous snapshot for this CLI, if any.
        now: Current epoch override (tests).
        opener: ``urllib.request.urlopen``-compatible callable (tests).
        min_interval: Configured seconds between endpoint attempts
            (``clis.claude_code.endpoint_min_interval_seconds``); defaults to
            :data:`CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS`.
    """
    now = now if now is not None else time.time()
    if min_interval is None:
        min_interval = CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS
    # checked_at is only ever written by this collector (and carried across
    # pane replacements by the poller), so its presence alone gates attempts.
    if (
        last_result
        and last_result.get("checked_at")
        and (
            now - last_result["checked_at"] < min_interval
            or now < last_result.get("retry_after_until", 0)
        )
    ):
        return last_result
    token, subscription, error = _claude_access_token(home)
    if token is None:
        snapshot = _snapshot("claude_oauth_endpoint", plan=subscription, error=error)
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
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
        if exc.code == 429:
            retry_after = _retry_after_seconds(exc, min_interval)
            snapshot["retry_after_until"] = now + retry_after
            LOGGER.warning(
                "claude usage endpoint locked out; retrying after %.0f s",
                retry_after,
            )
        return _carry_last_reading(snapshot, last_result)
    except (urllib.error.URLError, OSError, ValueError) as exc:
        LOGGER.error("claude usage endpoint failed: %s", exc)
        snapshot = _snapshot("claude_oauth_endpoint", plan=subscription, error=str(exc))
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    windows = _claude_windows(payload)
    if not windows:
        snapshot = _snapshot(
            "claude_oauth_endpoint",
            plan=subscription,
            error="no rate-limit windows in endpoint response",
        )
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
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
#: Codex /usage allowance for manually resetting a rolling limit.
_CODEX_RESETS_AVAILABLE = re.compile(
    r"\b(\d+)\s+resets?\s+available\b", re.I
)

#: A rendered "resets …" phrase and everything after it on the line. The reset
#: descriptor (clock, optional day) is pulled out of the trailing text.
_PANE_RESET = re.compile(r"\bresets?\b\s*(?:at\s+)?(.+)$", re.I)
#: A "HH:MM" 24-hour clock inside a reset descriptor.
_PANE_RESET_CLOCK = re.compile(r"\b([01]?\d|2[0-3]):([0-5]\d)\b")
#: A "D Mon" calendar day inside a reset descriptor (e.g. "9 Jul").
_PANE_RESET_DATE = re.compile(r"\b(\d{1,2})\s+([A-Za-z]{3})", re.I)
#: Three-letter month abbreviation -> month number (1-12).
_MONTH_ABBR = {
    name.lower(): number
    for number, name in enumerate(
        ("Jan", "Feb", "Mar", "Apr", "May", "Jun",
         "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"),
        start=1,
    )
}


def _pane_reset_epoch(line: str, now: float) -> int | None:
    """Convert a rendered "resets …" phrase into an absolute epoch.

    Codex renders reset times relative to the reader's clock — ``(resets
    20:12)`` for the rolling 5h window and ``(resets 09:10 on 9 Jul)`` for the
    weekly one — never as timestamps. A bare ``HH:MM`` is resolved to its next
    occurrence (today, or tomorrow if already past); a ``HH:MM on D Mon`` is
    resolved to that calendar day this year (or next if it has passed). Returns
    ``None`` when the phrase carries no parseable clock, so day-only wordings
    like "resets Mon" stay ``None`` rather than guessing a time.
    """
    reset = _PANE_RESET.search(line)
    if not reset:
        return None
    descriptor = reset.group(1)
    clock = _PANE_RESET_CLOCK.search(descriptor)
    if not clock:
        return None
    hour, minute = int(clock.group(1)), int(clock.group(2))
    base = _dt.datetime.fromtimestamp(now).astimezone()
    day = _PANE_RESET_DATE.search(descriptor)
    if day:
        month = _MONTH_ABBR.get(day.group(2).lower())
        if month is None:
            return None
        try:
            candidate = base.replace(
                month=month, day=int(day.group(1)),
                hour=hour, minute=minute, second=0, microsecond=0,
            )
        except ValueError:
            return None
        # A day/month already well behind us names next year's occurrence; the
        # one-day slack absorbs timezone skew around the boundary.
        if candidate.timestamp() < now - 86400:
            try:
                candidate = candidate.replace(year=candidate.year + 1)
            except ValueError:
                return None
        return int(candidate.timestamp())
    candidate = base.replace(hour=hour, minute=minute, second=0, microsecond=0)
    if candidate.timestamp() <= now:
        candidate += _dt.timedelta(days=1)
    return int(candidate.timestamp())


def _pane_window_reset(
    lines: list[str],
    keyword: re.Pattern[str],
    stop_keywords: list[re.Pattern[str]],
    now: float,
) -> int | None:
    """Find one window's reset epoch near its keyword line.

    The reset phrase sits on the window's own line or wraps onto the next
    (Codex prints the weekly reset on a continuation line). Scans the keyword
    line and up to two following lines, stopping early if another window's
    label begins, so a wrapped reset is captured without stealing the next
    window's.
    """
    for index, line in enumerate(lines):
        if not keyword.search(line):
            continue
        for offset in range(3):
            probe = index + offset
            if probe >= len(lines):
                break
            if offset and any(k.search(lines[probe]) for k in stop_keywords):
                break
            epoch = _pane_reset_epoch(lines[probe], now)
            if epoch is not None:
                return epoch
        return None
    return None


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
    now = now if now is not None else time.time()
    keywords = _PANE_WINDOW_KEYWORDS.get(cli_id, [])
    windows: list[dict[str, Any]] = []
    for label, keyword in keywords:
        percent = _pane_window_percent(lines, keyword)
        if percent is None:
            continue
        others = [other for other_label, other in keywords if other is not keyword]
        windows.append({
            "label": label,
            "used_percent": percent,
            "resets_at": _pane_window_reset(lines, keyword, others, now),
        })
    if not windows:
        return _snapshot("tmux_pane", error="no usage percentages found in pane")
    snapshot = _snapshot("tmux_pane", observed_at=now, windows=windows)
    if cli_id == "codex":
        resets = _CODEX_RESETS_AVAILABLE.search(text)
        snapshot["resets_available"] = (
            int(resets.group(1)) if resets else DEFAULT_CODEX_RESETS_AVAILABLE
        )
    return snapshot
