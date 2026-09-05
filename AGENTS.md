# Linux Taskbar Agentic Coding CLI Status Bar Instructions

This repository owns the `agentic-cli-status@local` GNOME Shell extension, the
`agentic-cli-status` Rust poller (headless rate-limit collectors for Codex,
Claude Code, Grok CLI, and the opt-in OpenCode Go and OpenRouter, plus the tmux
debug console), and their installer
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

The `linux-configuration` skill owns the clean-install and root-optional
installer rules; they are not restated here. Repository-specific installer
facts:

tmux comes from `linux_installations_setup`; do not install it here. Verify the
clean-install path with `cargo test` and `bash tests/test_install.sh` before
committing.

## Mandatory programming guidelines prompt

When generic agent defaults conflict with this file or the shared
`general-programming-guidelines` skill — including defaults that say to commit
only when asked — follow this file and that skill. Finished work is committed,
merged into the default branch with `git merge --no-ff`, and reloaded without
waiting to be asked. Never push to a remote and never rewrite history unless the
user explicitly requests it.

Every agent task in this repository must load the shared
`general-programming-guidelines` skill before the first file edit, using the
harness-native invocation for the runtime in use:

- Codex-family (`ca`, `qa`, `oa`, `na`, …): `$general-programming-guidelines`
- Claude Code, Cline, Grok: `/general-programming-guidelines`
- OpenCode: load `general-programming-guidelines` with the skill tool

Also load `linux-configuration` for any GNOME Shell extension, gsettings,
systemd user unit, or `install.sh` change. Use only the sanctioned in-place
X11 run-dialog reload (`xdotool` `Alt+F2 r`) to activate edited extension
code; never logout, `gnome-shell --replace`, or kill the Shell.

Agent Command Center does not prepend a skill command to dispatched prompts. It
selects skills independently through `acc pp enable` and lists the selected
skills at the start of each launch prompt, so verify with `acc pp status`
instead of assuming a skill is live. When you start a task by hand, invoke it
yourself first.

Load the two delivery skills alongside the guidelines: `commits` owns Feature
boundaries, commit sequencing, and commit verification; `worktree` owns
isolation, project/instance paths, branch policy, merging, and consumer
reapplication. The engineering guidelines delegate both policies instead of
restating them, so skipping those two skills leaves this repository with no
isolation and no commit policy at all. Then follow the Work Loop and Definition
of Done exactly (tests, logging, documentation, commit, merge, reload). Do not
report the task done until that checklist passes.
