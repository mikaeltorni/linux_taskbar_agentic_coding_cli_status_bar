"""Tests for the poll tick: aggregation, staleness, and the status file."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from agentic_cli_status import config as acs_config
from agentic_cli_status import poller
from tests.conftest import (
    GROK_BILLING_PERIOD_END,
    GROK_POST_RESET_PERIOD_END,
    OPENROUTER_ANALYTICS_FIXTURE,
    OPENCODE_GO_HTML_FIXTURE,
    codex_token_count_event,
    grok_billing_record,
    grok_billing_record_no_percent,
    write_codex_rollout,
    write_grok_log,
    write_grok_pane,
    write_opencode_go_credentials,
    write_openrouter_env,
)

# 2026-07-03T10:41:25Z, matching the conftest codex event timestamp.
NOW = 1783075290.0

GROK_PANE_45_LEFT = (
    "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
    "(period ends Jul 12) ─╯\n"
)
STALE_AFTER = 1800


def _grok_billing(used_percent: float, observed_at: float) -> dict:
    return {
        "source": "grok_logs",
        "observed_at": observed_at,
        "plan": "SuperGrok",
        "windows": [
            {"label": "week", "used_percent": used_percent, "resets_at": None},
        ],
        "error": None,
    }


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
        assert "resets_available" not in codex
        grok = status["clis"]["grok"]
        assert grok["windows"][0]["used_percent"] == 88.0
        # claude_code is default-off; the status entry still exists but is disabled.
        claude = status["clis"]["claude_code"]
        assert not claude["enabled"]
        on_disk = json.loads(poller.status_path(sandbox_home).read_text())
        assert on_disk["clis"]["codex"]["windows"][0]["used_percent"] == 57.0

    def test_previous_status_survives_daemon_restart(self, sandbox_home):
        """Daemon startup reloads metadata that is unavailable headlessly."""
        target = poller.status_path(sandbox_home)
        target.parent.mkdir(parents=True)
        target.write_text(json.dumps({
            "generated_at": NOW,
            "poll_interval_seconds": 5,
            "clis": {"codex": {"resets_available": 1}},
        }))

        previous = poller._load_previous_status(sandbox_home)

        assert previous["clis"]["codex"]["resets_available"] == 1

    def test_opencode_go_twice_writes_three_windows(self, sandbox_home, monkeypatch):
        """Two poll ticks with a fixture opener produce stable opencode_go windows."""
        _configure(sandbox_home)
        write_opencode_go_credentials(sandbox_home)
        calls = []

        def fake_opener(request, timeout=20):
            calls.append(1)
            from tests.conftest import OPENCODE_GO_HTML_FIXTURE
            from tests.test_collectors import _FakeResponse

            return _FakeResponse(OPENCODE_GO_HTML_FIXTURE.encode("utf-8"))

        monkeypatch.setattr(
            "agentic_cli_status.collectors.urllib.request.urlopen", fake_opener
        )
        first = poller.poll_once(home=sandbox_home, now=NOW)
        second = poller.poll_once(
            home=sandbox_home, previous=first, now=NOW + 301
        )
        for status in (first, second):
            og = status["clis"]["opencode_go"]
            assert og["enabled"] and not og.get("error")
            assert [(w["label"], w["used_percent"]) for w in og["windows"]] == [
                ("rolling", 0.0),
                ("week", 17.0),
                ("month", 8.0),
            ]
        assert len(calls) == 2

    def test_poll_once_with_openrouter_and_tmux_enabled(
        self, sandbox_home, monkeypatch
    ):
        """Default tmux-on config must not crash when openrouter is enabled."""
        cfg = acs_config.load_config()
        cfg["tmux"]["enabled"] = True
        for cli in acs_config.CLI_IDS:
            cfg["clis"][cli]["enabled"] = cli == "openrouter"
        acs_config.save_config(cfg)
        write_openrouter_env(sandbox_home)

        def fake_fetch(api_key, date_str, model_slug):
            return OPENROUTER_ANALYTICS_FIXTURE

        monkeypatch.setattr(
            "agentic_cli_status.collectors.fetch_analytics_rows", fake_fetch
        )
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=1783502400.0)
        assert status["clis"]["openrouter"]["windows"][0]["used_percent"] == 42.0
        status = poller.poll_once(home=sandbox_home, now=1783502400.0)
        assert status["clis"]["openrouter"]["windows"][0]["used_percent"] == 42.0

    def test_main_once_openrouter_entrypoint(self, sandbox_home, monkeypatch):
        """Real ``python -m agentic_cli_status.poller --once`` path."""
        cfg = acs_config.load_config()
        cfg["tmux"]["enabled"] = False
        for cli in acs_config.CLI_IDS:
            cfg["clis"][cli]["enabled"] = cli == "openrouter"
        acs_config.save_config(cfg)
        write_openrouter_env(sandbox_home)

        def fake_fetch(api_key, date_str, model_slug):
            return OPENROUTER_ANALYTICS_FIXTURE

        monkeypatch.setattr(
            "agentic_cli_status.collectors.fetch_analytics_rows", fake_fetch
        )
        frozen_now = 1783502400.0
        monkeypatch.setattr(poller.time, "time", lambda: frozen_now)
        assert poller.main(["--once"]) == 0
        on_disk = json.loads(poller.status_path(sandbox_home).read_text())
        window = on_disk["clis"]["openrouter"]["windows"][0]
        assert window["used_percent"] == 42.0
        assert window["resets_at"] == 1783555200

    def test_openrouter_twice_writes_stable_daily_window(self, sandbox_home, monkeypatch):
        """Two poll ticks with a fixture fetch produce stable openrouter windows."""
        _configure(sandbox_home)
        write_openrouter_env(sandbox_home)
        calls = []

        def fake_fetch(api_key, date_str, model_slug):
            calls.append(date_str)
            return OPENROUTER_ANALYTICS_FIXTURE

        monkeypatch.setattr(
            "agentic_cli_status.collectors.fetch_analytics_rows", fake_fetch
        )
        first = poller.poll_once(home=sandbox_home, now=1783502400.0)
        second = poller.poll_once(
            home=sandbox_home, previous=first, now=1783502459.0
        )
        for status in (first, second):
            or_snap = status["clis"]["openrouter"]
            assert or_snap["enabled"] and not or_snap.get("error")
            window = or_snap["windows"][0]
            assert window["label"] == "day"
            assert window["used_percent"] == 42.0
            assert window["resets_at"] == 1783555200
        assert len(calls) == 1

    def test_claude_endpoint_interval_reaches_collector(
        self, sandbox_home, monkeypatch
    ):
        """The configured endpoint interval is passed to collect_claude."""
        cfg = _configure(sandbox_home)
        cfg["clis"]["claude_code"]["enabled"] = True
        cfg["clis"]["claude_code"]["endpoint_min_interval_seconds"] = 90
        acs_config.save_config(cfg)
        seen = {}

        def fake_collect_claude(home, last, min_interval=None, now=None):
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

    def test_fresh_codex_headless_data_merges_periodic_reset_allowance(
        self, sandbox_home, monkeypatch
    ):
        """Auto mode polls /usage metadata without replacing fresher windows."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_codex_rollout(
            sandbox_home,
            "2026/07/03",
            "rollout-new.jsonl",
            [codex_token_count_event(57, 43)],
        )
        captures = []

        def _capture(session, cli_id):
            captures.append(cli_id)
            if cli_id == "codex":
                return "5h limit: 1% used\nWeekly limit: 2% used\n1 reset available\n"
            return None

        monkeypatch.setattr(poller, "refresh_and_capture", _capture)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        first = poller.poll_once(home=sandbox_home, now=NOW)
        codex = first["clis"]["codex"]
        assert codex["source"] == "codex_sessions"
        assert codex["windows"][0]["used_percent"] == 57.0
        assert codex["resets_available"] == 1
        assert codex["resets_checked_at"] == NOW
        assert captures.count("codex") == 1

        # Headless ticks do not contain reset metadata, so retain the latest
        # authoritative /usage result until the next metadata refresh.
        second = poller.poll_once(home=sandbox_home, previous=first, now=NOW + 5)
        assert second["clis"]["codex"]["resets_available"] == 1
        assert captures.count("codex") == 1

        # A later /usage heatmap has no allowance metadata, so retain the last
        # authoritative count rather than treating the missing text as zero.
        third = poller.poll_once(home=sandbox_home, previous=second, now=NOW + 61)
        assert third["clis"]["codex"]["resets_available"] == 1

        def _capture_without_reset(session, cli_id):
            return (
                "5h limit: 57% used\nWeekly limit: 43% used\n"
                if cli_id == "codex" else None
            )

        monkeypatch.setattr(poller, "refresh_and_capture", _capture_without_reset)
        fourth = poller.poll_once(home=sandbox_home, previous=third, now=NOW + 122)
        assert fourth["clis"]["codex"]["resets_available"] == 1
        assert fourth["clis"]["codex"]["resets_checked_at"] == NOW + 61

    def test_pane_fallback_keeps_endpoint_throttle_fields(
        self, sandbox_home, monkeypatch
    ):
        """A pane replacement must not discard the endpoint throttle marks.

        If checked_at/retry_after_until were dropped with the replaced
        snapshot, the next tick would call the Claude endpoint again — every
        poll tick — guaranteeing a 429 lockout at second-scale polling rates.
        """
        cfg = _configure(sandbox_home)
        cfg["clis"]["claude_code"]["enabled"] = True
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        claude_calls = []

        def fake_collect_claude(home, last, min_interval=None, now=None):
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
        cfg = _configure(sandbox_home)
        cfg["clis"]["claude_code"]["enabled"] = True
        acs_config.save_config(cfg)

        def fake_collect_claude(home, last, min_interval=None, now=None):
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

    def test_grok_live_capture_authoritative_over_billing(
        self, sandbox_home, monkeypatch
    ):
        """A successful live /usage capture wins without any saved pane."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        pane_text = (
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
            "(period ends Jul 12) ─╯\n"
        )
        monkeypatch.setattr(
            poller,
            "refresh_and_capture",
            lambda session, cli_id: pane_text if cli_id == "grok" else None,
        )
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "tmux_pane"
        assert grok["windows"][0]["used_percent"] == 55.0
        assert grok["windows"][0]["resets_at"] != 1783577040
        assert grok["observed_at"] == NOW

    def test_grok_headless_ignores_fresher_prior_tmux_reading(
        self, sandbox_home, monkeypatch
    ):
        """Headless mode uses billing even when last console is newer."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        cfg["clis"]["grok"]["source"] = "headless"
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(88, ts="2026-07-03T09:00:00.000Z")],
        )
        monkeypatch.setattr(poller, "refresh_and_capture", lambda *a, **k: None)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        previous = {
            "clis": {
                "grok": {
                    "source": "tmux_pane",
                    "observed_at": NOW + 3600,
                    "windows": [
                        {"label": "week", "used_percent": 55.0, "resets_at": 1},
                    ],
                },
            },
        }
        status = poller.poll_once(
            home=sandbox_home, previous=previous, now=NOW
        )
        grok = status["clis"]["grok"]
        assert grok["source"] == "grok_logs"
        assert grok["windows"][0]["used_percent"] == 88.0

    def test_grok_tmux_disabled_uses_billing_despite_saved_pane(
        self, sandbox_home, monkeypatch
    ):
        """Saved panes are ignored when the tmux console is disabled."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = False
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        write_grok_pane(
            sandbox_home,
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
            "(period ends Jul 12) ─╯\n",
        )
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "grok_logs"
        assert grok["windows"][0]["used_percent"] == 99.0

    def test_grok_console_backfills_reset_from_billing_when_pane_omits_it(
        self, sandbox_home, monkeypatch
    ):
        """Pane usage wins; billing period end fills a missing resets_at."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(72, ts="2026-07-03T09:00:00.000Z")],
        )
        monkeypatch.setattr(
            poller,
            "refresh_and_capture",
            lambda session, cli_id: (
                "Weekly limit left: 0%\n" if cli_id == "grok" else None
            ),
        )
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "tmux_pane"
        assert grok["windows"][0]["used_percent"] == 100.0
        assert grok["windows"][0]["resets_at"] == GROK_BILLING_PERIOD_END

    def test_grok_uses_saved_pane_when_live_capture_unparseable(
        self, sandbox_home, monkeypatch
    ):
        """A saved pane is used when the live tmux capture carries no usage text."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        pane_path = write_grok_pane(
            sandbox_home,
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
            "(period ends Jul 12) ─╯\n",
        )
        saved_at = NOW - 120
        os.utime(pane_path, (saved_at, saved_at))
        monkeypatch.setattr(
            poller,
            "refresh_and_capture",
            lambda *a, **k: "welcome to grok\n",
        )
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "tmux_pane"
        assert grok["windows"][0]["used_percent"] == 55.0
        assert grok["observed_at"] == saved_at

    def test_grok_falls_back_when_saved_pane_stale(
        self, sandbox_home, monkeypatch
    ):
        """An on-disk pane older than stale_after yields to billing logs."""
        import os

        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        pane_path = write_grok_pane(
            sandbox_home,
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
            "(period ends Jul 12) ─╯\n",
        )
        stale_at = NOW - 2000
        os.utime(pane_path, (stale_at, stale_at))
        monkeypatch.setattr(poller, "refresh_and_capture", lambda *a, **k: None)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "grok_logs"
        assert grok["windows"][0]["used_percent"] == 99.0

    def test_grok_falls_back_to_billing_when_console_missing(
        self, sandbox_home, monkeypatch
    ):
        """Headless billing is used when live capture and saved pane are absent."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        monkeypatch.setattr(poller, "refresh_and_capture", lambda *a, **k: None)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "grok_logs"
        assert grok["windows"][0]["used_percent"] == 99.0
        assert grok["windows"][0]["resets_at"] == GROK_BILLING_PERIOD_END

    def test_grok_post_reset_no_percent_stays_fresh_headless(
        self, sandbox_home, monkeypatch
    ):
        """A post-reset billing record with no percent must not break the display.

        After a weekly reset Grok's newest billing records omit
        ``creditUsagePercent``; the prior week's records still carry it but are
        expired. The poller must publish the fresh 0%-used window (the taskbar
        then shows 100% left) rather than falling through to "no billing
        records found" (empty windows, stale), which left the indicator broken.
        """
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [
                grok_billing_record(91.0, ts="2026-07-08T23:30:47.372Z"),
                grok_billing_record_no_percent(ts="2026-07-09T09:10:47.647Z"),
            ],
        )
        # No live capture / pane available: headless billing must still be valid.
        monkeypatch.setattr(poller, "refresh_and_capture", lambda *a, **k: None)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        status = poller.poll_once(home=sandbox_home, now=NOW)
        grok = status["clis"]["grok"]
        assert grok["source"] == "grok_logs"
        assert grok["windows"] == [
            {
                "label": "week",
                "used_percent": 0.0,
                "resets_at": GROK_POST_RESET_PERIOD_END,
            }
        ]
        assert grok["error"] is None
        assert grok["stale"] is False

    def test_grok_falls_back_when_last_pane_stale(
        self, sandbox_home, monkeypatch
    ):
        """A stale prior console reading yields to current billing data."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        monkeypatch.setattr(poller, "refresh_and_capture", lambda *a, **k: None)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        previous = {
            "clis": {
                "grok": {
                    "source": "tmux_pane",
                    "observed_at": NOW - 2000,
                    "windows": [
                        {"label": "week", "used_percent": 55.0, "resets_at": 1},
                    ],
                },
            },
        }
        status = poller.poll_once(
            home=sandbox_home, previous=previous, now=NOW
        )
        grok = status["clis"]["grok"]
        assert grok["source"] == "grok_logs"
        assert grok["windows"][0]["used_percent"] == 99.0

    def test_grok_once_cli_live_capture_twice(
        self, sandbox_home, monkeypatch
    ):
        """``main(['--once'])`` twice uses live capture (no saved pane)."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        monkeypatch.setattr(
            poller,
            "refresh_and_capture",
            lambda session, cli_id: (
                GROK_PANE_45_LEFT if cli_id == "grok" else None
            ),
        )
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        poll_epoch = {"value": NOW}
        monkeypatch.setattr(
            poller.time, "time", lambda: poll_epoch["value"]
        )

        assert poller.main(["--once"]) == 0
        first = json.loads(poller.status_path(sandbox_home).read_text())
        poll_epoch["value"] = NOW + 5
        assert poller.main(["--once"]) == 0
        second = json.loads(poller.status_path(sandbox_home).read_text())

        for status in (first, second):
            grok = status["clis"]["grok"]
            assert grok["source"] == "tmux_pane"
            assert grok["windows"][0]["used_percent"] == 55.0
            assert grok["windows"][0]["resets_at"] != 1783577040
        assert first["generated_at"] != second["generated_at"]
        assert first["clis"]["grok"]["observed_at"] == NOW
        assert second["clis"]["grok"]["observed_at"] == NOW + 5

    def test_grok_once_cli_uses_saved_pane_twice(
        self, sandbox_home, monkeypatch
    ):
        """``poller --once`` reads saved pane captures without in-process carry."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        write_grok_pane(
            sandbox_home,
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
            "(period ends Jul 12) ─╯\n",
        )
        monkeypatch.setattr(poller, "refresh_and_capture", lambda *a, **k: None)
        monkeypatch.setattr(poller, "ensure_console", lambda *a, **k: True)
        poll_times = iter([1_000_000.0, 1_000_005.0])

        def _fake_time():
            try:
                return next(poll_times)
            except StopIteration:
                return 1_000_010.0

        monkeypatch.setattr(poller.time, "time", _fake_time)

        assert poller.main(["--once"]) == 0
        first = json.loads(poller.status_path(sandbox_home).read_text())
        assert poller.main(["--once"]) == 0
        second = json.loads(poller.status_path(sandbox_home).read_text())

        for status in (first, second):
            grok = status["clis"]["grok"]
            assert grok["source"] == "tmux_pane"
            assert grok["windows"][0]["used_percent"] == 55.0
            assert grok["windows"][0]["resets_at"] != 1783577040
        assert first["generated_at"] != second["generated_at"]
        assert (
            first["clis"]["grok"]["observed_at"]
            == second["clis"]["grok"]["observed_at"]
        )
        assert (
            first["clis"]["grok"]["windows"][0]["resets_at"]
            == second["clis"]["grok"]["windows"][0]["resets_at"]
        )

    def test_grok_once_cli_live_capture_subprocess(
        self, sandbox_home, monkeypatch
    ):
        """Subprocess ``--once`` runs exercise live capture via patched entry."""
        cfg = _configure(sandbox_home)
        cfg["tmux"]["enabled"] = True
        acs_config.save_config(cfg)
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(99, ts="2026-07-08T10:00:00.000Z")],
        )
        repo = Path(__file__).resolve().parents[1]
        scratch = Path("/tmp/grok-goal-8d7dff26d62b/implementer")
        scratch.mkdir(parents=True, exist_ok=True)
        runner = scratch / "grok_once_live_runner.py"
        runner.write_text(
            f"""
import json
import os
import sys
from pathlib import Path

repo = Path({str(repo)!r})
sys.path.insert(0, str(repo))
os.environ["HOME"] = {str(sandbox_home)!r}
os.environ["XDG_CONFIG_HOME"] = {str(sandbox_home / ".config")!r}
os.environ["XDG_STATE_HOME"] = {str(sandbox_home / ".local/state")!r}

import agentic_cli_status.poller as poller

PANE = {GROK_PANE_45_LEFT!r}
poller.refresh_and_capture = lambda session, cli_id: PANE if cli_id == "grok" else None
poller.ensure_console = lambda *a, **k: True

poll_epoch = {{"value": {NOW!r}}}
poller.time.time = lambda: poll_epoch["value"]

scratch = Path({str(scratch)!r})
for idx in (1, 2):
    if idx == 2:
        poll_epoch["value"] = {NOW + 5!r}
    assert poller.main(["--once"]) == 0
    status = json.loads(poller.status_path().read_text())
    (scratch / f"grok_poll_{{idx}}.json").write_text(
        json.dumps(status, indent=2) + "\\n", encoding="utf-8"
    )
grok = status["clis"]["grok"]
assert grok["source"] == "tmux_pane"
assert grok["windows"][0]["used_percent"] == 55.0
assert grok["observed_at"] == {NOW + 5!r}
""",
            encoding="utf-8",
        )
        proc = subprocess.run(
            [sys.executable, str(runner)],
            cwd=repo,
            capture_output=True,
            text=True,
            check=False,
        )
        assert proc.returncode == 0, proc.stderr
        first = json.loads((scratch / "grok_poll_1.json").read_text())
        second = json.loads((scratch / "grok_poll_2.json").read_text())
        assert first["clis"]["grok"]["observed_at"] == NOW
        assert second["clis"]["grok"]["observed_at"] == NOW + 5
        assert first["generated_at"] != second["generated_at"]


class TestResolveGrokSnapshot:
    @pytest.mark.parametrize(
        "case",
        [
            pytest.param(
                {
                    "id": "headless_ignores_fresher_last_console",
                    "billing": _grok_billing(88.0, 1_000_000.0),
                    "last": {
                        "source": "tmux_pane",
                        "observed_at": NOW + 3600,
                        "windows": [
                            {"label": "week", "used_percent": 55.0, "resets_at": 1},
                        ],
                    },
                    "source_mode": "headless",
                    "tmux_enabled": True,
                    "expect_source": "grok_logs",
                    "expect_used": 88.0,
                    "expect_observed_at": 1_000_000.0,
                    "expect_resets_at": None,
                },
                id="headless_ignores_fresher_last_console",
            ),
            pytest.param(
                {
                    "id": "auto_live_authoritative",
                    "billing": _grok_billing(99.0, 1_780_000.0),
                    "live_text": GROK_PANE_45_LEFT,
                    "source_mode": "auto",
                    "tmux_enabled": True,
                    "tmux_throttled": False,
                    "expect_source": "tmux_pane",
                    "expect_used": 55.0,
                    "expect_observed_at": NOW,
                    "expect_resets_at_not": 1783577040,
                },
                id="auto_live_authoritative",
            ),
            pytest.param(
                {
                    "id": "saved_stale_falls_back_to_billing",
                    "billing": _grok_billing(99.0, 1_780_000.0),
                    "saved_text": GROK_PANE_45_LEFT,
                    "saved_at": NOW - 2000,
                    "source_mode": "auto",
                    "tmux_enabled": True,
                    "tmux_throttled": True,
                    "expect_source": "grok_logs",
                    "expect_used": 99.0,
                    "expect_observed_at": 1_780_000.0,
                    "expect_resets_at": None,
                },
                id="saved_stale_falls_back_to_billing",
            ),
            pytest.param(
                {
                    "id": "throttle_carries_fresh_last_console",
                    "billing": _grok_billing(99.0, 1_780_000.0),
                    "last": {
                        "source": "tmux_pane",
                        "observed_at": NOW - 120,
                        "windows": [
                            {"label": "week", "used_percent": 55.0, "resets_at": 42},
                        ],
                    },
                    "source_mode": "auto",
                    "tmux_enabled": True,
                    "tmux_throttled": True,
                    "expect_source": "tmux_pane",
                    "expect_used": 55.0,
                    "expect_observed_at": NOW - 120,
                    "expect_resets_at": 42,
                },
                id="throttle_carries_fresh_last_console",
            ),
            pytest.param(
                {
                    "id": "tmux_disabled_ignores_saved",
                    "billing": _grok_billing(99.0, 1_780_000.0),
                    "saved_text": GROK_PANE_45_LEFT,
                    "saved_at": NOW - 60,
                    "source_mode": "auto",
                    "tmux_enabled": False,
                    "expect_source": "grok_logs",
                    "expect_used": 99.0,
                    "expect_observed_at": 1_780_000.0,
                    "expect_resets_at": None,
                },
                id="tmux_disabled_ignores_saved",
            ),
        ],
    )
    def test_resolve_grok_snapshot_table(self, case):
        result = poller.resolve_grok_snapshot(
            case["billing"],
            live_text=case.get("live_text"),
            saved_text=case.get("saved_text"),
            saved_at=case.get("saved_at"),
            last=case.get("last"),
            source_mode=case["source_mode"],
            tmux_enabled=case["tmux_enabled"],
            now=NOW,
            stale_after=STALE_AFTER,
            tmux_throttled=case.get("tmux_throttled", False),
        )
        assert result["source"] == case["expect_source"]
        assert result["windows"][0]["used_percent"] == case["expect_used"]
        assert result["observed_at"] == case["expect_observed_at"]
        if "expect_resets_at" in case:
            assert result["windows"][0]["resets_at"] == case["expect_resets_at"]
        if "expect_resets_at_not" in case:
            assert result["windows"][0]["resets_at"] != case["expect_resets_at_not"]


class TestCarryWindowResets:
    def test_fills_missing_reset_from_first_fallback(self):
        snap = {"windows": [
            {"label": "week", "used_percent": 100.0, "resets_at": None},
        ]}
        headless = {"windows": [
            {"label": "week", "used_percent": 90.0, "resets_at": 4242},
        ]}
        result = poller._carry_window_resets(snap, headless, None)
        assert result["windows"][0]["resets_at"] == 4242
        # The input snapshot is not mutated in place.
        assert snap["windows"][0]["resets_at"] is None

    def test_own_reset_wins_and_earlier_fallback_is_preferred(self):
        snap = {"windows": [
            {"label": "5h", "used_percent": 50.0, "resets_at": 111},
            {"label": "week", "used_percent": 100.0, "resets_at": None},
        ]}
        headless = {"windows": [
            {"label": "week", "used_percent": 100.0, "resets_at": 999},
        ]}
        last = {"windows": [
            {"label": "week", "used_percent": 100.0, "resets_at": 555},
        ]}
        result = poller._carry_window_resets(snap, headless, last)
        assert result["windows"][0]["resets_at"] == 111  # own reset kept
        assert result["windows"][1]["resets_at"] == 999  # headless beats last

    def test_no_fallback_reset_leaves_snapshot_object_unchanged(self):
        snap = {"windows": [
            {"label": "week", "used_percent": 100.0, "resets_at": None},
        ]}
        result = poller._carry_window_resets(snap, None, {"windows": []})
        assert result is snap
