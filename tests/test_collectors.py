"""Tests for the headless collectors and the tmux pane parser."""

from __future__ import annotations

import io
import json

import pytest

from agentic_cli_status import collectors
from tests.conftest import (
    codex_token_count_event,
    grok_billing_record,
    write_claude_credentials,
    write_codex_rollout,
    write_grok_log,
)


class TestCollectCodex:
    def test_reads_last_rate_limits_event_of_newest_rollout(self, sandbox_home):
        write_codex_rollout(
            sandbox_home,
            "2026/07/02",
            "rollout-old.jsonl",
            [codex_token_count_event(10, 5)],
        )
        write_codex_rollout(
            sandbox_home,
            "2026/07/03",
            "rollout-new.jsonl",
            [codex_token_count_event(20, 15), codex_token_count_event(57, 43)],
        )
        result = collectors.collect_codex(sandbox_home)
        assert result["error"] is None
        assert result["plan"] == "plus"
        assert [w["used_percent"] for w in result["windows"]] == [57.0, 43.0]
        assert result["windows"][0] == {
            "label": "5h",
            "used_percent": 57.0,
            "resets_at": 1783080635,
        }
        assert result["observed_at"] == pytest.approx(1783075285, abs=2)

    def test_skips_rollouts_without_rate_limits(self, sandbox_home):
        write_codex_rollout(
            sandbox_home,
            "2026/07/03",
            "rollout-z-no-limits.jsonl",
            [{"timestamp": "t", "payload": {"type": "user_message"}}],
        )
        write_codex_rollout(
            sandbox_home,
            "2026/07/03",
            "rollout-a-limits.jsonl",
            [codex_token_count_event(30, 20)],
        )
        result = collectors.collect_codex(sandbox_home)
        assert [w["used_percent"] for w in result["windows"]] == [30.0, 20.0]

    def test_missing_sessions_dir_reports_error(self, sandbox_home):
        result = collectors.collect_codex(sandbox_home)
        assert result["windows"] == []
        assert "sessions" in result["error"]

    def test_malformed_lines_are_ignored(self, sandbox_home):
        day_dir = sandbox_home / ".codex" / "sessions" / "2026/07/03"
        day_dir.mkdir(parents=True)
        (day_dir / "rollout-x.jsonl").write_text(
            'not json "rate_limits"\n'
            + json.dumps(codex_token_count_event(11, 12))
            + "\n",
            encoding="utf-8",
        )
        result = collectors.collect_codex(sandbox_home)
        assert [w["used_percent"] for w in result["windows"]] == [11.0, 12.0]


class TestCollectGrok:
    def test_reads_last_billing_record(self, sandbox_home):
        write_grok_log(
            sandbox_home,
            "grok.log",
            [
                {"msg": "other"},
                grok_billing_record(42.5),
                grok_billing_record(100.0),
            ],
        )
        result = collectors.collect_grok(sandbox_home)
        assert result["error"] is None
        assert result["plan"] == "SuperGrok"
        assert result["windows"] == [
            {
                "label": "week",
                "used_percent": 100.0,
                "resets_at": 1783577040,
            }
        ]

    def test_missing_logs_dir_reports_error(self, sandbox_home):
        result = collectors.collect_grok(sandbox_home)
        assert result["windows"] == []
        assert "logs" in result["error"]

    def test_no_billing_records_reports_error(self, sandbox_home):
        write_grok_log(sandbox_home, "grok.log", [{"msg": "startup"}])
        result = collectors.collect_grok(sandbox_home)
        assert "billing" in result["error"]


class _FakeResponse(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


class TestCollectClaude:
    def _opener_returning(self, payload, calls):
        def opener(request, timeout):
            calls.append(request)
            return _FakeResponse(json.dumps(payload).encode())

        return opener

    def test_fetches_windows_from_endpoint(self, sandbox_home):
        write_claude_credentials(sandbox_home)
        calls = []
        payload = {
            "five_hour": {"utilization": 12.5, "resets_at": "2026-07-03T15:00:00Z"},
            "seven_day": {"utilization": 40, "resets_at": 1783577400},
        }
        result = collectors.collect_claude(
            sandbox_home, opener=self._opener_returning(payload, calls), now=1000.0
        )
        assert result["error"] is None
        assert result["plan"] == "max"
        assert result["observed_at"] == 1000.0
        assert [w["label"] for w in result["windows"]] == ["5h", "week"]
        assert result["windows"][1]["resets_at"] == 1783577400
        request = calls[0]
        assert request.get_header("Authorization") == "Bearer sk-test"
        assert request.get_header("User-agent").startswith("claude-code/")

    def test_used_percentage_field_variant(self, sandbox_home):
        write_claude_credentials(sandbox_home)
        payload = {"five_hour": {"used_percentage": 77}}
        result = collectors.collect_claude(
            sandbox_home, opener=self._opener_returning(payload, []), now=1.0
        )
        assert result["windows"][0]["used_percent"] == 77.0

    def test_endpoint_throttled_by_min_interval(self, sandbox_home):
        write_claude_credentials(sandbox_home)
        calls = []
        opener = self._opener_returning({"five_hour": {"utilization": 5}}, calls)
        first = collectors.collect_claude(sandbox_home, opener=opener, now=1000.0)
        second = collectors.collect_claude(
            sandbox_home, last_result=first, opener=opener, now=1100.0
        )
        third = collectors.collect_claude(
            sandbox_home, last_result=second, opener=opener, now=1000.0 + 181
        )
        assert second is first
        assert len(calls) == 2
        assert third["observed_at"] == 1181.0

    def test_failed_attempts_are_also_throttled(self, sandbox_home):
        write_claude_credentials(sandbox_home)
        calls = []

        def failing_opener(request, timeout):
            calls.append(request)
            raise collectors.urllib.error.URLError("offline")

        first = collectors.collect_claude(
            sandbox_home, opener=failing_opener, now=1000.0
        )
        second = collectors.collect_claude(
            sandbox_home, last_result=first, opener=failing_opener, now=1005.0
        )
        assert first["error"] and first["checked_at"] == 1000.0
        assert second is first
        assert len(calls) == 1

    def test_missing_credentials_reports_error(self, sandbox_home):
        result = collectors.collect_claude(sandbox_home, now=1.0)
        assert result["windows"] == []
        assert "credentials" in result["error"]

    def test_expired_token_reports_error_without_calling_endpoint(self, sandbox_home):
        write_claude_credentials(sandbox_home, expires_in_ms=-1000)
        calls = []
        result = collectors.collect_claude(
            sandbox_home, opener=self._opener_returning({}, calls), now=1.0
        )
        assert calls == []
        assert "expired" in result["error"]


class TestParsePane:
    def test_codex_status_pane(self):
        text = (
            "  /status\n"
            "  Token usage: 32,950 total\n"
            "  5h limit:     [########  ] 57% used (resets 14:30)\n"
            "  Weekly limit: [####      ] 43% used (resets Mon)\n"
        )
        result = collectors.parse_pane("codex", text, now=5.0)
        assert result["observed_at"] == 5.0
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("5h", 57.0),
            ("week", 43.0),
        ]

    def test_claude_usage_pane(self):
        text = (
            "Current session (5-hour limit)\n"
            "  12% used · resets at 3pm\n"
            "Current week (all models)\n"
            "  40% used · resets Tuesday\n"
        )
        result = collectors.parse_pane("claude_code", text, now=1.0)
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("5h", 12.0),
            ("week", 40.0),
        ]

    def test_grok_usage_pane(self):
        text = "Credits used this week: 100% (period ends Jul 9)\n"
        result = collectors.parse_pane("grok", text, now=1.0)
        assert result["windows"] == [
            {"label": "week", "used_percent": 100.0, "resets_at": None}
        ]

    def test_unparseable_pane_reports_error(self):
        result = collectors.parse_pane("codex", "welcome to codex\n", now=1.0)
        assert result["windows"] == []
        assert result["error"]


class TestTailLines:
    def test_small_file_returns_all_lines(self, tmp_path):
        target = tmp_path / "small.log"
        target.write_text("one\ntwo\nthree\n", encoding="utf-8")
        assert collectors._tail_lines(target) == ["one", "two", "three"]

    def test_large_file_reads_only_complete_tail_lines(self, tmp_path):
        target = tmp_path / "big.log"
        lines = [f"line-{i:06d}" for i in range(1000)]
        target.write_text("\n".join(lines) + "\n", encoding="utf-8")
        tail = collectors._tail_lines(target, max_bytes=200)
        assert tail == lines[-len(tail):]
        assert 0 < len(tail) < 1000
        assert all(line.startswith("line-") for line in tail)

    def test_grok_collector_finds_record_in_large_log(self, sandbox_home):
        records = [{"msg": f"noise-{i}"} for i in range(50)]
        records.append(grok_billing_record(66))
        write_grok_log(sandbox_home, "grok.log", records)
        result = collectors.collect_grok(sandbox_home)
        assert result["windows"][0]["used_percent"] == 66.0
