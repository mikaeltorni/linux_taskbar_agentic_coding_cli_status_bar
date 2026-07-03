#!/usr/bin/env bash
# status_setup.sh - Install functions for the agentic CLI status bar.
#
# Defines the unconditional core deploy (Python poller package + GNOME Shell
# extension + default config) and every component function referenced by
# installer/components.sh: per-CLI monitoring toggles with reconfigurable
# source options, the polling rate, the tmux debug console, and the poller's
# systemd user startup service. All functions are root-optional: everything
# lands under the target user's home and uses run_as_target when invoked via
# sudo from the master chain.

# The deployment copy list for the Python package. tests/test_deploy_contract.py
# asserts this list matches the files that actually exist in the package, so
# an installed poller can never fail on a missing module.
ACS_PACKAGE_FILES=(
  "__init__.py"
  "acs_logging.py"
  "config.py"
  "collectors.py"
  "tmux_console.py"
  "poller.py"
)

ACS_APP_DIR_REL=".local/share/agentic-cli-status"
ACS_SERVICE_NAME="agentic-cli-status.service"
ACS_TMUX_SESSION="agentic-cli-status"

# acs_app_dir - Echo the installed poller directory in the target home.
acs_app_dir() {
  printf '%s/%s\n' "$TARGET_HOME" "$ACS_APP_DIR_REL"
}

# acs_python_config - Run a short Python snippet against the installed
# agentic_cli_status package (falling back to the repo checkout before the
# core deploy has run) so config edits reuse the package's validation.
# Arguments: $1 - Python code (receives the package on sys.path).
acs_python_config() {
  local code="$1" pythonpath
  if [ -f "$(acs_app_dir)/agentic_cli_status/config.py" ]; then
    pythonpath="$(acs_app_dir)"
  else
    pythonpath="$SCRIPT_DIR"
  fi
  run_as_target env PYTHONPATH="$pythonpath" python3 -c "$code"
}

# acs_config_set KEY_PATH VALUE_JSON - Set one dotted config key (e.g.
# "clis.codex.enabled" true) through load_config/save_config so defaults and
# validation always apply. Runs as the target user.
acs_config_set() {
  local key_path="$1" value_json="$2"
  acs_log INFO "config set $key_path=$value_json"
  acs_python_config "
import json
from agentic_cli_status.config import load_config, save_config
config = load_config()
node = config
keys = '$key_path'.split('.')
for key in keys[:-1]:
    node = node.setdefault(key, {})
node[keys[-1]] = json.loads('$value_json')
save_config(config)
"
}

# acs_config_get KEY_PATH - Print one dotted config key's effective value.
acs_config_get() {
  local key_path="$1"
  acs_python_config "
from agentic_cli_status.config import load_config
node = load_config()
for key in '$key_path'.split('.'):
    node = node[key]
print(node if not isinstance(node, bool) else str(node).lower())
"
}

# acs_systemctl_user ARGS... - Run systemctl --user for the target user with
# the session bus, tolerating environments without a user manager (containers,
# CI) by reporting instead of failing the install.
acs_systemctl_user() {
  if ! run_as_target env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
      DBUS_SESSION_BUS_ADDRESS="$USER_BUS" systemctl --user "$@"; then
    msg "systemctl --user $* failed (no user session bus?); run it manually after login." >&2
    return 1
  fi
}

# ---------------------------------------------------------------------------
# Core deploy (always runs): poller package, extension, default config
# ---------------------------------------------------------------------------

# install_agentic_status_core - Deploy the Python poller package and the GNOME
# Shell extension, and seed the default config when none exists. Idempotent:
# re-running refreshes the installed copies without touching user settings.
install_agentic_status_core() {
  local app_dir pkg_dir file ext_dst
  app_dir="$(acs_app_dir)"
  pkg_dir="$app_dir/agentic_cli_status"
  msg "Deploying agentic-cli-status poller to $app_dir"
  run_as_target mkdir -p "$pkg_dir"
  for file in "${ACS_PACKAGE_FILES[@]}"; do
    if [ ! -f "$SCRIPT_DIR/agentic_cli_status/$file" ]; then
      msg "Missing package file agentic_cli_status/$file; aborting deploy." >&2
      return 1
    fi
    run_as_target cp "$SCRIPT_DIR/agentic_cli_status/$file" "$pkg_dir/$file"
  done

  ext_dst="$TARGET_HOME/.local/share/gnome-shell/extensions/$EXTENSION_UUID"
  msg "Deploying extension $EXTENSION_UUID"
  run_as_target mkdir -p "$ext_dst"
  run_as_target cp "$EXTENSION_SRC/extension.js" "$EXTENSION_SRC/metadata.json" \
    "$EXTENSION_SRC/stylesheet.css" "$ext_dst/"

  # Seed the default config only when absent so reinstalls never clobber the
  # user's tuned polling rate or per-CLI options.
  acs_python_config "
from agentic_cli_status.config import config_path, load_config, save_config
if not config_path().exists():
    save_config(load_config())
"

  if run_as_target env XDG_RUNTIME_DIR="$RUNTIME_DIR" \
      DBUS_SESSION_BUS_ADDRESS="$USER_BUS" DISPLAY="$DISPLAY_VAL" \
      gnome-extensions enable "$EXTENSION_UUID" 2>/dev/null; then
    msg "Extension enabled: $EXTENSION_UUID"
  else
    msg "Could not enable $EXTENSION_UUID now (no Shell session?); it activates on next login."
  fi
  msg "Core deploy complete. New extension code loads after the next Shell reload/login."
}

# ---------------------------------------------------------------------------
# Per-CLI monitoring components (codex / claude_code / grok)
# ---------------------------------------------------------------------------

# _acs_enable_cli CLI_ID - Enable monitoring for one CLI in the config.
_acs_enable_cli() {
  acs_config_set "clis.$1.enabled" "true"
  msg "Enabled $1 rate-limit monitoring."
}

# _acs_disable_cli CLI_ID - Disable monitoring for one CLI (uninstall action).
_acs_disable_cli() {
  acs_config_set "clis.$1.enabled" "false"
  msg "Disabled $1 rate-limit monitoring."
}

# _acs_detect_cli CLI_ID - Report installed when the poller package is deployed
# and the CLI is enabled in the config.
_acs_detect_cli() {
  [ -f "$(acs_app_dir)/agentic_cli_status/poller.py" ] || return 1
  [ "$(acs_config_get "clis.$1.enabled" 2>/dev/null)" = "true" ]
}

# _acs_configure_cli CLI_ID - Interactive per-CLI options: data source mode and
# staleness threshold. Every CLI exposes the same reconfigurable option set.
_acs_configure_cli() {
  local cli_id="$1" current_source current_stale source stale
  current_source="$(acs_config_get "clis.$cli_id.source")"
  current_stale="$(acs_config_get "clis.$cli_id.stale_after_seconds")"
  while true; do
    source=""
    read -r -e -i "$current_source" \
      -p "$cli_id data source [auto|headless|tmux]: " source </dev/tty || return 1
    case "$source" in
      auto|headless|tmux) break ;;
      *) msg "Enter auto, headless, or tmux." >&2 ;;
    esac
  done
  while true; do
    stale=""
    read -r -e -i "$current_stale" \
      -p "$cli_id staleness threshold in seconds (>=60): " stale </dev/tty || return 1
    if [[ "$stale" =~ ^[0-9]+$ ]] && (( stale >= 60 )); then
      break
    fi
    msg "Enter a whole number of seconds, 60 or more." >&2
  done
  acs_config_set "clis.$cli_id.source" "\"$source\""
  acs_config_set "clis.$cli_id.stale_after_seconds" "$stale"
  msg "$cli_id monitoring reconfigured: source=$source stale_after=${stale}s."
}

# _acs_cli_status CLI_ID - Menu-friendly one-line current option summary.
_acs_cli_status() {
  local enabled source stale
  enabled="$(acs_config_get "clis.$1.enabled" 2>/dev/null)" || { printf 'defaults\n'; return 0; }
  source="$(acs_config_get "clis.$1.source" 2>/dev/null)"
  stale="$(acs_config_get "clis.$1.stale_after_seconds" 2>/dev/null)"
  printf '%s, %s, stale %ss\n' "$enabled" "$source" "$stale"
}

# Component entry points (one thin wrapper per CLI so the manifest stays
# declarative and each CLI remains independently selectable/reconfigurable).
acs_install_codex()          { _acs_enable_cli codex; }
acs_uninstall_codex()        { _acs_disable_cli codex; }
acs_detect_codex()           { _acs_detect_cli codex; }
acs_configure_codex()        { _acs_configure_cli codex; }
acs_codex_status()           { _acs_cli_status codex; }

acs_install_claude_code()    { _acs_enable_cli claude_code; }
acs_uninstall_claude_code()  { _acs_disable_cli claude_code; }
acs_detect_claude_code()     { _acs_detect_cli claude_code; }
acs_configure_claude_code()  { _acs_configure_cli claude_code; }
acs_claude_code_status()     { _acs_cli_status claude_code; }

acs_install_grok()           { _acs_enable_cli grok; }
acs_uninstall_grok()         { _acs_disable_cli grok; }
acs_detect_grok()            { _acs_detect_cli grok; }
acs_configure_grok()         { _acs_configure_cli grok; }
acs_grok_status()            { _acs_cli_status grok; }

# ---------------------------------------------------------------------------
# Polling rate component
# ---------------------------------------------------------------------------

# acs_apply_polling_rate - Persist the current (or default) polling rate; the
# install action simply ensures the key exists with a valid value.
acs_apply_polling_rate() {
  acs_config_set "poll_interval_seconds" "$(acs_config_get poll_interval_seconds)"
  msg "Polling rate: every $(acs_config_get poll_interval_seconds)s."
}

# acs_configure_polling_rate - Prompt for how often the CLIs are checked.
# Range matches the poller's own validation (60-3600 seconds); the Claude
# endpoint additionally self-throttles to >=180s inside the collector.
acs_configure_polling_rate() {
  local current value
  current="$(acs_config_get poll_interval_seconds)"
  while true; do
    value=""
    read -r -e -i "$current" \
      -p "Poll every N seconds (60-3600): " value </dev/tty || return 1
    if [[ "$value" =~ ^[0-9]+$ ]] && (( value >= 60 && value <= 3600 )); then
      acs_config_set "poll_interval_seconds" "$value"
      msg "Polling rate saved: every ${value}s (applies on the next tick, no restart needed)."
      return 0
    fi
    msg "Enter a whole number from 60 to 3600." >&2
  done
}

# acs_polling_rate_status - Menu-friendly current polling rate.
acs_polling_rate_status() {
  printf 'every %ss\n' "$(acs_config_get poll_interval_seconds 2>/dev/null || echo 300)"
}

# ---------------------------------------------------------------------------
# tmux debug console component
# ---------------------------------------------------------------------------

# acs_install_tmux_console - Enable the background tmux console the poller
# maintains (one window per CLI). Requires tmux from linux_installations_setup.
acs_install_tmux_console() {
  if ! command -v tmux >/dev/null 2>&1; then
    msg "tmux is not installed (linux_installations_setup provides it); console stays enabled in config and activates once tmux exists."
  fi
  acs_config_set "tmux.enabled" "true"
  msg "tmux debug console enabled: attach with 'tmux attach -t $ACS_TMUX_SESSION'."
}

# acs_uninstall_tmux_console - Disable the console and kill its session.
acs_uninstall_tmux_console() {
  acs_config_set "tmux.enabled" "false"
  run_as_target tmux kill-session -t "$ACS_TMUX_SESSION" 2>/dev/null || true
  msg "tmux debug console disabled."
}

# acs_detect_tmux_console - Installed when the config has the console enabled.
acs_detect_tmux_console() {
  [ "$(acs_config_get tmux.enabled 2>/dev/null)" = "true" ]
}

# ---------------------------------------------------------------------------
# Startup service component
# ---------------------------------------------------------------------------

# acs_install_startup_service - Install and enable the systemd user service so
# polling starts at login and keeps running.
acs_install_startup_service() {
  local unit_dir="$TARGET_HOME/.config/systemd/user"
  run_as_target mkdir -p "$unit_dir"
  run_as_target cp "$SCRIPT_DIR/systemd/$ACS_SERVICE_NAME" "$unit_dir/$ACS_SERVICE_NAME"
  acs_log INFO "installed unit $unit_dir/$ACS_SERVICE_NAME"
  if acs_systemctl_user daemon-reload && \
     acs_systemctl_user enable --now "$ACS_SERVICE_NAME"; then
    msg "Poller service enabled and started ($ACS_SERVICE_NAME)."
  else
    msg "Poller service installed; enable it after login with: systemctl --user enable --now $ACS_SERVICE_NAME"
  fi
}

# acs_uninstall_startup_service - Stop and disable the poller service and
# remove its unit file.
acs_uninstall_startup_service() {
  acs_systemctl_user disable --now "$ACS_SERVICE_NAME" 2>/dev/null || true
  run_as_target rm -f "$TARGET_HOME/.config/systemd/user/$ACS_SERVICE_NAME"
  acs_systemctl_user daemon-reload 2>/dev/null || true
  msg "Poller service removed."
}

# acs_detect_startup_service - Installed when the unit file is present.
acs_detect_startup_service() {
  [ -f "$TARGET_HOME/.config/systemd/user/$ACS_SERVICE_NAME" ]
}
