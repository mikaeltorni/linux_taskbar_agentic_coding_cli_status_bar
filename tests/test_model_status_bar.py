"""Contract tests for the optional ACC model status bar extension.

The extension (acc-model-status@local) is deployed by its own installer
component, so three things must stay in lockstep:
  * the copy list in lib/status_setup.sh and the files on disk (a missing icon
    would install a bar with blank segments),
  * the component manifest entry (opt-in, own section, real functions),
  * the extension's rendering contract (payload + live Shell windows, no tmux).
"""

from __future__ import annotations

import json
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
EXT_DIR = REPO_ROOT / "acc-model-status@local"
SETUP = (REPO_ROOT / "lib" / "status_setup.sh").read_text(encoding="utf-8")
EXTENSION_JS = (EXT_DIR / "extension.js").read_text(encoding="utf-8")


def _copy_list() -> list[str]:
    """Parse the ACS_MODEL_STATUS_FILES array from lib/status_setup.sh."""
    match = re.search(r"ACS_MODEL_STATUS_FILES=\((.*?)\n\)", SETUP, re.S)
    assert match, "ACS_MODEL_STATUS_FILES array not found in lib/status_setup.sh"
    return re.findall(r'"([^"]+)"', match.group(1))


def _manifest_entry() -> list[str]:
    """Return the model_status_bar manifest entry split into its fields."""
    text = (REPO_ROOT / "installer" / "components.sh").read_text(encoding="utf-8")
    match = re.search(r'^\s*"(model_status_bar\|[^"]+)"\s*$', text, re.M)
    assert match, "model_status_bar entry missing from installer/components.sh"
    return match.group(1).split("|")


def test_copy_list_matches_extension_files():
    on_disk = sorted(
        str(p.relative_to(EXT_DIR))
        for p in EXT_DIR.rglob("*")
        if p.is_file()
    )
    assert sorted(_copy_list()) == on_disk


def test_every_shipped_icon_is_a_symbolic_svg():
    icons = [name for name in _copy_list() if name.startswith("icons/")]
    assert set(icons) == {
        "icons/acc-provider-api-symbolic.svg",
        "icons/acc-provider-claude-symbolic.svg",
        "icons/acc-provider-cline-symbolic.svg",
        "icons/acc-provider-codex-symbolic.svg",
        "icons/acc-provider-generic-symbolic.svg",
        "icons/acc-provider-grok-symbolic.svg",
        "icons/acc-provider-nvidia-symbolic.svg",
        "icons/acc-provider-opencode-symbolic.svg",
        "icons/acc-provider-openrouter-symbolic.svg",
        "icons/acc-provider-qwen-symbolic.svg",
    }
    for icon in icons:
        assert re.fullmatch(r"icons/acc-provider-[a-z0-9]+-symbolic\.svg", icon), icon
        assert "<svg" in (EXT_DIR / icon).read_text(encoding="utf-8")


def test_generic_fallback_icon_is_shipped():
    assert "icons/acc-provider-generic-symbolic.svg" in _copy_list()
    assert "acc-provider-generic-symbolic" in EXTENSION_JS


def test_component_is_optional_and_wired_to_real_functions():
    fields = _manifest_entry()
    component_id, _label, default, install_fn, detect_fn, uninstall_fn, section = fields[:7]

    assert component_id == "model_status_bar"
    assert default == "off", "the model status bar must stay opt-in"
    assert section == "Taskbar extensions", "it is separate from the poller sections"
    for func in (install_fn, detect_fn, uninstall_fn):
        assert re.search(rf"^{re.escape(func)}\(\) \{{", SETUP, re.M), func


def test_installer_exports_the_extension_source_and_uuid():
    install_sh = (REPO_ROOT / "install.sh").read_text(encoding="utf-8")
    assert 'MODEL_STATUS_UUID="acc-model-status@local"' in install_sh
    assert 'MODEL_STATUS_SRC="$SCRIPT_DIR/acc-model-status@local"' in install_sh
    assert "MODEL_STATUS_UUID MODEL_STATUS_SRC" in install_sh


def test_core_deploy_does_not_install_the_model_status_bar():
    core = re.search(r"install_agentic_status_core\(\) \{(.*?)\n\}", SETUP, re.S)
    assert core, "core deploy function not found"
    assert "MODEL_STATUS" not in core.group(1)


def test_metadata_declares_uuid_and_supported_shell_versions():
    metadata = json.loads((EXT_DIR / "metadata.json").read_text(encoding="utf-8"))
    assert metadata["uuid"] == "acc-model-status@local"
    assert EXT_DIR.name == metadata["uuid"], "directory name must match the UUID"
    assert "48" in metadata["shell-version"]


def _code_only(text: str) -> str:
    """Strip block and line comments so prose never satisfies a code assertion."""
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r"^\s*//.*$", "", text, flags=re.M)


def test_extension_uses_payload_without_external_polling():
    code = _code_only(EXTENSION_JS)
    assert "model_status_bar.json" in code
    assert "CODEX_AGENT_TRACKER_STATE_DIR" in code
    # Rendering only: no subprocesses, tmux commands, or network clients. The
    # payload's tmux_name identity field is allowed for title matching.
    for forbidden in ("GLib.spawn", "Gio.Subprocess", "Soup", "subprocess"):
        assert forbidden not in code, forbidden


def test_extension_scopes_payload_to_open_windows_across_all_workspaces():
    """Every open agent window renders with its actual GNOME workspace."""
    code = _code_only(EXTENSION_JS)
    for required in (
        "global.workspace_manager.get_active_workspace()",
        "global.get_window_actors()",
        "win.get_workspace()",
        "win.located_on_workspace(activeWorkspace)",
        "win.get_title()",
        "scopePayloadToWindows(payload, windows)",
        "workspace: match.workspace",
        "current_workspace: match.current",
    ):
        assert required in code, required
    # Counts cover matched open windows across every workspace and are never
    # copied from ACC's payload of live/detached sessions.
    assert "live: agents.length" in code
    assert "const running = agents.filter(agent =>" in code
    assert "const attention = agents.filter(agent =>" in code
    assert "providers.push({...provider, agents, live: agents.length, running, attention})" in code


def test_menu_is_workspace_sorted_and_marks_the_current_workspace():
    code = _code_only(EXTENSION_JS)
    assert "workspaceEntries.sort(compareWorkspaceEntries)" in code
    assert "Workspace ${workspace}" in code
    assert "current ? ' — current' : ''" in code
    assert "new PopupMenu.PopupMenuSection()" in code
    assert "workspaceSection.addMenuItem(heading)" in code
    assert "workspaceSection.addMenuItem(this._agentMenuItem" in code
    assert "workspaceSection.actor.add_style_class_name('ams-current-workspace-block')" in code

    css = (EXT_DIR / "stylesheet.css").read_text(encoding="utf-8")
    current = re.search(r"\.ams-current-workspace-block\s*\{(.*?)\}", css, re.S)
    assert current, "current workspace highlight style missing"
    assert re.search(r"background-color:\s*rgba\([^;]+,\s*0\.35\)", current.group(1))


def test_extension_refreshes_when_workspace_or_window_identity_changes():
    """Workspace switches, moves, opens, closes, and title updates are live."""
    code = _code_only(EXTENSION_JS)
    for signal in (
        "active-workspace-changed",
        "window-created",
        "workspace-changed",
        "notify::title",
        "unmanaged",
    ):
        assert f"'{signal}'" in code, signal
    assert "this._shellSignals" in code
    assert "this._windowSignals" in code


def test_extension_releases_timers_and_monitor_on_destroy():
    destroy = re.search(r"    destroy\(\) \{(.*?)\n    \}", EXTENSION_JS, re.S)
    assert destroy, "destroy() not found"
    body = destroy.group(1)
    assert body.count("GLib.source_remove") == 2
    assert "this._monitor.cancel()" in body
    assert "object.disconnect(id)" in body
    assert "win.disconnect(id)" in body
    assert "this._windowSignals.clear()" in body
    assert "super.destroy()" in body


def test_styles_used_by_the_extension_exist_in_the_stylesheet():
    css = (EXT_DIR / "stylesheet.css").read_text(encoding="utf-8")
    for style in ("ams-box", "ams-segment", "ams-count", "ams-active", "ams-idle",
                  "ams-attention", "ams-empty", "ams-stale"):
        assert f".{style}" in css, style
        assert style in EXTENSION_JS, style


def _phase_palette() -> dict[str, str]:
    """Return the PHASE_COLORS map declared by the extension."""
    match = re.search(r"const PHASE_COLORS = \{(.*?)\n\};", EXTENSION_JS, re.S)
    assert match, "PHASE_COLORS not found in extension.js"
    return dict(re.findall(r"(\w+):\s*'#([0-9a-f]{6})'", match.group(1)))


def test_phase_palette_matches_the_notes_menu_colors():
    """The menu must color phases exactly like the phrase automation notes UI."""
    palette = _phase_palette()
    # Mirrors AGENT_PHASE_BADGE_COLORS in phrase_automation notes_display.py.
    for phase, color in {
        "RUNNING": "e6c200",
        "ATTN": "ffb454",
        "DONE": "22c55e",
        "DRAFT": "84c32f",
        "STOPPED": "ef4444",
        "INTERRUPTED": "c084fc",
        "TRANSFERRED": "c45c5c",
        "UNKNOWN": "7f8ea3",
    }.items():
        assert palette.get(phase) == color, phase


def test_agent_rows_carry_project_tag_provider_icon_and_colored_phase_badge():
    row = re.search(r"    _agentMenuItem\(agent, provider\) \{(.*?)\n    \}", EXTENSION_JS, re.S)
    assert row, "_agentMenuItem() not found"
    body = row.group(1)
    # The persisted Phrase Automation project label comes first; workspace
    # numbers belong in group headings and must not be repeated on every row.
    assert "agent.project_tag" in body
    assert "ams-project-tag" in body
    assert "`W${workspace}`" not in body
    assert "ams-workspace" not in body
    assert body.index("ams-project-tag") < body.index("ams-phase")
    assert "gicon: this._icon(provider.icon)" in body
    assert "ams-menu-provider-icon" in body
    assert "text: `${provider.label" not in body
    assert "ams-current-workspace-block" not in body
    assert "phaseColor(phase)" in body
