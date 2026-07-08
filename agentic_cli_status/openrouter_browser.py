#!/usr/bin/env python3
"""Playwright browser I/O for OpenRouter session setup and activity fetches.

Setup opens a headed browser so the user can log in at
``https://openrouter.ai/activity``, then persists the authenticated storage
state (cookies) for headless reuse. The poller loads that state and calls the
same-origin ``/api/v1/activity`` endpoint through Playwright's request
context so Cloudflare and Clerk cookie auth stay intact.

Playwright is installed into ``~/.local/share/agentic-cli-status/venv`` by
the installer; :func:`_sync_playwright` imports it from that venv's
site-packages so the host ``python3`` poller does not need a global install.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Any, Callable

from .acs_logging import LOGGER

OPENROUTER_ACTIVITY_PAGE = "https://openrouter.ai/activity"
OPENROUTER_ACTIVITY_API = "https://openrouter.ai/api/v1/activity"
STORAGE_FILE_NAME = "openrouter-storage.json"
VENV_DIR_NAME = "venv"
APP_DIR_REL = ".local/share/agentic-cli-status"


def storage_state_path(home: Path | None = None) -> Path:
    """Return the Playwright storage-state file path under XDG config."""
    base = os.environ.get("XDG_CONFIG_HOME")
    root = Path(base) if base else (home or Path.home()) / ".config"
    return root / "agentic-cli-status" / STORAGE_FILE_NAME


def playwright_python(home: Path | None = None) -> str:
    """Return the Python executable that has Playwright installed."""
    override = os.environ.get("ACS_PLAYWRIGHT_PYTHON")
    if override:
        return override
    home = home or Path.home()
    venv_python = home / APP_DIR_REL / VENV_DIR_NAME / "bin" / "python"
    if venv_python.is_file():
        return str(venv_python)
    return sys.executable


def _playwright_site_packages(home: Path | None = None) -> Path | None:
    """Return the ACS venv site-packages directory when Playwright lives there."""
    home = home or Path.home()
    venv_lib = home / APP_DIR_REL / VENV_DIR_NAME / "lib"
    if not venv_lib.is_dir():
        return None
    for entry in sorted(venv_lib.iterdir()):
        if entry.name.startswith("python"):
            site = entry / "site-packages"
            if site.is_dir():
                return site
    return None


def _sync_playwright(home: Path | None = None) -> Callable[..., Any]:
    """Import ``sync_playwright`` from the ACS Playwright venv.

    Args:
        home: Target home directory (poller/installer).

    Raises:
        RuntimeError: When Playwright is not installed in the ACS venv.
    """
    site = _playwright_site_packages(home)
    if site is not None:
        site_str = str(site)
        if site_str not in sys.path:
            sys.path.insert(0, site_str)
    try:
        from playwright.sync_api import sync_playwright
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "Playwright is not installed; run: "
            "bash install.sh --configure-component openrouter_monitor"
        ) from exc
    return sync_playwright


def ensure_playwright(home: Path | None = None) -> None:
    """Install Playwright and Chromium into the ACS venv when missing.

    Args:
        home: Target home directory (installer/tests).
    """
    import subprocess

    home = home or Path.home()
    venv_dir = home / APP_DIR_REL / VENV_DIR_NAME
    python = Path(playwright_python(home))
    if not python.is_file():
        LOGGER.info("creating Playwright venv at %s", venv_dir)
        venv_dir.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [sys.executable, "-m", "venv", str(venv_dir)],
            check=True,
            capture_output=True,
            text=True,
        )
        python = venv_dir / "bin" / "python"
    try:
        subprocess.run(
            [str(python), "-c", "from playwright.sync_api import sync_playwright"],
            check=True,
            capture_output=True,
            text=True,
        )
    except subprocess.CalledProcessError:
        LOGGER.info("installing playwright into %s", venv_dir)
        subprocess.run(
            [str(python), "-m", "pip", "install", "playwright"],
            check=True,
            capture_output=True,
            text=True,
        )
    subprocess.run(
        [str(python), "-m", "playwright", "install", "chromium"],
        check=True,
        capture_output=True,
        text=True,
    )


def run_headed_login(
    storage_path: Path | None = None,
    login_url: str = OPENROUTER_ACTIVITY_PAGE,
    confirm: Any | None = None,
    home: Path | None = None,
) -> None:
    """Open a visible browser for OpenRouter login and save the session.

    Navigates to ``login_url``, waits for the user to finish signing in, then
    blocks on ``confirm`` (defaults to reading a line from stdin) before
    writing the Playwright storage state to ``storage_path``.

    Args:
        storage_path: Destination JSON file (mode 600 after write).
        login_url: Page that requires authentication.
        confirm: Callable invoked after the browser opens; return when login
            is complete. Tests inject a no-op; the installer passes a console
            prompt.
        home: Target home directory for Playwright venv resolution.
    """
    sync_playwright = _sync_playwright(home)
    target = storage_path or storage_state_path(home)
    target.parent.mkdir(parents=True, exist_ok=True)
    LOGGER.info("opening headed browser for OpenRouter login at %s", login_url)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=False)
        context = browser.new_context()
        page = context.new_page()
        page.goto(login_url, wait_until="domcontentloaded", timeout=120_000)
        if confirm is None:
            print(
                "\nLog in to OpenRouter in the browser window, then press Enter here.",
                flush=True,
            )
            confirm = lambda: input()
        confirm()
        context.storage_state(path=str(target))
        browser.close()
    target.chmod(0o600)
    LOGGER.info("saved OpenRouter session to %s", target)


def fetch_activity_payload(
    storage_path: Path,
    date_str: str,
    headless: bool = True,
    timeout_ms: int = 60_000,
    home: Path | None = None,
) -> dict[str, Any]:
    """Fetch ``/api/v1/activity`` for one UTC date using a saved session.

    Args:
        storage_path: Playwright storage-state JSON from :func:`run_headed_login`.
        date_str: UTC calendar day (``YYYY-MM-DD``).
        headless: When ``False``, runs a visible browser (debug only).
        timeout_ms: Request timeout in milliseconds.
        home: Target home directory for Playwright venv resolution.

    Returns:
        Parsed JSON body from the activity API.

    Raises:
        FileNotFoundError: When ``storage_path`` is absent.
        RuntimeError: On HTTP errors, missing Playwright, or malformed JSON.
    """
    if not storage_path.is_file():
        raise FileNotFoundError(f"OpenRouter session not found at {storage_path}")
    sync_playwright = _sync_playwright(home)

    url = f"{OPENROUTER_ACTIVITY_API}?date={date_str}"
    LOGGER.info("fetching OpenRouter activity for %s (headless=%s)", date_str, headless)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=headless)
        try:
            context = browser.new_context(storage_state=str(storage_path))
            response = context.request.get(url, timeout=timeout_ms)
            status_code = response.status
            body_text = response.text()
        finally:
            browser.close()
    if status_code >= 400:
        LOGGER.error("OpenRouter activity HTTP %s: %s", status_code, body_text[:200])
        raise RuntimeError(f"OpenRouter activity HTTP {status_code}")
    try:
        payload = json.loads(body_text)
    except json.JSONDecodeError as exc:
        raise RuntimeError("OpenRouter activity response is not JSON") from exc
    if not isinstance(payload, dict):
        raise RuntimeError("OpenRouter activity response root is not an object")
    return payload