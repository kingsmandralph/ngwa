//! The engine behind the window.
//!
//! All Matrix work runs on a tokio runtime on background threads. The window
//! sends it [`Command`]s and gets [`Event`]s back, and never waits on the
//! network itself, so it never freezes.

use std::sync::{Arc, mpsc};
use std::time::Duration;

use eframe::egui;
use futures_util::{StreamExt, pin_mut};
use matrix_sdk::{
    Client,
    ruma::{
        OwnedEventId, RoomId,
        api::client::receipt::create_receipt::v3::ReceiptType,
        events::room::message::{RoomMessageEventContent, RoomMessageEventContentWithoutRelation},
    },
};
use matrix_sdk_ui::{
    Timeline,
    eyeball_im::Vector,
    room_list_service::{RoomListItem, RoomListLoadingState, filters::new_filter_non_left},
    sync_service::State,
    timeline::RoomExt,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::matrix::{self, PAGE_SIZE, RoomRow};
use crate::timeline::{self, TimelineRow};

/// How many older events to ask for each time the user scrolls to the top.
const PAGE_OF_HISTORY: u16 = 40;

/// What the window asks the engine to do.
pub enum Command {
    Login {
        id: String,
        homeserver: String,
        password: String,
    },
    Logout,
    /// Show this room's messages, replacing any room already open.
    OpenRoom(String),
    /// Load older messages in the open room.
    LoadOlder,
    /// Send a text message to the open room, optionally as a reply.
    Send {
        body: String,
        reply_to: Option<String>,
    },
    /// Join the open room, which the user was invited to.
    AcceptInvite,
    /// Turn down the invite to the open room.
    DeclineInvite,
}

/// What the engine tells the window.
#[derive(Debug)]
pub enum Event {
    /// No saved session: show the sign-in screen.
    SignedOut,
    /// A sign-in attempt failed, with a message for the user.
    LoginFailed(String),
    /// A session is open for this user.
    SignedIn { user_id: String },
    /// The full room list, sorted, after any change.
    Rooms(Vec<RoomRow>),
    /// The server has answered with the room list at least once.
    ListLoaded,
    /// The state of the connection to the homeserver.
    Sync(SyncStatus),
    /// The open room's messages, oldest first, after any change.
    Timeline {
        room_id: String,
        rows: Vec<TimelineRow>,
    },
    /// Older messages are loading, or have finished loading.
    History {
        room_id: String,
        loading: bool,
        reached_start: bool,
    },
    /// Something went wrong that the user should see.
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SyncStatus {
    Connecting,
    Live,
    Offline,
    Failed(String),
}

/// The window's handle on the engine.
pub struct Backend {
    commands: UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
}

impl Backend {
    /// Start the engine on `runtime`. `ctx` is woken whenever an event
    /// arrives, so the window redraws only when something changed.
    pub fn start(runtime: &tokio::runtime::Handle, ctx: egui::Context, demo: bool) -> Self {
        let (commands, command_rx) = unbounded_channel();
        let (event_tx, events) = mpsc::channel();
        let out = Outbox { tx: event_tx, ctx };
        if demo {
            runtime.spawn(crate::demo::run(command_rx, out));
        } else {
            runtime.spawn(run(command_rx, out));
        }
        Self { commands, events }
    }

    pub fn send(&self, command: Command) {
        // Only fails if the engine has stopped, and then there is nobody to tell.
        let _ = self.commands.send(command);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

/// Sends events to the window and wakes it up.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl Outbox {
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

async fn run(mut commands: UnboundedReceiver<Command>, out: Outbox) {
    let mut client = match matrix::restore().await {
        Ok(client) => client,
        Err(e) => {
            out.send(Event::Error(format!("{e:#}")));
            None
        }
    };

    loop {
        let current = match client.take() {
            Some(current) => current,
            None => match sign_in(&mut commands, &out).await {
                Some(signed_in) => signed_in,
                None => return,
            },
        };

        let user_id = current
            .user_id()
            .map(ToString::to_string)
            .unwrap_or_default();
        out.send(Event::SignedIn { user_id });

        match run_session(&current, &mut commands, &out).await {
            Exit::Closed => return,
            Exit::Logout => {
                if let Err(e) = matrix::logout(Some(current)).await {
                    out.send(Event::Error(format!("{e:#}")));
                }
            }
        }
    }
}

/// Show the sign-in screen and wait until a sign-in succeeds.
/// Returns `None` if the window has closed.
async fn sign_in(commands: &mut UnboundedReceiver<Command>, out: &Outbox) -> Option<Client> {
    out.send(Event::SignedOut);
    loop {
        // Anything other than a sign-in is meaningless while signed out.
        if let Command::Login {
            id,
            homeserver,
            password,
        } = commands.recv().await?
        {
            let result = async {
                let (username, homeserver) = matrix::resolve_login(&id, &homeserver)?;
                matrix::login(&homeserver, &username, &password).await
            }
            .await;
            match result {
                Ok(client) => return Some(client),
                Err(e) => out.send(Event::LoginFailed(format!("{e:#}"))),
            }
        }
    }
}

enum Exit {
    Logout,
    Closed,
}

/// Keep the room list in sync until the user signs out or the window closes.
async fn run_session(
    client: &Client,
    commands: &mut UnboundedReceiver<Command>,
    out: &Outbox,
) -> Exit {
    out.send(Event::Sync(SyncStatus::Connecting));

    let sync = match matrix::sync_service(client).await {
        Ok(sync) => sync,
        Err(e) => {
            out.send(Event::Sync(SyncStatus::Failed(format!("{e:#}"))));
            return wait_for_logout(commands).await;
        }
    };
    let room_list = match sync.room_list_service().all_rooms().await {
        Ok(list) => list,
        Err(e) => {
            out.send(Event::Sync(SyncStatus::Failed(e.to_string())));
            return wait_for_logout(commands).await;
        }
    };

    let mut sync_state = sync.state();
    let mut loading = room_list.loading_state();
    let (entries, controller) = room_list.entries_with_dynamic_adapters(PAGE_SIZE);
    controller.set_filter(Box::new(new_filter_non_left()));
    pin_mut!(entries);

    // The list streams in from the local database first, then from the
    // server, so a returning user sees their rooms before the network answers.
    sync.start().await;

    let mut rooms: Vector<RoomListItem> = Vector::new();
    let mut list_loaded = false;
    let mut open: Option<OpenRoom> = None;
    let exit = loop {
        tokio::select! {
            Some(diffs) = entries.next() => {
                for diff in diffs {
                    diff.apply(&mut rooms);
                }
                out.send(Event::Rooms(RoomRow::from_items(&rooms).await));
            }
            Some(state) = loading.next(), if !list_loaded => {
                if matches!(state, RoomListLoadingState::Loaded { .. }) {
                    list_loaded = true;
                    out.send(Event::ListLoaded);
                }
            }
            Some(state) = sync_state.next() => {
                if let Some(status) = status_of(state) {
                    out.send(Event::Sync(status));
                }
            }
            command = commands.recv() => match command {
                Some(Command::Logout) => break Exit::Logout,
                Some(Command::OpenRoom(room_id)) => {
                    // Drop the previous room first, which stops its updates.
                    drop(open.take());
                    open = open_room(client, &room_id, out).await;
                }
                Some(Command::LoadOlder) => {
                    if let Some(room) = &open {
                        room.load_older(out);
                    }
                }
                Some(Command::Send { body, reply_to }) => {
                    if let Some(room) = &open {
                        room.send(body, reply_to, out);
                    }
                }
                Some(Command::AcceptInvite) => {
                    if let Some(room) = &open {
                        room.answer_invite(true, out);
                    }
                }
                Some(Command::DeclineInvite) => {
                    if let Some(room) = open.take() {
                        room.answer_invite(false, out);
                    }
                }
                Some(Command::Login { .. }) => {}
                None => break Exit::Closed,
            },
        }
    };
    drop(open);

    // Give the sync loop a moment to wind down cleanly.
    let _ = tokio::time::timeout(Duration::from_secs(3), sync.stop()).await;
    exit
}

fn status_of(state: State) -> Option<SyncStatus> {
    match state {
        State::Idle | State::Terminated => None,
        State::Running => Some(SyncStatus::Live),
        State::Offline => Some(SyncStatus::Offline),
        State::Error(e) => Some(SyncStatus::Failed(e.to_string())),
    }
}

async fn wait_for_logout(commands: &mut UnboundedReceiver<Command>) -> Exit {
    loop {
        match commands.recv().await {
            Some(Command::Logout) => return Exit::Logout,
            Some(_) => {}
            None => return Exit::Closed,
        }
    }
}

/// The room on screen: its timeline, and the task forwarding its changes.
struct OpenRoom {
    room_id: String,
    room: matrix_sdk::Room,
    timeline: Option<Arc<Timeline>>,
    forwarder: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for OpenRoom {
    fn drop(&mut self) {
        if let Some(task) = &self.forwarder {
            task.abort();
        }
    }
}

async fn open_room(client: &Client, room_id: &str, out: &Outbox) -> Option<OpenRoom> {
    let room = RoomId::parse(room_id)
        .ok()
        .and_then(|id| client.get_room(&id));
    let Some(room) = room else {
        out.send(Event::Error(
            "That room isn't available on this account.".into(),
        ));
        return None;
    };

    // An invite has no history to show until it is accepted.
    if room.state() == matrix_sdk::RoomState::Invited {
        return Some(OpenRoom {
            room_id: room_id.to_owned(),
            room,
            timeline: None,
            forwarder: None,
        });
    }

    let timeline = match room.timeline().await {
        Ok(timeline) => Arc::new(timeline),
        Err(e) => {
            out.send(Event::Error(format!("could not open the room: {e}")));
            return None;
        }
    };
    let forwarder = tokio::spawn(forward(timeline.clone(), room_id.to_owned(), out.clone()));
    Some(OpenRoom {
        room_id: room_id.to_owned(),
        room,
        timeline: Some(timeline),
        forwarder: Some(forwarder),
    })
}

/// Send the open room's messages to the window whenever they change, and
/// mark them read, since the user is looking at them.
async fn forward(timeline: Arc<Timeline>, room_id: String, out: Outbox) {
    let (mut items, stream) = timeline.subscribe().await;
    pin_mut!(stream);
    let publish = |items: &Vector<Arc<matrix_sdk_ui::timeline::TimelineItem>>| {
        out.send(Event::Timeline {
            room_id: room_id.clone(),
            rows: timeline::rows(items.iter()),
        });
    };
    publish(&items);

    // If the local cache holds only a few messages, fetch a screenful.
    if items.len() < 30 {
        load_older(timeline.clone(), room_id.clone(), out.clone());
    }
    let _ = timeline.mark_as_read(ReceiptType::Read).await;

    while let Some(diffs) = stream.next().await {
        for diff in diffs {
            diff.apply(&mut items);
        }
        publish(&items);
        let _ = timeline.mark_as_read(ReceiptType::Read).await;
    }
}

fn load_older(timeline: Arc<Timeline>, room_id: String, out: Outbox) {
    tokio::spawn(async move {
        out.send(Event::History {
            room_id: room_id.clone(),
            loading: true,
            reached_start: false,
        });
        let reached_start = match timeline.paginate_backwards(PAGE_OF_HISTORY).await {
            Ok(reached_start) => reached_start,
            Err(e) => {
                out.send(Event::Error(format!("could not load older messages: {e}")));
                false
            }
        };
        out.send(Event::History {
            room_id,
            loading: false,
            reached_start,
        });
    });
}

impl OpenRoom {
    fn load_older(&self, out: &Outbox) {
        if let Some(timeline) = &self.timeline {
            load_older(timeline.clone(), self.room_id.clone(), out.clone());
        }
    }

    fn send(&self, body: String, reply_to: Option<String>, out: &Outbox) {
        let Some(timeline) = self.timeline.clone() else {
            return;
        };
        let out = out.clone();
        tokio::spawn(async move {
            let reply_to = reply_to.and_then(|id| OwnedEventId::try_from(id).ok());
            // The message appears at once as a local echo; the timeline
            // marks it failed if the server never takes it.
            let result = match reply_to {
                Some(event_id) => timeline
                    .send_reply(
                        RoomMessageEventContentWithoutRelation::text_plain(body),
                        event_id,
                    )
                    .await
                    .map(drop),
                None => timeline
                    .send(RoomMessageEventContent::text_plain(body).into())
                    .await
                    .map(drop),
            };
            if let Err(e) = result {
                out.send(Event::Error(format!("could not send: {e}")));
            }
        });
    }

    fn answer_invite(&self, accept: bool, out: &Outbox) {
        let room = self.room.clone();
        let out = out.clone();
        tokio::spawn(async move {
            let result = if accept {
                room.join().await
            } else {
                room.leave().await
            };
            if let Err(e) = result {
                let action = if accept { "join" } else { "decline the invite" };
                out.send(Event::Error(format!("could not {action}: {e}")));
            }
        });
    }
}
