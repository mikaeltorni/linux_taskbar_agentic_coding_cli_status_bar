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


def test_opencode_go_monitor_uses_ogc_usage_command():
    assert SLASH_COMMANDS["opencode_go"] == "/ogc-usage"


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

    def test_openrouter_enabled_does_not_keyerror(self, monkeypatch):
        """Headless-only CLIs must be skipped, not looked up in CLI_COMMANDS."""
        calls = []

        def fake_tmux(args):
            calls.append(args)
            if args[0] == "list-windows":
                return subprocess.CompletedProcess(args, 0, "codex\n", "")
            return subprocess.CompletedProcess(args, 0, "", "")

        monkeypatch.setattr(tmux_console, "tmux_available", lambda: True)
        monkeypatch.setattr(tmux_console, "_tmux", fake_tmux)
        monkeypatch.setattr(
            tmux_console, "resolve_cli_binary", lambda cli_id: f"/usr/bin/{cli_id}"
        )
        assert tmux_console.ensure_console("acs", ["codex", "openrouter"]) is True
        assert tmux_console.tmux_console_cli_ids(
            ["codex", "openrouter", "grok"]
        ) == ["codex", "grok"]

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


class TestRefreshAndCapture:
    def test_codex_combines_usage_allowance_with_status_breakdown(
        self, monkeypatch
    ):
        """Capture /usage metadata, then obtain rate limits from /status."""
        calls = []
        captures = iter([
            "Usage\n› 1. Show usage\n  2. Redeem usage limit reset\n"
            "You have 1 usage limit reset available.\n",
            "5h limit: 99% left\nWeekly limit: 9% left\n",
        ])

        def fake_tmux(args):
            calls.append(args)
            stdout = next(captures) if args[0] == "capture-pane" else ""
            return subprocess.CompletedProcess(["tmux", *args], 0, stdout, "")

        monkeypatch.setattr(tmux_console, "_tmux", fake_tmux)
        text = tmux_console.refresh_and_capture("acs", "codex", sleeper=lambda _: None)

        assert "1 usage limit reset available" in text
        assert "Weekly limit: 9% left" in text
        send_keys = [args for args in calls if args[0] == "send-keys"]
        assert ["send-keys", "-t", "acs:codex", "/status"] in send_keys
        status_index = send_keys.index(
            ["send-keys", "-t", "acs:codex", "/status"]
        )
        assert send_keys[status_index - 2:status_index + 2] == [
            ["send-keys", "-t", "acs:codex", "Escape"],
            ["send-keys", "-t", "acs:codex", "C-u"],
            ["send-keys", "-t", "acs:codex", "/status"],
            ["send-keys", "-t", "acs:codex", "Enter"],
        ]

    def test_codex_captures_status_even_when_usage_is_a_heatmap(
        self, monkeypatch
    ):
        """Newer Codex /usage shows an activity heatmap, not a reset menu.

        The rate-limit windows live only in /status, so it must be captured
        regardless of what /usage rendered.
        """
        calls = []
        captures = iter([
            "Token activity   last 12 months\n daily · weekly · cumulative\n",
            "5h limit: [####] 92% left (resets 17:19)\n"
            "Weekly limit: [#] 7% left\n",
        ])

        def fake_tmux(args):
            calls.append(args)
            stdout = next(captures) if args[0] == "capture-pane" else ""
            return subprocess.CompletedProcess(["tmux", *args], 0, stdout, "")

        monkeypatch.setattr(tmux_console, "_tmux", fake_tmux)
        text = tmux_console.refresh_and_capture("acs", "codex", sleeper=lambda _: None)

        assert "5h limit" in text and "Weekly limit" in text
        status_sends = [
            args for args in calls
            if args == ["send-keys", "-t", "acs:codex", "/status"]
        ]
        assert len(status_sends) == 1

    def test_codex_retries_status_until_rate_limits_render(self, monkeypatch):
        """A cold /status first answers "refresh requested"; retry until it renders."""
        calls = []
        captures = iter([
            "Usage\n› 1. Show usage\nYou have 2 usage limit resets available.\n",
            "Refresh requested; run /status again shortly.\n",
            "5h limit: 99% left (resets 20:12)\nWeekly limit: 9% left\n",
        ])

        def fake_tmux(args):
            calls.append(args)
            stdout = next(captures) if args[0] == "capture-pane" else ""
            return subprocess.CompletedProcess(["tmux", *args], 0, stdout, "")

        monkeypatch.setattr(tmux_console, "_tmux", fake_tmux)
        text = tmux_console.refresh_and_capture("acs", "codex", sleeper=lambda _: None)

        assert "2 usage limit resets available" in text
        assert "5h limit: 99% left" in text
        status_sends = [
            args for args in calls
            if args == ["send-keys", "-t", "acs:codex", "/status"]
        ]
        assert len(status_sends) == 2
