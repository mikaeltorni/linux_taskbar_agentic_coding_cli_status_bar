#!/usr/bin/env python3
"""Headless rate-limit collectors for Codex, Claude Code, Grok, OpenCode Go, and OpenRouter.

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
  When the stored access token has expired (e.g. right after a reboot, before
  the user relaunches ``claude``), the collector first refreshes it headlessly
  via the OAuth token endpoint using the stored refresh token — but only while
  no ``claude`` process is running, so it can never race the CLI's own refresh
  and accidentally rotate the single-use refresh token out from under it. The
  rotated credentials are written back atomically.
- **Grok** — the CLI logs a ``billing: fetched credits config`` record (with
  ``creditUsagePercent`` and the weekly period) to its unified log
  (``~/.grok/logs/unified.jsonl``; historically a per-process ``*.log``)
  every time it starts or refreshes billing. The collector parses the newest
  record, and trusts it over the tmux ``/usage`` pane: the pane's persistent
  ``Weekly limit left`` border freezes between refreshes, while every billing
  fetch is logged. When a newer ``balance exhausted`` 402 error record follows
  the last billing fetch (with no later billing record), Grok has spent the
  remainder it never re-logs, so the window is reported as 100% used
  (0% left) until the next billing fetch — after the weekly reset —
  supersedes it.
- **OpenCode Go** — the ``opencode-go-usage`` plugin scrapes embedded
  ``rollingUsage``/``weeklyUsage``/``monthlyUsage`` JSON from the workspace
  ``/go`` page on ``opencode.ai``. The collector performs the same headless
  fetch with the same credentials (env vars or ``opencode-go-usage.json``),
  throttled to at least every five minutes.
- **OpenRouter** — the management API key saved by the installer calls
  ``POST /api/v1/analytics/query`` (read-only analytics, not chat/completions)
  for the current UTC day, sums ``request_count`` for the configured
  free-model slug, and reports usage against the daily ceiling (default
  1000) with ``resets_at`` at the next 00:00 UTC boundary.

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
import fcntl
import json
import os
import re
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from .acs_logging import LOGGER
from .config import (
    CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS,
    OPENROUTER_DAILY_LIMIT_DEFAULT,
    OPENROUTER_FETCH_MIN_INTERVAL_SECONDS,
    OPENROUTER_MODEL_SLUG_DEFAULT,
    OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS,
)
from .openrouter_client import fetch_analytics_rows, management_api_key

#: How many days of Codex session directories to scan backwards.
CODEX_SCAN_DAYS = 7

#: Codex can report the same rolling limit with reset epochs a few seconds
#: apart when concurrent sessions publish snapshots. Treat those as one logical
#: window so a later authoritative snapshot can replace an earlier one.
CODEX_SAME_RESET_TOLERANCE_SECONDS = 60

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

#: OAuth token-refresh endpoint Claude Code itself POSTs to with its refresh
#: token. Same host as :data:`CLAUDE_USAGE_URL`; the path and JSON body shape
#: (``grant_type=refresh_token`` + ``client_id`` + ``scope``) match the CLI's
#: own ``requestOAuthTokenRefresh`` so rotating refresh tokens round-trip
#: cleanly. Overridable via ``CLAUDE_CODE_OAUTH_TOKEN_URL`` for staging installs.
CLAUDE_OAUTH_TOKEN_URL = os.environ.get(
    "CLAUDE_CODE_OAUTH_TOKEN_URL", "https://api.anthropic.com/v1/oauth/token"
)
#: The client_id Claude Code registers with the OAuth provider. The CLI ships
#: this constant; we mirror it so refresh requests are accepted. A future CLI
#: build may rotate it, so ``CLAUDE_CODE_OAUTH_CLIENT_ID`` overrides it.
CLAUDE_OAUTH_CLIENT_ID = os.environ.get(
    "CLAUDE_CODE_OAUTH_CLIENT_ID", "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
)
#: Scopes requested on refresh when the credentials file lists none. Mirrors
#: the default scope set Claude Code requests at login.
CLAUDE_OAUTH_DEFAULT_SCOPES = (
    "user:file_upload",
    "user:inference",
    "user:mcp_servers",
    "user:profile",
    "user:sessions:claude_code",
)
#: Name of the lock file Claude Code uses to serialize OAuth refreshes. We hold
#: the same path (best-effort ``flock``) so two poller instances never refresh
#: concurrently and clobber a just-rotated refresh token.
CLAUDE_OAUTH_REFRESH_LOCK_NAME = ".oauth_refresh.lock"
#: Upper bound on the time we will wait for the refresh lock before giving up.
CLAUDE_OAUTH_REFRESH_LOCK_TIMEOUT_SECONDS = 5.0

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

#: Known Codex rolling-window durations (minutes) → status-bar labels.
#: Free / no-subscription accounts use a ~30-day primary (43200); Plus keeps
#: the classic ~5h primary (300) plus ~weekly secondary (10080). Labels are
#: derived from duration rather than payload key so a week-length primary is
#: not mislabeled as ``5h``.
_CODEX_WINDOW_DURATION_LABELS: tuple[tuple[int, str], ...] = (
    (300, "5h"),
    (10080, "week"),
    (43200, "month"),
)


def _codex_window_label(
    window_minutes: float | int | None, *, key: str
) -> str:
    """Map a Codex rate-limit window duration to a status-bar label.

    Prefer ``window_minutes`` when present: nearest of the known anchors
    (300→``5h``, 10080→``week``, 43200→``month``). When duration is missing
    or unusable, fall back to the legacy payload-key mapping
    (``primary``→``5h``, ``secondary``→``week``) so older rollouts without
    minutes still publish.

    Args:
        window_minutes: Rolling window length in minutes from the Codex
            ``rate_limits`` payload, or ``None`` when absent.
        key: Payload key (``primary`` or ``secondary``) used only as a
            fallback when ``window_minutes`` cannot be used.

    Returns:
        One of ``5h``, ``week``, or ``month``.
    """
    if window_minutes is not None:
        try:
            minutes = float(window_minutes)
        except (TypeError, ValueError):
            minutes = None
        if minutes is not None and minutes > 0:
            label = min(
                _CODEX_WINDOW_DURATION_LABELS,
                key=lambda item: abs(item[0] - minutes),
            )[1]
            return label
    return "5h" if key == "primary" else "week"


def collect_codex(home: Path, *, now: float | None = None) -> dict[str, Any]:
    """Collect Codex rate limits from the newest session rollout file.

    Scans up to :data:`CODEX_SCAN_DAYS` of ``~/.codex/sessions`` day
    directories newest-first and returns the newest state of each rate-limit
    window. Labels come from each window's ``window_minutes`` (5h / week /
    month), so free-tier monthly and primary-only weekly shapes publish
    correctly. Codex may run several concurrent 5h windows at once (Codex
    v0.142+), so the collector tracks the most-depleted genuinely distinct
    reset window rather than the literal final ``token_count`` event;
    otherwise a freshly-opened concurrent window logged last would make the
    bar jump to ~100% left (a spurious reset).

    Args:
        home: The user's home directory.
        now: Current epoch used to distinguish active windows from windows
            whose reset has already passed. Defaults to the system clock.
    """
    if now is None:
        now = time.time()
    sessions = home / ".codex" / "sessions"
    if not sessions.is_dir():
        return _snapshot("codex_sessions", error="no ~/.codex/sessions directory")
    day_dirs = sorted(sessions.glob("*/*/*"), key=lambda p: p.as_posix(), reverse=True)
    for day_dir in day_dirs[:CODEX_SCAN_DAYS]:
        files = sorted(day_dir.glob("rollout-*.jsonl"), reverse=True)
        for rollout in files:
            found = _codex_last_rate_limits(rollout, now=now)
            if found is not None:
                return found
    return _snapshot("codex_sessions", error="no rate_limits events found")


def _codex_last_rate_limits(rollout: Path, *, now: float) -> dict[str, Any] | None:
    """Return the snapshot for the newest state of each rate-limit window.

    Codex (v0.142+) can run several concurrent 5h ``primary`` windows at once
    (each with its own ``resets_at``), and later activity on a lightly-used
    window lands *after* the heavily-used one in the file. Picking the literal
    final ``rate_limits`` event therefore reports whichever window happened to
    log last, which makes the bar jump to ~100% left (a spurious "reset") whenever
    a fresh concurrent window is active. Keep the most-depleted reading across
    genuinely distinct *active* reset windows, but first coalesce reset epochs
    that are only seconds apart and retain the newest snapshot in that logical
    window. Codex emits those near-identical epochs for one rolling account
    window across concurrent sessions. Expired windows are excluded whenever
    an active replacement exists; otherwise an old exhausted window can mask
    the post-reset reading indefinitely.

    Window labels are derived from each entry's ``window_minutes`` (see
    :func:`_codex_window_label`) so free monthly (~43200) and primary-only
    weekly (~10080) shapes are not hard-mapped to ``5h`` / ``week`` by payload
    key alone.

    Args:
        rollout: Codex JSONL session rollout to inspect.
        now: Current epoch used to identify expired reset windows.
    """
    # Codex (v0.142+) can open several concurrent 5h windows at once, each
    # with its own resets_at, and later activity on a lightly-used window lands
    # *after* the heavily-used one in the file. Picking the literal final event
    # would then report the fresh window and make the bar jump to ~100% left (a
    # spurious "reset"). Track the most-depleted active window per label
    # instead: the window you are actively draining accrues usage fastest, so
    # the highest used_percent is the relevant one for a budget bar, and it is
    # immune to the ordering artifact. A reset epoch can drift by a few seconds
    # between snapshots of the same rolling account window, so first collapse
    # those near-identical epochs to their latest event. Only then choose the
    # most depleted truly distinct, unexpired window for the budget bar.
    candidates: dict[str, list[dict[str, Any]]] = {}
    plan_type: str | None = None
    try:
        for order, line in enumerate(_tail_lines(rollout)):
            if '"rate_limits"' not in line:
                continue
            try:
                event = json.loads(line)
            except ValueError:
                continue
            payload = event.get("payload") or {}
            limits = payload.get("rate_limits")
            if not isinstance(limits, dict):
                continue
            ts = _to_epoch(event.get("timestamp"))
            observed_at = ts if ts is not None else rollout.stat().st_mtime
            for key in ("primary", "secondary"):
                window = limits.get(key)
                if not isinstance(window, dict):
                    continue
                # A window without a usable used_percent is meaningless (it
                # would otherwise default to 0.0 and render as 100% left), so
                # skip it rather than publish a misleading reading.
                used = _to_percent(window.get("used_percent"))
                if used is None:
                    continue
                label = _codex_window_label(window.get("window_minutes"), key=key)
                reset = _to_epoch(window.get("resets_at"))
                candidate = {
                    "used_percent": used,
                    "resets_at": reset,
                    "observed_at": observed_at,
                    "order": order,
                }
                label_candidates = candidates.setdefault(label, [])
                same_window = next(
                    (
                        existing
                        for existing in label_candidates
                        if reset is not None
                        and existing["resets_at"] is not None
                        and abs(reset - existing["resets_at"])
                        <= CODEX_SAME_RESET_TOLERANCE_SECONDS
                    ),
                    None,
                )
                if same_window is None:
                    label_candidates.append(candidate)
                elif (candidate["observed_at"], candidate["order"]) >= (
                    same_window["observed_at"],
                    same_window["order"],
                ):
                    label_candidates[label_candidates.index(same_window)] = candidate
                if limits.get("plan_type"):
                    plan_type = limits["plan_type"]
    except OSError as exc:
        LOGGER.error("cannot read codex rollout %s: %s", rollout, exc)
        return None
    windows = []
    selected: dict[str, dict[str, Any]] = {}
    for label, label_candidates in candidates.items():
        active_candidates = [
            candidate
            for candidate in label_candidates
            if candidate["resets_at"] is None or candidate["resets_at"] > now
        ]
        selectable = active_candidates or label_candidates
        selected[label] = max(
            selectable,
            key=lambda candidate: (
                candidate["used_percent"],
                candidate["observed_at"],
                candidate["order"],
            ),
        )
    for label, candidate in selected.items():
        windows.append(
            {
                "label": label,
                "used_percent": candidate["used_percent"],
                "resets_at": candidate["resets_at"],
            }
        )
    if not windows:
        return None
    newest_observed = max(candidate["observed_at"] for candidate in selected.values())
    labels = [window["label"] for window in windows]
    LOGGER.debug(
        "codex rate_limits from %s plan=%s labels=%s",
        rollout.name,
        plan_type,
        labels,
    )
    return _snapshot(
        "codex_sessions",
        observed_at=newest_observed,
        plan=plan_type,
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
        # The access token has expired. The poller refreshes it itself via
        # :func:`_claude_refresh_oauth_token` when it is safe to do so (no
        # ``claude`` process is running, so we cannot race the CLI's own
        # refresh); otherwise the caller reports this condition and carries
        # the last good reading until Claude Code next launches.
        return None, oauth.get("subscriptionType"), "access token expired"
    return token, oauth.get("subscriptionType"), None


def _claude_is_running(proc_root: Path = Path("/proc")) -> bool:
    """Return True when a Claude Code process appears to be running.

    Scans ``/proc/*/cmdline`` (read-only, no CLI spawn) for an executable
    whose basename is ``claude`` or a ``cli.js`` shipped inside the
    ``claude-code`` npm package. Used to gate poller-side OAuth refreshes so
    we never rotate the refresh token while the CLI holds it in memory.
    """
    try:
        entries = [p for p in proc_root.iterdir() if p.name.isdigit()]
    except OSError:
        return False
    for entry in entries:
        try:
            raw = (entry / "cmdline").read_bytes()
        except OSError:
            continue
        if not raw:
            continue
        for part in raw.split(b"\x00"):
            if not part:
                continue
            arg = part.decode("utf-8", "ignore")
            base = Path(arg).name
            if base == "claude":
                return True
            if base == "cli.js" and "claude-code" in arg:
                return True
    return False


def _claude_refresh_lock_path(home: Path) -> Path:
    """Return the path to Claude Code's OAuth refresh lock file."""
    return home / ".claude" / CLAUDE_OAUTH_REFRESH_LOCK_NAME


def _claude_refresh_oauth_token(
    home: Path,
    now: float | None = None,
    opener: Any = None,
    is_running: Any = None,
    lock_timeout: float | None = None,
) -> tuple[str | None, str | None, str | None]:
    """Refresh Claude Code's OAuth access token using the stored refresh token.

    OAuth refresh tokens are **single-use and rotating**: every successful
    refresh invalidates the previous refresh token and issues a new one. If
    two processes refresh concurrently one wins, the other's stored refresh
    token becomes invalid, and the CLI silently logs the user out. To stay
    safe this helper:

    * skips entirely when a ``claude`` process is running (the CLI refreshes
      its own token, and our writing the rotated token back to disk would
      race it);
    * holds an ``flock`` on Claude Code's own ``~/.claude/.oauth_refresh.lock``
      so two poller instances never refresh at once;
    * writes the rotated ``accessToken``/``refreshToken``/``expiresAt`` back
      to ``~/.claude/.credentials.json`` atomically, preserving every other
      field Claude Code stores there.

    A successful refresh does not consume plan limits: it only rotates the
    auth credential at the OAuth token endpoint, the same call Claude Code
    makes on its own.

    Args:
        home: The user's home directory.
        now: Current epoch override (tests).
        opener: ``urllib.request.urlopen``-compatible callable (tests).
        is_running: Callable returning whether ``claude`` is running
            (tests). Defaults to :func:`_claude_is_running`.
        lock_timeout: Seconds to wait for the refresh lock (tests). Defaults
            to :data:`CLAUDE_OAUTH_REFRESH_LOCK_TIMEOUT_SECONDS`.

    Returns:
        ``(access_token, subscription, error)`` — ``access_token`` is None
        with an error message when the refresh was skipped or failed. A skip
        is *not* an error: when ``claude`` is running we return
        ``("skipped: claude running", subscription, None)``-style sentinel —
        actually we return ``(None, subscription, "skipped: claude running")``
        so the caller can fall back to carrying the last reading.
    """
    now = now if now is not None else time.time()
    if is_running is None:
        is_running = _claude_is_running
    if lock_timeout is None:
        lock_timeout = CLAUDE_OAUTH_REFRESH_LOCK_TIMEOUT_SECONDS
    if is_running():
        return None, None, "skipped: claude running"
    creds_file = home / ".claude" / ".credentials.json"
    try:
        creds = json.loads(creds_file.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return None, None, f"credentials unavailable: {exc}"
    oauth = dict(creds.get("claudeAiOauth") or {})
    refresh_token = oauth.get("refreshToken")
    if not refresh_token:
        return None, oauth.get("subscriptionType"), "no refresh token in credentials"
    refresh_expires_at = oauth.get("refreshTokenExpiresAt")
    if (
        isinstance(refresh_expires_at, (int, float))
        and refresh_expires_at / 1000 < now
    ):
        return (
            None,
            oauth.get("subscriptionType"),
            "refresh token expired",
        )
    scopes = oauth.get("scopes")
    if not isinstance(scopes, list) or not scopes:
        scopes = list(CLAUDE_OAUTH_DEFAULT_SCOPES)
    body = json.dumps(
        {
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLAUDE_OAUTH_CLIENT_ID,
            "scope": " ".join(scopes),
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        CLAUDE_OAUTH_TOKEN_URL,
        data=body,
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "User-Agent": claude_user_agent(home),
            "anthropic-beta": CLAUDE_OAUTH_BETA,
        },
        method="POST",
    )
    opener = opener or urllib.request.urlopen
    lock_path = _claude_refresh_lock_path(home)
    lock_fd: int | None = None
    try:
        try:
            lock_path.parent.mkdir(parents=True, exist_ok=True)
            lock_fd = os.open(
                lock_path,
                os.O_CREAT | os.O_RDWR,
                0o600,
            )
        except OSError as exc:
            LOGGER.warning("claude oauth refresh lock unavailable: %s", exc)
            lock_fd = None
        if lock_fd is not None:
            deadline = now + lock_timeout
            while True:
                try:
                    fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except OSError:
                    if time.time() >= deadline:
                        LOGGER.warning("claude oauth refresh lock timed out")
                        return (
                            None,
                            oauth.get("subscriptionType"),
                            "refresh lock timed out",
                        )
                    time.sleep(0.1)
            # Re-check after acquiring the lock: another poller may have just
            # refreshed, or ``claude`` may have launched while we waited.
            if is_running():
                return None, oauth.get("subscriptionType"), "skipped: claude running"
        try:
            with opener(request, timeout=15) as response:
                payload = json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as exc:
            body_text = ""
            try:
                body_text = exc.read().decode("utf-8", "replace")[:200]
            except OSError:
                pass
            LOGGER.error("claude oauth refresh HTTP %s: %s", exc.code, body_text)
            return (
                None,
                oauth.get("subscriptionType"),
                f"refresh HTTP {exc.code}",
            )
        except (urllib.error.URLError, OSError, ValueError) as exc:
            LOGGER.error("claude oauth refresh failed: %s", exc)
            return None, oauth.get("subscriptionType"), f"refresh failed: {exc}"
        new_access = payload.get("access_token")
        if not isinstance(new_access, str) or not new_access:
            return (
                None,
                oauth.get("subscriptionType"),
                "refresh response missing access_token",
            )
        oauth["accessToken"] = new_access
        new_refresh = payload.get("refresh_token")
        if isinstance(new_refresh, str) and new_refresh:
            oauth["refreshToken"] = new_refresh
        if isinstance(payload.get("expires_in"), (int, float)):
            oauth["expiresAt"] = int((now + float(payload["expires_in"])) * 1000)
        if isinstance(payload.get("refresh_token_expires_in"), (int, float)):
            oauth["refreshTokenExpiresAt"] = int(
                (now + float(payload["refresh_token_expires_in"])) * 1000
            )
        creds["claudeAiOauth"] = oauth
        try:
            creds_file.parent.mkdir(parents=True, exist_ok=True)
            tmp_fd, tmp_path = tempfile.mkstemp(
                dir=str(creds_file.parent), prefix=".credentials.", suffix=".tmp"
            )
            with os.fdopen(tmp_fd, "w") as fh:
                json.dump(creds, fh, indent=2)
            try:
                old_mode = creds_file.stat().st_mode & 0o777
            except OSError:
                old_mode = 0o600
            os.chmod(tmp_path, old_mode or 0o600)
            os.replace(tmp_path, creds_file)
        except OSError as exc:
            LOGGER.error("claude oauth refresh write-back failed: %s", exc)
            return (
                new_access,
                oauth.get("subscriptionType"),
                f"refresh write-back failed: {exc}",
            )
        LOGGER.info("claude oauth refresh succeeded; credentials updated")
        return new_access, oauth.get("subscriptionType"), None
    finally:
        if lock_fd is not None:
            try:
                fcntl.flock(lock_fd, fcntl.LOCK_UN)
            except OSError:
                pass
            try:
                os.close(lock_fd)
            except OSError:
                pass


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
        snapshot["plan"] = snapshot.get("plan") or last_result.get("plan")
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
    refresh_opener: Any = None,
    is_running: Any = None,
) -> dict[str, Any]:
    """Collect Claude Code rate limits from the OAuth usage endpoint.

    Enforces a minimum interval between endpoint *attempts* (the endpoint
    grants only a small call budget and 429-locks aggressive pollers for
    minutes, so short poll ticks must not turn failures into a retry storm);
    within the window the previous result is returned unchanged. A 429's
    Retry-After lockout is honored on top of the interval, and failed
    attempts keep the last good reading (see :func:`_carry_last_reading`).
    Every attempted snapshot carries ``checked_at``.

    When the on-disk access token has expired, the collector first tries a
    headless OAuth refresh (see :func:`_claude_refresh_oauth_token`) so the
    indicator recovers after a reboot without waiting for the user to
    relaunch ``claude``. The refresh is skipped while ``claude`` is running
    (the CLI refreshes its own token) and on any failure the last good
    reading is carried as before.

    Args:
        home: The user's home directory.
        last_result: The previous snapshot for this CLI, if any.
        now: Current epoch override (tests).
        opener: ``urllib.request.urlopen``-compatible callable for the
            *usage* endpoint (tests).
        min_interval: Configured seconds between endpoint attempts
            (``clis.claude_code.endpoint_min_interval_seconds``); defaults to
            :data:`CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS`.
        refresh_opener: ``urllib.request.urlopen``-compatible callable for
            the *token refresh* endpoint (tests). Defaults to
            :func:`urllib.request.urlopen`.
        is_running: Callable returning whether ``claude`` is running, used to
            gate the refresh (tests). Defaults to
            :func:`_claude_is_running`.
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
        if error == "access token expired":
            # Try a headless refresh so the indicator recovers after a reboot
            # without waiting for the user to relaunch ``claude``. Safe only
            # while no ``claude`` process holds the refresh token in memory
            # (see :func:`_claude_refresh_oauth_token`); if it is skipped or
            # fails we fall through to carrying the last reading below.
            refreshed, subscription, refresh_error = _claude_refresh_oauth_token(
                home, now=now, opener=refresh_opener, is_running=is_running
            )
            if refreshed:
                token = refreshed
                error = None
            elif refresh_error and refresh_error != "skipped: claude running":
                LOGGER.info("claude oauth refresh not attempted: %s", refresh_error)
        if token is None:
            snapshot = _snapshot(
                "claude_oauth_endpoint", plan=subscription, error=error
            )
            snapshot["checked_at"] = now
            snapshot = _carry_last_reading(snapshot, last_result)
            if error == "access token expired" and snapshot["windows"]:
                # An expired token is a transient Claude Code recovers from on
                # its next launch (or that the poller just tried to refresh
                # above). While a recent reading is still carried, suppress the
                # error rather than flashing "access token expired" in the
                # popup; if the token really stays expired, the carried reading
                # ages into the ordinary stale marker on its own.
                snapshot["error"] = None
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
# Grok: billing records in ~/.grok/logs/unified.jsonl (historically *.log)
# ---------------------------------------------------------------------------

#: Substring in a Grok log record's ``ctx.error`` that signals the weekly
#: credit balance is fully spent — a 402 ``Payment Required`` from the Grok
#: Build backend (``... Grok Build usage balance exhausted``). Grok does not
#: publish a follow-up ``fetched credits config`` record for the failed turn,
#: so without this marker the freshest billing record would keep reporting
#: the last pre-exhaustion percentage (e.g. 98% used) and the bar could never
#: reach 0% left even after the balance is drained.
_GROK_BALANCE_EXHAUSTED = "balance exhausted"


def collect_grok(home: Path) -> dict[str, Any]:
    """Collect Grok credit usage from the newest CLI billing log record.

    Grok CLI logs ``billing: fetched credits config`` with
    ``creditUsagePercent`` and the weekly usage period to its unified log
    whenever it starts or refreshes billing, so the newest record mirrors the
    TUI ``/usage`` view. The billing log is Grok's authoritative live source:
    the persistent ``Weekly limit left`` border in the tmux ``/usage`` pane
    freezes between refreshes (and current Grok builds do not submit
    ``/usage`` via ``send-keys`` the way older ones did), so the poller trusts
    the log over the pane.

    A newer ``balance exhausted`` 402 error record (see
    :data:`_GROK_BALANCE_EXHAUSTED`) that follows the last billing fetch —
    with no later billing record — means the balance is fully spent, so the
    window is reported as 100% used (0% left) until the next billing fetch
    supersedes it (typically after the weekly reset).

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
    """Return the snapshot for the last billing record in one grok log file.

    Also detects a newer ``balance exhausted`` 402 error record (see
    :data:`_GROK_BALANCE_EXHAUSTED`): when one follows the last billing fetch
    with no later billing record, the weekly balance is fully spent and the
    window is reported as 100% used (0% left) — the same state the Grok TUI
    reaches once it has spent the remainder Grok never re-logs as a billing
    record. A later billing fetch (e.g. after the weekly reset) supersedes the
    exhaustion marker, so the reading self-heals.
    """
    last_ctx: dict[str, Any] | None = None
    last_billing_ts: float | None = None
    last_exhaust_ts: float | None = None
    try:
        for line in _tail_lines(log_file):
            try:
                record = json.loads(line)
            except ValueError:
                continue
            ctx = record.get("ctx") or {}
            if isinstance(ctx.get("config"), dict) and "fetched credits config" in line:
                last_ctx = ctx
                last_billing_ts = _to_epoch(record.get("ts"))
                continue
            error = ctx.get("error")
            if isinstance(error, str) and _GROK_BALANCE_EXHAUSTED in error.lower():
                exhaust_ts = _to_epoch(record.get("ts"))
                if exhaust_ts is not None and (
                    last_exhaust_ts is None or exhaust_ts > last_exhaust_ts
                ):
                    last_exhaust_ts = exhaust_ts
    except OSError as exc:
        LOGGER.error("cannot read grok log %s: %s", log_file, exc)
        return None
    if last_ctx is None:
        return None
    billing = last_ctx["config"]
    # Grok omits ``creditUsagePercent`` from ``fetched credits config`` records
    # published right after a weekly reset, when the brand-new period has seen
    # no usage yet. That is a real, fresh reading of a 0%-used (100% left)
    # window, not a missing sample: fall back to 0 used rather than discarding
    # the record. Discarding it would make ``collect_grok`` skip the newest
    # post-reset period and report "no billing records found" (empty windows,
    # stale) until the next paid request repopulates the field.
    if "creditUsagePercent" in billing:
        used = _to_percent(billing.get("creditUsagePercent"))
    else:
        used = 0.0
    period = billing.get("currentPeriod")
    resets_at = (
        _to_epoch(period.get("end"))
        if isinstance(period, dict)
        else None
    )
    if resets_at is None:
        resets_at = _to_epoch(billing.get("billingPeriodEnd"))
    observed = (
        float(last_billing_ts) if last_billing_ts is not None else log_file.stat().st_mtime
    )
    exhausted = (
        last_exhaust_ts is not None
        and (last_billing_ts is None or last_exhaust_ts > last_billing_ts)
    )
    if exhausted:
        # The balance is drained past the last logged percentage; report 0%
        # left (100% used) and stamp the observation at the exhaustion event
        # so the bar moves the moment Grok records the 402, not the next time
        # it happens to re-fetch billing.
        used = 100.0
        observed = float(last_exhaust_ts)
        LOGGER.info(
            "grok credit balance exhausted (402 at %s newer than last "
            "billing fetch at %s); reporting 0%% left until the next "
            "billing fetch",
            last_exhaust_ts,
            last_billing_ts,
        )
    return _snapshot(
        "grok_logs",
        observed_at=observed,
        plan=last_ctx.get("subscriptionTier"),
        windows=[{"label": "week", "used_percent": used, "resets_at": resets_at}],
    )


# ---------------------------------------------------------------------------
# OpenCode Go: opencode.ai workspace /go page (opencode-go-usage plugin)
# ---------------------------------------------------------------------------

OPENCODE_GO_USAGE_URL = "https://opencode.ai/workspace/{workspace_id}/go"
OPENCODE_GO_CONFIG_FILE = "opencode-go-usage.json"
_OPENCODE_GO_USAGE_PATTERNS = {
    "rolling": re.compile(r"rollingUsage:\$R\[\d+\]=(\{[^}]+\})"),
    "week": re.compile(r"weeklyUsage:\$R\[\d+\]=(\{[^}]+\})"),
    "month": re.compile(r"monthlyUsage:\$R\[\d+\]=(\{[^}]+\})"),
}
_OPENCODE_GO_JS_KEYS = re.compile(
    r"([{,]\s*)([a-zA-Z_][a-zA-Z0-9_]*)(\s*:)"
)


def _opencode_go_config_paths(home: Path) -> list[Path]:
    """Return candidate opencode-go-usage.json paths (plugin order)."""
    return [
        home / ".config" / "opencode" / OPENCODE_GO_CONFIG_FILE,
        home / ".opencode" / OPENCODE_GO_CONFIG_FILE,
    ]


def _opencode_go_credentials(
    home: Path,
) -> tuple[str | None, str | None, str | None]:
    """Load OpenCode Go workspace ID and auth cookie.

    Returns:
        ``(workspace_id, auth_cookie, error)`` — both credentials are None
        with an error message when unavailable.
    """
    env_workspace = os.environ.get("OPENCODE_GO_WORKSPACE_ID")
    env_cookie = os.environ.get("OPENCODE_GO_AUTH_COOKIE")
    if env_workspace and env_cookie:
        return env_workspace, env_cookie, None
    for path in _opencode_go_config_paths(home):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        workspace = payload.get("workspaceId")
        cookie = payload.get("authCookie")
        if workspace and cookie:
            return workspace, cookie, None
    return None, None, "OpenCode Go credentials unavailable"


def _opencode_go_usage_from_html(html: str, now: float) -> list[dict[str, Any]]:
    """Parse rolling/weekly/monthly usage windows from a /go page body."""
    windows: list[dict[str, Any]] = []
    for label, pattern in _OPENCODE_GO_USAGE_PATTERNS.items():
        match = pattern.search(html)
        if not match:
            continue
        json_str = _OPENCODE_GO_JS_KEYS.sub(r'\1"\2"\3', match.group(1))
        try:
            payload = json.loads(json_str)
        except ValueError:
            continue
        used = _to_percent(payload.get("usagePercent"))
        if used is None:
            continue
        reset_in = payload.get("resetInSec")
        resets_at = (
            int(now + int(reset_in))
            if isinstance(reset_in, (int, float))
            else None
        )
        windows.append(
            {"label": label, "used_percent": used, "resets_at": resets_at}
        )
    return windows


def collect_opencode_go(
    home: Path,
    last_result: dict[str, Any] | None = None,
    now: float | None = None,
    opener: Any = None,
    min_interval: float | None = None,
) -> dict[str, Any]:
    """Collect OpenCode Go usage from the workspace /go page.

    Enforces a minimum interval between fetch attempts (default 300 s,
    matching the opencode-go-usage plugin cache). Failed attempts keep the
    last good reading (see :func:`_carry_last_reading`).

    Args:
        home: The user's home directory.
        last_result: The previous snapshot for this CLI, if any.
        now: Current epoch override (tests).
        opener: ``urllib.request.urlopen``-compatible callable (tests).
        min_interval: Configured seconds between fetch attempts
            (``clis.opencode_go.fetch_min_interval_seconds``).
    """
    now = now if now is not None else time.time()
    if min_interval is None:
        min_interval = OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS
    if (
        last_result
        and last_result.get("checked_at")
        and now - last_result["checked_at"] < min_interval
        and last_result.get("windows")
        and not last_result.get("error")
    ):
        return last_result
    workspace, cookie, error = _opencode_go_credentials(home)
    if workspace is None or cookie is None:
        snapshot = _snapshot("opencode_go_page", error=error)
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    if not re.fullmatch(r"wrk_[a-zA-Z0-9]+", workspace):
        snapshot = _snapshot(
            "opencode_go_page", error="invalid workspace ID format"
        )
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    url = OPENCODE_GO_USAGE_URL.format(workspace_id=workspace)
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": (
                "Mozilla/5.0 (X11; Linux x86_64; rv:148.0) "
                "Gecko/20100101 Firefox/148.0"
            ),
            "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            "Cookie": f"auth={cookie}",
        },
    )
    opener = opener or urllib.request.urlopen
    snapshot: dict[str, Any]
    try:
        with opener(request, timeout=20) as response:
            html = response.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        LOGGER.error("opencode go page HTTP %s", exc.code)
        snapshot = _snapshot("opencode_go_page", error=f"HTTP {exc.code}")
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    except (urllib.error.URLError, OSError, ValueError) as exc:
        LOGGER.error("opencode go page failed: %s", exc)
        snapshot = _snapshot("opencode_go_page", error=str(exc))
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    windows = _opencode_go_usage_from_html(html, now)
    if not windows:
        snapshot = _snapshot(
            "opencode_go_page", error="no usage windows in page response"
        )
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    snapshot = _snapshot(
        "opencode_go_page", observed_at=now, windows=windows
    )
    snapshot["checked_at"] = now
    return snapshot


# ---------------------------------------------------------------------------
# OpenRouter: /api/v1/analytics/query (management API key from installer setup)
# ---------------------------------------------------------------------------


def utc_date_string(now: float) -> str:
    """Return the UTC calendar date for ``now`` as ``YYYY-MM-DD``."""
    return _dt.datetime.fromtimestamp(now, tz=_dt.timezone.utc).strftime("%Y-%m-%d")


def next_utc_midnight_epoch(now: float) -> int:
    """Return the epoch of the next 00:00 UTC boundary after ``now``."""
    current = _dt.datetime.fromtimestamp(now, tz=_dt.timezone.utc)
    next_day = current.date() + _dt.timedelta(days=1)
    midnight = _dt.datetime(
        next_day.year, next_day.month, next_day.day, tzinfo=_dt.timezone.utc
    )
    return int(midnight.timestamp())


def model_matches_slug(item_model: str, model_slug: str) -> bool:
    """Return whether an activity row's model matches the configured slug.

    Honors exact ``model`` equality first, then allows short slugs such as
    ``nemotron-3-ultra`` to match ``nvidia/nemotron-3-ultra-550b-a55b:free``.
    """
    if not item_model or not model_slug:
        return False
    if item_model == model_slug:
        return True
    slug = model_slug.strip()
    model = item_model.strip()
    if slug.lower() in model.lower():
        return True
    base = slug.split(":")[0]
    return model.startswith(base) or base in model


def _activity_row_date(item: dict[str, Any]) -> str:
    """Normalize an activity row date to ``YYYY-MM-DD``."""
    raw = item.get("date")
    if not isinstance(raw, str) or len(raw) < 10:
        return ""
    return raw[:10]


def openrouter_requests_from_analytics(
    rows: list[dict[str, Any]],
    model_slug: str,
) -> int:
    """Sum ``request_count`` analytics rows for ``model_slug``.

    Args:
        rows: Analytics ``data`` rows for one UTC day.
        model_slug: Configured model slug filter.

    Returns:
        Total request count for the day (0 when no rows match).
    """
    total = 0
    for item in rows:
        model = item.get("model") or ""
        if not isinstance(model, str) or not model_matches_slug(model, model_slug):
            continue
        count = item.get("request_count")
        if isinstance(count, int) and count >= 0:
            total += count
        elif isinstance(count, str) and count.isdigit():
            total += int(count)
    return total


def openrouter_window_from_requests(
    requests_today: int,
    now: float,
    daily_limit: int = OPENROUTER_DAILY_LIMIT_DEFAULT,
) -> list[dict[str, Any]]:
    """Build the daily usage window from a request count.

    Args:
        requests_today: Requests consumed on the current UTC day.
        now: Current epoch (reset boundary).
        daily_limit: Configured daily free-request ceiling.

    Returns:
        A single-element window list with ``label`` ``day``.
    """
    if daily_limit <= 0:
        used = 100.0
    else:
        used = _to_percent(requests_today / daily_limit * 100.0) or 0.0
    return [
        {
            "label": "day",
            "used_percent": used,
            "resets_at": next_utc_midnight_epoch(now),
        }
    ]


def openrouter_requests_today(
    payload: dict[str, Any],
    model_slug: str,
    date_str: str,
) -> int:
    """Sum ``requests`` for ``model_slug`` on one UTC activity day.

    Legacy helper for the completed-day ``/api/v1/activity`` payload shape.
    """
    total = 0
    for item in payload.get("data") or []:
        if not isinstance(item, dict):
            continue
        if _activity_row_date(item) != date_str:
            continue
        model = item.get("model") or item.get("model_permaslug") or ""
        if not isinstance(model, str) or not model_matches_slug(model, model_slug):
            continue
        requests = item.get("requests")
        if isinstance(requests, int) and requests >= 0:
            total += requests
    return total


def openrouter_window_from_activity(
    payload: dict[str, Any],
    model_slug: str,
    now: float,
    daily_limit: int = OPENROUTER_DAILY_LIMIT_DEFAULT,
) -> list[dict[str, Any]]:
    """Build the daily usage window from a legacy activity API payload."""
    date_str = utc_date_string(now)
    requests_today = openrouter_requests_today(payload, model_slug, date_str)
    return openrouter_window_from_requests(requests_today, now, daily_limit)


def collect_openrouter(
    home: Path,
    last_result: dict[str, Any] | None = None,
    now: float | None = None,
    fetch_fn: Any = None,
    min_interval: float | None = None,
    model_slug: str | None = None,
    daily_limit: int | None = None,
) -> dict[str, Any]:
    """Collect OpenRouter free-model daily usage from the activity API.

    Uses the management API key saved during installer setup. Fetch attempts
    are throttled (default 60 s). Failed attempts keep the last good reading
    (see :func:`_carry_last_reading`).

    Args:
        home: The user's home directory.
        last_result: The previous snapshot for this CLI, if any.
        now: Current epoch override (tests).
        fetch_fn: ``(api_key, date_str, model_slug) -> rows`` override (tests).
        min_interval: Configured seconds between fetch attempts.
        model_slug: Override for ``clis.openrouter.model_slug``.
        daily_limit: Override for ``clis.openrouter.daily_limit``.
    """
    now = now if now is not None else time.time()
    if min_interval is None:
        min_interval = OPENROUTER_FETCH_MIN_INTERVAL_SECONDS
    if (
        last_result
        and last_result.get("checked_at")
        and now - last_result["checked_at"] < min_interval
    ):
        return last_result
    slug = (model_slug or OPENROUTER_MODEL_SLUG_DEFAULT).strip()
    limit = daily_limit if daily_limit is not None else OPENROUTER_DAILY_LIMIT_DEFAULT
    api_key = management_api_key(home)
    if not api_key:
        snapshot = _snapshot(
            "openrouter_activity",
            error=(
                "OpenRouter management API key unavailable; "
                "run openrouter_monitor configure"
            ),
        )
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    date_str = utc_date_string(now)
    fetch = fetch_fn or (
        lambda key, day, model: fetch_analytics_rows(key, day, model)
    )
    snapshot: dict[str, Any]
    try:
        rows = fetch(api_key, date_str, slug)
    except (
        OSError,
        RuntimeError,
        FileNotFoundError,
        ModuleNotFoundError,
        ImportError,
        json.JSONDecodeError,
    ) as exc:
        LOGGER.error("openrouter analytics fetch failed: %s", exc)
        snapshot = _snapshot("openrouter_activity", error=str(exc))
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    requests_today = openrouter_requests_from_analytics(rows, slug)
    windows = openrouter_window_from_requests(requests_today, now, limit)
    if not windows:
        snapshot = _snapshot(
            "openrouter_activity",
            error="no daily usage window from activity response",
        )
        snapshot["checked_at"] = now
        return _carry_last_reading(snapshot, last_result)
    snapshot = _snapshot(
        "openrouter_activity",
        observed_at=now,
        plan=slug,
        windows=windows,
    )
    snapshot["checked_at"] = now
    return snapshot


# ---------------------------------------------------------------------------
# tmux pane fallback: parse rendered /status | /usage TUI text
# ---------------------------------------------------------------------------

#: Per-CLI window keywords used to find "<window label> … NN% left|used"
#: lines in the rendered TUI output. Grouped as (label, keyword regex).
_PANE_WINDOW_KEYWORDS: dict[str, list[tuple[str, re.Pattern[str]]]] = {
    "codex": [
        ("5h", re.compile(r"5h(?:our)?\s+limit", re.I)),
        ("week", re.compile(r"week(?:ly)?\s+limit", re.I)),
        # Inactive / free-tier accounts show a monthly quota instead of 5h+week
        # (live "/status": "Monthly limit: … NN% left"). Keep after 5h/week so
        # subscribed panes still publish the short windows when all three appear.
        ("month", re.compile(r"month(?:ly)?\s+limit", re.I)),
    ],
    "claude_code": [
        ("5h", re.compile(r"current\s+session|5-?h(?:our)?\s+limit", re.I)),
        ("week", re.compile(r"current\s+week|week(?:ly)?\s+limit", re.I)),
        ("fable", re.compile(r"(?:current\s+week[^\n]*\b)?fable(?:\s+only)?", re.I)),
    ],
    "grok": [
        ("week", re.compile(r"week(?:ly)?\s+limit|credits?|usage", re.I)),
    ],
    "opencode_go": [
        ("rolling", re.compile(r"\brolling\b", re.I)),
        ("week", re.compile(r"\bweekly\b", re.I)),
        ("month", re.compile(r"\bmonthly\b", re.I)),
    ],
}

_PANE_PERCENT = re.compile(r"(\d+(?:\.\d+)?)\s*%")
#: Codex /usage allowance for manually resetting a rolling limit.
_CODEX_RESETS_AVAILABLE = re.compile(
    r"\b(\d+)\s+(?:usage\s+limit\s+)?resets?\s+available\b", re.I
)

#: A rendered "resets …" phrase and everything after it on the line. The reset
#: descriptor (clock, optional day) is pulled out of the trailing text.
_PANE_RESET = re.compile(r"\bresets?\b\s*(?:at\s+)?(.+)$", re.I)
#: A "HH:MM" 24-hour clock inside a reset descriptor.
_PANE_RESET_CLOCK = re.compile(r"\b([01]?\d|2[0-3]):([0-5]\d)\b")
#: A "D Mon" calendar day inside a reset descriptor (e.g. "9 Jul").
_PANE_RESET_DATE = re.compile(r"\b(\d{1,2})\s+([A-Za-z]{3})", re.I)
#: OpenCode Go pane: "resets in 4h 57m", "resets in 2d 18h", "resets in 59m".
_PANE_RESET_IN = re.compile(
    r"\bresets?\s+in\s+"
    r"(?:(\d+)d\s*)?"
    r"(?:(\d+)h\s*)?"
    r"(?:(\d+)m)?",
    re.I,
)
#: Grok /usage pane: "(period ends Jul 9)", "period ends in 2d 5h".
_PANE_PERIOD_END = re.compile(r"\bperiod\s+ends?\b\s*(.+)$", re.I)

#: Three-letter month abbreviation -> month number (1-12).
_MONTH_ABBR = {
    name.lower(): number
    for number, name in enumerate(
        ("Jan", "Feb", "Mar", "Apr", "May", "Jun",
         "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"),
        start=1,
    )
}


def _pane_period_end_epoch(line: str, now: float) -> int | None:
    """Convert a Grok "period ends …" phrase into an absolute epoch."""
    match = _PANE_PERIOD_END.search(line)
    if not match:
        return None
    descriptor = match.group(1).strip().rstrip(")")
    duration = _pane_reset_duration_epoch(f"resets {descriptor}", now)
    if duration is not None:
        return duration
    day_month = re.search(r"\b(\d{1,2})\s+([A-Za-z]{3})\b", descriptor, re.I)
    if day_month:
        day, month_abbr = int(day_month.group(1)), day_month.group(2)
    else:
        month_day = re.search(r"\b([A-Za-z]{3})\s+(\d{1,2})\b", descriptor, re.I)
        if not month_day:
            return None
        month_abbr, day = month_day.group(1), int(month_day.group(2))
    month = _MONTH_ABBR.get(month_abbr.lower())
    if month is None:
        return None
    base = _dt.datetime.fromtimestamp(now).astimezone()
    try:
        candidate = base.replace(
            month=month, day=day, hour=23, minute=59, second=59, microsecond=0,
        )
    except ValueError:
        return None
    if candidate.timestamp() < now - 86400:
        try:
            candidate = candidate.replace(year=candidate.year + 1)
        except ValueError:
            return None
    return int(candidate.timestamp())


def _pane_reset_duration_epoch(line: str, now: float) -> int | None:
    """Convert an OpenCode Go "resets in …" duration phrase into an epoch."""
    match = _PANE_RESET_IN.search(line)
    if not match or not any(match.groups()):
        return None
    days = int(match.group(1) or 0)
    hours = int(match.group(2) or 0)
    minutes = int(match.group(3) or 0)
    total_seconds = days * 86400 + hours * 3600 + minutes * 60
    if total_seconds <= 0:
        return None
    return int(now + total_seconds)


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
            epoch = _pane_period_end_epoch(lines[probe], now)
            if epoch is None:
                epoch = _pane_reset_duration_epoch(lines[probe], now)
            if epoch is None:
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
        cli_id: One of ``codex``, ``claude_code``, ``grok``, ``opencode_go``.
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
    snapshot = _snapshot(
        "tmux_pane",
        observed_at=now if windows else None,
        windows=windows,
        error=None if windows else "no usage percentages found in pane",
    )
    if cli_id == "codex":
        resets = _CODEX_RESETS_AVAILABLE.search(text)
        # Codex 0.142 changed /usage from the reset menu to an activity
        # heatmap, which carries no allowance metadata. Absence is therefore
        # unknown, not an authoritative zero; publish only an explicit count
        # so the poller can retain the last observed allowance.
        if resets:
            snapshot["resets_available"] = int(resets.group(1))
    return snapshot
