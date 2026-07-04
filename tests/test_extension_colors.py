"""Color-gradient contract for the GNOME extension's percent coloring.

The extension colors every rendered percentage individually: 100% left is
green, 1% left is orange, everything between is linearly interpolated per RGB
channel (full 8-bit precision), and 0% left snaps to pure red. The pure
helpers live in extension.js, which cannot be imported outside GNOME Shell,
so this test extracts the helper functions plus their constants and executes
them under gjs. Skips when gjs is unavailable (e.g. headless CI).
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
EXTENSION_JS = REPO_ROOT / "agentic-cli-status@local" / "extension.js"

#: Top-level `function name(...) {...}` helpers the gradient logic lives in.
HELPER_FUNCTIONS = (
    "percentLeft",
    "percentColor",
    "resetClock",
    "resetCountdown",
    "segmentMarkup",
)

#: Top-level constants those helpers close over.
HELPER_CONSTANTS = (
    "GRADIENT_FULL_RGB",
    "GRADIENT_LOW_RGB",
    "EMPTY_COLOR",
    "NEUTRAL_COLOR",
)


def _extract_function(source: str, name: str) -> str:
    """Return the full text of a top-level JS function by brace counting."""
    match = re.search(rf"^function {name}\(", source, re.M)
    assert match, f"function {name} not found in extension.js"
    depth = 0
    for index in range(match.start(), len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[match.start(): index + 1]
    raise AssertionError(f"unbalanced braces extracting {name}")


def _extract_constant(source: str, name: str) -> str:
    """Return a top-level `const NAME = ...;` line from extension.js."""
    match = re.search(rf"^const {name} = .*?;", source, re.M)
    assert match, f"const {name} not found in extension.js"
    return match.group(0)


@pytest.fixture(scope="module")
def js_results() -> dict:
    """Run the extracted helpers under gjs and return their JSON output."""
    if shutil.which("gjs") is None:
        pytest.skip("gjs not installed")
    source = EXTENSION_JS.read_text(encoding="utf-8")
    snippets = [_extract_constant(source, name) for name in HELPER_CONSTANTS]
    snippets += [_extract_function(source, name) for name in HELPER_FUNCTIONS]
    probe = """
const GLib = imports.gi.GLib;
const NOW = 1783158600;
const results = {
  color_100: percentColor(100),
  color_1: percentColor(1),
  color_0: percentColor(0),
  color_50: percentColor(50),
  color_99: percentColor(99),
  color_2: percentColor(2),
  left_clamped: percentLeft({used_percent: 130}),
  dash: segmentMarkup('CX', {windows: []}),
  segment: segmentMarkup('CC', {windows: [
    {used_percent: 0}, {used_percent: 99}, {used_percent: 100}]}),
  weekly_exhausted_grok: segmentMarkup('GK', {windows: [
    {label: 'week', used_percent: 100, resets_at: 1783617000}]
  }, NOW),
  weekly_exhausted_codex: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 17940},
    {label: 'week', used_percent: 100, resets_at: 1783617000}]
  }, NOW),
  five_hour_exhausted_codex: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 17940},
    {label: 'week', used_percent: 59, resets_at: 1783617000}]
  }, NOW),
  five_hour_exhausted_claude: segmentMarkup('CC', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 17940},
    {label: 'week', used_percent: 59, resets_at: 1783617000},
    {label: 'fable', used_percent: 59, resets_at: 1783617000}]
  }, NOW),
};
print(JSON.stringify(results));
"""
    script = "\n".join(snippets) + probe
    completed = subprocess.run(
        ["gjs", "-c", script],
        capture_output=True,
        text=True,
        check=True,
        env={**os.environ, "TZ": "UTC"},
    )
    return json.loads(completed.stdout)


def _lerp_color(left: int) -> str:
    """Reference implementation of the 8-bit gradient (100→green, 1→orange)."""
    full = (0x8F, 0xF0, 0xA4)
    low = (0xFF, 0x7B, 0x63)
    t = (left - 1) / 99
    channels = (round(l + (f - l) * t) for f, l in zip(full, low))
    return "#" + "".join(f"{c:02x}" for c in channels)


def test_gradient_endpoints(js_results):
    assert js_results["color_100"] == "#8ff0a4"
    assert js_results["color_1"] == "#ff7b63"
    assert js_results["color_0"] == "#ff0000"


def test_gradient_uses_full_rgb_precision(js_results):
    assert js_results["color_99"] == _lerp_color(99)
    assert js_results["color_50"] == _lerp_color(50)
    assert js_results["color_2"] == _lerp_color(2)
    # Adjacent inputs must actually move the color (no coarse banding).
    assert js_results["color_99"] != js_results["color_100"]
    assert js_results["color_2"] != js_results["color_1"]


def test_percent_left_is_clamped(js_results):
    assert js_results["left_clamped"] == 0


def test_dash_uses_neutral_color(js_results):
    assert js_results["dash"] == '<span foreground="#9a9996">CX –</span>'


def test_segment_colors_each_window_and_tag_follows_first_window(js_results):
    segment = js_results["segment"]
    # Tag takes the first (5h / grok weekly) window's color: 100% left → green.
    assert segment.startswith('<span foreground="#8ff0a4">CC ')
    # Each window renders in its own color: 100% green, 1% orange, 0% red.
    assert '>100<' in segment and '#8ff0a4' in segment
    assert '<span foreground="#ff7b63">1</span>' in segment
    assert '<span foreground="#ff0000">0</span>' in segment
    # Separators between numbers are neutral.
    assert '<span foreground="#9a9996">·</span>' in segment
    # The "%" sign is omitted to reduce clutter.
    assert '%' not in segment


def test_exhausted_weekly_shows_only_red_reset_clock(js_results):
    expected = (
        '<span foreground="#ff0000">GK </span>'
        '<span foreground="#ff0000">Thu 17:10</span>'
    )
    assert js_results["weekly_exhausted_grok"] == expected
    assert js_results["weekly_exhausted_codex"] == expected.replace("GK", "CX")
    assert "5h" not in js_results["weekly_exhausted_codex"]


def test_exhausted_five_hour_shows_red_countdown_then_weekly_values(js_results):
    red_countdown = '<span foreground="#ff0000">4h59m</span>'
    separator = '<span foreground="#9a9996">-</span>'
    weekly = '<span foreground="#d2aa7d">41</span>'
    assert js_results["five_hour_exhausted_codex"] == (
        '<span foreground="#ff0000">CX </span>'
        + red_countdown + separator + weekly
    )
    assert js_results["five_hour_exhausted_claude"] == (
        '<span foreground="#ff0000">CC </span>'
        + red_countdown + separator + weekly + separator + weekly
    )
