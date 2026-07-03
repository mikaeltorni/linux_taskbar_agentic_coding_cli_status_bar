# Linux Taskbar Agentic Coding CLI Status Bar Instructions

This repository owns the `agentic-cli-status@local` GNOME Shell extension, the
`agentic_cli_status` Python poller (headless rate-limit collectors for Codex,
Claude Code, and Grok CLI plus the tmux debug console), and their installer
integration. Keep agentic CLI rate-limit monitoring in this repo; the CLIs
themselves are installed by `linux_codex_claude_code_setup`, and the in-TUI
status-bar rendering (statusline segments) stays in `claude_code_status_bar`.

Monitoring must never consume plan limits: collectors only read data the CLIs
already store locally (Codex session rollouts, Grok billing logs) or their
billing/metadata endpoint (Claude OAuth usage), and the tmux console only ever
types rate-limit slash commands (`/status`, `/usage`) — never a prompt. Cline
is deliberately unmonitored (no usage surface yet); do not add it until it
grows one.

## Clean Installation Compatibility

All changes must remain compatible with a clean installation run through
`installation_scripts/install.sh`. Do not rely on packages, files, settings, or
manual steps that exist only on the current machine (tmux comes from
`linux_installations_setup`). Add every required dependency, asset,
configuration step, and migration to this repo's installer so a fresh checkout
can reproduce the complete setup.

Keep installation steps idempotent and verify the clean-install path for every
change with tests (`python3 -m pytest tests/` and `bash tests/test_install.sh`)
before committing.
