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
const PAGE_SIZE: usize = 500;

async fn build_client(homeserver: &str, db_passphrase: &str) -> Result<Client> {
    Client::builder()
        .server_name_or_homeserver_url(homeserver)
        .sqlite_store(session::store_dir()?, Some(db_passphrase))
        .build()
        .await
        .with_context(|| format!("could not reach the homeserver for {homeserver}"))
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
    let client = build_client(homeserver, &db_passphrase).await?;

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
    let client = build_client(&stored.homeserver, &stored.db_passphrase).await?;
    client
        .restore_session(stored.session)
        .await
        .context("could not resume the saved session")?;
    Ok(Some(client))
}

/// Sign out on the server (best effort) and forget everything stored locally.
pub async fn logout() -> Result<()> {
    if let Some(client) = restore().await?
        && let Err(e) = client.matrix_auth().logout().await
    {
        eprintln!(
            "warning: the server did not confirm sign-out ({e}); forgetting it locally anyway"
        );
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
pub struct RoomRow {
    pub name: String,
    pub is_invite: bool,
    pub unread: u64,
    pub mentions: u64,
}

impl RoomRow {
    fn from_item(item: &RoomListItem) -> Self {
        let name = item
            .cached_display_name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| item.room_id().to_string());
        let counts = item.unread_notification_counts();
        Self {
            name,
            is_invite: item.state() == matrix_sdk::RoomState::Invited,
            unread: counts.notification_count,
            mentions: counts.highlight_count,
        }
    }
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

    Ok(rooms.iter().map(RoomRow::from_item).collect())
}
