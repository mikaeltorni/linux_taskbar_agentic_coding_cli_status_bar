# Linux Taskbar Agentic Coding CLI Status Bar Instructions

This repository owns the `agentic-cli-status@local` GNOME Shell extension, the
`agentic-cli-status` Rust poller (headless rate-limit collectors for Codex,
Claude Code, Grok CLI, the opt-in OpenCode Go and OpenRouter, and the opt-in
Cursor spending-page scrape, plus the tmux debug console), and their installer
integration. Keep agentic CLI rate-limit monitoring in this repo; the CLIs
themselves are installed by `linux_codex_claude_code_setup`, and the in-TUI
status-bar rendering (statusline segments) stays in `claude_code_status_bar`.

Monitoring must never consume plan limits: collectors only read data the CLIs
already store locally (Codex session rollouts, Grok billing logs), their
billing/metadata endpoint (Claude OAuth usage), a headless HTML scrape of
the OpenCode Go workspace page (same source as the `opencode-go-usage` plugin),
the OpenRouter analytics API (management API key from installer setup;
read-only, not chat/completions), or a headless scrape of the Cursor spending dashboard's usage endpoints
(`/api/usage-summary` and `/api/dashboard/get-sand-usage-status`; the HTML
`/dashboard/spending` URL 307-loops through a web bootstrap and is not
fetched) with a session cookie; included Pro pools only — Cursor Models,
Other Models, Grok Bot, usage-limit reset; on-demand usage is never collected),
and the tmux console only ever types rate-limit slash commands (`/status`,
`/usage`, `/ogc-usage`) — never a prompt. Cursor is not a CLI and has no tmux
window. Cline is deliberately unmonitored (no usage surface yet); do not add
it until it grows one.

## Clean Installation Compatibility

The `linux-configuration` skill owns the clean-install and root-optional
installer rules; they are not restated here. Repository-specific installer
facts:

tmux comes from `linux_installations_setup`; do not install it here. Verify the
clean-install path with `cargo test` and `bash tests/test_install.sh` before
committing.

## Shared workflow

This file intentionally contains project-specific ownership, data-safety,
component, integration, and deployment facts. The reusable workflow rules
live in the selected skills and must not be copied into every project file.

Before editing, read this project file, then load:

- `general-programming-guidelines` for the engineering workflow;
- `commits` for capability boundaries and commit verification;
- `worktree` for isolation, branch, merge, and consumer reapplication; and
- `linux-configuration` when the task touches GNOME, desktop settings,
  systemd user services, or any `install.sh`.

Those skills are the cross-project source of truth for workflow, delivery,
desktop deployment, clean-install compatibility, and root-optional installer
rules. Project-specific sections here may narrow ownership or add required
verification, but must not restate or contradict those shared policies.
