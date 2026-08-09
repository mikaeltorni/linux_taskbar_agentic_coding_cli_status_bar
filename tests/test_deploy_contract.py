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
    assert "EnvironmentFile=-%h/.config/agentic-cli-status/.env" in unit
    assert "WantedBy=default.target" in unit


# --- Read-only installer contract -------------------------------------------
#
# The master installer (installation_scripts/scripts/component_selection.sh)
# decides which nested screens to surface with _cs_repo_exposes_flag(), which
# grants the full contract to any repo delegating to isc_route_readonly_args.
# A hand-maintained flag list previously omitted the configure flags, hiding
# this repo's six configure screens from the aggregate menu and, when such a
# flag was passed anyway, dropping through to main() -- printing the banner on
# stdout and deploying the core as a side effect of a query.

READONLY_FLAGS = (
    "--list-components",
    "--list-configurable-components",
    "--list-select-configure-components",
    "--list-component-config-values",
    "--configure-component",
    "--export-selection",
    "--detect",
    "--uninstall",
)


def test_installer_delegates_readonly_routing_to_the_framework():
    """Routing must use the shared helper, not a local flag list."""
    text = (REPO_ROOT / "install.sh").read_text(encoding="utf-8")

    assert "isc_route_readonly_args" in text
    assert '--list-components|--export-selection|--detect' not in text, (
        "hand-rolled readonly flag list reintroduced"
    )


def test_master_installer_detects_every_readonly_flag():
    """Mirror _cs_repo_exposes_flag: every contract flag must be advertised."""
    candidates = [
        REPO_ROOT / "install.sh",
        REPO_ROOT / "installer" / "components.sh",
        REPO_ROOT / "scripts" / "install_standalone.sh",
    ]
    sources = [p.read_text(encoding="utf-8") for p in candidates if p.is_file()]
    delegates = any("isc_route_readonly_args" in s for s in sources)

    for flag in READONLY_FLAGS:
        assert delegates or any(flag in s for s in sources), (
            f"master installer cannot detect {flag}"
        )


def test_core_deploy_stays_out_of_the_readonly_path():
    """The core deploy must remain reachable only from main()."""
    text = (REPO_ROOT / "install.sh").read_text(encoding="utf-8")
    routing = text.split("isc_route_readonly_args", 1)[0]
    tail = routing.rsplit("main() {", 1)[-1]

    # install_agentic_status_core is called inside main(), never before routing.
    assert tail.count("install_agentic_status_core") <= 1
