#!/usr/bin/env python3
"""Centralized logging for the agentic CLI status poller.

Every Python module in this repository imports ``LOGGER``, ``get_logger``, and
``log_call`` from here. The sink is environment-aware:

- ``ACS_LOG_DIR`` env var, when set, forces a file sink in that directory
  (used by tests and by developers).
- A repository checkout (a ``.git`` sibling of this package) logs to the
  repo-root ``.log/`` directory, which is gitignored.
- An installed copy (e.g. ``~/.local/share/agentic-cli-status``) logs to
  stderr so the systemd user service journal is the canonical sink.

Logging is best-effort and never aborts the program.
"""

from __future__ import annotations

import functools
import inspect
import logging
import os
import sys
from pathlib import Path
from typing import Any, Callable, TypeVar

LOG_FORMAT = "%(asctime)s %(levelname)s [%(name)s] %(message)s"
DEFAULT_LOG_FILE = "agentic-cli-status.log"
F = TypeVar("F", bound=Callable[..., Any])


def _log_dir() -> Path | None:
    """Return the directory for the file sink, or None for a stderr sink.

    Returns:
        The ``ACS_LOG_DIR`` override, the repo-root ``.log/`` when this module
        runs from a git checkout, or ``None`` when the installed copy should
        log to stderr (journald).
    """
    override = os.environ.get("ACS_LOG_DIR")
    if override:
        return Path(override)
    repo_root = Path(__file__).resolve().parents[1]
    if (repo_root / ".git").exists():
        return repo_root / ".log"
    return None


def get_logger(name: str, log_file: str = DEFAULT_LOG_FILE) -> logging.Logger:
    """Return an idempotent project logger with an environment-aware sink.

    Args:
        name: Logger name shown in every record.
        log_file: File name used when a file sink is selected.

    Returns:
        A configured logger. If the file sink cannot be created, the logger
        falls back to stderr so callers never crash on logging.
    """
    logger = logging.getLogger(name)
    logger.setLevel(logging.DEBUG)
    logger.propagate = False
    if logger.handlers:
        return logger
    handler: logging.Handler
    log_dir = _log_dir()
    if log_dir is not None:
        try:
            log_dir.mkdir(parents=True, exist_ok=True)
            handler = logging.FileHandler(log_dir / log_file, encoding="utf-8")
        except OSError:
            handler = logging.StreamHandler(sys.stderr)
    else:
        handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter(LOG_FORMAT))
    logger.addHandler(handler)
    return logger


def log_call(logger: logging.Logger, level: int = logging.DEBUG) -> Callable[[F], F]:
    """Trace a function by logging every argument on entry and the return value.

    Each record carries the wrapped function's ``file:qualname`` so logs are
    searchable without manual context.

    Args:
        logger: Centralized logger obtained from :func:`get_logger`.
        level: Logging level for the entry and exit records.

    Returns:
        A decorator that wraps the target callable.
    """

    def decorator(func: F) -> F:
        location = f"{Path(func.__code__.co_filename).name}:{func.__qualname__}"
        signature = inspect.signature(func)

        @functools.wraps(func)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            bound = signature.bind(*args, **kwargs)
            bound.apply_defaults()
            rendered = ", ".join(
                f"{name}={value!r}" for name, value in bound.arguments.items()
            )
            logger.log(level, "ENTER %s(%s)", location, rendered)
            result = func(*args, **kwargs)
            logger.log(level, "EXIT %s -> %r", location, result)
            return result

        return wrapper  # type: ignore[return-value]

    return decorator


LOGGER = get_logger("agentic-cli-status", DEFAULT_LOG_FILE)
