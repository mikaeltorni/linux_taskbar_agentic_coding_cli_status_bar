"""Shared fixtures: a sandboxed home with XDG dirs and fake CLI data trees."""

from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

# Epoch for ``grok_billing_record``'s default ``currentPeriod.end``.
GROK_BILLING_PERIOD_END = 1783577040
# Epoch for ``grok_billing_record_no_percent``'s default post-reset period end.
GROK_POST_RESET_PERIOD_END = 1784181840


@pytest.fixture
def sandbox_home(tmp_path, monkeypatch):
    """Provide an isolated home directory with XDG env vars pointed inside."""
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(home / ".config"))
    monkeypatch.setenv("XDG_STATE_HOME", str(home / ".local" / "state"))
    monkeypatch.setenv("ACS_LOG_DIR", str(tmp_path / "logs"))
    monkeypatch.delenv("OPENROUTER_MANAGEMENT_API_KEY", raising=False)
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


def write_grok_pane(home: Path, text: str) -> Path:
    """Write a saved Grok tmux pane capture under the poller state directory."""
    from agentic_cli_status.config import state_dir

    pane_dir = state_dir(home) / "panes"
    pane_dir.mkdir(parents=True, exist_ok=True)
    target = pane_dir / "grok.txt"
    target.write_text(text, encoding="utf-8")
    return target


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


def grok_billing_record_no_percent(
    ts: str = "2026-07-09T09:10:47.647Z",
    tier: str = "SuperGrok",
    period_start: str = "2026-07-09T06:04:00.368952+00:00",
    period_end: str = "2026-07-16T06:04:00.368952+00:00",
) -> dict:
    """Build a post-reset Grok record with no ``creditUsagePercent`` field.

    Grok publishes ``fetched credits config`` records for the new weekly
    period immediately after a reset, omitting ``creditUsagePercent`` because
    the fresh period has seen no usage yet. ``collect_grok`` must treat this
    as a real 0%-used reading rather than discarding it.
    """
    return {
        "ts": ts,
        "src": "shell",
        "lvl": "info",
        "msg": "billing: fetched credits config",
        "ctx": {
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": period_start,
                    "end": period_end,
                },
                "billingPeriodStart": period_start,
                "billingPeriodEnd": period_end,
                "historyLen": 0,
            },
            "subscriptionTier": tier,
        },
    }


def write_opencode_go_credentials(
    home: Path,
    workspace_id: str = "wrk_test123",
    auth_cookie: str = "Fe26.2**testcookie",
) -> Path:
    """Write a fake opencode-go-usage.json credentials file."""
    config_dir = home / ".config" / "opencode"
    config_dir.mkdir(parents=True, exist_ok=True)
    target = config_dir / "opencode-go-usage.json"
    target.write_text(
        json.dumps(
            {
                "workspaceId": workspace_id,
                "authCookie": auth_cookie,
                "refreshIntervalMinutes": 5,
            }
        ),
        encoding="utf-8",
    )
    return target


OPENCODE_GO_HTML_FIXTURE = (
    "<html><body>"
    "rollingUsage:$R[0]={usagePercent:0,resetInSec:17820}"
    "weeklyUsage:$R[1]={usagePercent:17,resetInSec:234000}"
    "monthlyUsage:$R[2]={usagePercent:8,resetInSec:2588400}"
    "</body></html>"
)

OPENROUTER_ANALYTICS_FIXTURE = [
    {
        "model": "nvidia/nemotron-3-ultra-550b-a55b-20260604",
        "variant": "free",
        "request_count": "420",
    },
    {
        "model": "qwen/qwen3.5-flash-20260224",
        "variant": "standard",
        "request_count": "3",
    },
]

OPENROUTER_ACTIVITY_FIXTURE = {
    "data": [
        {
            "date": "2026-07-08",
            "model": "nvidia/nemotron-3-ultra-550b-a55b:free",
            "model_permaslug": "nvidia/nemotron-3-ultra-550b-a55b:free",
            "endpoint_id": "ep-1",
            "provider_name": "Nvidia",
            "requests": 420,
            "usage": 0.0,
            "byok_usage_inference": 0.0,
            "prompt_tokens": 1000,
            "completion_tokens": 500,
            "reasoning_tokens": 0,
        },
        {
            "date": "2026-07-08",
            "model": "openai/gpt-4.1",
            "model_permaslug": "openai/gpt-4.1",
            "endpoint_id": "ep-2",
            "provider_name": "OpenAI",
            "requests": 3,
            "usage": 0.01,
            "byok_usage_inference": 0.0,
            "prompt_tokens": 50,
            "completion_tokens": 25,
            "reasoning_tokens": 0,
        },
        {
            "date": "2026-07-07",
            "model": "nvidia/nemotron-3-ultra-550b-a55b:free",
            "model_permaslug": "nvidia/nemotron-3-ultra-550b-a55b:free",
            "endpoint_id": "ep-1",
            "provider_name": "Nvidia",
            "requests": 999,
            "usage": 0.0,
            "byok_usage_inference": 0.0,
            "prompt_tokens": 1000,
            "completion_tokens": 500,
            "reasoning_tokens": 0,
        },
    ]
}


def write_openrouter_env(
    home: Path,
    api_key: str = "sk-or-v1-test-management-key",
) -> Path:
    """Write a fake OpenRouter management-key .env file."""
    config_dir = home / ".config" / "agentic-cli-status"
    config_dir.mkdir(parents=True, exist_ok=True)
    target = config_dir / ".env"
    target.write_text(
        f"OPENROUTER_MANAGEMENT_API_KEY={api_key}\n",
        encoding="utf-8",
    )
    target.chmod(0o600)
    return target


def write_claude_credentials(
    home: Path,
    token: str = "sk-test",
    expires_in_ms: int = 3_600_000,
    refresh_token: str | None = None,
    refresh_expires_in_ms: int = 86_400_000 * 30,
    scopes: list[str] | None = None,
) -> Path:
    """Write a fake Claude Code OAuth credentials file.

    Args:
        home: Sandbox home directory.
        token: Access token to store.
        expires_in_ms: Milliseconds from now until the access token expires.
            Negative values produce an already-expired token.
        refresh_token: Refresh token to store. When omitted no refresh token
            is written (mirroring the original minimal credentials shape).
        refresh_expires_in_ms: Milliseconds until the refresh token expires.
        scopes: OAuth scope list; defaults to the production default set.
    """
    import time

    creds_dir = home / ".claude"
    creds_dir.mkdir(parents=True, exist_ok=True)
    target = creds_dir / ".credentials.json"
    oauth: dict = {
        "accessToken": token,
        "expiresAt": int(time.time() * 1000) + expires_in_ms,
        "subscriptionType": "max",
    }
    if refresh_token is not None:
        oauth["refreshToken"] = refresh_token
        oauth["refreshTokenExpiresAt"] = (
            int(time.time() * 1000) + refresh_expires_in_ms
        )
        oauth["scopes"] = scopes or [
            "user:file_upload",
            "user:inference",
            "user:mcp_servers",
            "user:profile",
            "user:sessions:claude_code",
        ]
    target.write_text(json.dumps({"claudeAiOauth": oauth}), encoding="utf-8")
    return target
