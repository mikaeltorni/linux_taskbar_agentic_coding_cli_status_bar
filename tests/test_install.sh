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

# Keep rustup cargo reachable even though BIN_DIR is prepended for mocks, and
# pin the real toolchain dirs so HOME=sandbox does not hide rustup.
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

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
    FAIL=$((FAIL + 1)); echo "FAIL: $message"
  else
    PASS=$((PASS + 1))
  fi
}

# assert_eq: Assert two strings are equal.
assert_eq() {
  local got="$1" want="$2" message="$3"
  if [ "$got" = "$want" ]; then
    PASS=$((PASS + 1))
  else
    FAIL=$((FAIL + 1))
    echo "FAIL: $message (got '$got', want '$want')"
  fi
}

# cfg_get KEY - Read one dotted config key from the sandbox via the installed
# Rust poller, honoring the sandbox XDG paths.
cfg_get() {
  HOME="$TARGET_HOME" \
    XDG_CONFIG_HOME="$TARGET_HOME/.config" \
    XDG_STATE_HOME="$TARGET_HOME/.local/state" \
    "$TARGET_HOME/.local/bin/agentic-cli-status" config-get "$1"
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

# isc_functions_dir - locate the shared framework checkout for the sandbox run.
#   The sandbox HOME hides install.sh's usual ~/projects fallback, and the
#   sibling directory only exists for the live checkout (not for a git
#   worktree), so both layouts are probed here and handed over explicitly
#   instead of letting the installer reach the network.
isc_functions_dir() {
  local candidate
  for candidate in "${ISC_FUNCTIONS_DIR:-}" \
                   "$REPO_ROOT/../linux_installation_scripts_functions" \
                   "$REPO_ROOT/../../linux_installation_scripts_functions" \
                   "$HOME/projects/linux_installation_scripts_functions"; do
    [ -n "$candidate" ] || continue
    if [ -f "$candidate/component_loader.sh" ]; then
      (cd "$candidate" && pwd)
      return 0
    fi
  done
  return 1
}
ISC_FUNCTIONS_DIR_RESOLVED="$(isc_functions_dir || true)"

run_installer() {
  PATH="$BIN_DIR:$PATH" SUDO_USER=mk DISPLAY=:99 \
    HOME="$TARGET_HOME" \
    ISC_FUNCTIONS_DIR="$ISC_FUNCTIONS_DIR_RESOLVED" \
    XDG_CONFIG_HOME="$TARGET_HOME/.config" \
    XDG_STATE_HOME="$TARGET_HOME/.local/state" \
    CARGO_HOME="$CARGO_HOME" \
    RUSTUP_HOME="$RUSTUP_HOME" \
    ACS_LOG_DIR="$TMP_DIR/logs" \
    bash "$REPO_ROOT/install.sh" "$@"
}

# --- Default install: core deploy + all default-on components -------------
run_installer >"$TMP_DIR/install-1.out" 2>"$TMP_DIR/install-1.err" || {
  FAIL=$((FAIL + 1)); echo "FAIL: default install exited non-zero"
  echo "----- install stdout -----"; cat "$TMP_DIR/install-1.out"
  echo "----- install stderr -----"; cat "$TMP_DIR/install-1.err"
}

ACS_BIN_PATH="$TARGET_HOME/.local/bin/agentic-cli-status"
assert_file_exists "$ACS_BIN_PATH" "Should deploy the Rust poller binary"
if [ -x "$ACS_BIN_PATH" ]; then
  PASS=$((PASS + 1))
else
  FAIL=$((FAIL + 1)); echo "FAIL: deployed poller should be executable"
fi
if [ -d "$TARGET_HOME/.local/share/agentic-cli-status/agentic_cli_status" ]; then
  FAIL=$((FAIL + 1)); echo "FAIL: former Python package directory should be removed"
else
  PASS=$((PASS + 1))
fi
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
for cli in codex claude_code grok opencode_go openrouter cursor; do
  assert_file_contains "$CONFIG_FILE" "\"$cli\"" "Config should have a $cli entry"
done
# A default install must enable every default-on CLI; Claude Code is opt-in via
# --select claude_code_monitor and must remain hidden until selected.
for cli in codex grok; do
  assert_eq "$(cfg_get "clis.$cli.enabled")" "true" "default install must enable $cli"
done
assert_eq "$(cfg_get clis.claude_code.enabled)" "false" \
  "default install must keep claude_code disabled"
assert_eq "$(cfg_get clis.opencode_go.enabled)" "false" \
  "OpenCode Go is opt-in and must stay off after a default install"
assert_eq "$(cfg_get clis.openrouter.enabled)" "false" \
  "OpenRouter is opt-in and must stay off after a default install"
assert_eq "$(cfg_get clis.cursor.enabled)" "false" \
  "Cursor spending is opt-in and must stay off after a default install"
assert_eq "$(cfg_get codex_reset_forecast.enabled)" "true" \
  "Codex reset forecasting must be enabled by a default install"
assert_file_exists "$TARGET_HOME/.local/bin/crs" \
  "Core deploy should install the crs setup shorthand"
assert_eq "$(cfg_get clis.cursor.fetch_min_interval_seconds)" "30" \
  "Default Cursor website scrape interval should be 30 seconds"
assert_not_contains "$CONFIG_FILE" '"cline"' "Cline must not be monitored"
assert_file_contains "$CONFIG_FILE" '"poll_interval_seconds": 5' \
  "Default polling rate should be 5 seconds"
assert_file_contains "$CONFIG_FILE" '"endpoint_min_interval_seconds": 180' \
  "Claude endpoint interval should be seeded configurable at 180 seconds"
assert_file_contains "$TMP_DIR/install-1.out" "seconds" \
  "Install output should state the polling rate is in seconds"

# --- Non-interactive installer parameter overrides the shared cadence ------
ACS_POLL_INTERVAL_SECONDS=7 run_installer --select polling_rate \
  >"$TMP_DIR/install-poll-rate.out" 2>"$TMP_DIR/install-poll-rate.err" || {
    FAIL=$((FAIL + 1)); echo "FAIL: polling-rate parameter install exited non-zero"; }
assert_eq "$(cfg_get poll_interval_seconds)" "7" \
  "installer polling-rate parameter should persist"

# Restore the requested default for the remaining installer assertions.
ACS_POLL_INTERVAL_SECONDS=5 run_installer --select polling_rate \
  >"$TMP_DIR/install-poll-default.out" 2>"$TMP_DIR/install-poll-default.err" || {
    FAIL=$((FAIL + 1)); echo "FAIL: restoring default polling rate exited non-zero"; }
if ACS_POLL_INTERVAL_SECONDS=2 run_installer --select polling_rate \
  >"$TMP_DIR/install-poll-invalid.out" 2>"$TMP_DIR/install-poll-invalid.err"; then
  FAIL=$((FAIL + 1)); echo "FAIL: invalid polling-rate parameter should fail"
else
  PASS=$((PASS + 1))
fi
assert_file_contains "$CONFIG_FILE" '"poll_interval_seconds": 5' \
  "Invalid polling-rate parameter should preserve the prior value"

UNIT_FILE="$TARGET_HOME/.config/systemd/user/agentic-cli-status.service"
assert_file_exists "$UNIT_FILE" "Should install the systemd user unit"
assert_file_contains "$UNIT_FILE" "agentic-cli-status --daemon" \
  "Unit should start the Rust poller"
assert_not_contains "$UNIT_FILE" "python3" "Unit must not invoke Python"
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
assert_eq "$(cfg_get clis.grok.enabled)" "false" "grok should be disabled"
assert_eq "$(cfg_get clis.codex.enabled)" "true" "codex should stay enabled"
assert_eq "$(cfg_get clis.claude_code.enabled)" "false" "claude_code should stay disabled"

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
assert_eq "$(cfg_get clis.opencode_go.enabled)" "false" "opencode_go should be disabled"
assert_eq "$(cfg_get clis.grok.enabled)" "false" "grok should stay disabled"
assert_eq "$(cfg_get clis.codex.enabled)" "true" "codex should stay enabled"
assert_eq "$(cfg_get clis.claude_code.enabled)" "false" "claude_code should stay disabled"

# --- OpenRouter component enables openrouter and can be disabled alone ------
run_installer --select openrouter_monitor >"$TMP_DIR/install-or.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --select openrouter_monitor exited non-zero"; }
assert_eq "$(cfg_get clis.openrouter.enabled)" "true" "openrouter should be enabled"
assert_eq "$(cfg_get clis.openrouter.source)" "headless" "openrouter source is headless"
assert_eq "$(cfg_get clis.openrouter.model_slug)" "stealth/ox-alpha" \
  "openrouter default model slug"
assert_eq "$(cfg_get clis.openrouter.daily_limit)" "1000" "openrouter default daily limit"
# An opt-in that the user made must survive later default installs; only an
# explicit --uninstall may take the OR segment back off the bar.
run_installer >"$TMP_DIR/install-or-keep.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: default install after openrouter opt-in exited non-zero"; }
assert_eq "$(cfg_get clis.openrouter.enabled)" "true" \
  "a default install must keep an opted-in openrouter enabled"
run_installer --uninstall openrouter_monitor >"$TMP_DIR/uninstall-or.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --uninstall openrouter_monitor exited non-zero"; }
assert_eq "$(cfg_get clis.openrouter.enabled)" "false" "openrouter should be disabled"
assert_eq "$(cfg_get clis.opencode_go.enabled)" "false" "opencode_go should stay disabled"
assert_eq "$(cfg_get clis.codex.enabled)" "true" "codex should stay enabled"

# --- Cursor spending: env-var/crs setup without a TTY ----------------------
assert_eq "$(cfg_get clis.cursor.enabled)" "false" \
  "cursor should stay off until crs setup or --select cursor_monitor"
CRS_AUTH_COOKIE="workos-test-cookie" CRS_FETCH_INTERVAL_SECONDS=45 \
  HOME="$TARGET_HOME" \
  XDG_CONFIG_HOME="$TARGET_HOME/.config" \
  XDG_STATE_HOME="$TARGET_HOME/.local/state" \
  PATH="$TARGET_HOME/.local/bin:$PATH" \
  crs setup >"$TMP_DIR/crs-setup.out" 2>"$TMP_DIR/crs-setup.err" || {
    FAIL=$((FAIL + 1)); echo "FAIL: crs setup exited non-zero";
    echo "----- crs setup stdout -----"; cat "$TMP_DIR/crs-setup.out";
    echo "----- crs setup stderr -----"; cat "$TMP_DIR/crs-setup.err"; }
assert_eq "$(cfg_get clis.cursor.enabled)" "true" \
  "crs setup should enable Cursor monitoring without a TTY"
assert_eq "$(cfg_get clis.cursor.source)" "headless" "cursor source is headless"
assert_eq "$(cfg_get clis.cursor.fetch_min_interval_seconds)" "45" \
  "crs setup should persist CRS_FETCH_INTERVAL_SECONDS"
assert_file_exists "$TARGET_HOME/.config/agentic-cli-status/cursor-spending.json" \
  "crs setup should write the spending cookie file"
assert_file_contains "$TARGET_HOME/.config/agentic-cli-status/cursor-spending.json" \
  "workos-test-cookie" "cookie file should store the session cookie"
assert_not_contains "$TMP_DIR/crs-setup.out" "workos-test-cookie" \
  "crs setup stdout must not print the cookie"
assert_not_contains "$TMP_DIR/crs-setup.err" "workos-test-cookie" \
  "crs setup stderr must not print the cookie"
run_installer --uninstall cursor_monitor >"$TMP_DIR/uninstall-crs.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --uninstall cursor_monitor exited non-zero"; }
assert_eq "$(cfg_get clis.cursor.enabled)" "false" "cursor should be disabled"
CRS_AUTH_COOKIE="workos-test-cookie" \
  HOME="$TARGET_HOME" \
  XDG_CONFIG_HOME="$TARGET_HOME/.config" \
  XDG_STATE_HOME="$TARGET_HOME/.local/state" \
  PATH="$TARGET_HOME/.local/bin:$PATH" \
  crs setup >"$TMP_DIR/crs-setup-default.out" 2>"$TMP_DIR/crs-setup-default.err" || {
    FAIL=$((FAIL + 1)); echo "FAIL: crs setup (default interval) exited non-zero"; }
assert_eq "$(cfg_get clis.cursor.fetch_min_interval_seconds)" "30" \
  "crs setup without CRS_FETCH_INTERVAL_SECONDS should default to 30s"
CRS_AUTH_COOKIE="workos-select-cookie" CRS_FETCH_INTERVAL_SECONDS=12 \
  run_installer --select cursor_monitor >"$TMP_DIR/install-crs.out" 2>&1 || {
    FAIL=$((FAIL + 1)); echo "FAIL: --select cursor_monitor exited non-zero"; }
assert_eq "$(cfg_get clis.cursor.enabled)" "true" \
  "--select cursor_monitor should enable cursor"
assert_eq "$(cfg_get clis.cursor.fetch_min_interval_seconds)" "12" \
  "installer should apply CRS_FETCH_INTERVAL_SECONDS without a TTY"
# --- A default install re-enables previously-uninstalled default-on CLIs ---
# grok (and the other default-on CLIs) were just uninstalled above; a plain
# default install must bring them back, or they would stay hidden from the
# taskbar (the extension drops any cli with enabled=false). claude_code stays
# off unless explicitly selected.
run_installer >"$TMP_DIR/reinstall.out" 2>&1 || { FAIL=$((FAIL + 1)); echo "FAIL: default reinstall exited non-zero"; }
for cli in codex grok; do
  assert_eq "$(cfg_get "clis.$cli.enabled")" "true" "default reinstall must re-enable $cli"
done
assert_eq "$(cfg_get clis.claude_code.enabled)" "false" \
  "default reinstall must keep claude_code disabled"
assert_eq "$(cfg_get clis.opencode_go.enabled)" "false" \
  "A default install must NOT re-enable the opt-in OpenCode Go monitor"
assert_eq "$(cfg_get clis.openrouter.enabled)" "false" \
  "A default install must NOT re-enable the opt-in OpenRouter monitor"
assert_eq "$(cfg_get clis.cursor.enabled)" "true" \
  "an opted-in cursor monitor must survive a later default install"

# --- claude_code_monitor remains selectable explicitly ----------------------
run_installer --select claude_code_monitor >"$TMP_DIR/install-cc.out" 2>&1 || {
  FAIL=$((FAIL + 1)); echo "FAIL: --select claude_code_monitor exited non-zero"; }
assert_eq "$(cfg_get clis.claude_code.enabled)" "true" \
  "--select claude_code_monitor should enable claude_code"
assert_eq "$(cfg_get clis.codex.enabled)" "true" "codex should stay enabled"

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
  ISC_FUNCTIONS_DIR="$ISC_FUNCTIONS_DIR_RESOLVED" \
  bash "$REPO_ROOT/install.sh" --list-components >"$TMP_DIR/list.out" 2>/dev/null || {
    FAIL=$((FAIL + 1)); echo "FAIL: --list-components exited non-zero"; }
for ID in codex_monitor claude_code_monitor grok_monitor opencode_go_monitor openrouter_monitor cursor_monitor polling_rate tmux_console startup_service; do
  assert_file_contains "$TMP_DIR/list.out" "$ID" "--list-components should list $ID"
done
assert_eq "$(awk -F '\t' '$1 == "cursor_monitor" {print $3}' "$TMP_DIR/list.out")" "off" \
  "--list-components should mark cursor_monitor opt-in"
assert_eq "$(awk -F '\t' '$1 == "claude_code_monitor" {print $3}' "$TMP_DIR/list.out")" "off" \
  "--list-components should mark claude_code_monitor opt-in"
assert_not_contains "$TMP_DIR/list.out" "cline" "--list-components must not offer cline"
if [ -e "$LIST_HOME/.local/share/agentic-cli-status" ] || [ -e "$LIST_HOME/.local/bin/agentic-cli-status" ]; then
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
