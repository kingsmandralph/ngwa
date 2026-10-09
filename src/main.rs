//! Ngwa: a fast, native Matrix client.
//!
//! `ngwa` opens the desktop window. The terminal commands (`login`, `rooms`,
//! `logout`) share the same engine and are handy for scripting and debugging.

// The sync loop's future nests many async calls; the default limit is too low.
#![recursion_limit = "256"]
// On Windows, open as a normal app without a console window behind it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod backend;
mod cli;
mod demo;
mod gui;
mod matrix;
mod session;
mod timeline;

use std::io;
use std::time::Instant;

use anyhow::{Result, bail};

const USAGE: &str = "\
Ngwa — a fast, native Matrix client

Usage:
  ngwa           Open the app
  ngwa login     Sign in from the terminal
  ngwa rooms     List your rooms in the terminal
  ngwa logout    Sign out and forget this device
  ngwa --demo    Open the app with made-up rooms, no account needed
  ngwa --version Show the version
  ngwa help      Show this message

Set RUST_LOG=matrix_sdk=debug (or info) to see what the SDK is doing.";

fn main() {
    let started = Instant::now();
    if std::env::args().nth(1).is_some() {
        attach_terminal();
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            // The SDK logs routine first-sync events (missing account data,
            // no key backup) as errors. Show only Ngwa's own warnings unless
            // RUST_LOG asks for more.
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "off,ngwa=warn".into()),
        )
        .with_writer(io::stderr)
        .init();

    if let Err(e) = run(started) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(started: Instant) -> Result<()> {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        None => gui::run(false, started),
        Some("--demo") => gui::run(true, started),
        Some("login") => terminal(cli::login()),
        Some("rooms") => terminal(cli::rooms()),
        Some("logout") => terminal(cli::logout()),
        Some("--version" | "-V" | "version") => {
            println!("ngwa {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

/// Run a terminal command to completion on a fresh async runtime.
fn terminal(command: impl Future<Output = Result<()>>) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(command)
}

/// The Windows build has no console of its own, so terminal commands
/// borrow the one they were started from. Elsewhere this does nothing.
fn attach_terminal() {
    #[cfg(windows)]
    // SAFETY: AttachConsole has no preconditions; failure (no parent
    // console, e.g. started from Explorer) is harmless and ignored.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}
