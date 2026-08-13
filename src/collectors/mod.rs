//! Headless rate-limit collectors and tmux pane parser.
//!
//! Each collector reads data the CLI has already written locally (or exposes
//! on a billing/metadata endpoint) so polling never sends a prompt.

pub mod claude;
pub mod codex;
pub mod grok;
pub mod opencode_go;
pub mod openrouter;
pub mod pane;
pub mod util;

pub use claude::{claude_is_running, claude_user_agent, collect_claude};
pub use codex::{codex_window_label, collect_codex};
pub use grok::collect_grok;
pub use opencode_go::collect_opencode_go;
pub use openrouter::collect_openrouter;
pub use pane::parse_pane;
pub use util::{tail_lines, to_epoch, to_percent, TAIL_READ_BYTES};
