"""Tests for the headless collectors and the tmux pane parser."""

from __future__ import annotations

import datetime as dt
import io
import json

import pytest

from agentic_cli_status import collectors
from tests.conftest import (
    GROK_BILLING_PERIOD_END,
    GROK_POST_RESET_PERIOD_END,
    OPENCODE_GO_HTML_FIXTURE,
    codex_token_count_event,
    grok_billing_record,
    grok_billing_record_no_percent,
    write_claude_credentials,
    write_codex_rollout,
    write_grok_log,
    write_opencode_go_credentials,
    write_openrouter_env,
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
        assert "resets_available" not in result
        assert [w["used_percent"] for w in result["windows"]] == [57.0, 43.0]
        assert result["windows"][0] == {
            "label": "5h",
            "used_percent": 57.0,
            "resets_at": 1783080635,
        }
        assert result["observed_at"] == pytest.approx(1783075285, abs=2)

    def test_reports_most_depleted_window_when_concurrent_5h_windows_interleave(
        self, sandbox_home
    ):
        # Codex v0.142+ can open several concurrent 5h windows, each with its
        # own resets_at. A lightly-used concurrent window is logged *after* the
        # heavily-used one, so the naive "last event in file" approach would
        # report the fresh window (near 100% left) and make the bar look reset.
        base = "2026/07/03"
        events = [
            # User's real window, drained to 25%, resets at 1783080635.
            codex_token_count_event(25, 4, timestamp="2026-07-03T10:40:00Z"),
            # Fresh concurrent 5h window with a different reset, only 3% used,
            # logged last — this is the trap.
            {
                "timestamp": "2026-07-03T10:45:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {"total_token_usage": {"total_tokens": 1}},
                    "rate_limits": {
                        "limit_id": "codex",
                        "primary": {
                            "used_percent": 3.0,
                            "window_minutes": 300,
                            "resets_at": 1783085000,
                        },
                        "secondary": {
                            "used_percent": 1.0,
                            "window_minutes": 10080,
                            "resets_at": 1783579000,
                        },
                        "plan_type": "plus",
                    },
                },
            },
        ]
        write_codex_rollout(sandbox_home, base, "rollout-x.jsonl", events)
        result = collectors.collect_codex(sandbox_home)
        by_label = {w["label"]: w for w in result["windows"]}
        # Exactly one window per label, and the most-depleted (real) window wins.
        assert list(by_label) == ["5h", "week"]
        assert by_label["5h"]["used_percent"] == 25.0
        assert by_label["week"]["used_percent"] == 4.0

    def test_uses_newest_snapshot_when_reset_epochs_describe_same_window(
        self, sandbox_home
    ):
        """A later post-limit reading must replace an earlier exhausted one.

        Codex can report reset epochs a few seconds apart for the same account
        window across concurrent sessions. After one session reaches 100%, a
        later snapshot for that same window may show the current session's
        lower usage. Selecting the largest percentage across both snapshots
        would keep the stale exhausted reading visible until the reset.
        """
        events = [
            {
                "timestamp": "2026-07-09T23:24:02Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "primary": {
                            "used_percent": 100.0,
                            "resets_at": 1783653454,
                        },
                        "secondary": {
                            "used_percent": 16.0,
                            "resets_at": 1784240254,
                        },
                        "plan_type": "plus",
                    }
                },
            },
            {
                "timestamp": "2026-07-09T23:24:20Z",
                "type": "event_msg",
                "payload": {
                    "rate_limits": {
                        "primary": {
                            "used_percent": 6.0,
                            "resets_at": 1783653479,
                        },
                        "secondary": {
                            "used_percent": 1.0,
                            "resets_at": 1784240279,
                        },
                        "plan_type": "plus",
                    }
                },
            },
        ]
        write_codex_rollout(sandbox_home, "2026/07/10", "rollout-x.jsonl", events)

        result = collectors.collect_codex(sandbox_home)
        by_label = {window["label"]: window for window in result["windows"]}

        assert by_label["5h"] == {
            "label": "5h",
            "used_percent": 6.0,
            "resets_at": 1783653479,
        }
        assert by_label["week"] == {
            "label": "week",
            "used_percent": 1.0,
            "resets_at": 1784240279,
        }
        assert result["observed_at"] == pytest.approx(1783639460, abs=2)

    def test_skips_window_missing_used_percent(self, sandbox_home):
        # A window object without a usable used_percent must not be published
        # (it would otherwise default to 0.0 and render as 100% left).
        events = [
            {
                "timestamp": "2026-07-03T10:40:00Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {"total_token_usage": {"total_tokens": 1}},
                    "rate_limits": {
                        "limit_id": "codex",
                        "primary": {"window_minutes": 300, "resets_at": 1783080635},
                        "secondary": {
                            "used_percent": 12.0,
                            "window_minutes": 10080,
                            "resets_at": 1783577400,
                        },
                        "plan_type": "plus",
                    },
                },
            },
        ]
        write_codex_rollout(sandbox_home, "2026/07/03", "rollout-x.jsonl", events)
        result = collectors.collect_codex(sandbox_home)
        assert [w["label"] for w in result["windows"]] == ["week"]
        assert result["windows"][0]["used_percent"] == 12.0

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


class TestParseCodexPane:
    def test_reads_available_resets_from_usage(self):
        pane = (
            "5h limit: 61% used\n"
            "Weekly limit: 22% used\n"
            "1 reset available\n"
        )
        result = collectors.parse_pane("codex", pane, now=1000.0)
        assert result["resets_available"] == 1

    def test_heatmap_without_reset_count_is_not_authoritative(self):
        result = collectors.parse_pane(
            "codex",
            "Token activity   last 12 months\n"
            "5h limit: 61% used\nWeekly limit: 22% used\n",
            now=1000.0,
        )
        assert "resets_available" not in result

    def test_reads_reset_count_even_when_usage_breakdown_is_missing(self):
        result = collectors.parse_pane(
            "codex", "You have 2 usage limit resets available.\n", now=1000.0
        )
        assert result["windows"] == []
        assert result["resets_available"] == 2


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
                "resets_at": GROK_BILLING_PERIOD_END,
            }
        ]

    def test_billing_period_end_from_current_period(self, sandbox_home):
        write_grok_log(
            sandbox_home,
            "grok.log",
            [grok_billing_record(12.0)],
        )
        result = collectors.collect_grok(sandbox_home)
        assert result["windows"][0]["resets_at"] == GROK_BILLING_PERIOD_END

    def test_post_reset_record_without_percent_is_0_used(self, sandbox_home):
        """A reset record missing ``creditUsagePercent`` is 0% used, not dropped.

        Grok publishes ``fetched credits config`` for the new weekly period
        right after a reset without ``creditUsagePercent`` (no usage yet). The
        collector must surface it as a fresh 0%-used window instead of skipping
        to an expired prior-week record and reporting "no billing records".
        """
        write_grok_log(
            sandbox_home,
            "grok.log",
            [
                grok_billing_record(91.0, ts="2026-07-08T23:30:47.372Z"),
                grok_billing_record_no_percent(ts="2026-07-09T09:10:47.647Z"),
            ],
        )
        result = collectors.collect_grok(sandbox_home)
        assert result["error"] is None
        assert result["windows"] == [
            {
                "label": "week",
                "used_percent": 0.0,
                "resets_at": GROK_POST_RESET_PERIOD_END,
            }
        ]
        assert result["observed_at"] is not None

    def test_only_post_reset_records_still_reports_window(self, sandbox_home):
        """Even a log whose newest records all lack the field stays valid."""
        write_grok_log(
            sandbox_home,
            "grok.log",
            [
                grok_billing_record_no_percent(ts="2026-07-09T09:10:47.647Z"),
                grok_billing_record_no_percent(ts="2026-07-09T09:10:49.499Z"),
            ],
        )
        result = collectors.collect_grok(sandbox_home)
        assert result["error"] is None
        assert result["windows"] == [
            {
                "label": "week",
                "used_percent": 0.0,
                "resets_at": GROK_POST_RESET_PERIOD_END,
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

    def test_fetches_fable_scoped_weekly_limit(self, sandbox_home):
        """Claude's limits array should expose the model-scoped Fable usage."""
        write_claude_credentials(sandbox_home)
        payload = {
            "five_hour": {"utilization": 12.5},
            "seven_day": {"utilization": 40},
            "limits": [
                {
                    "kind": "session",
                    "percent": 12.5,
                    "scope": None,
                },
                {
                    "kind": "weekly_scoped",
                    "percent": 62,
                    "resets_at": "2026-07-05T14:00:00Z",
                    "scope": {
                        "model": {"display_name": "Fable", "id": None},
                        "surface": None,
                    },
                },
            ],
        }
        result = collectors.collect_claude(
            sandbox_home, opener=self._opener_returning(payload, []), now=1.0
        )
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("5h", 12.5),
            ("week", 40.0),
            ("fable", 62.0),
        ]
        assert result["windows"][2]["resets_at"] == 1783260000

    def test_ignores_unknown_or_duplicate_scoped_limits(self, sandbox_home):
        """Scoped rows without a model and legacy duplicates are not rendered."""
        write_claude_credentials(sandbox_home)
        payload = {
            "seven_day_opus": {"utilization": 25},
            "limits": [
                {"kind": "weekly_scoped", "percent": 25,
                 "scope": {"model": {"display_name": "Opus"}}},
                {"kind": "weekly_scoped", "percent": 10, "scope": None},
            ],
        }
        result = collectors.collect_claude(
            sandbox_home, opener=self._opener_returning(payload, []), now=1.0
        )
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("opus", 25.0),
        ]

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

    def _http_error(self, code, headers=None):
        import email.message

        message = email.message.Message()
        for key, value in (headers or {}).items():
            message[key] = value
        return collectors.urllib.error.HTTPError(
            collectors.CLAUDE_USAGE_URL, code, "err", message, io.BytesIO(b"")
        )

    def test_error_preserves_previous_windows(self, sandbox_home):
        """A failed refresh must keep showing the last good reading."""
        write_claude_credentials(sandbox_home)
        good = collectors.collect_claude(
            sandbox_home,
            opener=self._opener_returning({"five_hour": {"utilization": 5}}, []),
            now=1000.0,
        )

        def failing_opener(request, timeout):
            raise collectors.urllib.error.URLError("offline")

        result = collectors.collect_claude(
            sandbox_home, last_result=good, opener=failing_opener, now=1200.0
        )
        assert result["error"]
        assert result["windows"] == good["windows"]
        assert result["observed_at"] == 1000.0
        assert result["checked_at"] == 1200.0

    def test_429_preserves_windows_and_honors_retry_after(self, sandbox_home):
        """A 429 keeps the last reading and blocks retries for Retry-After."""
        write_claude_credentials(sandbox_home)
        calls = []
        good = collectors.collect_claude(
            sandbox_home,
            opener=self._opener_returning({"five_hour": {"utilization": 5}}, []),
            now=1000.0,
        )

        def limited_opener(request, timeout):
            calls.append(request)
            raise self._http_error(429, {"Retry-After": "300"})

        limited = collectors.collect_claude(
            sandbox_home, last_result=good, opener=limited_opener, now=1200.0
        )
        assert limited["error"] == "HTTP 429"
        assert limited["windows"] == good["windows"]
        assert limited["retry_after_until"] == 1500.0
        # Past the min interval but inside the Retry-After lockout: no attempt.
        blocked = collectors.collect_claude(
            sandbox_home, last_result=limited, opener=limited_opener, now=1450.0
        )
        assert blocked is limited
        assert len(calls) == 1
        # After the lockout expires the endpoint is attempted again.
        collectors.collect_claude(
            sandbox_home, last_result=limited, opener=limited_opener, now=1501.0
        )
        assert len(calls) == 2

    def test_throttle_applies_to_pane_replaced_snapshot(self, sandbox_home):
        """checked_at carried on a pane snapshot still blocks endpoint calls."""
        write_claude_credentials(sandbox_home)
        calls = []
        opener = self._opener_returning({"five_hour": {"utilization": 5}}, calls)
        pane_last = {
            "source": "tmux_pane",
            "windows": [{"label": "5h", "used_percent": 8.0, "resets_at": None}],
            "checked_at": 1000.0,
        }
        blocked = collectors.collect_claude(
            sandbox_home, last_result=pane_last, opener=opener, now=1100.0
        )
        assert blocked is pane_last
        assert calls == []

    def test_custom_min_interval_is_honored(self, sandbox_home):
        """The configured endpoint interval overrides the 180 s default."""
        write_claude_credentials(sandbox_home)
        calls = []
        opener = self._opener_returning({"five_hour": {"utilization": 5}}, calls)
        first = collectors.collect_claude(
            sandbox_home, opener=opener, now=1000.0, min_interval=60
        )
        blocked = collectors.collect_claude(
            sandbox_home, last_result=first, opener=opener, now=1059.0,
            min_interval=60,
        )
        collectors.collect_claude(
            sandbox_home, last_result=first, opener=opener, now=1061.0,
            min_interval=60,
        )
        assert blocked is first
        assert len(calls) == 2

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

    def test_expired_token_keeps_last_reading_without_alarming(self, sandbox_home):
        """A briefly expired token self-heals; don't flash an error over data."""
        write_claude_credentials(sandbox_home, expires_in_ms=-1000)
        last = {
            "windows": [{"label": "5h", "used_percent": 20.0, "resets_at": None}],
            "observed_at": 0.5,
            "plan": "pro",
        }
        result = collectors.collect_claude(
            sandbox_home, last_result=last, opener=self._opener_returning({}, []),
            now=1.0,
        )
        assert result["error"] is None
        assert result["windows"] == last["windows"]


class TestClaudeOAuthRefresh:
    """Headless OAuth token refresh so the indicator recovers post-reboot."""

    def _opener_returning(self, payload, calls):
        def opener(request, timeout):
            calls.append(request)
            return _FakeResponse(json.dumps(payload).encode())

        return opener

    def _refresh_opener(self, payload, calls):
        def opener(request, timeout):
            calls.append(request)
            body = json.loads(request.data.decode("utf-8"))
            assert body["grant_type"] == "refresh_token"
            assert body["client_id"] == collectors.CLAUDE_OAUTH_CLIENT_ID
            return _FakeResponse(json.dumps(payload).encode())

        return opener

    def test_refreshes_expired_token_then_fetches_usage(self, sandbox_home):
        write_claude_credentials(
            sandbox_home, expires_in_ms=-1000, refresh_token="rt-old"
        )
        refresh_calls = []
        refresh_opener = self._refresh_opener(
            {
                "access_token": "sk-new",
                "refresh_token": "rt-new",
                "expires_in": 28800,
                "refresh_token_expires_in": 2592000,
            },
            refresh_calls,
        )
        usage_calls = []
        usage_opener = self._opener_returning(
            {"five_hour": {"utilization": 7}, "seven_day": {"utilization": 41}},
            usage_calls,
        )
        result = collectors.collect_claude(
            sandbox_home,
            opener=usage_opener,
            refresh_opener=refresh_opener,
            is_running=lambda: False,
            now=1000.0,
        )
        assert result["error"] is None
        assert result["observed_at"] == 1000.0
        assert [w["label"] for w in result["windows"]] == ["5h", "week"]
        # The usage call used the rotated access token.
        assert usage_calls[0].get_header("Authorization") == "Bearer sk-new"
        # The rotated credentials were written back to disk.
        import json as _json
        from pathlib import Path

        on_disk = _json.loads(
            (sandbox_home / ".claude" / ".credentials.json").read_text()
        )["claudeAiOauth"]
        assert on_disk["accessToken"] == "sk-new"
        assert on_disk["refreshToken"] == "rt-new"
        assert on_disk["expiresAt"] == int((1000.0 + 28800) * 1000)
        assert on_disk["refreshTokenExpiresAt"] == int((1000.0 + 2592000) * 1000)
        assert len(refresh_calls) == 1

    def test_skips_refresh_while_claude_is_running(self, sandbox_home):
        write_claude_credentials(
            sandbox_home, expires_in_ms=-1000, refresh_token="rt-old"
        )
        refresh_calls = []

        def refresh_opener(request, timeout):
            refresh_calls.append(request)
            return _FakeResponse(b"{}")

        last = {
            "windows": [{"label": "5h", "used_percent": 50.0, "resets_at": None}],
            "observed_at": 900.0,
            "plan": "pro",
        }
        result = collectors.collect_claude(
            sandbox_home,
            last_result=last,
            opener=self._opener_returning({}, []),
            refresh_opener=refresh_opener,
            is_running=lambda: True,
            now=1000.0,
        )
        # No refresh attempt, no usage attempt: carried reading shown as-is.
        assert refresh_calls == []
        assert result["windows"] == last["windows"]
        assert result["error"] is None

    def test_refresh_failure_carries_last_reading(self, sandbox_home):
        write_claude_credentials(
            sandbox_home, expires_in_ms=-1000, refresh_token="rt-old"
        )

        def refresh_opener(request, timeout):
            raise collectors.urllib.error.HTTPError(
                collectors.CLAUDE_OAUTH_TOKEN_URL,
                400,
                "bad",
                __import__("email").message.Message(),
                io.BytesIO(b'{"error":"invalid_grant"}'),
            )

        last = {
            "windows": [{"label": "week", "used_percent": 50.0, "resets_at": None}],
            "observed_at": 900.0,
            "plan": "pro",
        }
        result = collectors.collect_claude(
            sandbox_home,
            last_result=last,
            opener=self._opener_returning({}, []),
            refresh_opener=refresh_opener,
            is_running=lambda: False,
            now=1000.0,
        )
        assert result["windows"] == last["windows"]
        # The carried reading hides the expired-token transient.
        assert result["error"] is None

    def test_refresh_without_refresh_token_does_not_call_endpoint(self, sandbox_home):
        write_claude_credentials(sandbox_home, expires_in_ms=-1000)
        refresh_calls = []

        def refresh_opener(request, timeout):
            refresh_calls.append(request)
            return _FakeResponse(b"{}")

        result = collectors.collect_claude(
            sandbox_home,
            opener=self._opener_returning({}, []),
            refresh_opener=refresh_opener,
            is_running=lambda: False,
            now=1.0,
        )
        assert refresh_calls == []
        assert "expired" in result["error"]

    def test_expired_refresh_token_is_not_used(self, sandbox_home):
        import time

        write_claude_credentials(
            sandbox_home,
            expires_in_ms=-1000,
            refresh_token="rt-old",
            refresh_expires_in_ms=-1000,
        )
        refresh_calls = []

        def refresh_opener(request, timeout):
            refresh_calls.append(request)
            return _FakeResponse(b"{}")

        result = collectors.collect_claude(
            sandbox_home,
            opener=self._opener_returning({}, []),
            refresh_opener=refresh_opener,
            is_running=lambda: False,
            now=time.time(),
        )
        assert refresh_calls == []
        assert "expired" in result["error"]

    def test_is_running_detects_claude_binary(self, sandbox_home, tmp_path):
        proc = tmp_path / "proc"
        proc.mkdir()
        # Unrelated process should be ignored.
        (proc / "1").mkdir()
        (proc / "1" / "cmdline").write_bytes(b"/usr/bin/bash\x00")
        # The standalone claude binary should match.
        (proc / "2").mkdir()
        (proc / "2" / "cmdline").write_bytes(b"/home/u/.local/bin/claude\x00")
        assert collectors._claude_is_running(proc_root=proc) is True

    def test_is_running_detects_node_launched_claude_code(self, sandbox_home, tmp_path):
        proc = tmp_path / "proc"
        proc.mkdir()
        (proc / "1").mkdir()
        (proc / "1" / "cmdline").write_bytes(
            b"node\x00/opt/@anthropic-ai/claude-code/cli.js\x00"
        )
        assert collectors._claude_is_running(proc_root=proc) is True

    def test_is_running_false_when_no_claude(self, sandbox_home, tmp_path):
        proc = tmp_path / "proc"
        proc.mkdir()
        (proc / "1").mkdir()
        (proc / "1" / "cmdline").write_bytes(b"/usr/bin/python3\x00script.py\x00")
        assert collectors._claude_is_running(proc_root=proc) is False



class TestParsePane:
    def test_codex_status_pane_used_variant(self):
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

    def test_codex_status_pane_left_variant(self):
        # Real v0.142 rendering: percentages are what is LEFT, and the weekly
        # reset wraps onto the next line.
        text = (
            "│  5h limit:             [████████████████████] 99% left (resets 20:12) │\n"
            "│  Weekly limit:         [██████████░░░░░░░░░░] 49% left                │\n"
            "│                        (resets 09:10 on 9 Jul)                        │\n"
        )
        result = collectors.parse_pane("codex", text, now=5.0)
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("5h", 1.0),
            ("week", 51.0),
        ]

    def test_codex_pane_parses_reset_times_into_epochs(self):
        # On boot the tmux /status fallback is the only Codex source, so its
        # reset phrases must survive as real epochs (not be dropped to null).
        now = dt.datetime(2026, 7, 4, 17, 25).astimezone().timestamp()
        text = (
            "│  5h limit:      [████████████████████] 99% left (resets 20:12) │\n"
            "│  Weekly limit:  [██████████░░░░░░░░░░] 49% left                │\n"
            "│                 (resets 09:10 on 9 Jul)                        │\n"
        )
        result = collectors.parse_pane("codex", text, now=now)
        by_label = {w["label"]: w["resets_at"] for w in result["windows"]}
        # 5h clock-only reset resolves to today at 20:12 (still ahead of now).
        assert by_label["5h"] == int(
            dt.datetime(2026, 7, 4, 20, 12).astimezone().timestamp())
        # Weekly reset carries its explicit calendar day.
        assert by_label["week"] == int(
            dt.datetime(2026, 7, 9, 9, 10).astimezone().timestamp())

    def test_codex_pane_clock_already_past_rolls_to_tomorrow(self):
        # A bare rolling-window clock that already passed today names tomorrow.
        now = dt.datetime(2026, 7, 4, 21, 0).astimezone().timestamp()
        text = "  5h limit: [########  ] 57% used (resets 20:12)\n"
        result = collectors.parse_pane("codex", text, now=now)
        assert result["windows"][0]["resets_at"] == int(
            dt.datetime(2026, 7, 5, 20, 12).astimezone().timestamp())

    def test_codex_pane_day_only_reset_stays_null(self):
        # "resets Mon" carries no clock, so guessing a time would be wrong.
        now = dt.datetime(2026, 7, 4, 17, 25).astimezone().timestamp()
        text = "  Weekly limit: [####      ] 43% used (resets Mon)\n"
        result = collectors.parse_pane("codex", text, now=now)
        assert result["windows"][0]["resets_at"] is None

    def test_grok_weekly_limit_left_pane(self):
        text = "╰────────────────── Weekly limit left: 0% · Composer 2.5 ─╯\n"
        result = collectors.parse_pane("grok", text, now=1.0)
        assert result["windows"] == [
            {"label": "week", "used_percent": 100.0, "resets_at": None}
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

    def test_claude_usage_pane_includes_fable(self):
        text = (
            "Current session (5-hour limit)\n"
            "  12% used · resets at 3pm\n"
            "Current week (all models)\n"
            "  40% used · resets Tuesday\n"
            "Current week (Fable only)\n"
            "  62% used · resets Tuesday\n"
        )
        result = collectors.parse_pane("claude_code", text, now=1.0)
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("5h", 12.0),
            ("week", 40.0),
            ("fable", 62.0),
        ]

    def test_grok_usage_pane(self):
        now = dt.datetime(2026, 7, 4, 12, 0).astimezone().timestamp()
        text = "Credits used this week: 100% (period ends Jul 9)\n"
        result = collectors.parse_pane("grok", text, now=now)
        expected_reset = int(
            dt.datetime(2026, 7, 9, 23, 59, 59).astimezone().timestamp()
        )
        assert result["windows"] == [
            {"label": "week", "used_percent": 100.0, "resets_at": expected_reset}
        ]

    def test_grok_weekly_limit_left_with_period_ends(self):
        now = dt.datetime(2026, 7, 4, 12, 0).astimezone().timestamp()
        text = (
            "╰────────────────── Weekly limit left: 45% · Composer 2.5 "
            "(period ends Jul 12) ─╯\n"
        )
        result = collectors.parse_pane("grok", text, now=now)
        expected_reset = int(
            dt.datetime(2026, 7, 12, 23, 59, 59).astimezone().timestamp()
        )
        assert result["windows"] == [
            {"label": "week", "used_percent": 55.0, "resets_at": expected_reset}
        ]

    def test_grok_period_ends_in_duration(self):
        now = 1_000_000.0
        text = "Weekly limit left: 10% (period ends in 2d 5h)\n"
        result = collectors.parse_pane("grok", text, now=now)
        assert result["windows"][0]["used_percent"] == 90.0
        assert result["windows"][0]["resets_at"] == int(now + 2 * 86400 + 5 * 3600)

    def test_unparseable_pane_reports_error(self):
        result = collectors.parse_pane("codex", "welcome to codex\n", now=1.0)
        assert result["windows"] == []
        assert result["error"]

    def test_opencode_go_ogc_usage_pane(self):
        text = (
            "OpenCode Go Usage:\n"
            "  Rolling:  0%  (resets in 4h 57m)\n"
            "  Weekly:   17% (resets in 2d 18h)\n"
            "  Monthly:  8%  (resets in 29d 22h)\n"
        )
        now = 1000.0
        result = collectors.parse_pane("opencode_go", text, now=now)
        assert result["error"] is None
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("rolling", 0.0),
            ("week", 17.0),
            ("month", 8.0),
        ]
        by_label = {w["label"]: w["resets_at"] for w in result["windows"]}
        assert by_label["rolling"] == int(now + 4 * 3600 + 57 * 60)
        assert by_label["week"] == int(now + 2 * 86400 + 18 * 3600)
        assert by_label["month"] == int(now + 29 * 86400 + 22 * 3600)


class TestCollectOpenCodeGo:
    def _opener_returning(self, html: str, calls: list):
        def opener(request, timeout=20):
            calls.append(request.full_url)
            return _FakeResponse(html.encode("utf-8"))

        return opener

    def test_parses_html_fixture(self, sandbox_home):
        write_opencode_go_credentials(sandbox_home)
        calls = []
        now = 2000.0
        result = collectors.collect_opencode_go(
            sandbox_home,
            opener=self._opener_returning(OPENCODE_GO_HTML_FIXTURE, calls),
            now=now,
        )
        assert result["error"] is None
        assert result["source"] == "opencode_go_page"
        assert len(calls) == 1
        assert "wrk_test123" in calls[0]
        assert [(w["label"], w["used_percent"]) for w in result["windows"]] == [
            ("rolling", 0.0),
            ("week", 17.0),
            ("month", 8.0),
        ]
        assert result["windows"][0]["resets_at"] == int(now + 17820)
        assert result["windows"][1]["resets_at"] == int(now + 234000)
        assert result["windows"][2]["resets_at"] == int(now + 2588400)

    def test_throttles_within_fetch_interval(self, sandbox_home):
        write_opencode_go_credentials(sandbox_home)
        calls = []
        opener = self._opener_returning(OPENCODE_GO_HTML_FIXTURE, calls)
        first = collectors.collect_opencode_go(
            sandbox_home, opener=opener, now=1000.0, min_interval=300
        )
        blocked = collectors.collect_opencode_go(
            sandbox_home,
            last_result=first,
            opener=opener,
            now=1200.0,
            min_interval=300,
        )
        collectors.collect_opencode_go(
            sandbox_home,
            last_result=first,
            opener=opener,
            now=1301.0,
            min_interval=300,
        )
        assert blocked is first
        assert len(calls) == 2

    def test_empty_last_reading_bypasses_fetch_throttle(self, sandbox_home):
        write_opencode_go_credentials(sandbox_home)
        calls = []
        opener = self._opener_returning(OPENCODE_GO_HTML_FIXTURE, calls)
        last = {
            "source": "tmux_pane",
            "windows": [],
            "error": "no usage percentages found in pane",
            "checked_at": 1000.0,
        }
        result = collectors.collect_opencode_go(
            sandbox_home,
            last_result=last,
            opener=opener,
            now=1100.0,
            min_interval=300,
        )
        assert result["windows"]
        assert len(calls) == 1

    def test_missing_credentials_reports_error(self, sandbox_home):
        result = collectors.collect_opencode_go(sandbox_home, now=1.0)
        assert result["windows"] == []
        assert "credentials" in result["error"]

    def test_failed_fetch_keeps_last_reading(self, sandbox_home):
        write_opencode_go_credentials(sandbox_home)
        last = {
            "windows": [{"label": "week", "used_percent": 17.0, "resets_at": 3000}],
            "observed_at": 1000.0,
        }

        def failing_opener(request, timeout=20):
            raise OSError("network down")

        result = collectors.collect_opencode_go(
            sandbox_home, last_result=last, opener=failing_opener, now=2000.0
        )
        assert result["windows"] == last["windows"]
        assert result["error"]


class TestCollectOpenRouter:
    def test_requests_today_honors_model_slug(self):
        from tests.conftest import OPENROUTER_ACTIVITY_FIXTURE

        total = collectors.openrouter_requests_today(
            OPENROUTER_ACTIVITY_FIXTURE, "tencent/hy3:free", "2026-07-08"
        )
        assert total == 420

    def test_short_slug_hy3_matches_hy3_free(self):
        from tests.conftest import OPENROUTER_ACTIVITY_FIXTURE

        total = collectors.openrouter_requests_today(
            OPENROUTER_ACTIVITY_FIXTURE, "hy3", "2026-07-08"
        )
        assert total == 420

    def test_used_percent_and_resets_at(self):
        from tests.conftest import OPENROUTER_ACTIVITY_FIXTURE

        now = 1783502400.0  # 2026-07-08 09:20 UTC
        windows = collectors.openrouter_window_from_activity(
            OPENROUTER_ACTIVITY_FIXTURE, "tencent/hy3:free", now, daily_limit=1000
        )
        assert len(windows) == 1
        assert windows[0]["label"] == "day"
        assert windows[0]["used_percent"] == 42.0
        assert windows[0]["resets_at"] == collectors.next_utc_midnight_epoch(now)

    def test_utc_midnight_boundary_before_and_after(self):
        before = 1783468800.0  # 2026-07-08 00:00:00 UTC
        after = 1783468801.0  # one second into the day
        assert collectors.next_utc_midnight_epoch(before) == 1783555200
        assert collectors.next_utc_midnight_epoch(after) == 1783555200

    def test_requests_from_analytics_rows(self):
        from tests.conftest import OPENROUTER_ANALYTICS_FIXTURE

        total = collectors.openrouter_requests_from_analytics(
            OPENROUTER_ANALYTICS_FIXTURE, "tencent/hy3:free"
        )
        assert total == 420

    def test_collect_openrouter_with_fixture_fetch(self, sandbox_home):
        from tests.conftest import OPENROUTER_ANALYTICS_FIXTURE, write_openrouter_env

        write_openrouter_env(sandbox_home)
        now = 1783502400.0
        calls = []

        def fetch_fn(api_key, date_str, model_slug):
            calls.append((api_key, date_str, model_slug))
            return OPENROUTER_ANALYTICS_FIXTURE

        result = collectors.collect_openrouter(
            sandbox_home,
            fetch_fn=fetch_fn,
            now=now,
            model_slug="tencent/hy3:free",
            daily_limit=1000,
        )
        assert result["error"] is None
        assert result["source"] == "openrouter_activity"
        assert calls == [
            ("sk-or-v1-test-management-key", "2026-07-08", "tencent/hy3:free")
        ]
        assert result["windows"][0]["used_percent"] == 42.0

    def test_throttles_within_fetch_interval(self, sandbox_home):
        from tests.conftest import OPENROUTER_ANALYTICS_FIXTURE, write_openrouter_env

        write_openrouter_env(sandbox_home)
        calls = []

        def fetch_fn(api_key, date_str, model_slug):
            calls.append(1)
            return OPENROUTER_ANALYTICS_FIXTURE

        first = collectors.collect_openrouter(
            sandbox_home, fetch_fn=fetch_fn, now=1000.0, min_interval=300
        )
        blocked = collectors.collect_openrouter(
            sandbox_home,
            last_result=first,
            fetch_fn=fetch_fn,
            now=1200.0,
            min_interval=300,
        )
        collectors.collect_openrouter(
            sandbox_home,
            last_result=first,
            fetch_fn=fetch_fn,
            now=1301.0,
            min_interval=300,
        )
        assert blocked is first
        assert len(calls) == 2

    def test_missing_key_reports_error(self, sandbox_home):
        result = collectors.collect_openrouter(sandbox_home, now=1.0)
        assert result["windows"] == []
        assert "management API key" in result["error"]

    def test_fetch_failure_does_not_raise(self, sandbox_home, monkeypatch):
        write_openrouter_env(sandbox_home)

        def raise_fetch(*args, **kwargs):
            raise RuntimeError("OpenRouter activity HTTP 403")

        monkeypatch.setattr(collectors, "fetch_analytics_rows", raise_fetch)
        result = collectors.collect_openrouter(sandbox_home, now=1000.0)
        assert "403" in result["error"]
        assert result["windows"] == []

    def test_activity_row_date_handles_timestamp(self):
        row = {"date": "2026-07-07 00:00:00"}
        assert collectors._activity_row_date(row) == "2026-07-07"


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

