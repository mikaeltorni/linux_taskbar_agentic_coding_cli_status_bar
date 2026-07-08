"""Color-gradient contract for the GNOME extension's percent coloring.

The extension colors every rendered percentage individually: 100% left is
green, the midpoint is yellow, 1% left is orange; each half is linearly
interpolated per RGB channel (full 8-bit precision) so mid values stay vivid
instead of collapsing toward white, and 0% left snaps to pure red. The pure
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
    "resetPassed",
    "percentLeft",
    "percentColor",
    "resetClock",
    "resetDate",
    "resetCountdown",
    "resetsText",
    "resetAvailabilityText",
    "segmentMarkup",
)

#: Top-level constants those helpers close over.
HELPER_CONSTANTS = (
    "GRADIENT_FULL_RGB",
    "GRADIENT_MID_RGB",
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
  countdown_59m: resetCountdown(NOW + 3540, NOW),
  countdown_60m: resetCountdown(NOW + 3600, NOW),
  rounded_reset_clock: resetClock(1783259999),
  rounded_details_reset: resetsText(1783259999),
  one_reset_available: resetAvailabilityText(1),
  multiple_resets_available: resetAvailabilityText(2),
  missing_resets_available: resetAvailabilityText(undefined),
  zero_resets_available: resetAvailabilityText(0),
  fable_exhausted: segmentMarkup('CC', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW + 3300},
    {label: 'week', used_percent: 59, resets_at: 1783617000},
    {label: 'fable', used_percent: 100, resets_at: 1783259999}]
  }, NOW),
  reset_passed_left_full: percentLeft(
    {used_percent: 100, resets_at: NOW - 60}, NOW),
  reset_future_left_exhausted: percentLeft(
    {used_percent: 100, resets_at: NOW + 60}, NOW),
  boot_stale_five_hour_replenished: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 100, resets_at: NOW - 3600},
    {label: 'week', used_percent: 82, resets_at: NOW + 100000}]
  }, NOW),
  boot_stale_weekly_replenished: segmentMarkup('CX', {windows: [
    {label: '5h', used_percent: 40, resets_at: NOW + 3600},
    {label: 'week', used_percent: 100, resets_at: NOW - 60}]
  }, NOW),
  opencode_go_segment: segmentMarkup('OG', {windows: [
    {label: 'rolling', used_percent: 0, resets_at: NOW + 17820},
    {label: 'week', used_percent: 83, resets_at: NOW + 234000},
    {label: 'month', used_percent: 92, resets_at: NOW + 2588400}]
  }, NOW),
  opencode_go_month_exhausted: segmentMarkup('OG', {windows: [
    {label: 'rolling', used_percent: 0, resets_at: NOW + 17820},
    {label: 'week', used_percent: 83, resets_at: NOW + 234000},
    {label: 'month', used_percent: 100, resets_at: NOW + 2588400}]
  }, NOW),
  monthly_reset_date: resetDate(NOW + 2588400),
  monthly_details_reset: resetsText(NOW + 2588400, 'month'),
  openrouter_segment: segmentMarkup('OR', {windows: [
    {label: 'day', used_percent: 42, resets_at: NOW + 52000}]
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
    """Reference gradient: 100→green, midpoint→yellow, 1→orange."""
    full = (0x8F, 0xF0, 0xA4)
    mid = (0xF8, 0xE4, 0x5C)
    low = (0xFF, 0x7B, 0x63)
    midpoint = (1 + 100) / 2
    if left <= midpoint:
        start, end, t = low, mid, (left - 1) / (midpoint - 1)
    else:
        start, end, t = mid, full, (left - midpoint) / (100 - midpoint)
    channels = (round(s + (e - s) * t) for s, e in zip(start, end))
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
    weekly = '<span foreground="#f9d05d">41</span>'
    assert js_results["five_hour_exhausted_codex"] == (
        '<span foreground="#ff0000">CX </span>'
        + red_countdown + separator + weekly
    )
    assert js_results["five_hour_exhausted_claude"] == (
        '<span foreground="#ff0000">CC </span>'
        + red_countdown + separator + weekly + separator + weekly
    )


def test_countdown_hides_zero_hours_below_one_hour(js_results):
    assert js_results["countdown_59m"] == "59m"
    assert js_results["countdown_60m"] == "1h00m"


def test_window_whose_reset_passed_reads_as_replenished(js_results):
    # A window whose reset moment has elapsed has rolled over to full, so it
    # must not keep echoing the stale pre-reset exhaustion.
    assert js_results["reset_passed_left_full"] == 100
    assert js_results["reset_future_left_exhausted"] == 0


def test_boot_stale_five_hour_flips_from_dead_countdown_to_available(js_results):
    # On a fresh boot the newest reading can name a 5h reset that already
    # passed; instead of a stuck red "0m" the segment shows it replenished.
    segment = js_results["boot_stale_five_hour_replenished"]
    assert segment == (
        '<span foreground="#8ff0a4">CX </span>'
        '<span foreground="#8ff0a4">100</span>'
        '<span foreground="#9a9996">·</span>'
        '<span foreground="#fd9f61">18</span>'
    )
    assert '0m' not in segment
    assert '#ff0000' not in segment  # nothing exhausted/red


def test_boot_stale_weekly_flips_from_reset_clock_to_available(js_results):
    # A weekly limit whose reset already passed is full again, not exhausted,
    # so it renders its percentages rather than the red reset clock.
    segment = js_results["boot_stale_weekly_replenished"]
    assert segment.startswith('<span foreground="#')
    assert '<span foreground="#8ff0a4">100</span>' in segment  # weekly replenished
    assert '>60<' in segment   # 5h at 40% used -> 60% left, still normal
    assert '#ff0000' not in segment  # no red reset clock


def test_exhausted_fable_shows_rounded_red_reset_clock_inline(js_results):
    separator = '<span foreground="#9a9996">-</span>'
    assert js_results["rounded_reset_clock"] == "Sun 14:00"
    assert js_results["rounded_details_reset"] == " — resets Sun 14:00"
    assert js_results["fable_exhausted"] == (
        '<span foreground="#ff0000">CC </span>'
        '<span foreground="#ff0000">55m</span>'
        + separator
        + '<span foreground="#f9d05d">41</span>'
        + separator
        + '<span foreground="#ff0000">Sun 14:00</span>'
    )


def test_cli_tags_place_openrouter_after_opencode_go():
    source = EXTENSION_JS.read_text(encoding="utf-8")
    og_index = source.index("['opencode_go', 'OG']")
    or_index = source.index("['openrouter', 'OR']")
    assert og_index < or_index
    assert "['grok', 'GK']" in source[:og_index]


def test_openrouter_segment_renders_daily_window(js_results):
    segment = js_results["openrouter_segment"]
    assert segment.startswith('<span foreground="#')
    assert '>58<' in segment  # 42% used -> 58% left


def test_opencode_go_segment_colors_three_windows(js_results):
    segment = js_results["opencode_go_segment"]
    assert segment.startswith('<span foreground="#8ff0a4">OG ')
    assert '<span foreground="#8ff0a4">100</span>' in segment
    assert 'foreground="#9a9996">17</span>' not in segment
    assert 'foreground="#9a9996">8</span>' not in segment
    assert '>17</span>' in segment and '>8</span>' in segment
    assert '<span foreground="#9a9996">·</span>' in segment


CALENDAR_DATE_RE = re.compile(r"\d{1,2} [A-Z][a-z]{2}")
WEEKDAY_TIME_RE = re.compile(
    r"(Mon|Tue|Wed|Thu|Fri|Sat|Sun) \d{2}:\d{2}"
)


def test_exhausted_monthly_shows_calendar_date_not_weekday(js_results):
    segment = js_results["opencode_go_month_exhausted"]
    assert CALENDAR_DATE_RE.search(segment)
    assert not WEEKDAY_TIME_RE.search(segment)
    assert '<span foreground="#ff0000">3 Aug</span>' in segment


def test_monthly_reset_date_formatter(js_results):
    assert js_results["monthly_reset_date"] == "3 Aug"
    assert CALENDAR_DATE_RE.fullmatch(js_results["monthly_reset_date"])
    assert not WEEKDAY_TIME_RE.search(js_results["monthly_reset_date"])


def test_monthly_details_reset_uses_calendar_date(js_results):
    assert js_results["monthly_details_reset"] == " — resets 3 Aug"
    assert CALENDAR_DATE_RE.search(js_results["monthly_details_reset"])
    assert not WEEKDAY_TIME_RE.search(js_results["monthly_details_reset"])


def test_reset_availability_wording(js_results):
    assert js_results["one_reset_available"] == " — 1 reset available"
    assert js_results["multiple_resets_available"] == " — 2 resets available"
    assert js_results["missing_resets_available"] == ""
    assert js_results["zero_resets_available"] == ""
