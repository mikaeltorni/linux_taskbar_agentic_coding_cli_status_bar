# Linux Taskbar Agentic Coding CLI Status Bar Instructions

This repository owns the `agentic-cli-status@local` GNOME Shell extension, the
`agentic_cli_status` Python poller (headless rate-limit collectors for Codex,
Claude Code, Grok CLI, OpenCode Go, and OpenRouter plus the tmux debug console), and their installer
integration. Keep agentic CLI rate-limit monitoring in this repo; the CLIs
themselves are installed by `linux_codex_claude_code_setup`, and the in-TUI
status-bar rendering (statusline segments) stays in `claude_code_status_bar`.

Monitoring must never consume plan limits: collectors only read data the CLIs
already store locally (Codex session rollouts, Grok billing logs), their
billing/metadata endpoint (Claude OAuth usage), a headless HTML scrape of
the OpenCode Go workspace page (same source as the `opencode-go-usage` plugin),
or the OpenRouter analytics API (management API key from installer setup;
read-only, not chat/completions),
and the tmux console only ever types rate-limit slash commands (`/status`,
`/usage`, `/ogc-usage`) — never a prompt. Cline
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


## Mandatory programming guidelines prompt

Every agent task in this repository must load the shared
`general-programming-guidelines` skill before the first file edit, using the
harness-native invocation for the runtime in use:

- Codex-family (`ca`, `qa`, `oa`, `na`, …): `$general-programming-guidelines`
- Claude Code, Cline, Grok: `/general-programming-guidelines`
- OpenCode: load `general-programming-guidelines` with the skill tool

Agent Command Center prepends this bare invocation to every dispatched prompt, so the
harness activates the skill before reading the task. When you start a task by hand, invoke it
yourself first. Then follow its Work Loop — dedicated worktree branch before
the first edit, tests, logging, documentation — and do not report the task done
until its Definition of Done checklist passes.
Always finish the delivery: commit in the worktree, merge into the default
branch with `git merge --no-ff`, then reload whatever consumes the change
(installer, skill deployment, service, session). You do not need to be asked.
Never push to a remote and never rewrite history unless the user explicitly
requests it.

