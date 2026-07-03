"""Agentic CLI status: rate-limit polling for Codex, Claude Code, and Grok.

Modules:
    acs_logging   Centralized environment-aware logging.
    config        Config file schema, defaults, load/save.
    collectors    Headless rate-limit collectors + tmux pane parser.
    tmux_console  Background tmux debug console management.
    poller        Aggregating poll loop and status-file writer.
"""
