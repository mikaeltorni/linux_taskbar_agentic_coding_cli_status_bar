//! `agentic-cli-status` — headless rate-limit poller for the GNOME taskbar.
//!
//! Replaces the former Python package. The GNOME Shell extension still reads
//! `status.json`; this binary is the only runtime that writes it.
//!
//! Usage:
//!     agentic-cli-status --once     # single tick, then exit
//!     agentic-cli-status --daemon   # loop at the configured rate
//!     agentic-cli-status --show     # pretty-print the status file
//!     agentic-cli-status config-get KEY
//!     agentic-cli-status config-set KEY JSON_VALUE

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use agentic_cli_status::config::load_config;
use agentic_cli_status::installer_ops;
use agentic_cli_status::logging;
use agentic_cli_status::poller::{poll_once, run_daemon, status_path, PollHooks};

#[derive(Parser)]
#[command(
    name = "agentic-cli-status",
    about = "Headless rate-limit poller for Codex, Claude Code, Grok, OpenCode Go, OpenRouter, and Cursor spending",
    version
)]
struct Cli {
    /// Run one tick and exit.
    #[arg(long, conflicts_with_all = ["daemon", "show"])]
    once: bool,
    /// Poll at the configured rate until SIGTERM/SIGINT.
    #[arg(long, conflicts_with_all = ["once", "show"])]
    daemon: bool,
    /// Print the current status file.
    #[arg(long, conflicts_with_all = ["once", "daemon"])]
    show: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print one dotted config key's effective value.
    ConfigGet { key: String },
    /// Set one dotted config key from a JSON value.
    ConfigSet { key: String, value: String },
    /// Seed the default config file when it is missing.
    ConfigInit,
    /// Exit 0 when an OpenRouter management API key is configured.
    HasOpenrouterKey,
    /// Register the opencode-go-usage plugin in opencode.json.
    RegisterOpencodePlugin { path: PathBuf },
    /// Write OpenCode Go workspace ID and auth cookie.
    WriteOpencodeCreds {
        path: PathBuf,
        workspace: String,
        cookie: String,
    },
    /// Exit 0 when OpenCode Go credentials in PATH are valid.
    OpencodeCredsValid { path: PathBuf },
    /// Print the stored OpenCode Go workspace ID.
    OpencodeWorkspaceId { path: PathBuf },
    /// Cursor spending-page monitor (not a coding CLI).
    Crs {
        #[command(subcommand)]
        command: CrsCommand,
    },
    /// Write the Cursor spending-page session cookie.
    WriteCursorCreds { path: PathBuf, cookie: String },
    /// Exit 0 when Cursor spending credentials in PATH are valid.
    CursorCredsValid { path: PathBuf },
}

#[derive(Subcommand)]
enum CrsCommand {
    /// Enable CRS from CRS_AUTH_COOKIE / CRS_FETCH_INTERVAL_SECONDS (no TTY).
    Setup,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = if let Some(command) = cli.command {
        match command {
            Command::ConfigGet { key } => installer_ops::config_get(&key),
            Command::ConfigSet { key, value } => installer_ops::config_set(&key, &value),
            Command::ConfigInit => installer_ops::config_init(),
            Command::HasOpenrouterKey => installer_ops::has_openrouter_key(None),
            Command::RegisterOpencodePlugin { path } => {
                installer_ops::register_opencode_plugin(&path)
            }
            Command::WriteOpencodeCreds {
                path,
                workspace,
                cookie,
            } => installer_ops::write_opencode_creds(&path, &workspace, &cookie),
            Command::OpencodeCredsValid { path } => installer_ops::opencode_creds_valid(&path),
            Command::OpencodeWorkspaceId { path } => installer_ops::opencode_workspace_id(&path),
            Command::Crs { command } => match command {
                CrsCommand::Setup => installer_ops::crs_setup(None),
            },
            Command::WriteCursorCreds { path, cookie } => {
                installer_ops::write_cursor_creds(&path, &cookie)
            }
            Command::CursorCredsValid { path } => installer_ops::cursor_creds_valid(&path),
        }
    } else if cli.show {
        let target = status_path(None);
        match fs::read_to_string(&target) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(exc) => {
                eprintln!("no status available at {}: {exc}", target.display());
                1
            }
        }
    } else if cli.once {
        logging::info("running one poll tick");
        let _ = poll_once(None, None, None, None, None, &PollHooks::default());
        0
    } else if cli.daemon {
        run_daemon(None, PollHooks::default());
        0
    } else {
        eprintln!("specify --once, --daemon, --show, or a subcommand");
        let _ = load_config(None);
        2
    };
    ExitCode::from(code as u8)
}
