"""Deployment contract: the installer copy list matches the Python package.

The installed poller imports only what lib/status_setup.sh copies, so a module
added to the package but not to ACS_PACKAGE_FILES would crash the installed
service on a missing import. This test keeps the two in lockstep.
"""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def _copy_list() -> set[str]:
    """Parse the ACS_PACKAGE_FILES array from lib/status_setup.sh."""
    text = (REPO_ROOT / "lib" / "status_setup.sh").read_text(encoding="utf-8")
    match = re.search(r"ACS_PACKAGE_FILES=\((.*?)\)", text, re.S)
    assert match, "ACS_PACKAGE_FILES array not found in lib/status_setup.sh"
    return set(re.findall(r'"([^"]+)"', match.group(1)))


def test_copy_list_matches_package_files():
    package_files = {
        p.name for p in (REPO_ROOT / "agentic_cli_status").glob("*.py")
    }
    assert _copy_list() == package_files


def test_systemd_unit_starts_the_packaged_module():
    unit = (REPO_ROOT / "systemd" / "agentic-cli-status.service").read_text(
        encoding="utf-8"
    )
    assert "agentic_cli_status.poller --daemon" in unit
    assert "PYTHONPATH=%h/.local/share/agentic-cli-status" in unit
    assert "WantedBy=default.target" in unit
