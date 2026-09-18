//! Headless rate-limit collectors and tmux pane parser.
//!
//! Each collector reads data the CLI has already written locally (or exposes
//! on a billing/metadata endpoint) so polling never sends a prompt.
//! Cursor monitoring scrapes `cursor.com/dashboard/spending` with a session
//! cookie and never sends a prompt.
//!
//! `codex_reset_forecast` is the one exception to "local data": it reads
//! third-party public forecasts of the next Codex weekly-limit reset.

pub mod claude;
pub mod codex;
pub mod codex_reset_forecast;
pub mod cursor;
pub mod grok;
pub mod opencode_go;
pub mod openrouter;
pub mod pane;
pub mod util;

pub use claude::{claude_is_running, claude_user_agent, collect_claude};
pub use codex::{
    codex_window_label, collect_codex, collect_codex_accounts, load_codex_account_registry,
    selected_codex_home,
};
pub use codex_reset_forecast::{
    collect_codex_reset_forecast, forecast_site, FORECAST_FETCH_MIN_INTERVAL_SECONDS,
    FORECAST_SITES,
};
pub use cursor::{collect_cursor, parse_dashboard_payloads, parse_spending_page};
pub use grok::collect_grok;
pub use opencode_go::collect_opencode_go;
pub use openrouter::collect_openrouter;
pub use pane::{codex_status_send_failed, parse_pane};
pub use util::{tail_lines, to_epoch, to_percent, TAIL_READ_BYTES};
