"""Shared fixtures: a sandboxed home with XDG dirs and fake CLI data trees."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


@pytest.fixture
def sandbox_home(tmp_path, monkeypatch):
    """Provide an isolated home directory with XDG env vars pointed inside."""
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(home / ".config"))
    monkeypatch.setenv("XDG_STATE_HOME", str(home / ".local" / "state"))
    monkeypatch.setenv("ACS_LOG_DIR", str(tmp_path / "logs"))
    return home


def write_codex_rollout(home: Path, day: str, name: str, events: list[dict]) -> Path:
    """Write a fake Codex rollout jsonl file under the sessions tree.

    Args:
        home: Sandbox home directory.
        day: ``YYYY/MM/DD`` day path.
        name: Rollout file name.
        events: JSON events, one per line.
    """
    day_dir = home / ".codex" / "sessions" / day
    day_dir.mkdir(parents=True, exist_ok=True)
    target = day_dir / name
    target.write_text(
        "\n".join(json.dumps(event) for event in events) + "\n", encoding="utf-8"
    )
    return target


def codex_token_count_event(
    primary_pct: float,
    secondary_pct: float,
    timestamp: str = "2026-07-03T10:41:25.148Z",
    plan: str = "plus",
) -> dict:
    """Build a Codex token_count event with a rate_limits payload."""
    return {
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {"total_token_usage": {"total_tokens": 1}},
            "rate_limits": {
                "limit_id": "codex",
                "primary": {
                    "used_percent": primary_pct,
                    "window_minutes": 300,
                    "resets_at": 1783080635,
                },
                "secondary": {
                    "used_percent": secondary_pct,
                    "window_minutes": 10080,
                    "resets_at": 1783577400,
                },
                "plan_type": plan,
            },
        },
    }


def write_grok_log(home: Path, name: str, records: list[dict]) -> Path:
    """Write a fake Grok CLI log file with JSON records, one per line."""
    logs_dir = home / ".grok" / "logs"
    logs_dir.mkdir(parents=True, exist_ok=True)
    target = logs_dir / name
    target.write_text(
        "\n".join(json.dumps(record) for record in records) + "\n", encoding="utf-8"
    )
    return target


def grok_billing_record(
    percent: float,
    ts: str = "2026-07-03T10:36:48.739Z",
    tier: str = "SuperGrok",
) -> dict:
    """Build a Grok 'billing: fetched credits config' log record."""
    return {
        "ts": ts,
        "src": "shell",
        "lvl": "info",
        "msg": "billing: fetched credits config",
        "ctx": {
            "config": {
                "creditUsagePercent": percent,
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-07-02T06:04:00.368952+00:00",
                    "end": "2026-07-09T06:04:00.368952+00:00",
                },
                "billingPeriodEnd": "2026-07-09T06:04:00.368952+00:00",
            },
            "subscriptionTier": tier,
        },
    }


def write_claude_credentials(
    home: Path, token: str = "sk-test", expires_in_ms: int = 3_600_000
) -> Path:
    """Write a fake Claude Code OAuth credentials file."""
    import time

    creds_dir = home / ".claude"
    creds_dir.mkdir(parents=True, exist_ok=True)
    target = creds_dir / ".credentials.json"
    target.write_text(
        json.dumps(
            {
                "claudeAiOauth": {
                    "accessToken": token,
                    "expiresAt": int(time.time() * 1000) + expires_in_ms,
                    "subscriptionType": "max",
                }
            }
        ),
        encoding="utf-8",
    )
    return target
