#!/usr/bin/env bash
# check.sh — Run the full local/CI verification suite for this repository.
#
# Steps (in order, each must pass before the next runs):
#   1. cargo fmt --all -- --check
#   2. cargo clippy --all-targets -- -D warnings
#   3. cargo test
#   4. bash -n on every shell script (install.sh, scripts/, lib/, tests/)
#   5. bash tests/test_install.sh
#
# Usage:
#   bash scripts/check.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

log() { printf '[check] %s\n' "$*" >&2; }

# ensure_cargo_on_path — Prefer rustup cargo when present.
ensure_cargo_on_path() {
  if command -v cargo >/dev/null 2>&1; then
    return 0
  fi
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  command -v cargo >/dev/null 2>&1
}

# shell_scripts — Every shell script this repo ships, checked with `bash -n`.
shell_scripts() {
  local f
  for f in "$REPO_ROOT"/install.sh "$REPO_ROOT"/scripts/*.sh \
    "$REPO_ROOT"/lib/*.sh "$REPO_ROOT"/tests/*.sh "$REPO_ROOT"/installer/*.sh; do
    [ -f "$f" ] && printf '%s\n' "$f"
  done
}

run_cargo_fmt_check() {
  log "cargo fmt --all -- --check"
  (cd "$REPO_ROOT" && cargo fmt --all -- --check)
}

run_cargo_clippy() {
  log "cargo clippy --all-targets -- -D warnings"
  (cd "$REPO_ROOT" && cargo clippy --all-targets -- -D warnings)
}

run_cargo_test() {
  log "cargo test"
  (cd "$REPO_ROOT" && cargo test)
}

run_shell_syntax_checks() {
  log "bash -n on every shell script"
  local script
  while IFS= read -r script; do
    log "  bash -n ${script#"$REPO_ROOT"/}"
    bash -n "$script"
  done < <(shell_scripts)
}

run_install_test() {
  log "bash tests/test_install.sh"
  bash "$REPO_ROOT/tests/test_install.sh"
}

main() {
  ensure_cargo_on_path || {
    log "cargo not found (install rustup or apt install cargo)"
    exit 1
  }

  run_cargo_fmt_check
  run_cargo_clippy
  run_cargo_test
  run_shell_syntax_checks
  run_install_test

  log "All checks passed."
}

main "$@"
