#!/usr/bin/env bash
# components.sh - Component manifest for linux_taskbar_agentic_coding_cli_status_bar.
#
# install.sh deploys the core unconditionally (Python poller package, GNOME
# Shell taskbar extension, default config) so the indicator always works, then
# routes the selectable behavior through component_main from the shared
# component runtime. Each of the three monitored CLIs is its own component
# with the full reconfigurable option set (source mode, staleness threshold)
# so every CLI can be enabled, disabled, and re-tuned independently. Cline is
# deliberately not listed: it has no usage/rate-limit surface yet.
#
# Entry format:
#   "id|label|default|function|detect_fn|uninstall_fn|section|requires|configure_fn|config_status_fn"
# The functions live in lib/status_setup.sh, sourced by install.sh before this
# manifest.

ISC_REPO_NAME="linux_taskbar_agentic_coding_cli_status_bar"
ISC_REPO_LABEL="Agentic coding CLI rate-limit status bar"

ISC_COMPONENTS=(
  "codex_monitor|Codex rate limits (5h + weekly, /usage data)|on|acs_install_codex|acs_detect_codex|acs_uninstall_codex|Monitored CLIs||acs_configure_codex|acs_codex_status"
  "claude_code_monitor|Claude Code rate limits (5h + weekly, /usage data)|off|acs_install_claude_code|acs_detect_claude_code|acs_uninstall_claude_code|Monitored CLIs||acs_configure_claude_code|acs_claude_code_status"
  "grok_monitor|Grok CLI credit usage (weekly, /usage data)|on|acs_install_grok|acs_detect_grok|acs_uninstall_grok|Monitored CLIs||acs_configure_grok|acs_grok_status"
  "opencode_go_monitor|OpenCode Go usage (→ credentials + rolling/weekly/monthly)|on|acs_install_opencode_go|acs_detect_opencode_go|acs_uninstall_opencode_go|Monitored CLIs||acs_configure_opencode_go|acs_opencode_go_status"
  "openrouter_monitor|OpenRouter free-model daily usage (activity page, OR segment)|on|acs_install_openrouter|acs_detect_openrouter|acs_uninstall_openrouter|Monitored CLIs||acs_configure_openrouter|acs_openrouter_status"
  "polling_rate|Polling rate in seconds (how often limits are checked)|on|acs_apply_polling_rate|||Poller||acs_configure_polling_rate|acs_polling_rate_status"
  "tmux_console|Background tmux debug console (slash-command refresh)|on|acs_install_tmux_console|acs_detect_tmux_console|acs_uninstall_tmux_console|Poller"
  "startup_service|Start poller at login (systemd user service)|on|acs_install_startup_service|acs_detect_startup_service|acs_uninstall_startup_service|Poller"
)
