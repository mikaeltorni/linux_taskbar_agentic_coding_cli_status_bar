#!/usr/bin/env bash
# Test the agentic CLI status bar installer without sudo or desktop side effects.
#
# Runs install.sh against a sandbox home with mocked getent/sudo/systemctl/
# gnome-extensions/tmux binaries and asserts the core deploy, the default
# component set, config writes, idempotence, and the side-effect-free
# --list-components contract.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_DIR="$(mktemp -d /tmp/agentic-cli-status-test.XXXXXX)"
BIN_DIR="$TMP_DIR/bin"
TARGET_HOME="$TMP_DIR/home"
EXTENSION_UUID="agentic-cli-status@local"
CALL_LOG="$TMP_DIR/calls.log"
PASS=0
FAIL=0

# assert_file_exists: Assert that a path exists, updating PASS/FAIL counters.
# Arguments: $1 - path to check, $2 - message printed on failure.
assert_file_exists() {
  local path="$1" message="$2"
  if [ -e "$path" ]; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
    echo "FAIL: $message (missing: $path)"
  fi
}

# assert_file_contains: Assert that a file contains a fixed-string needle.
# Arguments: $1 - path, $2 - literal substring to find, $3 - failure message.
assert_file_contains() {
  local path="$1" needle="$2" message="$3"
  if grep -Fq "$needle" "$path" 2>/dev/null; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
    echo "FAIL: $message"
  fi
}

# assert_not_contains: Assert that a file does NOT contain a substring.
# Arguments: $1 - path, $2 - literal substring, $3 - failure message.
assert_not_contains() {
  local path="$1" needle="$2" message="$3"
  if grep -Fq "$needle" "$path" 2>/dev/null; then
    FAIL=$((FAIL + 1))
    echo "FAIL: $message"
  else
    PASS=$((PASS + 1))
  fi
}

mkdir -p "$BIN_DIR" "$TARGET_HOME"
: >"$CALL_LOG"

cat >"$BIN_DIR/getent" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "passwd" && "\${2:-}" == "mk" ]]; then
  printf '%s\n' "mk:x:1000:1000::$TARGET_HOME:/bin/bash"
  exit 0
fi
exec /usr/bin/getent "\$@"
EOF

cat >"$BIN_DIR/sudo" <<'EOF'
#!/usr/bin/env bash
args=()
while (($#)); do
  case "$1" in
    -H) shift || true ;;
    -u) shift; shift ;;
    *) args+=("$1"); shift ;;
  esac
done
exec "${args[@]}"
EOF

for mock in systemctl gnome-extensions tmux; do
  cat >"$BIN_DIR/$mock" <<EOF
#!/usr/bin/env bash
printf '%s %s\n' "$mock" "\$*" >>"$CALL_LOG"
exit 0
EOF
done
chmod +x "$BIN_DIR"/*

run_installer() {
  PATH="$BIN_DIR:$PATH" SUDO_USER=mk DISPLAY=:99 \
    HOME="$TARGET_HOME" \
    XDG_CONFIG_HOME="$TARGET_HOME/.config" \
    XDG_STATE_HOME="$TARGET_HOME/.local/state" \
    ACS_LOG_DIR="$TMP_DIR/pylogs" \
    bash "$REPO_ROOT/install.sh" "$@"
}

# --- Default install: core deploy + all default-on components -------------
run_installer >"$TMP_DIR/install-1.out" 2>"$TMP_DIR/install-1.err" || {
  FAIL=$((FAIL + 1)); echo "FAIL: default install exited non-zero"; }

APP_DIR="$TARGET_HOME/.local/share/agentic-cli-status"
for module in __init__.py acs_logging.py config.py collectors.py openrouter_client.py tmux_console.py poller.py; do
  assert_file_exists "$APP_DIR/agentic_cli_status/$module" "Should deploy $module"
done
EXT_DIR="$TARGET_HOME/.local/share/gnome-shell/extensions/$EXTENSION_UUID"
for asset in extension.js metadata.json stylesheet.css; do
  assert_file_exists "$EXT_DIR/$asset" "Should deploy extension $asset"
done
assert_file_contains "$EXT_DIR/metadata.json" "\"uuid\": \"$EXTENSION_UUID\"" \
  "metadata.json should carry the right uuid"
assert_file_contains "$EXT_DIR/extension.js" "poll_interval_seconds" \
  "Extension refresh timer should follow the configured polling rate"

CONFIG_FILE="$TARGET_HOME/.config/agentic-cli-status/config.json"
assert_file_exists "$CONFIG_FILE" "Should seed the default config"
for cli in codex claude_code grok opencode_go openrouter; do
  assert_file_contains "$CONFIG_FILE" "\"$cli\"" "Config should have a $cli entry"
done
# A default install must enable every default-on CLI (codex/claude_code/grok/
# opencode_go/openrouter): the extension hides any CLI whose status entry has
# enabled=false, so a disabled default-on CLI would vanish from the bar with
# no obvious path back on a plain default install.
PATH="$BIN_DIR:$PATH" python3 - "$CONFIG_FILE" <<'CHECK' || { FAIL=$((FAIL + 1)); echo "FAIL: default install must enable all default-on CLIs"; }
import json, sys
config = json.load(open(sys.argv[1]))
for cli in ("codex", "claude_code", "grok", "opencode_go", "openrouter"):
    assert config["clis"][cli]["enabled"] is True, cli
CHECK
PASS=$((PASS + 1))
assert_not_contains "$CONFIG_FILE" '"cline"' "Cline must not be monitored"
assert_file_contains "$CONFIG_FILE" '"poll_interval_seconds": 5' \
  "Default polling rate should be 5 seconds"
assert_file_contains "$CONFIG_FILE" '"endpoint_min_interval_seconds": 180' \
  "Claude endpoint interval should be seeded configurable at 180 seconds"
assert_file_contains "$TMP_DIR/install-1.out" "seconds" \
  "Install output should state the polling rate is in seconds"

UNIT_FILE="$TARGET_HOME/.config/systemd/user/agentic-cli-status.service"
assert_file_exists "$UNIT_FILE" "Should install the systemd user unit"
assert_file_contains "$CALL_LOG" "systemctl --user enable --now agentic-cli-status.service" \
  "Should enable the poller service"
assert_file_contains "$CALL_LOG" "gnome-extensions enable $EXTENSION_UUID" \
  "Should enable the extension"
assert_file_exists "$REPO_ROOT/.log/install.log" \
  "Installer should write the centralized Bash log"

# --- Idempotence: re-running must not fail or corrupt the config ----------
CONFIG_BEFORE="$(cat "$CONFIG_FILE")"
run_installer >"$TMP_DIR/install-2.out" 2>"$TMP_DIR/install-2.err" || {
  FAIL=$((FAIL + 1)); echo "FAIL: re-run install exited non-zero"; }
if [ "$CONFIG_BEFORE" = "$(cat "$CONFIG_FILE")" ]; then
  PASS=$((PASS + 1))
else
  FAIL=$((FAIL + 1)); echo "FAIL: re-run should preserve the existing config"
fi

# --- Uninstall of one CLI component flips only its config flag ------------
run_installer --uninstall grok_monitor >"$TMP_DIR/uninstall.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --uninstall grok_monitor exited non-zero"; }
PATH="$BIN_DIR:$PATH" python3 - "$CONFIG_FILE" <<'PY' || { FAIL=$((FAIL + 1)); echo "FAIL: grok should be disabled, others untouched"; }
import json, sys
config = json.load(open(sys.argv[1]))
assert config["clis"]["grok"]["enabled"] is False
assert config["clis"]["codex"]["enabled"] is True
assert config["clis"]["claude_code"]["enabled"] is True
PY
PASS=$((PASS + 1))

# --- OpenCode Go component registers the plugin and can be disabled alone ---
run_installer --select opencode_go_monitor >"$TMP_DIR/install-og.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --select opencode_go_monitor exited non-zero"; }
OPENCODE_CONFIG="$TARGET_HOME/.config/opencode/opencode.json"
assert_file_exists "$OPENCODE_CONFIG" "OpenCode Go install should seed opencode.json"
assert_file_contains "$OPENCODE_CONFIG" '"opencode-go-usage"' \
  "opencode.json should list the opencode-go-usage plugin"
assert_file_contains "$REPO_ROOT/lib/status_setup.sh" "Application tab" \
  "Installer should document Application → Cookies auth cookie lookup"
assert_file_contains "$REPO_ROOT/lib/status_setup.sh" "PASTE_YOUR_AUTH_COOKIE_HERE" \
  "Installer should support editing the config file for long auth cookies"
assert_file_contains "$REPO_ROOT/lib/status_setup.sh" "press →" \
  "Installer menu should tell users to press right-arrow to configure credentials"
assert_file_contains "$REPO_ROOT/lib/status_setup.sh" "creds missing" \
  "Installer status should surface missing OpenCode Go credentials in the menu"
run_installer --uninstall opencode_go_monitor >"$TMP_DIR/uninstall-og.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --uninstall opencode_go_monitor exited non-zero"; }
PATH="$BIN_DIR:$PATH" python3 - "$CONFIG_FILE" <<'PY' || { FAIL=$((FAIL + 1)); echo "FAIL: opencode_go should be disabled, others untouched"; }
import json, sys
config = json.load(open(sys.argv[1]))
assert config["clis"]["opencode_go"]["enabled"] is False
assert config["clis"]["grok"]["enabled"] is False
assert config["clis"]["codex"]["enabled"] is True
assert config["clis"]["claude_code"]["enabled"] is True
PY
PASS=$((PASS + 1))

# --- OpenRouter component enables openrouter and can be disabled alone ------
run_installer --select openrouter_monitor >"$TMP_DIR/install-or.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --select openrouter_monitor exited non-zero"; }
PATH="$BIN_DIR:$PATH" python3 - "$CONFIG_FILE" <<'PY' || { FAIL=$((FAIL + 1)); echo "FAIL: openrouter should be enabled with defaults"; }
import json, sys
config = json.load(open(sys.argv[1]))
or_cli = config["clis"]["openrouter"]
assert or_cli["enabled"] is True
assert or_cli["source"] == "headless"
assert or_cli["model_slug"] == "tencent/hy3:free"
assert or_cli["daily_limit"] == 1000
PY
run_installer --uninstall openrouter_monitor >"$TMP_DIR/uninstall-or.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --uninstall openrouter_monitor exited non-zero"; }
PATH="$BIN_DIR:$PATH" python3 - "$CONFIG_FILE" <<'PY' || { FAIL=$((FAIL + 1)); echo "FAIL: openrouter should be disabled, others untouched"; }
import json, sys
config = json.load(open(sys.argv[1]))
assert config["clis"]["openrouter"]["enabled"] is False
assert config["clis"]["opencode_go"]["enabled"] is False
assert config["clis"]["codex"]["enabled"] is True
PY
PASS=$((PASS + 1))
# --- A default install re-enables previously-uninstalled default-on CLIs ---
# grok (and the other default-on CLIs) were just uninstalled above; a plain
# default install must bring them back, or they would stay hidden from the
# taskbar (the extension drops any cli with enabled=false).
run_installer >"$TMP_DIR/reinstall.out" 2>&1 || { FAIL=$((FAIL + 1)); echo "FAIL: default reinstall exited non-zero"; }
PATH="$BIN_DIR:$PATH" python3 - "$CONFIG_FILE" <<'CHECK' || { FAIL=$((FAIL + 1)); echo "FAIL: default reinstall must re-enable disabled default-on CLIs"; }
import json, sys
config = json.load(open(sys.argv[1]))
for cli in ("codex", "claude_code", "grok", "opencode_go", "openrouter"):
    assert config["clis"][cli]["enabled"] is True, cli
CHECK
PASS=$((PASS + 1))

# --- --list-components must be side-effect free ----------------------------
LIST_HOME="$TMP_DIR/list-home"
mkdir -p "$LIST_HOME"
cat >"$BIN_DIR/getent" <<EOF
#!/usr/bin/env bash
if [[ "\${1:-}" == "passwd" && "\${2:-}" == "mk" ]]; then
  printf '%s\n' "mk:x:1000:1000::$LIST_HOME:/bin/bash"
  exit 0
fi
exec /usr/bin/getent "\$@"
EOF
chmod +x "$BIN_DIR/getent"
PATH="$BIN_DIR:$PATH" SUDO_USER=mk DISPLAY=:99 HOME="$LIST_HOME" \
  XDG_CONFIG_HOME="$LIST_HOME/.config" XDG_STATE_HOME="$LIST_HOME/.local/state" \
  bash "$REPO_ROOT/install.sh" --list-components >"$TMP_DIR/list.out" 2>/dev/null || {
    FAIL=$((FAIL + 1)); echo "FAIL: --list-components exited non-zero"; }
for ID in codex_monitor claude_code_monitor grok_monitor opencode_go_monitor openrouter_monitor polling_rate tmux_console startup_service; do
  assert_file_contains "$TMP_DIR/list.out" "$ID" "--list-components should list $ID"
done
assert_not_contains "$TMP_DIR/list.out" "cline" "--list-components must not offer cline"
if [ -e "$LIST_HOME/.local/share/agentic-cli-status" ]; then
  FAIL=$((FAIL + 1)); echo "FAIL: --list-components should not deploy the poller"
else
  PASS=$((PASS + 1))
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"
rm -rf "$TMP_DIR"
if [ "$FAIL" -ne 0 ]; then
  exit 1
fi
