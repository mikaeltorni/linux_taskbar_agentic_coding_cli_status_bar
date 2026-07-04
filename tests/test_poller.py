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
        assert codex["resets_available"] == 1
        grok = status["clis"]["grok"]
        assert grok["windows"][0]["used_percent"] == 88.0
        # claude has no credentials in the sandbox: enabled but stale + error.
        claude = status["clis"]["claude_code"]
        assert claude["enabled"] and claude["stale"] and claude["error"]
        on_disk = json.loads(poller.status_path(sandbox_home).read_text())
        assert on_disk["clis"]["codex"]["windows"][0]["used_percent"] == 57.0

    def test_claude_endpoint_interval_reaches_collector(
        self, sandbox_home, monkeypatch
    ):
        """The configured endpoint interval is passed to collect_claude."""
        cfg = _configure(sandbox_home)
        cfg["clis"]["claude_code"]["endpoint_min_interval_seconds"] = 90
        acs_config.save_config(cfg)
        seen = {}

        def fake_collect_claude(home, last, min_interval=None):
            seen["min_interval"] = min_interval
            return {"windows": [], "error": "mocked"}

        monkeypatch.setattr(poller, "collect_claude", fake_collect_claude)
        poller.poll_once(home=sandbox_home, now=NOW)
        assert seen["min_interval"] == 90

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
        pane_text = "5h limit: 61% used\nWeekly limit: 22% used\n1 reset available\n"
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
        assert codex["resets_available"] == 1
        assert ("agentic-cli-status", "codex") in captured
        pane_file = poller.state_dir(sandbox_home) / "panes" / "codex.txt"
        assert pane_file.read_text() == pane_text

    def test_pane_fallback_keeps_endpoint_throttle_fields(
        self, sandbox_home, monkeypatch
    ):
        """A pane replacement must not discard the endpoint throttle marks.

        If checked_at/retry_after_until were dropped with the replaced
        snapshot, the next tick would call the Claude endpoint again — every
        poll tick — guaranteeing a 429 lockout at second-scale polling rates.
        """
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        claude_calls = []

        def fake_collect_claude(home, last, min_interval=None):
            claude_calls.append(NOW)
            return {
                "source": "claude_oauth_endpoint",
                "observed_at": None,
                "plan": None,
                "windows": [],
                "error": "HTTP 429",
                "checked_at": NOW,
                "retry_after_until": NOW + 300,
            }

        monkeypatch.setattr(poller, "collect_claude", fake_collect_claude)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        monkeypatch.setattr(
            poller,
            "refresh_and_capture",
            lambda session, cli_id: (
                "Current session: 8% used\nCurrent week: 39% used\n"
                if cli_id == "claude_code"
                else None
            ),
        )
        status = poller.poll_once(home=sandbox_home, now=NOW)
        claude = status["clis"]["claude_code"]
        assert claude["source"] == "tmux_pane"
        assert claude["checked_at"] == NOW
        assert claude["retry_after_until"] == NOW + 300

    def test_carried_error_is_logged_once_not_per_tick(
        self, sandbox_home, monkeypatch, caplog
    ):
        """An unchanged CLI error must not be re-warned on every poll tick."""
        _configure(sandbox_home)

        def fake_collect_claude(home, last, min_interval=None):
            return {"windows": [], "error": "HTTP 429", "checked_at": NOW}

        monkeypatch.setattr(poller, "collect_claude", fake_collect_claude)
        # The centralized logger does not propagate to root, so caplog's
        # handler is attached to it directly for the duration of the test.
        poller.LOGGER.addHandler(caplog.handler)
        try:
            first = poller.poll_once(home=sandbox_home, now=NOW)
            poller.poll_once(home=sandbox_home, previous=first, now=NOW + 5)
        finally:
            poller.LOGGER.removeHandler(caplog.handler)
        warnings = [
            record for record in caplog.records
            if "HTTP 429" in record.getMessage()
        ]
        assert len(warnings) == 1

    def test_pane_reading_survives_later_stale_headless_ticks(
        self, sandbox_home, monkeypatch
    ):
        """A fresh pane capture must not flap back to older headless data.

        Codex only appends rate_limits events when a prompt runs, so after a
        /usage pane refresh the headless rollout reading stays older until
        real usage resumes. The next ticks must keep the newer pane reading
        (and the headless plan) instead of reverting for ~60 s at a time.
        """
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        # Day-old rollout: stale at NOW, triggers the pane refresh.
        write_codex_rollout(
            sandbox_home,
            "2026/07/02",
            "rollout-old.jsonl",
            [codex_token_count_event(55, 65, timestamp="2026-07-02T10:41:25Z")],
        )
        monkeypatch.setattr(
            poller,
            "refresh_and_capture",
            lambda session, cli_id: (
                "5h limit: 99% left\nWeekly limit: 34% left\n"
                if cli_id == "codex"
                else None
            ),
        )
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        first = poller.poll_once(home=sandbox_home, now=NOW)
        codex = first["clis"]["codex"]
        assert codex["source"] == "tmux_pane"
        assert codex["windows"][0]["used_percent"] == 1.0
        assert codex["plan"] == "plus"
        # Next tick: headless still returns the old rollout, no tmux refresh
        # (throttled) — the newer pane reading must be carried forward.
        second = poller.poll_once(home=sandbox_home, previous=first, now=NOW + 5)
        codex = second["clis"]["codex"]
        assert codex["source"] == "tmux_pane"
        assert codex["windows"][0]["used_percent"] == 1.0
        assert codex["plan"] == "plus"
        assert codex["stale"] is False
        # Once real usage writes a newer rollout event, headless wins again.
        write_codex_rollout(
            sandbox_home,
            "2026/07/03",
            "rollout-new.jsonl",
            [codex_token_count_event(12, 40, timestamp="2026-07-03T10:41:35Z")],
        )
        third = poller.poll_once(home=sandbox_home, previous=second, now=NOW + 10)
        codex = third["clis"]["codex"]
        assert codex["source"] == "codex_sessions"
        assert codex["windows"][0]["used_percent"] == 12.0

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
