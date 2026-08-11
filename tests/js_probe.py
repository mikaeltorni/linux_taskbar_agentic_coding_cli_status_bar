"""Run pure helpers from a GNOME Shell extension under gjs.

`extension.js` files import `gi://…` and `resource:///org/gnome/shell/…`, so
they cannot be imported by a test process. The pure helpers inside them can
still be executed: this module lifts a named function or constant out of the
source with brace counting and runs the selected snippets plus a probe script
under gjs, returning the probe's JSON output.

Usage:
    snippets = [extract_constant(src, "FOO"), extract_function(src, "bar")]
    results = run_gjs(snippets, "print(JSON.stringify({x: bar(1)}));")
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess

import pytest


def extract_function(source: str, name: str) -> str:
    """Return the full text of a top-level JS function by brace counting.

    @param source: whole extension.js text.
    @param name: top-level `function name(...)` to lift out.
    @returns: the function's source, braces balanced.
    """
    match = re.search(rf"^function {name}\(", source, re.M)
    assert match, f"function {name} not found"
    depth = 0
    for index in range(match.start(), len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[match.start(): index + 1]
    raise AssertionError(f"unbalanced braces extracting {name}")


def extract_constant(source: str, name: str) -> str:
    """Return a single-line top-level `const NAME = ...;` declaration.

    @param source: whole extension.js text.
    @param name: constant to lift out.
    @returns: the declaration line, including its semicolon.
    """
    match = re.search(rf"^const {name} = .*?;", source, re.M)
    assert match, f"const {name} not found"
    return match.group(0)


def run_gjs(snippets: list[str], probe: str, env: dict | None = None) -> dict:
    """Execute extracted snippets plus a probe under gjs and parse its JSON.

    Skips the calling test when gjs is not installed (headless CI).

    @param snippets: extracted constants/functions, in dependency order.
    @param probe: JS that prints one JSON document on stdout.
    @param env: process environment for gjs (defaults to the test's own, which
        matters for time-zone dependent helpers).
    @returns: the parsed document.
    """
    if shutil.which("gjs") is None:
        pytest.skip("gjs not installed")
    script = "\n".join([*snippets, probe])
    completed = subprocess.run(
        ["gjs", "-c", script],
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )
    assert completed.returncode == 0, completed.stderr
    return json.loads(completed.stdout)
