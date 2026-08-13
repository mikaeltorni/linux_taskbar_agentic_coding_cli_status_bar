#!/usr/bin/env bash
# acs_bin.sh — Resolve and invoke the agentic-cli-status Rust poller.
#
# Sourced by install.sh / lib/status_setup.sh after SCRIPT_DIR is set. Builds
# the binary on demand via scripts/build_agentic_cli_status.sh, then runs
# subcommands that replaced the former Python package.
#
# Public functions:
#   ensure_acs_bin  — build/locate dist/agentic-cli-status; sets ACS_BIN
#   acs_bin         — ensure_acs_bin then run it with "$@" as the target user

# ensure_acs_bin — Locate or build dist/agentic-cli-status; export ACS_BIN.
ensure_acs_bin() {
  local build_script bin
  build_script="${SCRIPT_DIR:?SCRIPT_DIR unset}/scripts/build_agentic_cli_status.sh"
  if [ -n "${ACS_BIN:-}" ] && [ -x "$ACS_BIN" ]; then
    return 0
  fi
  if [ ! -x "$build_script" ]; then
    if declare -F msg >/dev/null 2>&1; then
      msg "Missing build helper: $build_script"
    else
      printf 'Missing build helper: %s\n' "$build_script" >&2
    fi
    return 1
  fi
  bin="$(bash "$build_script" --print)" || return 1
  ACS_BIN="$bin"
  export ACS_BIN
}

# acs_bin — Run the poller CLI as the target user.
acs_bin() {
  ensure_acs_bin || return 1
  if declare -F run_as_target >/dev/null 2>&1; then
    run_as_target env \
      HOME="$TARGET_HOME" \
      XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$TARGET_HOME/.config}" \
      XDG_STATE_HOME="${XDG_STATE_HOME:-$TARGET_HOME/.local/state}" \
      "$ACS_BIN" "$@"
  else
    "$ACS_BIN" "$@"
  fi
}
