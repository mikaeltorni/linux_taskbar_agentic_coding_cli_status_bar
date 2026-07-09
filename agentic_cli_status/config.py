#!/usr/bin/env python3
"""Configuration for the agentic CLI status poller.

The single JSON config file lives at
``${XDG_CONFIG_HOME:-~/.config}/agentic-cli-status/config.json`` and is owned
by the installer components (each per-CLI component and the polling-rate
component edit individual keys). The poller re-reads it on every tick so
reconfiguration never requires a restart.

Every CLI entry supports the same option set so each CLI is independently
reconfigurable:

- ``enabled``: whether this CLI is monitored at all.
- ``source``: ``auto`` (headless first, tmux pane as fallback/refresh),
  ``headless`` (never touch tmux), or ``tmux`` (pane capture only).
- ``stale_after_seconds``: age after which headless data counts as stale and,
  in ``auto`` mode, a tmux slash-command refresh is attempted.

``claude_code`` additionally supports ``endpoint_min_interval_seconds`` — how
often the OAuth usage endpoint may be called (60-3600 s, default 180): the
endpoint enforces its own small call budget with 429 lockouts, so it cannot
follow second-scale polling rates.
"""

from __future__ import annotations

import copy
import json
import logging
import os
from pathlib import Path
from typing import Any

from .acs_logging import LOGGER

CONFIG_DIR_NAME = "agentic-cli-status"
CONFIG_FILE_NAME = "config.json"

#: CLI ids in display order. Cline is intentionally absent: it has no usage /
#: rate-limit surface yet (no /usage equivalent).
CLI_IDS = ("codex", "claude_code", "grok", "opencode_go", "openrouter")

#: The slash command each CLI's TUI uses to display its rate limits/usage.
SLASH_COMMANDS = {
    "codex": "/usage",
    "claude_code": "/usage",
    "grok": "/usage",
    "opencode_go": "/ogc-usage",
}

#: Polling rate bounds, in SECONDS. Local file reads are cheap, so the floor
#: is low; slower sources protect themselves (the Claude endpoint keeps its
#: own >=180s floor and tmux refreshes are rate-limited separately).
MIN_POLL_INTERVAL_SECONDS = 5
MAX_POLL_INTERVAL_SECONDS = 3600

#: Floor for the per-CLI staleness threshold (independent of the poll rate).
MIN_STALE_AFTER_SECONDS = 60

#: The Claude OAuth usage endpoint enforces a small call budget (measured
#: 2026-07-03: a short burst is allowed, then HTTP 429 with a ~300 s
#: Retry-After lockout), so its check interval is bounded independently of
#: the polling rate. The default is the proven-safe 180 s; the floor is the
#: lowest value that stays near the observed budget. The effective value is
#: the install-configurable ``clis.claude_code.endpoint_min_interval_seconds``.
CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS = 180
MIN_CLAUDE_ENDPOINT_INTERVAL_SECONDS = 60

#: OpenCode Go fetches opencode.ai HTML on a 5-minute cache (matching the
#: opencode-go-usage plugin); the poller honors the same floor independently
#: of the second-scale poll tick.
OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS = 300
MIN_OPENCODE_GO_FETCH_INTERVAL_SECONDS = 300

#: OpenRouter activity API fetches are throttled independently of the poll
#: tick so second-scale polling does not hammer the site.
OPENROUTER_FETCH_MIN_INTERVAL_SECONDS = 60
MIN_OPENROUTER_FETCH_INTERVAL_SECONDS = 60

#: Default free-model daily request ceiling for the configured model slug.
OPENROUTER_DAILY_LIMIT_DEFAULT = 1000
MIN_OPENROUTER_DAILY_LIMIT = 1
MAX_OPENROUTER_DAILY_LIMIT = 100_000

#: Default model slug (Tencent Hy3 free tier).
OPENROUTER_MODEL_SLUG_DEFAULT = "tencent/hy3:free"

DEFAULT_CONFIG: dict[str, Any] = {
    "poll_interval_seconds": 5,
    "tmux": {
        "enabled": True,
        "session": "agentic-cli-status",
    },
    "clis": {
        "codex": {
            "enabled": True,
            "source": "auto",
            "stale_after_seconds": 1800,
        },
        "claude_code": {
            "enabled": True,
            "source": "auto",
            "stale_after_seconds": 1800,
            "endpoint_min_interval_seconds": CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS,
        },
        "grok": {
            "enabled": True,
            "source": "auto",
            "stale_after_seconds": 1800,
        },
        "opencode_go": {
            "enabled": True,
            "source": "auto",
            "stale_after_seconds": 1800,
            "fetch_min_interval_seconds": OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS,
        },
        "openrouter": {
            "enabled": True,
            "source": "headless",
            "stale_after_seconds": 1800,
            "fetch_min_interval_seconds": OPENROUTER_FETCH_MIN_INTERVAL_SECONDS,
            "model_slug": OPENROUTER_MODEL_SLUG_DEFAULT,
            "daily_limit": OPENROUTER_DAILY_LIMIT_DEFAULT,
        },
    },
}

VALID_SOURCES = ("auto", "headless", "tmux")


def config_path(home: Path | None = None) -> Path:
    """Return the config file path, honoring ``XDG_CONFIG_HOME``.

    Args:
        home: Optional home directory override (used by tests).
    """
    base = os.environ.get("XDG_CONFIG_HOME")
    root = Path(base) if base else (home or Path.home()) / ".config"
    return root / CONFIG_DIR_NAME / CONFIG_FILE_NAME


def state_dir(home: Path | None = None) -> Path:
    """Return the state directory, honoring ``XDG_STATE_HOME``.

    Args:
        home: Optional home directory override (used by tests).
    """
    base = os.environ.get("XDG_STATE_HOME")
    root = Path(base) if base else (home or Path.home()) / ".local" / "state"
    return root / CONFIG_DIR_NAME


def _merge_defaults(base: dict[str, Any], override: dict[str, Any]) -> dict[str, Any]:
    """Recursively overlay ``override`` on a deep copy of ``base``."""
    merged = copy.deepcopy(base)
    for key, value in override.items():
        if isinstance(value, dict) and isinstance(merged.get(key), dict):
            merged[key] = _merge_defaults(merged[key], value)
        else:
            merged[key] = value
    return merged


def load_config(path: Path | None = None) -> dict[str, Any]:
    """Load the config file merged over defaults, validating key ranges.

    Missing or unreadable files yield the defaults so the poller and the
    extension always have a complete configuration to work with.

    Args:
        path: Optional explicit config file path (defaults to
            :func:`config_path`).

    Returns:
        The effective configuration dictionary.
    """
    target = path or config_path()
    raw: dict[str, Any] = {}
    try:
        raw = json.loads(target.read_text(encoding="utf-8"))
        if not isinstance(raw, dict):
            raise ValueError("config root is not an object")
    except FileNotFoundError:
        LOGGER.info("config not found at %s; using defaults", target)
    except (OSError, ValueError) as exc:
        LOGGER.error("unreadable config %s (%s); using defaults", target, exc)
        raw = {}
    config = _merge_defaults(DEFAULT_CONFIG, raw)

    interval = config.get("poll_interval_seconds")
    if not isinstance(interval, int) or not (
        MIN_POLL_INTERVAL_SECONDS <= interval <= MAX_POLL_INTERVAL_SECONDS
    ):
        LOGGER.warning(
            "poll_interval_seconds %r out of range [%d, %d]; using default %d",
            interval,
            MIN_POLL_INTERVAL_SECONDS,
            MAX_POLL_INTERVAL_SECONDS,
            DEFAULT_CONFIG["poll_interval_seconds"],
        )
        config["poll_interval_seconds"] = DEFAULT_CONFIG["poll_interval_seconds"]

    for cli_id in CLI_IDS:
        cli = config["clis"].setdefault(cli_id, {})
        defaults = DEFAULT_CONFIG["clis"][cli_id]
        if cli_id == "openrouter":
            if cli.get("source") != "headless":
                LOGGER.warning(
                    "cli %s only supports headless collection; forcing headless",
                    cli_id,
                )
                cli["source"] = "headless"
        elif cli.get("source") not in VALID_SOURCES:
            LOGGER.warning(
                "cli %s has invalid source %r; using %r",
                cli_id,
                cli.get("source"),
                defaults["source"],
            )
            cli["source"] = defaults["source"]
        stale = cli.get("stale_after_seconds")
        if not isinstance(stale, int) or stale < MIN_STALE_AFTER_SECONDS:
            cli["stale_after_seconds"] = defaults["stale_after_seconds"]
        cli["enabled"] = bool(cli.get("enabled", defaults["enabled"]))

    claude = config["clis"]["claude_code"]
    endpoint_interval = claude.get("endpoint_min_interval_seconds")
    if not isinstance(endpoint_interval, int) or not (
        MIN_CLAUDE_ENDPOINT_INTERVAL_SECONDS
        <= endpoint_interval
        <= MAX_POLL_INTERVAL_SECONDS
    ):
        LOGGER.warning(
            "claude endpoint_min_interval_seconds %r out of range [%d, %d]; "
            "using default %d",
            endpoint_interval,
            MIN_CLAUDE_ENDPOINT_INTERVAL_SECONDS,
            MAX_POLL_INTERVAL_SECONDS,
            CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS,
        )
        claude["endpoint_min_interval_seconds"] = (
            CLAUDE_ENDPOINT_MIN_INTERVAL_SECONDS
        )
    opencode = config["clis"]["opencode_go"]
    fetch_interval = opencode.get("fetch_min_interval_seconds")
    if not isinstance(fetch_interval, int) or not (
        MIN_OPENCODE_GO_FETCH_INTERVAL_SECONDS
        <= fetch_interval
        <= MAX_POLL_INTERVAL_SECONDS
    ):
        LOGGER.warning(
            "opencode_go fetch_min_interval_seconds %r out of range [%d, %d]; "
            "using default %d",
            fetch_interval,
            MIN_OPENCODE_GO_FETCH_INTERVAL_SECONDS,
            MAX_POLL_INTERVAL_SECONDS,
            OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS,
        )
        opencode["fetch_min_interval_seconds"] = (
            OPENCODE_GO_FETCH_MIN_INTERVAL_SECONDS
        )
    openrouter = config["clis"]["openrouter"]
    or_fetch = openrouter.get("fetch_min_interval_seconds")
    if not isinstance(or_fetch, int) or not (
        MIN_OPENROUTER_FETCH_INTERVAL_SECONDS
        <= or_fetch
        <= MAX_POLL_INTERVAL_SECONDS
    ):
        LOGGER.warning(
            "openrouter fetch_min_interval_seconds %r out of range [%d, %d]; "
            "using default %d",
            or_fetch,
            MIN_OPENROUTER_FETCH_INTERVAL_SECONDS,
            MAX_POLL_INTERVAL_SECONDS,
            OPENROUTER_FETCH_MIN_INTERVAL_SECONDS,
        )
        openrouter["fetch_min_interval_seconds"] = (
            OPENROUTER_FETCH_MIN_INTERVAL_SECONDS
        )
    daily_limit = openrouter.get("daily_limit")
    if not isinstance(daily_limit, int) or not (
        MIN_OPENROUTER_DAILY_LIMIT <= daily_limit <= MAX_OPENROUTER_DAILY_LIMIT
    ):
        LOGGER.warning(
            "openrouter daily_limit %r out of range [%d, %d]; using default %d",
            daily_limit,
            MIN_OPENROUTER_DAILY_LIMIT,
            MAX_OPENROUTER_DAILY_LIMIT,
            OPENROUTER_DAILY_LIMIT_DEFAULT,
        )
        openrouter["daily_limit"] = OPENROUTER_DAILY_LIMIT_DEFAULT
    model_slug = openrouter.get("model_slug")
    if not isinstance(model_slug, str) or not model_slug.strip():
        openrouter["model_slug"] = OPENROUTER_MODEL_SLUG_DEFAULT
    else:
        openrouter["model_slug"] = model_slug.strip()
    return config


def save_config(config: dict[str, Any], path: Path | None = None) -> None:
    """Atomically write ``config`` as pretty JSON to the config file.

    Args:
        config: Full configuration dictionary to persist.
        path: Optional explicit config file path.
    """
    target = path or config_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp = target.with_suffix(".tmp")
    tmp.write_text(json.dumps(config, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, target)
    LOGGER.log(logging.INFO, "config saved to %s", target)
