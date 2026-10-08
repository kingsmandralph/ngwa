//! Terminal commands: `ngwa login`, `ngwa rooms`, `ngwa logout`.
//! Handy for scripting and for debugging without the window.

use std::io::{self, Write};

use anyhow::{Result, bail};

use crate::matrix;

pub async fn login() -> Result<()> {
    let id = prompt("Matrix ID (like @you:matrix.org) or username: ")?;
    // A full Matrix ID already names its server; only ask when it doesn't.
    let homeserver = if matrix::split_matrix_id(&id).is_some() {
        String::new()
    } else if id.is_empty() {
        bail!("a username is required");
    } else {
        prompt("Homeserver (press Enter for matrix.org): ")?
    };
    let (username, homeserver) = matrix::resolve_login(&id, &homeserver)?;
    let password = rpassword::prompt_password("Password: ")?;

    let client = matrix::login(&homeserver, &username, &password).await?;
    let user = client
        .user_id()
        .map(ToString::to_string)
        .unwrap_or(username);
    println!(
        "Signed in as {user}. Run `ngwa` to open the app, or `ngwa rooms` to list your rooms."
    );
    Ok(())
}

pub async fn rooms() -> Result<()> {
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

pub async fn logout() -> Result<()> {
    // Sign out on the server if the session still opens; forget it either way.
    let client = match matrix::restore().await {
        Ok(client) => client,
        Err(e) => {
            eprintln!("warning: {e:#}; forgetting the session locally anyway");
            None
        }
    };
    matrix::logout(client).await?;
    println!("Signed out.");
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_owned())
}
