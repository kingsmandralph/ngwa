//! `ngwa --demo`: the app with made-up rooms and conversations, no network.
//!
//! Useful for working on the interface without an account, and for
//! screenshots. It answers every command the real engine does: rooms open,
//! messages send (with a short "sending" state), replies quote, older
//! history loads in pages, invites can be accepted, and every few seconds a
//! message arrives somewhere.

use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;

use crate::backend::{Command, Event, Outbox, SyncStatus};
use crate::matrix::RoomRow;
use crate::timeline::{BodyKind, MessageRow, ReplyPreview, SendState, TimelineRow};

const ME: (&str, &str) = ("You", "@you:example.org");

const PEOPLE: &[(&str, &str)] = &[
    ("Ada Okafor", "@ada:example.org"),
    ("Chidi", "@chidi:example.org"),
    ("Tolu", "@tolu:example.org"),
    ("Maria", "@maria:example.org"),
    ("Kenji", "@kenji:example.org"),
    ("Sam", "@sam:example.org"),
];

const LINES: &[&str] = &[
    "Morning all",
    "Has anyone tried the new sliding sync build?",
    "Yes, the room list comes up almost instantly now",
    "That's the whole point 😄",
    "I pushed a fix for the scroll jumping, can someone check it?",
    "Looks good to me. One nit in the review.",
    "Are we still on for Saturday?",
    "Yes! 4pm at the usual place",
    "Can't make it this week, sorry",
    "No worries, next time",
    "Reading through the spec for read receipts now",
    "The threading proposal is worth a look too",
    "Coffee first, then code",
    "Same here",
    "Does anyone know a good egui tutorial?",
    "The egui demo app is the best documentation honestly",
    "Shipping it 🚀",
    "Nice work everyone",
];

/// (name, unread, mentions, number of messages, pages of older history)
const ROOMS: &[(&str, u64, u64, usize, usize)] = &[
    ("Ada Okafor", 2, 2, 12, 0),
    ("Ngwa development", 14, 1, 30, 1),
    ("Rust", 37, 0, 30, 1),
    ("Matrix HQ", 120, 0, 30, 3),
    ("Chidi", 0, 0, 8, 0),
    ("Lagos Rust meetup", 5, 0, 16, 0),
    ("Bug bounty crew", 0, 0, 10, 0),
    ("Element Web", 0, 0, 10, 0),
    ("Mum", 1, 1, 6, 0),
    ("egui", 0, 0, 14, 0),
    ("Weekend football", 9, 0, 14, 0),
    ("Synapse admins", 0, 0, 10, 0),
    ("Book club", 0, 0, 8, 0),
    ("Family", 3, 0, 12, 0),
    ("Release planning", 0, 0, 10, 0),
];

struct DemoRoom {
    row: RoomRow,
    /// Oldest first.
    messages: Vec<MessageRow>,
    older_pages: usize,
    seed: usize,
}

struct Demo {
    rooms: Vec<DemoRoom>,
    open: Option<String>,
    next_key: usize,
    now: i64,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Demo {
    fn new() -> Self {
        let now = now_ms();
        let mut demo = Self {
            rooms: Vec::new(),
            open: None,
            next_key: 0,
            now,
        };
        for (i, &(name, unread, mentions, count, older_pages)) in ROOMS.iter().enumerate() {
            let messages = demo.history(i, count, now - (i as i64) * 41 * 60_000);
            let mut room = DemoRoom {
                row: RoomRow {
                    id: format!("!demo{i}:example.org"),
                    name: name.to_owned(),
                    is_invite: false,
                    unread,
                    mentions,
                    preview: None,
                    timestamp: None,
                },
                messages,
                older_pages,
                seed: i,
            };
            refresh_preview(&mut room);
            demo.rooms.push(room);
        }
        demo.rooms.insert(
            3,
            DemoRoom {
                row: RoomRow {
                    id: "!invite:example.org".into(),
                    name: "Open source Africa".into(),
                    is_invite: true,
                    unread: 0,
                    mentions: 0,
                    preview: Some("Invited you".into()),
                    timestamp: Some(now - 3 * 3_600_000),
                },
                messages: Vec::new(),
                older_pages: 0,
                seed: 99,
            },
        );
        demo
    }

    fn key(&mut self) -> String {
        self.next_key += 1;
        format!("demo-{}", self.next_key)
    }

    /// `count` messages ending at `end`, spread over the last few days.
    fn history(&mut self, seed: usize, count: usize, end: i64) -> Vec<MessageRow> {
        let mut messages = Vec::with_capacity(count);
        let mut time = end;
        for n in 0..count {
            let i = seed * 7 + n * 3;
            let (name, id) = if n % 5 == 3 {
                ME
            } else {
                PEOPLE[(seed + n / 2) % PEOPLE.len()]
            };
            let key = self.key();
            messages.push(MessageRow {
                key: key.clone(),
                event_id: Some(format!("$event-{key}")),
                sender_id: id.into(),
                sender_name: name.into(),
                is_own: id == ME.1,
                body: LINES[i % LINES.len()].into(),
                kind: BodyKind::Text,
                timestamp: time,
                reply: None,
                edited: n % 11 == 4,
                state: SendState::Sent,
            });
            // Mostly quick back-and-forth, with an occasional long gap.
            time -= if n % 9 == 8 {
                14 * 3_600_000
            } else {
                90_000 + (i as i64 % 7) * 60_000
            };
        }
        messages.reverse();
        messages
    }

    fn room(&mut self, id: &str) -> Option<&mut DemoRoom> {
        self.rooms.iter_mut().find(|r| r.row.id == id)
    }

    fn room_rows(&self) -> Vec<RoomRow> {
        self.rooms.iter().map(|r| r.row.clone()).collect()
    }

    fn timeline(&self, id: &str) -> Option<Vec<TimelineRow>> {
        let room = self.rooms.iter().find(|r| r.row.id == id)?;
        let mut rows = Vec::new();
        if room.older_pages == 0 && !room.row.is_invite {
            rows.push(TimelineRow::Start);
        }
        let mut last_day = None;
        for message in &room.messages {
            let day = day_of(message.timestamp);
            if last_day != Some(day) {
                rows.push(TimelineRow::Day(message.timestamp));
                last_day = Some(day);
            }
            rows.push(TimelineRow::Message(message.clone()));
        }
        Some(rows)
    }

    /// Move a room to the top of the list, like a new message does.
    fn lift(&mut self, id: &str) {
        if let Some(index) = self.rooms.iter().position(|r| r.row.id == id) {
            let room = self.rooms.remove(index);
            self.rooms.insert(0, room);
        }
    }
}

fn day_of(ms: i64) -> chrono::NaiveDate {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.date_naive())
        .unwrap_or_default()
}

fn refresh_preview(room: &mut DemoRoom) {
    if let Some(last) = room.messages.last() {
        let who = if last.is_own {
            "You"
        } else {
            last.sender_name.as_str()
        };
        room.row.preview = Some(format!("{who}: {}", last.body));
        room.row.timestamp = Some(last.timestamp);
    }
}

pub async fn run(mut commands: UnboundedReceiver<Command>, out: Outbox) {
    let mut demo = Demo::new();
    let mut signed_in = std::env::var_os("NGWA_DEMO_SIGNED_OUT").is_none();
    let mut tick = tokio::time::interval(Duration::from_secs(6));
    tick.tick().await;
    let mut step = 0usize;

    if signed_in {
        show_session(&out, &demo);
    } else {
        out.send(Event::SignedOut);
    }

    loop {
        tokio::select! {
            _ = tick.tick(), if signed_in => {
                incoming(&mut demo, &mut step, &out);
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                handle(command, &mut demo, &mut signed_in, &out).await;
            }
        }
    }
}

/// A message from someone else lands: in the open room it appears in the
/// timeline; elsewhere it bumps the unread count and lifts the room.
fn incoming(demo: &mut Demo, step: &mut usize, out: &Outbox) {
    *step += 1;
    let open = demo.open.clone();
    let target = if let Some(open) = open.clone().filter(|_| step.is_multiple_of(3)) {
        open
    } else {
        let index = 4 + (*step * 5) % (demo.rooms.len() - 4);
        demo.rooms[index].row.id.clone()
    };
    let key = demo.key();
    let (name, id) = PEOPLE[*step % PEOPLE.len()];
    let Some(room) = demo.room(&target) else {
        return;
    };
    if room.row.is_invite {
        return;
    }
    room.messages.push(MessageRow {
        key: key.clone(),
        event_id: Some(format!("$event-{key}")),
        sender_id: id.into(),
        sender_name: name.into(),
        is_own: false,
        body: LINES[(*step * 5) % LINES.len()].into(),
        kind: BodyKind::Text,
        timestamp: now_ms(),
        reply: None,
        edited: false,
        state: SendState::Sent,
    });
    if open.as_deref() != Some(target.as_str()) {
        room.row.unread += 1;
    }
    refresh_preview(room);
    demo.lift(&target);
    out.send(Event::Rooms(demo.room_rows()));
    if open.as_deref() == Some(target.as_str()) {
        send_timeline(demo, &target, out);
    }
}

fn send_timeline(demo: &Demo, id: &str, out: &Outbox) {
    if let Some(rows) = demo.timeline(id) {
        out.send(Event::Timeline {
            room_id: id.to_owned(),
            rows,
        });
    }
}

async fn handle(command: Command, demo: &mut Demo, signed_in: &mut bool, out: &Outbox) {
    match command {
        Command::Login { password, .. } => {
            tokio::time::sleep(Duration::from_millis(600)).await;
            if password.is_empty() {
                out.send(Event::LoginFailed(
                    "sign-in failed: Invalid username or password".into(),
                ));
            } else {
                *signed_in = true;
                *demo = Demo::new();
                show_session(out, demo);
            }
        }
        Command::Logout => {
            *signed_in = false;
            out.send(Event::SignedOut);
        }
        Command::OpenRoom(id) => {
            demo.open = Some(id.clone());
            if let Some(room) = demo.room(&id) {
                room.row.unread = 0;
                room.row.mentions = 0;
            }
            out.send(Event::Rooms(demo.room_rows()));
            send_timeline(demo, &id, out);
        }
        Command::LoadOlder => {
            let Some(id) = demo.open.clone() else { return };
            out.send(Event::History {
                room_id: id.clone(),
                loading: true,
                reached_start: false,
            });
            tokio::time::sleep(Duration::from_millis(700)).await;
            let Some(room) = demo.rooms.iter().find(|r| r.row.id == id) else {
                return;
            };
            let (seed, pages, oldest) = (
                room.seed,
                room.older_pages,
                room.messages.first().map_or(demo.now, |m| m.timestamp),
            );
            if pages > 0 {
                let mut older = demo.history(seed + pages * 3, 15, oldest - 5 * 60_000);
                let room = demo.room(&id).expect("room exists");
                older.append(&mut room.messages);
                room.messages = older;
                room.older_pages -= 1;
                send_timeline(demo, &id, out);
            }
            let reached_start = demo.room(&id).is_some_and(|r| r.older_pages == 0);
            out.send(Event::History {
                room_id: id,
                loading: false,
                reached_start,
            });
        }
        Command::Send { body, reply_to } => {
            let Some(id) = demo.open.clone() else { return };
            let key = demo.key();
            let Some(room) = demo.room(&id) else { return };
            let reply = reply_to.and_then(|event_id| {
                room.messages
                    .iter()
                    .find(|m| m.event_id.as_deref() == Some(event_id.as_str()))
                    .map(|m| ReplyPreview {
                        sender_name: m.sender_name.clone(),
                        body: m.body.clone(),
                    })
            });
            room.messages.push(MessageRow {
                key: key.clone(),
                event_id: None,
                sender_id: ME.1.into(),
                sender_name: ME.0.into(),
                is_own: true,
                body,
                kind: BodyKind::Text,
                timestamp: now_ms(),
                reply,
                edited: false,
                state: SendState::Sending,
            });
            send_timeline(demo, &id, out);

            // The "server" accepts it a moment later.
            tokio::time::sleep(Duration::from_millis(500)).await;
            let Some(room) = demo.room(&id) else { return };
            if let Some(message) = room.messages.iter_mut().find(|m| m.key == key) {
                message.state = SendState::Sent;
                message.event_id = Some(format!("$event-{key}"));
            }
            refresh_preview(room);
            demo.lift(&id);
            out.send(Event::Rooms(demo.room_rows()));
            send_timeline(demo, &id, out);
        }
        Command::AcceptInvite => {
            let Some(id) = demo.open.clone() else { return };
            tokio::time::sleep(Duration::from_millis(400)).await;
            let messages = demo.history(5, 9, now_ms());
            let Some(room) = demo.room(&id) else { return };
            room.row.is_invite = false;
            room.messages = messages;
            refresh_preview(room);
            out.send(Event::Rooms(demo.room_rows()));
            send_timeline(demo, &id, out);
        }
        Command::DeclineInvite => {
            let Some(id) = demo.open.take() else { return };
            demo.rooms.retain(|r| r.row.id != id);
            out.send(Event::Rooms(demo.room_rows()));
        }
    }
}

fn show_session(out: &Outbox, demo: &Demo) {
    out.send(Event::SignedIn {
        user_id: ME.1.into(),
    });
    out.send(Event::Rooms(demo.room_rows()));
    out.send(Event::ListLoaded);
    out.send(Event::Sync(SyncStatus::Live));
}
