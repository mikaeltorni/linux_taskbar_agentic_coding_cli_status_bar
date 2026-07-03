#!/usr/bin/env bash
# install.sh - Install the agentic coding CLI rate-limit status bar.
#
# Deploys the Python poller package and the agentic-cli-status@local GNOME
# Shell extension into the target user's home (root-optional; nothing needs
# sudo), then routes the selectable behavior through the shared installer
# component framework: per-CLI monitoring for Codex, Claude Code, and Grok
# CLI (each with reconfigurable source/staleness options), the polling rate,
# the background tmux debug console, and the login startup service. The
# poller only reads data the CLIs already store locally (or their billing
# metadata endpoint) and only ever types rate-limit slash commands into its
# tmux console, so monitoring never consumes plan limits.
#
# Usage:
#   bash install.sh                   # interactive component menu (TTY), else defaults
#   bash install.sh --default         # core + every default-on component, no prompts
#   bash install.sh --all             # core + every component, no prompts
#   bash install.sh --select a,b,c    # core + only the listed component ids
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
export TARGET_USER TARGET_UID TARGET_HOME SCRIPT_DIR RUNTIME_DIR USER_BUS \
  DISPLAY_VAL EXTENSION_UUID EXTENSION_SRC

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

# load_component_framework: Load the shared installer component framework from
# a sibling checkout of linux_installation_scripts_functions, falling back to
# an on-demand download so a standalone clean install works without it.
# Returns: 0 once isc_activate_components has run.
load_component_framework() {
  local d
  for d in "${ISC_FUNCTIONS_DIR:-}" \
           "$SCRIPT_DIR/../linux_installation_scripts_functions" \
           "$HOME/projects/linux_installation_scripts_functions"; do
    [[ -n "$d" && -f "$d/component_loader.sh" ]] && { source "$d/component_loader.sh"; break; }
  done
  declare -F isc_activate_components >/dev/null 2>&1 || \
    source <(curl -fsSL "https://raw.githubusercontent.com/mikaeltorni/linux_installation_scripts_functions/${ISC_FUNCTIONS_REF:-master}/component_loader.sh")
  isc_activate_components
  # Shared install-time helpers (run_as_target, need_cmd, …). They ship a plain
  # msg; redefine the logging-aware version afterwards so it wins repo-wide.
  isc_load_helpers
  msg() {
    printf "\n==> %s\n" "$*"
    acs_log INFO "$*"
  }
}

# main: Deploy the poller + extension core unconditionally, then route the
# selectable components through the shared component framework.
# Arguments: $@ - forwarded to component_main.
# Returns: the exit status of component_main.
main() {
  msg "=== Agentic Coding CLI Status Bar Setup ==="
  load_component_framework
  source "$SCRIPT_DIR/lib/status_setup.sh"
  source "$SCRIPT_DIR/installer/components.sh"
  if [ -z "${ISC_COMPONENTS:-}" ]; then
    echo "ERROR: ISC_COMPONENTS manifest is not defined." >&2
    exit 1
  fi
  install_agentic_status_core
  msg "Configuring agentic CLI monitoring options"
  component_main "$@"
}

# Listing/help must print only their own output (the master installer parses
# --list-components); skip the core deploy and completion message for those.
case "${1:-}" in
  --list-components|--export-selection|--detect|--help|-h|--uninstall|--uninstall=*)
    load_component_framework
    source "$SCRIPT_DIR/lib/status_setup.sh"
    source "$SCRIPT_DIR/installer/components.sh"
    component_main "$@"
    exit $?
    ;;
esac

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
