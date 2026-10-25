#!/usr/bin/env bash
# install.sh - Install the agentic coding CLI rate-limit status bar.
#
# Deploys the Rust poller binary (`~/.local/bin/agentic-cli-status`) and the
# agentic-cli-status@local GNOME Shell extension into the target user's home
# (root-optional; nothing needs sudo), then routes the selectable behavior
# through the shared installer component framework: per-CLI monitoring for
# Codex, Claude Code, and Grok CLI (each with reconfigurable source/staleness
# options), the polling rate, the background tmux debug console, and the login
# startup service. The poller only reads data the CLIs already store locally
# (or their billing metadata endpoint) and only ever types rate-limit slash
# commands into its tmux console, so monitoring never consumes plan limits.
#
# Usage:
#   bash install.sh                   # interactive component menu (TTY), else defaults
#   bash install.sh --default         # core + every default-on component, no prompts
#   bash install.sh --all             # core + every component, no prompts
#   bash install.sh --select a,b,c    # core + only the listed component ids
#   ACS_POLL_INTERVAL_SECONDS=10 bash install.sh --select polling_rate
#   bash install.sh --list-components # machine-readable component list (no deploy)
#   bash install.sh --help            # usage
#   sudo bash install.sh              # also supported (clean-install chain)

set -euo pipefail

TARGET_USER="${SUDO_USER:-$USER}"
TARGET_UID="$(id -u "$TARGET_USER")"
TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_DIR="/run/user/$TARGET_UID"
USER_BUS="unix:path=${RUNTIME_DIR}/bus"
DISPLAY_VAL="${DISPLAY:-:0}"
EXTENSION_UUID="agentic-cli-status@local"
EXTENSION_SRC="$SCRIPT_DIR/agentic-cli-status@local"
# Optional, separately installable Agent Command Center model status bar. It is
# its own GNOME Shell extension so it can be enabled/disabled independently of
# the rate-limit indicator above (component: model_status_bar).
MODEL_STATUS_UUID="acc-model-status@local"
MODEL_STATUS_SRC="$SCRIPT_DIR/acc-model-status@local"
export TARGET_USER TARGET_UID TARGET_HOME SCRIPT_DIR RUNTIME_DIR USER_BUS \
  DISPLAY_VAL EXTENSION_UUID EXTENSION_SRC MODEL_STATUS_UUID MODEL_STATUS_SRC

source "$SCRIPT_DIR/lib/logging.sh"

# msg: Print a highlighted progress message to stdout and mirror it to the
# centralized installer log. Defined before the framework helpers load so this
# logging-aware version (not the framework's plain printf) is used throughout.
# Arguments: $* - the message text to display.
# Returns: 0 always.
msg() {
  printf "\n==> %s\n" "$*"
  acs_log INFO "$*"
}

# Load the shared framework from the maintained offline support package.
# Parameters: none.
# Returns: zero after activation; a failing command aborts setup.
load_component_framework() {
  # Load licensed, generated support without requiring a private checkout.
  printf 'parameters=none\n' >&2
  if [[ -n "${ACS_FRAMEWORK_LOADED:-}" ]]; then
    printf '0\n' >&2
    return 0
  fi
  ACS_FRAMEWORK_LOADED=1
  source "$SCRIPT_DIR/.installer-support/bootstrap.sh"
  isc_activate_components
  # Shared install-time helpers (run_as_target, need_cmd, …). They ship a plain
  # msg; redefine the logging-aware version afterwards so it wins repo-wide.
  isc_load_helpers
  # Print a component status message using this owner's diagnostics.
  # Parameters: positional arguments - message words.
  # Returns: zero after the message is displayed.
  msg() {
    printf 'parameters=%s\n' "$*" >&2
    printf "\n==> %s\n" "$*"
    acs_log INFO "$*"
    printf '0\n' >&2
  }
  printf '0\n' >&2
}

# Apply a prepared selection after acceptance, preserving core failure propagation.
# Parameters: $@ - already parsed installer arguments. Returns: zero on completion.
main() {
  printf 'arguments=%s\n' "$*" >&2
  isc_plan_install
  if [[ "$ISC_PLAN_ACTION" == cancel ]]; then printf '0\n' >&2; return 0; fi
  if [[ "$ISC_PLAN_ACTION" == install ]]; then
    msg "=== Agentic Coding CLI Status Bar Setup ==="
    install_agentic_status_core
  fi
  _isc_apply_plan "$ISC_ARGUMENT_ORIGIN"
  printf '0\n' >&2
  return 0
}

# Listing/help/configure must print only their own output and must not deploy
# the core (the master installer parses --list-components,
# --list-configurable-components, and --list-component-config-values).
#
# Route them through the framework's shared helper rather than a hand-maintained
# flag list: the local list omitted --list-configurable-components,
# --list-select-configure-components, --list-component-config-values, and
# --configure-component, so the master's _cs_repo_exposes_flag could not detect
# them and this repository's six nested configure screens (per-CLI monitors and
# the polling rate) were unreachable from the aggregate menu. The helper also
# matches the flag anywhere in argv, not just argv[1].
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  load_component_framework
  source "$SCRIPT_DIR/lib/status_setup.sh"
  source "$SCRIPT_DIR/installer/components.sh"
  if isc_route_readonly_args "$@"; then exit $?; fi
  main "$@"
fi
