"""Tests for config defaults, validation, and round-tripping."""

from __future__ import annotations

import json

from agentic_cli_status import config as acs_config


class TestLoadConfig:
    def test_defaults_when_missing(self, sandbox_home):
        cfg = acs_config.load_config()
        # The polling rate is expressed in SECONDS and defaults to 5.
        assert acs_config.DEFAULT_POLL_INTERVAL_SECONDS == 5
        assert (
            cfg["poll_interval_seconds"]
            == acs_config.DEFAULT_POLL_INTERVAL_SECONDS
        )
        assert set(cfg["clis"]) == set(acs_config.CLI_IDS)
        default_on = {"codex", "claude_code", "grok", "opencode_go", "openrouter"}
        for cli in acs_config.CLI_IDS:
            assert cfg["clis"][cli]["enabled"] is (cli in default_on)
            expected_source = "headless" if cli == "openrouter" else "auto"
            assert cfg["clis"][cli]["source"] == expected_source

    def test_cline_is_not_monitored(self):
        assert "cline" not in acs_config.CLI_IDS

    def test_partial_config_merges_over_defaults(self, sandbox_home):
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps(
                {
                    "poll_interval_seconds": 120,
                    "clis": {"grok": {"enabled": False}},
                }
            ),
            encoding="utf-8",
        )
        cfg = acs_config.load_config()
        assert cfg["poll_interval_seconds"] == 120
        assert cfg["clis"]["grok"]["enabled"] is False
        assert cfg["clis"]["grok"]["source"] == "auto"
        assert cfg["clis"]["codex"]["enabled"] is True

    def test_out_of_range_interval_falls_back(self, sandbox_home):
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(json.dumps({"poll_interval_seconds": 2}), encoding="utf-8")
        assert acs_config.load_config()["poll_interval_seconds"] == 5

    def test_opencode_go_fetch_interval_defaults_and_floor(self, sandbox_home):
        assert (
            acs_config.load_config()["clis"]["opencode_go"][
                "fetch_min_interval_seconds"
            ]
            == 300
        )
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps(
                {"clis": {"opencode_go": {"fetch_min_interval_seconds": 60}}}
            ),
            encoding="utf-8",
        )
        assert (
            acs_config.load_config()["clis"]["opencode_go"][
                "fetch_min_interval_seconds"
            ]
            == 300
        )

    def test_openrouter_defaults_and_validation(self, sandbox_home):
        openrouter = acs_config.load_config()["clis"]["openrouter"]
        assert openrouter["source"] == "headless"
        assert openrouter["model_slug"] == "nvidia/nemotron-3-ultra-550b-a55b:free"
        assert openrouter["daily_limit"] == 1000
        assert openrouter["fetch_min_interval_seconds"] == 60
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps(
                {
                    "clis": {
                        "openrouter": {
                            "fetch_min_interval_seconds": 30,
                            "daily_limit": 0,
                            "model_slug": "  ",
                        }
                    }
                }
            ),
            encoding="utf-8",
        )
        fixed = acs_config.load_config()["clis"]["openrouter"]
        assert fixed["fetch_min_interval_seconds"] == 60
        assert fixed["daily_limit"] == 1000
        assert fixed["model_slug"] == "nvidia/nemotron-3-ultra-550b-a55b:free"

    def test_claude_endpoint_interval_defaults_and_floor(self, sandbox_home):
        """The Claude endpoint interval defaults to 180 s and floors at 60 s."""
        assert (
            acs_config.load_config()["clis"]["claude_code"][
                "endpoint_min_interval_seconds"
            ]
            == 180
        )
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps(
                {"clis": {"claude_code": {"endpoint_min_interval_seconds": 5}}}
            ),
            encoding="utf-8",
        )
        assert (
            acs_config.load_config()["clis"]["claude_code"][
                "endpoint_min_interval_seconds"
            ]
            == 180
        )
        path.write_text(
            json.dumps(
                {"clis": {"claude_code": {"endpoint_min_interval_seconds": 60}}}
            ),
            encoding="utf-8",
        )
        assert (
            acs_config.load_config()["clis"]["claude_code"][
                "endpoint_min_interval_seconds"
            ]
            == 60
        )

    def test_openrouter_source_always_headless(self, sandbox_home):
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps({"clis": {"openrouter": {"source": "auto"}}}),
            encoding="utf-8",
        )
        assert acs_config.load_config()["clis"]["openrouter"]["source"] == "headless"

    def test_invalid_source_falls_back(self, sandbox_home):
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text(
            json.dumps({"clis": {"codex": {"source": "carrier-pigeon"}}}),
            encoding="utf-8",
        )
        assert acs_config.load_config()["clis"]["codex"]["source"] == "auto"

    def test_corrupt_file_yields_defaults(self, sandbox_home):
        path = acs_config.config_path()
        path.parent.mkdir(parents=True)
        path.write_text("{nope", encoding="utf-8")
        assert acs_config.load_config()["poll_interval_seconds"] == 5


class TestSaveConfig:
    def test_round_trip(self, sandbox_home):
        cfg = acs_config.load_config()
        cfg["poll_interval_seconds"] = 240
        cfg["clis"]["claude_code"]["source"] = "headless"
        acs_config.save_config(cfg)
        loaded = acs_config.load_config()
        assert loaded["poll_interval_seconds"] == 240
        assert loaded["clis"]["claude_code"]["source"] == "headless"

    def test_write_is_atomic(self, sandbox_home):
        acs_config.save_config(acs_config.load_config())
        leftovers = list(acs_config.config_path().parent.glob("*.tmp"))
        assert leftovers == []
