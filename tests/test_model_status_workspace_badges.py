"""Per-workspace attention badges of the ACC model status bar extension.

The panel shows one flashing purple `W<n>:<count>` badge per workspace that
holds agents waiting on the user (needs input, interrupted/stopped, or done),
so a glance at the bar tells you *where* to go. The same badges repeat in the
drop-down's workspace headings.

The counting helper is pure, so it runs under gjs (tests/js_probe.py); the
wiring into the panel, the menu and teardown is asserted against the source.
"""

from __future__ import annotations

import re
from pathlib import Path

from js_probe import extract_constant, extract_function, run_gjs

REPO_ROOT = Path(__file__).resolve().parents[1]
EXT_DIR = REPO_ROOT / "acc-model-status@local"
EXTENSION_JS = (EXT_DIR / "extension.js").read_text(encoding="utf-8")
STYLESHEET = (EXT_DIR / "stylesheet.css").read_text(encoding="utf-8")

#: Payload shaped like scopePayloadToWindows() output: agents already carry the
#: workspace of their live window.
PAYLOAD = """
const PAYLOAD = {providers: [
  {kind: 'claude', agents: [
    {phase: 'RUNNING', workspace: 5, current_workspace: true},
    {phase: 'ATTN', workspace: 4, current_workspace: false}]},
  {kind: 'codex', agents: [
    {phase: 'DRAFT', workspace: 4, current_workspace: false},
    {phase: 'DONE', workspace: 2, current_workspace: false},
    {phase: 'done', workspace: 2, current_workspace: false},
    {phase: 'INTERRUPTED', workspace: 5, current_workspace: true},
    {phase: 'DONE', workspace: 0, current_workspace: false}]},
  {kind: 'grok', agents: [
    {phase: 'STOPPED', workspace: 5, current_workspace: true}]},
]};
"""


def _counts(probe_payload: str = "PAYLOAD") -> dict:
    """Run workspaceAttentionCounts() under gjs against a JS payload literal."""
    snippets = [
        extract_constant(EXTENSION_JS, "NEEDS_USER_PHASES"),
        extract_function(EXTENSION_JS, "workspaceAttentionCounts"),
        PAYLOAD,
    ]
    probe = f"print(JSON.stringify({{entries: workspaceAttentionCounts({probe_payload})}}));"
    return run_gjs(snippets, probe)


def test_badges_count_waiting_agents_per_workspace_in_workspace_order():
    entries = _counts()["entries"]
    assert [entry["label"] for entry in entries] == ["W2:2", "W4:1", "W5:2"]
    assert [entry["workspace"] for entry in entries] == [2, 4, 5]
    assert [entry["count"] for entry in entries] == [2, 1, 2]


def test_running_and_draft_agents_never_raise_a_badge():
    entries = _counts("{providers: [{kind: 'claude', agents: ["
                      "{phase: 'RUNNING', workspace: 3},"
                      "{phase: 'DRAFT', workspace: 3}]}]}")["entries"]
    assert entries == []


def test_agents_without_a_resolved_workspace_are_skipped():
    """Workspace 0 means "no live window found" — there is nowhere to switch."""
    entries = _counts("{providers: [{kind: 'codex', agents: ["
                      "{phase: 'DONE', workspace: 0}]}]}")["entries"]
    assert entries == []


def test_badge_marks_the_current_workspace():
    entries = _counts()["entries"]
    current = {entry["workspace"]: entry["current"] for entry in entries}
    assert current == {2: False, 4: False, 5: True}


def test_missing_or_empty_payload_yields_no_badges():
    assert _counts("{}")["entries"] == []
    assert _counts("{providers: []}")["entries"] == []


def test_panel_and_menu_both_render_the_badges():
    assert "this._renderWorkspaceAttention(" in EXTENSION_JS
    # The menu reuses the same counts, so the bar and the drop-down agree.
    # One call site only (the other hit is the function definition itself).
    assert len(re.findall(r"=\s*workspaceAttentionCounts\(payload\)", EXTENSION_JS)) == 1
    assert "_renderMenu(payload, stale, attention)" in EXTENSION_JS
    assert "ams-ws-attn" in EXTENSION_JS


def test_flashing_is_driven_by_one_timer_that_is_released_on_destroy():
    assert "FLASH_INTERVAL_MS" in EXTENSION_JS
    destroy = EXTENSION_JS[EXTENSION_JS.index("    destroy() {"):]
    assert "this._flashId" in destroy, "flash timer must be removed in destroy()"
    assert "GLib.source_remove(this._flashId)" in destroy
    # Actors are re-collected on every repaint, never kept after a rebuild.
    refresh = EXTENSION_JS[EXTENSION_JS.index("    _refresh() {"):]
    assert "this._flashActors = [];" in refresh


def test_stylesheet_defines_the_bright_purple_badge_and_its_dim_phase():
    for style_class in ("ams-ws-attn", "ams-ws-attn-dim", "ams-ws-attn-current"):
        assert f".{style_class} {{" in STYLESHEET
