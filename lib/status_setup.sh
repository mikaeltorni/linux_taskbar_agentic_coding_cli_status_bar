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
  "openrouter_browser.py"
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

# acs_playwright_python - Echo the ACS venv python (Playwright) when present.
acs_playwright_python() {
  local venv_python="$TARGET_HOME/.local/share/agentic-cli-status/venv/bin/python"
  if [ -x "$venv_python" ]; then
    printf '%s\n' "$venv_python"
    return 0
  fi
  printf '%s\n' "python3"
}

# acs_python_openrouter - Run Python against the package using the Playwright venv.
acs_python_openrouter() {
  local code="$1" pythonpath python
  if [ -f "$(acs_app_dir)/agentic_cli_status/openrouter_browser.py" ]; then
    pythonpath="$(acs_app_dir)"
  else
    pythonpath="$SCRIPT_DIR"
  fi
  python="$(acs_playwright_python)"
  run_as_target env PYTHONPATH="$pythonpath" "$python" -c "$code"
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

# acs_configure_claude_code - Claude options: the generic per-CLI set plus the
# usage-endpoint check interval. The endpoint grants only a small call budget
# (429 + multi-minute lockout when polled aggressively), so its interval is
# bounded to 60-3600 seconds independently of the main polling rate.
acs_configure_claude_code() {
  _acs_configure_cli claude_code || return 1
  local current value
  current="$(acs_config_get clis.claude_code.endpoint_min_interval_seconds)"
  while true; do
    value=""
    read -r -e -i "$current" \
      -p "Claude usage-endpoint check interval in SECONDS (60-3600, default 180; lower risks 429 lockouts): " \
      value </dev/tty || return 1
    if [[ "$value" =~ ^[0-9]+$ ]] && (( value >= 60 && value <= 3600 )); then
      acs_config_set "clis.claude_code.endpoint_min_interval_seconds" "$value"
      msg "Claude endpoint interval saved: every ${value}s (applies on the next tick)."
      return 0
    fi
    msg "Enter a whole number of seconds from 60 to 3600." >&2
  done
}

# acs_claude_code_status - Generic per-CLI summary plus the endpoint interval.
acs_claude_code_status() {
  local base interval
  base="$(_acs_cli_status claude_code)"
  interval="$(acs_config_get clis.claude_code.endpoint_min_interval_seconds 2>/dev/null || true)"
  if [ -n "$interval" ]; then
    printf '%s, endpoint %ss\n' "$base" "$interval"
  else
    printf '%s\n' "$base"
  fi
}

acs_install_grok()           { _acs_enable_cli grok; }
acs_uninstall_grok()         { _acs_disable_cli grok; }
acs_detect_grok()            { _acs_detect_cli grok; }
acs_configure_grok()         { _acs_configure_cli grok; }
acs_grok_status()            { _acs_cli_status grok; }

# ---------------------------------------------------------------------------
# OpenCode Go monitoring component
# ---------------------------------------------------------------------------

# acs_opencode_config_path - Echo the OpenCode config file in the target home.
acs_opencode_config_path() {
  printf '%s/.config/opencode/opencode.json\n' "$TARGET_HOME"
}

# acs_opencode_go_usage_config_path - Echo the shared credential config file.
acs_opencode_go_usage_config_path() {
  printf '%s/.config/opencode/opencode-go-usage.json\n' "$TARGET_HOME"
}

# acs_register_opencode_go_plugin - Add opencode-go-usage to opencode.json.
acs_register_opencode_go_plugin() {
  local config_path plugin
  config_path="$(acs_opencode_config_path)"
  plugin="opencode-go-usage"
  run_as_target mkdir -p "$(dirname "$config_path")"
  acs_python_config "
import json
from pathlib import Path
path = Path('$config_path')
config = {}
if path.exists():
    try:
        config = json.loads(path.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        config = {}
plugins = config.setdefault('plugin', [])
if '$plugin' not in plugins:
    plugins.append('$plugin')
path.write_text(json.dumps(config, indent=2) + '\n', encoding='utf-8')
"
  msg "Registered opencode-go-usage in $config_path (OpenCode installs it on next startup)."
}

# acs_opencode_go_cookie_help - Print how to copy the auth cookie from a browser.
# The cookie is NOT in the page HTML or Styles panel; it lives under Application
# → Cookies while logged in at opencode.ai.
acs_opencode_go_cookie_help() {
  msg "OpenCode Go auth cookie — how to find it in Chrome or Firefox:"
  msg "  1. Log in at https://opencode.ai (open your workspace /go page)."
  msg "  2. Open DevTools: F12 or Ctrl+Shift+I."
  msg "  3. Open the Application tab (Chrome) or Storage tab (Firefox)."
  msg "  4. Left sidebar: Cookies → https://opencode.ai"
  msg "  5. In the table, find the row named auth (not in Elements or Styles)."
  msg "  6. Double-click the Value column and copy the full string (Fe26.2…)."
  msg "Searching the page HTML for fe26 will not work — use Application → Cookies."
}

# acs_write_opencode_go_config - Write workspace + cookie into the shared config.
acs_write_opencode_go_config() {
  local config_path="$1" workspace="$2" cookie="$3"
  run_as_target mkdir -p "$(dirname "$config_path")"
  run_as_target env ACS_OGC_WORKSPACE="$workspace" ACS_OGC_COOKIE="$cookie" \
    python3 -c "
import json, os
from pathlib import Path
path = Path('$config_path')
existing = {}
if path.exists():
    try:
        existing = json.loads(path.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        existing = {}
existing['workspaceId'] = os.environ['ACS_OGC_WORKSPACE']
existing['authCookie'] = os.environ['ACS_OGC_COOKIE']
existing.setdefault('refreshIntervalMinutes', 5)
existing.setdefault('showAtSessionStart', True)
path.write_text(json.dumps(existing, indent=2) + '\n', encoding='utf-8')
path.chmod(0o600)
"
}

# acs_opencode_go_config_valid - Return 0 when workspace + auth cookie are set.
acs_opencode_go_config_valid() {
  local config_path="$1"
  run_as_target python3 -c "
import json, sys
from pathlib import Path
path = Path('$config_path')
if not path.is_file():
    sys.exit(1)
try:
    data = json.loads(path.read_text(encoding='utf-8'))
except (OSError, ValueError):
    sys.exit(1)
workspace = data.get('workspaceId') or ''
cookie = data.get('authCookie') or ''
if not workspace.startswith('wrk_'):
    sys.exit(1)
if cookie in ('', 'PASTE_YOUR_AUTH_COOKIE_HERE') or len(cookie) < 20:
    sys.exit(1)
" 2>/dev/null
}

# acs_opencode_go_credential_status - One-line credential summary for the menu.
acs_opencode_go_credential_status() {
  local config_path
  config_path="$(acs_opencode_go_usage_config_path)"
  if [ -n "${OPENCODE_GO_WORKSPACE_ID:-}" ] && [ -n "${OPENCODE_GO_AUTH_COOKIE:-}" ]; then
    printf 'creds from env'
    return 0
  fi
  if acs_opencode_go_config_valid "$config_path"; then
    printf 'creds in file'
    return 0
  fi
  printf 'creds missing (press →)'
}

# acs_configure_opencode_go_credentials - Prompt for workspace ID and auth
# cookie, writing them to the shared config file the collector reads.
acs_configure_opencode_go_credentials() {
  local config_path workspace cookie current choice cookie_file editor
  config_path="$(acs_opencode_go_usage_config_path)"
  current=""
  if [ -f "$config_path" ]; then
    current="$(run_as_target python3 -c "
import json
from pathlib import Path
data = json.loads(Path('$config_path').read_text(encoding='utf-8'))
print(data.get('workspaceId', ''))
" 2>/dev/null || true)"
  fi
  msg "Workspace ID: copy wrk_… from your browser URL:"
  msg "  https://opencode.ai/workspace/WORKSPACE_ID/go"
  while true; do
    workspace=""
    read -r -e -i "$current" \
      -p "OpenCode Go workspace ID: " workspace </dev/tty || return 1
    if [[ "$workspace" =~ ^wrk_[a-zA-Z0-9]+$ ]]; then
      break
    fi
    msg "Enter a workspace ID like wrk_abc123 from your OpenCode workspace URL." >&2
  done
  acs_opencode_go_cookie_help
  msg "Auth cookie input — pick one:"
  msg "  [1] Paste here (visible — recommended if your terminal accepts paste)"
  msg "  [2] Edit config file (recommended for long cookies)"
  msg "  [3] Read cookie from a text file (one line, no quotes)"
  while true; do
    choice=""
    read -r -p "Choice [1/2/3] (default 2): " choice </dev/tty || return 1
    case "${choice:-2}" in
      1|2|3) break ;;
      *) msg "Enter 1, 2, or 3." >&2 ;;
    esac
  done
  case "$choice" in
    1)
      msg "Paste the full auth cookie value (input is visible, not hidden):"
      read -r -p "auth cookie: " cookie </dev/tty || return 1
      if [ -z "$cookie" ]; then
        msg "Auth cookie cannot be empty." >&2
        return 1
      fi
      acs_write_opencode_go_config "$config_path" "$workspace" "$cookie"
      ;;
    2)
      acs_write_opencode_go_config "$config_path" "$workspace" "PASTE_YOUR_AUTH_COOKIE_HERE"
      msg "Config template written to $config_path"
      msg "  1. Open that file in your editor."
      msg "  2. Replace PASTE_YOUR_AUTH_COOKIE_HERE with the auth cookie Value."
      msg "  3. Save the file and return here."
      editor="${EDITOR:-${VISUAL:-nano}}"
      read -r -p "Open in $editor now? [Y/n]: " choice </dev/tty || return 1
      case "${choice:-Y}" in
        [Yy]|"")
          run_as_target "$editor" "$config_path" </dev/tty || true
          ;;
      esac
      read -r -p "Press Enter when authCookie is saved (or type skip): " choice </dev/tty || return 1
      if [ "$choice" = "skip" ]; then
        msg "Skipped auth cookie; edit $config_path later." >&2
        return 1
      fi
      if ! acs_opencode_go_config_valid "$config_path"; then
        msg "authCookie still missing or invalid in $config_path." >&2
        return 1
      fi
      run_as_target chmod 600 "$config_path"
      ;;
    3)
      read -r -p "Path to file containing the auth cookie: " cookie_file </dev/tty || return 1
      if [ ! -f "$cookie_file" ]; then
        msg "File not found: $cookie_file" >&2
        return 1
      fi
      cookie="$(tr -d '\r\n' <"$cookie_file")"
      if [ -z "$cookie" ]; then
        msg "Cookie file is empty." >&2
        return 1
      fi
      acs_write_opencode_go_config "$config_path" "$workspace" "$cookie"
      ;;
  esac
  msg "OpenCode Go credentials saved to $config_path (mode 600)."
}

# acs_install_opencode_go - Enable monitoring, register the plugin, and offer
# credential setup when none are configured yet (including placeholder files).
acs_install_opencode_go() {
  local config_path cred_status update
  config_path="$(acs_opencode_go_usage_config_path)"
  _acs_enable_cli opencode_go
  acs_register_opencode_go_plugin
  if [ -n "${OPENCODE_GO_WORKSPACE_ID:-}" ] && [ -n "${OPENCODE_GO_AUTH_COOKIE:-}" ]; then
    return 0
  fi
  cred_status="$(acs_opencode_go_credential_status)"
  if ! acs_opencode_go_config_valid "$config_path"; then
    msg "OpenCode Go needs a workspace ID and auth cookie ($cred_status)."
    if [ -t 0 ]; then
      acs_configure_opencode_go_credentials || \
        msg "Skipped credential setup; select opencode_go_monitor and press → in the installer menu."
    else
      msg "Non-interactive install: set OPENCODE_GO_WORKSPACE_ID/OPENCODE_GO_AUTH_COOKIE or run: bash install.sh --configure-component opencode_go_monitor"
    fi
    return 0
  fi
  if [ -t 0 ]; then
    msg "OpenCode Go credentials: $cred_status."
    update=""
    read -r -p "Update credentials now? [y/N]: " update </dev/tty || return 0
    case "$update" in
      [Yy]*) acs_configure_opencode_go_credentials || return 1 ;;
    esac
  fi
}

acs_uninstall_opencode_go() { _acs_disable_cli opencode_go; }
acs_detect_opencode_go()    { _acs_detect_cli opencode_go; }

# acs_configure_opencode_go - Credentials and/or per-CLI monitoring options.
acs_configure_opencode_go() {
  local choice
  if [ ! -t 0 ]; then
    _acs_configure_cli opencode_go
    return $?
  fi
  msg "OpenCode Go configure — in the installer menu, highlight opencode_go_monitor and:"
  msg "  • press → (right arrow) to open this screen anytime"
  msg "  • or press space on an installed [✓] row, then Enter to re-run install"
  msg ""
  msg "What to configure?"
  msg "  [1] Credentials only (workspace ID + auth cookie)"
  msg "  [2] Monitoring options only (source + staleness)"
  msg "  [3] Both"
  while true; do
    choice=""
    read -r -p "Choice [1/2/3] (default 1): " choice </dev/tty || return 1
    case "${choice:-1}" in
      1|2|3) break ;;
      *) msg "Enter 1, 2, or 3." >&2 ;;
    esac
  done
  case "$choice" in
    1) acs_configure_opencode_go_credentials ;;
    2) _acs_configure_cli opencode_go ;;
    3)
      _acs_configure_cli opencode_go || return 1
      acs_configure_opencode_go_credentials || return 1
      ;;
    *)
      acs_configure_opencode_go_credentials
      ;;
  esac
}

acs_opencode_go_status() {
  local base cred
  base="$(_acs_cli_status opencode_go)"
  cred="$(acs_opencode_go_credential_status)"
  printf '%s, %s\n' "$base" "$cred"
}

# ---------------------------------------------------------------------------
# OpenRouter monitoring component
# ---------------------------------------------------------------------------

# acs_openrouter_storage_path - Echo the Playwright session cache file.
acs_openrouter_storage_path() {
  printf '%s/.config/agentic-cli-status/openrouter-storage.json\n' "$TARGET_HOME"
}

# acs_ensure_playwright - Install Playwright + Chromium into the ACS venv.
acs_ensure_playwright() {
  local venv_dir app_dir python
  app_dir="$(acs_app_dir)"
  venv_dir="$app_dir/venv"
  python="$venv_dir/bin/python"
  if [ -x "$python" ] && "$python" -c "from playwright.sync_api import sync_playwright" 2>/dev/null; then
    return 0
  fi
  msg "Installing Playwright for OpenRouter session setup (one-time)…"
  run_as_target mkdir -p "$app_dir"
  if [ ! -x "$python" ]; then
    run_as_target python3 -m venv "$venv_dir"
    python="$venv_dir/bin/python"
  fi
  run_as_target "$python" -m pip install playwright
  run_as_target "$python" -m playwright install chromium
  msg "Playwright ready at $venv_dir"
}

# acs_configure_openrouter_session - Headed browser login + console confirm.
acs_configure_openrouter_session() {
  local storage_path
  storage_path="$(acs_openrouter_storage_path)"
  acs_ensure_playwright || return 1
  if [ ! -t 0 ]; then
    msg "OpenRouter session setup requires an interactive terminal." >&2
    return 1
  fi
  msg "Opening a browser to https://openrouter.ai/activity — log in, then confirm here."
  acs_python_openrouter "
from pathlib import Path
from agentic_cli_status.openrouter_browser import run_headed_login
path = Path('$storage_path')
path.parent.mkdir(parents=True, exist_ok=True)
run_headed_login(path, home=Path('$TARGET_HOME'))
path.chmod(0o600)
" || return 1
  msg "OpenRouter session saved to $storage_path (mode 600)."
}

# acs_configure_openrouter_model - Prompt for the monitored model slug + limit.
acs_configure_openrouter_model() {
  local current_slug current_limit slug limit
  current_slug="$(acs_config_get clis.openrouter.model_slug 2>/dev/null || echo tencent/hy3:free)"
  current_limit="$(acs_config_get clis.openrouter.daily_limit 2>/dev/null || echo 1000)"
  while true; do
    slug=""
    read -r -e -i "$current_slug" \
      -p "OpenRouter model slug to monitor (e.g. tencent/hy3:free): " slug </dev/tty || return 1
    if [ -n "$slug" ]; then
      break
    fi
    msg "Model slug cannot be empty." >&2
  done
  while true; do
    limit=""
    read -r -e -i "$current_limit" \
      -p "Daily free-request limit for that model (default 1000): " limit </dev/tty || return 1
    if [[ "$limit" =~ ^[0-9]+$ ]] && (( limit >= 1 && limit <= 100000 )); then
      break
    fi
    msg "Enter a whole number from 1 to 100000." >&2
  done
  acs_config_set "clis.openrouter.model_slug" "\"$slug\""
  acs_config_set "clis.openrouter.daily_limit" "$limit"
  msg "OpenRouter model saved: $slug (daily limit $limit requests, UTC midnight reset)."
}

# acs_install_openrouter - Enable monitoring and run setup when no session exists.
acs_install_openrouter() {
  _acs_enable_cli openrouter
  if [ ! -f "$(acs_openrouter_storage_path)" ]; then
    msg "OpenRouter headless monitoring needs a one-time browser login."
    if [ -t 0 ]; then
      acs_configure_openrouter_session || \
        msg "Skipped session setup; configure later via the installer menu."
      acs_configure_openrouter_model || \
        msg "Skipped model slug setup; configure later via the installer menu."
    else
      msg "Non-interactive install: run configure later for session + model slug."
    fi
  fi
}

acs_uninstall_openrouter() { _acs_disable_cli openrouter; }
acs_detect_openrouter()     { _acs_detect_cli openrouter; }

# acs_configure_openrouter - Staleness, browser login, and model slug (headless only).
acs_configure_openrouter() {
  local current_stale stale
  if [ ! -t 0 ]; then
    msg "OpenRouter configure requires an interactive terminal." >&2
    return 1
  fi
  current_stale="$(acs_config_get clis.openrouter.stale_after_seconds)"
  while true; do
    stale=""
    read -r -e -i "$current_stale" \
      -p "openrouter staleness threshold in seconds (>=60): " stale </dev/tty || return 1
    if [[ "$stale" =~ ^[0-9]+$ ]] && (( stale >= 60 )); then
      break
    fi
    msg "Enter a whole number of seconds, 60 or more." >&2
  done
  acs_config_set "clis.openrouter.stale_after_seconds" "$stale"
  acs_config_set "clis.openrouter.source" "\"headless\""
  acs_configure_openrouter_session || return 1
  acs_configure_openrouter_model || return 1
  msg "OpenRouter monitoring reconfigured: source=headless stale_after=${stale}s."
}

# acs_openrouter_status - Per-CLI summary plus model slug and daily limit.
acs_openrouter_status() {
  local base slug limit
  base="$(_acs_cli_status openrouter)"
  slug="$(acs_config_get clis.openrouter.model_slug 2>/dev/null || true)"
  limit="$(acs_config_get clis.openrouter.daily_limit 2>/dev/null || true)"
  if [ -n "$slug" ] && [ -n "$limit" ]; then
    printf '%s, %s, limit %s/day UTC\n' "$base" "$slug" "$limit"
  else
    printf '%s\n' "$base"
  fi
}

# ---------------------------------------------------------------------------
# Polling rate component
# ---------------------------------------------------------------------------

# acs_apply_polling_rate - Persist the current (or default) polling rate; the
# install action simply ensures the key exists with a valid value.
acs_apply_polling_rate() {
  acs_config_set "poll_interval_seconds" "$(acs_config_get poll_interval_seconds)"
  msg "Polling rate: every $(acs_config_get poll_interval_seconds) seconds."
}

# acs_configure_polling_rate - Prompt for how often the CLIs are checked,
# using the same prefilled typeable field as the other reconfigurable
# components. The value is ALWAYS in seconds (default 5); the range matches
# the poller's own validation (5-3600). Slow sources protect themselves: the
# Claude endpoint keeps its >=180s floor and tmux refreshes are rate-limited
# inside the poller regardless of this setting.
acs_configure_polling_rate() {
  local current value
  current="$(acs_config_get poll_interval_seconds)"
  while true; do
    value=""
    read -r -e -i "$current" \
      -p "Poll every N SECONDS (5-3600, default 5): " value </dev/tty || return 1
    if [[ "$value" =~ ^[0-9]+$ ]] && (( value >= 5 && value <= 3600 )); then
      acs_config_set "poll_interval_seconds" "$value"
      msg "Polling rate saved: every ${value} seconds (applies on the next tick, no restart needed)."
      return 0
    fi
    msg "Enter a whole number of seconds from 5 to 3600." >&2
  done
}

# acs_polling_rate_status - Menu-friendly current polling rate (in seconds).
acs_polling_rate_status() {
  printf 'every %s seconds\n' "$(acs_config_get poll_interval_seconds 2>/dev/null || echo 5)"
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
