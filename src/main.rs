//! Ngwa: a fast, native Matrix client.
//!
//! Milestone 1 is a terminal program that proves the core works against a
//! real homeserver: sign in, resume the session, and list your rooms via
//! sliding sync. The egui window comes in Milestone 2.

mod matrix;
mod session;

use std::io::{self, Write};

use anyhow::{Result, bail};

const USAGE: &str = "\
Ngwa — a fast, native Matrix client (milestone 1: terminal preview)

Usage:
  ngwa login     Sign in with a username and password
  ngwa rooms     List your rooms (default)
  ngwa logout    Sign out and forget this device
  ngwa help      Show this message

Set RUST_LOG=info (or debug) to see what the SDK is doing.";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(io::stderr)
        .init();

    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let command = std::env::args().nth(1);
    match command.as_deref() {
        None | Some("rooms") => rooms().await,
        Some("login") => login().await,
        Some("logout") => {
            matrix::logout().await?;
            println!("Signed out.");
            Ok(())
        }
        Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
}

async fn login() -> Result<()> {
    let homeserver = prompt("Homeserver [matrix.org]: ")?;
    let homeserver = if homeserver.is_empty() {
        "matrix.org".to_owned()
    } else {
        homeserver
    };
    let username = prompt("Username: ")?;
    if username.is_empty() {
        bail!("a username is required");
    }
    let password = rpassword::prompt_password("Password: ")?;

    let client = matrix::login(&homeserver, &username, &password).await?;
    let user = client
        .user_id()
        .map(ToString::to_string)
        .unwrap_or(username);
    println!("Signed in as {user}. Run `ngwa rooms` to see your rooms.");
    Ok(())
}

async fn rooms() -> Result<()> {
    let Some(client) = matrix::restore().await? else {
        bail!("not signed in. Run `ngwa login` first.");
    };

    let started = std::time::Instant::now();
    let rooms = matrix::fetch_room_list(&client).await?;
    let elapsed = started.elapsed();

    if rooms.is_empty() {
        println!("No rooms yet.");
    }
    for room in &rooms {
        let marker = if room.is_invite {
            "invite"
        } else if room.mentions > 0 {
            "@"
        } else if room.unread > 0 {
            "•"
        } else {
            ""
        };
        let count = if room.unread > 0 {
            room.unread.to_string()
        } else {
            String::new()
        };
        println!("{marker:>6} {count:>4}  {}", room.name);
    }
    println!(
        "\n{} rooms, loaded in {:.1}s (including the settle wait).",
        rooms.len(),
        elapsed.as_secs_f32()
    );
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}
