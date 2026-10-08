//! The engine behind the window.
//!
//! All Matrix work runs on a tokio runtime on background threads. The window
//! sends it [`Command`]s and gets [`Event`]s back, and never waits on the
//! network itself, so it never freezes.

use std::sync::mpsc;
use std::time::Duration;

use eframe::egui;
use futures_util::{StreamExt, pin_mut};
use matrix_sdk::Client;
use matrix_sdk_ui::{
    eyeball_im::Vector,
    room_list_service::{RoomListItem, RoomListLoadingState, filters::new_filter_non_left},
    sync_service::State,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::matrix::{self, PAGE_SIZE, RoomRow};

/// What the window asks the engine to do.
pub enum Command {
    Login {
        id: String,
        homeserver: String,
        password: String,
    },
    Logout,
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
        match commands.recv().await? {
            Command::Login {
                id,
                homeserver,
                password,
            } => {
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
            Command::Logout => {}
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
    let exit = loop {
        tokio::select! {
            Some(diffs) = entries.next() => {
                for diff in diffs {
                    diff.apply(&mut rooms);
                }
                out.send(Event::Rooms(rooms.iter().map(RoomRow::from_item).collect()));
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
                Some(Command::Login { .. }) => {}
                None => break Exit::Closed,
            },
        }
    };

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
            Some(Command::Login { .. }) => {}
            None => return Exit::Closed,
        }
    }
}
