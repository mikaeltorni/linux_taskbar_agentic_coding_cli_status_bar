"""Tests for the poll tick: aggregation, staleness, and the status file."""

from __future__ import annotations

import json

from agentic_cli_status import config as acs_config
from agentic_cli_status import poller
from tests.conftest import (
    codex_token_count_event,
    grok_billing_record,
    write_codex_rollout,
    write_grok_log,
)

# 2026-07-03T10:41:25Z, matching the conftest codex event timestamp.
NOW = 1783075290.0


def _configure(sandbox_home, **overrides):
    """Write a config with tmux disabled (tests never touch real tmux)."""
    cfg = acs_config.load_config()
    cfg["tmux"]["enabled"] = False
    for key, value in overrides.items():
        cfg[key] = value
    acs_config.save_config(cfg)
    return cfg


class TestPollOnce:
    def test_aggregates_enabled_clis_and_writes_status(self, sandbox_home):
        _configure(sandbox_home)
        write_codex_rollout(
            sandbox_home, "2026/07/03", "rollout-a.jsonl", [codex_token_count_event(57, 43)]
        )
        write_grok_log(sandbox_home, "grok.log", [grok_billing_record(88)])
        status = poller.poll_once(home=sandbox_home, now=NOW)
        assert status["generated_at"] == NOW
        assert status["poll_interval_seconds"] == 5
        codex = status["clis"]["codex"]
        assert codex["enabled"] and not codex["stale"]
        assert codex["windows"][0]["used_percent"] == 57.0
        grok = status["clis"]["grok"]
        assert grok["windows"][0]["used_percent"] == 88.0
        # claude has no credentials in the sandbox: enabled but stale + error.
        claude = status["clis"]["claude_code"]
        assert claude["enabled"] and claude["stale"] and claude["error"]
        on_disk = json.loads(poller.status_path(sandbox_home).read_text())
        assert on_disk["clis"]["codex"]["windows"][0]["used_percent"] == 57.0

    def test_disabled_cli_is_skipped(self, sandbox_home):
        cfg = _configure(sandbox_home)
        cfg["clis"]["codex"]["enabled"] = False
        acs_config.save_config(cfg)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        assert status["clis"]["codex"] == {"enabled": False}

    def test_old_data_is_marked_stale(self, sandbox_home):
        _configure(sandbox_home)
        write_codex_rollout(
            sandbox_home,
            "2026/07/01",
            "rollout-old.jsonl",
            [codex_token_count_event(9, 9, timestamp="2026-07-01T00:00:00Z")],
        )
        status = poller.poll_once(home=sandbox_home, now=NOW)
        codex = status["clis"]["codex"]
        assert codex["windows"][0]["used_percent"] == 9.0
        assert codex["stale"] is True

    def test_headless_mode_never_calls_tmux(self, sandbox_home, monkeypatch):
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        for cli in acs_config.CLI_IDS:
            cfg["clis"][cli]["source"] = "headless"
        acs_config.save_config(cfg)

        def _boom(*args, **kwargs):
            raise AssertionError("tmux must not be touched in headless mode")

        monkeypatch.setattr(poller, "refresh_and_capture", _boom)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        poller.poll_once(home=sandbox_home, now=NOW)

    def test_stale_auto_mode_uses_pane_fallback(self, sandbox_home, monkeypatch):
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        pane_text = "5h limit: 61% used\nWeekly limit: 22% used\n"
        captured = []

        def _fake_capture(session, cli_id):
            captured.append((session, cli_id))
            return pane_text if cli_id == "codex" else None

        monkeypatch.setattr(poller, "refresh_and_capture", _fake_capture)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        codex = status["clis"]["codex"]
        assert codex["source"] == "tmux_pane"
        assert codex["windows"][0]["used_percent"] == 61.0
        assert ("agentic-cli-status", "codex") in captured
        pane_file = poller.state_dir(sandbox_home) / "panes" / "codex.txt"
        assert pane_file.read_text() == pane_text

    def test_tmux_refresh_is_throttled_between_ticks(self, sandbox_home, monkeypatch):
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        attempts = []

        def _fake_capture(session, cli_id):
            attempts.append(cli_id)
            return None

        monkeypatch.setattr(poller, "refresh_and_capture", _fake_capture)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        first = poller.poll_once(home=sandbox_home, now=NOW)
        codex_attempts = attempts.count("codex")
        assert codex_attempts == 1
        assert first["clis"]["codex"]["tmux_attempt_at"] == NOW
        # A second tick 5 seconds later must not re-attempt the refresh.
        second = poller.poll_once(home=sandbox_home, previous=first, now=NOW + 5)
        assert attempts.count("codex") == codex_attempts
        assert second["clis"]["codex"]["tmux_attempt_at"] == NOW
        # After the throttle window it attempts again.
        poller.poll_once(home=sandbox_home, previous=second, now=NOW + 61)
        assert attempts.count("codex") == codex_attempts + 1
