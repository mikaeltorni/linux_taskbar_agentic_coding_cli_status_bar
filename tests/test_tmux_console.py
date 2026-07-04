"""Tests for tmux console binary resolution.

The poller runs as a systemd user service whose PATH lacks per-app bin dirs
(notably ~/.grok/bin), and a tmux window whose command cannot be resolved dies
instantly — so windows must always be created from an absolute binary path.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from agentic_cli_status import tmux_console
from agentic_cli_status.config import SLASH_COMMANDS


def test_codex_monitor_uses_usage_command():
    """Codex reset availability is exposed by /usage, not /status."""
    assert SLASH_COMMANDS["codex"] == "/usage"


class TestEnsureConsole:
    """Console windows must be created with TTS/tracker suppression env vars.

    The console runs real CLI TUIs as monitoring surfaces; without the env
    opt-outs their lifecycle hooks announce fake agent sessions (e.g.
    "Project mk started") at every login.
    """

    def _fake_tmux(self, calls, has_session=False):
        def run(args):
            calls.append(args)
            ok = not (args[0] == "has-session" and not has_session)
            return subprocess.CompletedProcess(
                ["tmux", *args], 0 if ok else 1, stdout="", stderr=""
            )

        return run

    def test_new_session_and_window_set_suppression_env(self, monkeypatch):
        calls = []
        monkeypatch.setattr(tmux_console, "tmux_available", lambda: True)
        monkeypatch.setattr(tmux_console, "_tmux", self._fake_tmux(calls))
        monkeypatch.setattr(
            tmux_console, "resolve_cli_binary", lambda cli_id: f"/usr/bin/{cli_id}"
        )

        assert tmux_console.ensure_console("acs", ["claude_code", "codex"]) is True

        creations = [c for c in calls if c[0] in {"new-session", "new-window"}]
        assert [c[0] for c in creations] == ["new-session", "new-window"]
        for args in creations:
            joined = " ".join(args)
            assert "-e ATTS_SUPPRESS_ANNOUNCEMENTS=1" in joined
            assert "-e CODEX_AGENT_SKIP_TRACKER=1" in joined


class TestResolveCliBinary:
    def test_prefers_path_lookup(self, sandbox_home, monkeypatch):
        monkeypatch.setattr(
            tmux_console.shutil, "which", lambda name: f"/usr/bin/{name}"
        )
        assert tmux_console.resolve_cli_binary("codex") == "/usr/bin/codex"

    def test_falls_back_to_known_install_dirs(self, sandbox_home, monkeypatch):
        monkeypatch.setattr(tmux_console.shutil, "which", lambda name: None)
        grok = sandbox_home / ".grok" / "bin" / "grok"
        grok.parent.mkdir(parents=True)
        grok.write_text("#!/bin/sh\n", encoding="utf-8")
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: sandbox_home))
        assert tmux_console.resolve_cli_binary("grok") == str(grok)

    def test_missing_binary_returns_none(self, sandbox_home, monkeypatch):
        monkeypatch.setattr(tmux_console.shutil, "which", lambda name: None)
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: sandbox_home))
        assert tmux_console.resolve_cli_binary("claude_code") is None
