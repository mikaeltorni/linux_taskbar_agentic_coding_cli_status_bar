#!/usr/bin/env python3
"""OpenRouter analytics client using a management API key.

The poller calls ``POST /api/v1/analytics/query`` with ``Authorization:
Bearer`` to read daily request counts (including the current UTC day, which
the legacy ``/api/v1/activity`` endpoint omits until the day completes). This
never invokes chat/completions, so monitoring does not consume free-model
request quota. Keys are read from ``OPENROUTER_MANAGEMENT_API_KEY`` in the
process environment or ``~/.config/agentic-cli-status/.env`` (mode 600).
"""

from __future__ import annotations

import datetime as _dt
import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Callable

from .acs_logging import LOGGER

OPENROUTER_ANALYTICS_QUERY_API = "https://openrouter.ai/api/v1/analytics/query"
ENV_VAR_NAME = "OPENROUTER_MANAGEMENT_API_KEY"
ENV_FILE_NAME = ".env"
CONFIG_DIR_NAME = "agentic-cli-status"


def env_file_path(home: Path | None = None) -> Path:
    """Return the OpenRouter secrets ``.env`` path under XDG config."""
    base = os.environ.get("XDG_CONFIG_HOME")
    root = Path(base) if base else (home or Path.home()) / ".config"
    return root / CONFIG_DIR_NAME / ENV_FILE_NAME


def _parse_env_file(path: Path) -> dict[str, str]:
    """Parse ``KEY=VALUE`` lines from a dotenv-style file."""
    values: dict[str, str] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return values
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, _, value = stripped.partition("=")
        key = key.strip()
        if not key:
            continue
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1]
        values[key] = value
    return values


def management_api_key(home: Path | None = None) -> str | None:
    """Return the configured management API key, if any."""
    override = os.environ.get(ENV_VAR_NAME)
    if override and override.strip():
        return override.strip()
    path = env_file_path(home)
    if not path.is_file():
        return None
    value = _parse_env_file(path).get(ENV_VAR_NAME)
    if value and value.strip():
        return value.strip()
    return None


def analytics_day_range(date_str: str) -> tuple[str, str]:
    """Return inclusive-exclusive UTC ISO bounds for one calendar day."""
    start = _dt.datetime.strptime(date_str, "%Y-%m-%d").replace(
        tzinfo=_dt.timezone.utc
    )
    end = start + _dt.timedelta(days=1)
    fmt = "%Y-%m-%dT%H:%M:%SZ"
    return start.strftime(fmt), end.strftime(fmt)


def fetch_analytics_rows(
    api_key: str,
    date_str: str,
    model_slug: str,
    timeout: float = 60.0,
    opener: Callable[..., Any] | None = None,
) -> list[dict[str, Any]]:
    """Fetch per-model request rows for one UTC day from the analytics API.

    Args:
        api_key: OpenRouter management API key (read-only analytics auth).
        date_str: UTC calendar day (``YYYY-MM-DD``).
        model_slug: Configured slug; ``:free`` suffix adds a ``variant=free``
            filter so paid traffic on the same model is excluded.
        timeout: Request timeout in seconds.
        opener: ``urllib.request.urlopen``-compatible callable (tests).

    Returns:
        Analytics rows with ``model``, optional ``variant``, and
        ``request_count``.

    Raises:
        RuntimeError: On HTTP errors or malformed JSON.
    """
    start, end = analytics_day_range(date_str)
    body: dict[str, Any] = {
        "metrics": ["request_count"],
        "dimensions": ["model", "variant"],
        "time_range": {"start": start, "end": end},
        "limit": 100,
    }
    if model_slug.strip().endswith(":free"):
        body["filters"] = [{"field": "variant", "operator": "eq", "value": "free"}]
    request = urllib.request.Request(
        OPENROUTER_ANALYTICS_QUERY_API,
        data=json.dumps(body).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Accept": "application/json",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    opener = opener or urllib.request.urlopen
    LOGGER.info("fetching OpenRouter analytics for %s (management key)", date_str)
    try:
        with opener(request, timeout=timeout) as response:
            status_code = response.status
            body_text = response.read().decode("utf-8", errors="replace")
    except urllib.error.HTTPError as exc:
        status_code = exc.code
        body_text = exc.read().decode("utf-8", errors="replace")
    except (urllib.error.URLError, OSError, ValueError) as exc:
        raise RuntimeError(f"OpenRouter analytics request failed: {exc}") from exc
    if status_code >= 400:
        LOGGER.error("OpenRouter analytics HTTP %s: %s", status_code, body_text[:200])
        raise RuntimeError(f"OpenRouter analytics HTTP {status_code}")
    try:
        payload = json.loads(body_text)
    except json.JSONDecodeError as exc:
        raise RuntimeError("OpenRouter analytics response is not JSON") from exc
    if not isinstance(payload, dict):
        raise RuntimeError("OpenRouter analytics response root is not an object")
    data = payload.get("data")
    if not isinstance(data, dict):
        raise RuntimeError("OpenRouter analytics response missing data object")
    rows = data.get("data")
    if not isinstance(rows, list):
        raise RuntimeError("OpenRouter analytics response missing data rows")
    return [row for row in rows if isinstance(row, dict)]