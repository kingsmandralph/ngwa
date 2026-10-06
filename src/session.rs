//! Where Ngwa keeps a signed-in account between runs.
//!
//! Secrets (access token, device ID, and the passphrase that encrypts the
//! local database) live in the OS credential store: Keychain on macOS,
//! Credential Manager on Windows, Secret Service on Linux. The encrypted
//! local database itself lives in the platform data directory.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use matrix_sdk::authentication::matrix::MatrixSession;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "chat.ngwa.Ngwa";
const KEYRING_ACCOUNT: &str = "session";

/// Everything needed to resume a session without signing in again.
#[derive(Serialize, Deserialize)]
pub struct StoredSession {
    /// The homeserver URL the session belongs to.
    pub homeserver: String,
    /// Passphrase for the local SQLite store (state, crypto, event cache).
    pub db_passphrase: String,
    /// Matrix user ID, device ID and access token.
    pub session: MatrixSession,
}

fn entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).context(if cfg!(target_os = "linux") {
        "the system credential store is not available \
             (is a Secret Service such as GNOME Keyring or KWallet running?)"
    } else {
        "the system credential store is not available"
    })
}

impl StoredSession {
    pub fn load() -> Result<Option<Self>> {
        match entry()?.get_password() {
            Ok(json) => Ok(Some(
                serde_json::from_str(&json).context("saved session is corrupt")?,
            )),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow!(e).context("could not read the saved session")),
        }
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string(self)?;
        entry()?
            .set_password(&json)
            .context("could not save the session to the credential store")
    }

    pub fn delete() -> Result<()> {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(anyhow!(e).context("could not remove the saved session")),
        }
    }
}

/// A fresh random passphrase for a new local database.
pub fn new_db_passphrase() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no system randomness: {e}"))?;
    Ok(hex::encode(bytes))
}

/// Directory for the encrypted local database.
pub fn store_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("chat", "ngwa", "Ngwa")
        .context("could not determine a data directory for this user")?;
    Ok(dirs.data_dir().join("store"))
}
