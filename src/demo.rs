//! `ngwa --demo`: the window with made-up rooms and no network.
//!
//! Useful for working on the interface without an account, and for
//! screenshots. A message "arrives" every few seconds so live updates
//! can be seen.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::backend::{Command, Event, Outbox, SyncStatus};
use crate::matrix::RoomRow;

const ROOMS: &[(&str, u64, u64)] = &[
    ("Ada Okafor", 2, 2),
    ("Ngwa development", 14, 1),
    ("Rust", 37, 0),
    ("Matrix HQ", 120, 0),
    ("Chidi", 0, 0),
    ("Lagos Rust meetup", 5, 0),
    ("Bug bounty crew", 0, 0),
    ("Element Web", 0, 0),
    ("Mum", 1, 1),
    ("egui", 0, 0),
    ("Weekend football", 9, 0),
    ("Synapse admins", 0, 0),
    ("Book club", 0, 0),
    ("Family", 3, 0),
    ("Matrix.org (Official Account)", 0, 0),
    ("Release planning", 0, 0),
];

fn demo_rooms() -> Vec<RoomRow> {
    let mut rooms: Vec<RoomRow> = ROOMS
        .iter()
        .enumerate()
        .map(|(i, &(name, unread, mentions))| RoomRow {
            id: format!("!demo{i}:example.org"),
            name: name.to_owned(),
            is_invite: false,
            unread,
            mentions,
        })
        .collect();
    rooms.insert(
        3,
        RoomRow {
            id: "!invite:example.org".into(),
            name: "Open source Africa".into(),
            is_invite: true,
            unread: 0,
            mentions: 0,
        },
    );
    rooms
}

pub async fn run(mut commands: UnboundedReceiver<Command>, out: Outbox) {
    let mut signed_in = std::env::var_os("NGWA_DEMO_SIGNED_OUT").is_none();
    let mut rooms = demo_rooms();
    let mut tick = tokio::time::interval(Duration::from_secs(4));
    let mut step = 0usize;

    if signed_in {
        show_session(&out, &rooms);
    } else {
        out.send(Event::SignedOut);
    }

    loop {
        tokio::select! {
            _ = tick.tick(), if signed_in => {
                // A new message lands in a quieter room and lifts it to the top.
                step += 1;
                let index = 4 + (step * 5) % (rooms.len() - 4);
                let mut room = rooms.remove(index);
                room.unread += 1;
                rooms.insert(0, room);
                out.send(Event::Rooms(rooms.clone()));
            }
            command = commands.recv() => match command {
                Some(Command::Login { password, .. }) => {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    if password.is_empty() {
                        out.send(Event::LoginFailed(
                            "sign-in failed: Invalid username or password".into(),
                        ));
                    } else {
                        signed_in = true;
                        rooms = demo_rooms();
                        show_session(&out, &rooms);
                    }
                }
                Some(Command::Logout) => {
                    signed_in = false;
                    out.send(Event::SignedOut);
                }
                None => return,
            },
        }
    }
}

fn show_session(out: &Outbox, rooms: &[RoomRow]) {
    out.send(Event::SignedIn {
        user_id: "@you:example.org".into(),
    });
    out.send(Event::Rooms(rooms.to_vec()));
    out.send(Event::ListLoaded);
    out.send(Event::Sync(SyncStatus::Live));
}
