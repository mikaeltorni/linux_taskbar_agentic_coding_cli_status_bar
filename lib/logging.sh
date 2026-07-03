#!/usr/bin/env bash
# Centralized Bash logging for linux_taskbar_agentic_coding_cli_status_bar.
#
# Source this file from Bash scripts that need repository-local diagnostics.
# Logs are best-effort and go to .log/install.log without changing stdout or
# stderr contracts.

if [[ -z "${AGENTIC_CLI_STATUS_LOGGING_SH_LOADED:-}" ]]; then
  AGENTIC_CLI_STATUS_LOGGING_SH_LOADED=1

  _acs_log_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  ACS_LOG_DIR_SH="${AGENTIC_CLI_STATUS_LOG_DIR:-$_acs_log_root/.log}"
  ACS_INSTALL_LOG="${ACS_INSTALL_LOG:-$ACS_LOG_DIR_SH/install.log}"

  # acs_log: Append one timestamped line to the centralized installer log.
  # Arguments: $1 - level, $* - message text.
  # Returns: 0 always; logging failures are intentionally ignored.
  acs_log() {
    local level="${1:-INFO}"
    shift || true
    mkdir -p "$ACS_LOG_DIR_SH" 2>/dev/null || return 0
    printf '%s [%s] %s\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$level" "$*" \
      >>"$ACS_INSTALL_LOG" 2>/dev/null || true
  }
fi
