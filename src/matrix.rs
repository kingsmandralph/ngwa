//! The bridge between Ngwa and matrix-rust-sdk: signing in, resuming a
//! session, and reading the room list through sliding sync.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{StreamExt, pin_mut};
use matrix_sdk::Client;
use matrix_sdk_ui::{
    eyeball_im::Vector,
    room_list_service::{RoomListItem, RoomListLoadingState, filters::new_filter_non_left},
    sync_service::{self, SyncService},
};

use crate::session::{self, StoredSession};

/// How long to wait for the first room list from the server.
const FIRST_LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// Once the list starts arriving, stop when it has been quiet this long.
const SETTLE_TIME: Duration = Duration::from_millis(1500);
/// How many rooms to ask the room list for.
pub(crate) const PAGE_SIZE: usize = 500;

/// Where to find the homeserver when building a client.
#[derive(Clone, Copy)]
enum Server<'a> {
    /// What the user typed: a server name like matrix.org, or a URL.
    /// Resolved through `.well-known` discovery, which needs the network.
    Discover(&'a str),
    /// The exact URL saved from an earlier sign-in. No network needed,
    /// so a saved session opens instantly, even offline.
    Known(&'a str),
}

async fn build_client(server: Server<'_>, db_passphrase: &str) -> Result<Client> {
    let builder = match server {
        Server::Discover(name) => Client::builder().server_name_or_homeserver_url(name),
        Server::Known(url) => Client::builder().homeserver_url(url),
    };
    let name = match server {
        Server::Discover(s) | Server::Known(s) => s,
    };
    builder
        .sqlite_store(session::store_dir()?, Some(db_passphrase))
        .build()
        .await
        .with_context(|| format!("could not reach the homeserver for {name}"))
}

/// Sign in with a password, start a fresh local store, and save the session.
pub async fn login(homeserver: &str, username: &str, password: &str) -> Result<Client> {
    // A new login is a new device: never reuse another device's local store.
    let dir = session::store_dir()?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("could not clear old data in {}", dir.display()))?;
    }

    let db_passphrase = session::new_db_passphrase()?;
    let client = build_client(Server::Discover(homeserver), &db_passphrase).await?;

    client
        .matrix_auth()
        .login_username(username, password)
        .initial_device_display_name("Ngwa")
        .await
        .context("sign-in failed")?;

    let session = client
        .matrix_auth()
        .session()
        .context("signed in, but the server returned no session")?;

    StoredSession {
        homeserver: client.homeserver().to_string(),
        db_passphrase,
        session,
    }
    .save()?;

    Ok(client)
}

/// Resume the saved session, if there is one.
pub async fn restore() -> Result<Option<Client>> {
    let Some(stored) = StoredSession::load()? else {
        return Ok(None);
    };
    let client = build_client(Server::Known(&stored.homeserver), &stored.db_passphrase).await?;
    client
        .restore_session(stored.session)
        .await
        .context("could not resume the saved session")?;
    Ok(Some(client))
}

/// Sign out on the server (best effort) and forget everything stored locally.
///
/// Takes the client by value: every other handle to it (sync service, room
/// list) must already be dropped, so the local database can be deleted.
pub async fn logout(client: Option<Client>) -> Result<()> {
    if let Some(client) = client {
        if let Err(e) = client.matrix_auth().logout().await {
            tracing::warn!(
                "the server did not confirm sign-out ({e}); forgetting it locally anyway"
            );
        }
        drop(client);
    }
    StoredSession::delete()?;
    let dir = session::store_dir()?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("could not delete {}", dir.display()))?;
    }
    Ok(())
}

/// One row of the room list, ready to display.
#[derive(Clone, Debug)]
pub struct RoomRow {
    pub id: String,
    pub name: String,
    pub is_invite: bool,
    pub unread: u64,
    pub mentions: u64,
    /// "Ada: see you tomorrow", from the room's latest message.
    pub preview: Option<String>,
    /// When the latest message was sent, in milliseconds since the epoch.
    pub timestamp: Option<i64>,
}

impl RoomRow {
    pub(crate) async fn from_item(item: &RoomListItem) -> Self {
        let name = item
            .cached_display_name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| item.room_id().to_string());
        let counts = item.unread_notification_counts();
        let (preview, timestamp) = latest_message(item).await;
        Self {
            id: item.room_id().to_string(),
            name,
            is_invite: item.state() == matrix_sdk::RoomState::Invited,
            unread: counts.notification_count,
            mentions: counts.highlight_count,
            preview,
            timestamp,
        }
    }

    /// Rows for a whole room list, in order.
    pub(crate) async fn from_items(items: &Vector<RoomListItem>) -> Vec<Self> {
        let mut rows = Vec::with_capacity(items.len());
        for item in items {
            rows.push(Self::from_item(item).await);
        }
        rows
    }
}

async fn latest_message(item: &RoomListItem) -> (Option<String>, Option<i64>) {
    use matrix_sdk_ui::timeline::{LatestEventValue, RoomExt, TimelineDetails};

    let room: &matrix_sdk::Room = item;
    let (timestamp, is_own, profile, sender, content) = match room.latest_event().await {
        LatestEventValue::Remote {
            timestamp,
            is_own,
            profile,
            sender,
            content,
        } => (timestamp, is_own, profile, sender, content),
        // Local events are ones this device is sending, so they are ours.
        LatestEventValue::Local {
            timestamp,
            profile,
            sender,
            content,
            ..
        } => (timestamp, true, profile, sender, content),
        LatestEventValue::RemoteInvite { timestamp, .. } => {
            return (Some("Invited you".into()), Some(millis(timestamp)));
        }
        LatestEventValue::None => return (None, None),
    };
    let Some(text) = crate::timeline::preview(&content) else {
        return (None, Some(millis(timestamp)));
    };
    let who = if is_own {
        "You".to_owned()
    } else {
        match profile {
            TimelineDetails::Ready(p) => p
                .display_name
                .unwrap_or_else(|| sender.localpart().to_owned()),
            _ => sender.localpart().to_owned(),
        }
    };
    (Some(format!("{who}: {text}")), Some(millis(timestamp)))
}

pub(crate) fn millis(ts: matrix_sdk::ruma::MilliSecondsSinceUnixEpoch) -> i64 {
    i64::try_from(u64::from(ts.0)).unwrap_or(i64::MAX)
}

/// Turn what the user typed into a username and homeserver to sign in with.
///
/// A full Matrix ID (`@you:example.org`) names its own server and
/// `homeserver` is ignored; otherwise an empty `homeserver` means matrix.org.
pub fn resolve_login(id: &str, homeserver: &str) -> Result<(String, String)> {
    let id = id.trim();
    if id.is_empty() {
        bail!("enter your Matrix ID or username");
    }
    if let Some((_, server)) = split_matrix_id(id) {
        return Ok((id.to_owned(), server.to_owned()));
    }
    let homeserver = match homeserver.trim() {
        "" => "matrix.org",
        other => other,
    };
    if !looks_like_server(homeserver) {
        bail!(
            "`{homeserver}` doesn't look like a homeserver address. \
             Leave it empty for matrix.org, or use one like example.org."
        );
    }
    Ok((id.trim_start_matches('@').to_owned(), homeserver.to_owned()))
}

/// Split `@user:server` into its parts; `None` for a bare username.
pub fn split_matrix_id(id: &str) -> Option<(&str, &str)> {
    let (local, server) = id.strip_prefix('@')?.split_once(':')?;
    (!local.is_empty() && !server.is_empty()).then_some((local, server))
}

/// Catch an obvious mistake (such as a username typed where the server goes)
/// before trying to reach it over the network.
fn looks_like_server(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("localhost")
        || (s.contains('.') && !s.contains(char::is_whitespace))
}

/// Build the sync service the desktop app runs for as long as it is open.
/// Offline mode makes it wait out network drops instead of stopping.
pub async fn sync_service(client: &Client) -> Result<SyncService> {
    SyncService::builder(client.clone())
        .with_offline_mode()
        .build()
        .await
        .context("could not start syncing (does this homeserver support sliding sync?)")
}

/// Sync with sliding sync until the room list has loaded and settled, then
/// return it in the same order Element X and other modern clients use.
pub async fn fetch_room_list(client: &Client) -> Result<Vec<RoomRow>> {
    let sync = SyncService::builder(client.clone())
        .build()
        .await
        .context("could not start syncing (does this homeserver support sliding sync?)")?;
    sync.start().await;

    let result = read_room_list(&sync).await;
    sync.stop().await;
    result
}

async fn read_room_list(sync: &SyncService) -> Result<Vec<RoomRow>> {
    let room_list = sync.room_list_service().all_rooms().await?;

    // Wait for the first sliding-sync response, failing fast on sync errors.
    let mut loading = room_list.loading_state();
    let mut sync_state = sync.state();
    let wait = async {
        loop {
            if matches!(loading.get(), RoomListLoadingState::Loaded { .. }) {
                return Ok(());
            }
            tokio::select! {
                _ = loading.next() => {}
                Some(state) = sync_state.next() => {
                    if let sync_service::State::Error(e) = state {
                        bail!("sync failed: {e}");
                    }
                }
            }
        }
    };
    tokio::time::timeout(FIRST_LOAD_TIMEOUT, wait)
        .await
        .context("timed out waiting for the room list")??;

    // Read the sorted, filtered list and keep applying updates until it settles.
    let (stream, controller) = room_list.entries_with_dynamic_adapters(PAGE_SIZE);
    controller.set_filter(Box::new(new_filter_non_left()));
    pin_mut!(stream);

    let mut rooms: Vector<RoomListItem> = Vector::new();
    while let Ok(Some(diffs)) = tokio::time::timeout(SETTLE_TIME, stream.next()).await {
        for diff in diffs {
            diff.apply(&mut rooms);
        }
    }

    Ok(RoomRow::from_items(&rooms).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_matrix_id_names_its_server() {
        assert_eq!(
            split_matrix_id("@ghalahad:matrix.org"),
            Some(("ghalahad", "matrix.org"))
        );
        assert_eq!(split_matrix_id("ghalahad"), None);
        assert_eq!(split_matrix_id("@ghalahad"), None);
        assert_eq!(split_matrix_id("@:matrix.org"), None);
    }

    #[test]
    fn usernames_are_not_servers() {
        assert!(looks_like_server("matrix.org"));
        assert!(looks_like_server("https://matrix.example.com"));
        assert!(looks_like_server("localhost:8008"));
        assert!(!looks_like_server("ghalahad"));
        assert!(!looks_like_server("my server.org"));
    }

    #[test]
    fn resolve_login_picks_the_server() {
        let r = |id, hs| resolve_login(id, hs).map_err(|e| e.to_string());
        assert_eq!(
            r("@a:example.org", "ignored.org"),
            Ok(("@a:example.org".into(), "example.org".into()))
        );
        assert_eq!(r("a", ""), Ok(("a".into(), "matrix.org".into())));
        assert_eq!(
            r("@a", "example.org"),
            Ok(("a".into(), "example.org".into()))
        );
        assert!(r("a", "ghalahad").is_err());
        assert!(r("  ", "").is_err());
    }
}
