//! Agentic CLI status: rate-limit polling for Codex, Claude Code, Grok,
//! OpenCode Go, and OpenRouter.
//!
//! Modules:
//! - `logging` — centralized environment-aware logging
//! - `config` — config file schema, defaults, load/save
//! - `collectors` — headless rate-limit collectors + tmux pane parser
//! - `tmux` — background tmux debug console management
//! - `poller` — aggregating poll loop and status-file writer
//! - `installer_ops` — installer JSON helpers that replaced Python snippets

#![allow(clippy::too_many_arguments, clippy::type_complexity)]

pub mod collectors;
pub mod config;
pub mod http;
pub mod installer_ops;
pub mod logging;
pub mod openrouter_client;
pub mod poller;
pub mod snapshot;
pub mod tmux;

#[cfg(test)]
mod env_test_lock;
#[cfg(test)]
pub mod test_support;
