"""Tests for tmux console binary resolution.

The poller runs as a systemd user service whose PATH lacks per-app bin dirs
(notably ~/.grok/bin), and a tmux window whose command cannot be resolved dies
instantly — so windows must always be created from an absolute binary path.
"""

from __future__ import annotations

from pathlib import Path

from agentic_cli_status import tmux_console


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
