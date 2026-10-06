//! Where Ngwa keeps a signed-in account between runs.
//!
//! Secrets (access token, device ID, and the passphrase that encrypts the
//! local database) live in the OS credential store: Keychain on macOS,
//! Credential Manager on Windows, Secret Service on Linux. The encrypted
//! local database itself lives in the platform data directory.
//!
//! Some Linux setups have no credential store at all (WSL, servers, minimal
//! window managers). There, the session falls back to a file in the config
//! directory that only the current user can read.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use matrix_sdk::authentication::matrix::MatrixSession;
use serde::{Deserialize, Serialize};

const KEYRING_SERVICE: &str = "chat.ngwa.Ngwa";
const KEYRING_ACCOUNT: &str = "session";
const SESSION_FILE: &str = "session.json";

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

/// Where the session is kept on this machine.
enum Backend {
    Keyring(keyring::Entry),
    File(PathBuf),
}

impl Backend {
    fn detect() -> Result<Self> {
        match keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT) {
            Ok(entry) => Ok(Self::Keyring(entry)),
            Err(_) => Ok(Self::File(session_file()?)),
        }
    }
}

impl StoredSession {
    pub fn load() -> Result<Option<Self>> {
        let json = match Backend::detect()? {
            Backend::Keyring(entry) => match entry.get_password() {
                Ok(json) => json,
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(e) => return Err(anyhow!(e).context("could not read the saved session")),
            },
            Backend::File(path) => match std::fs::read_to_string(&path) {
                Ok(json) => json,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => {
                    return Err(anyhow!(e))
                        .with_context(|| format!("could not read {}", path.display()));
                }
            },
        };
        Ok(Some(
            serde_json::from_str(&json).context("saved session is corrupt")?,
        ))
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_string(self)?;
        match Backend::detect()? {
            Backend::Keyring(entry) => entry
                .set_password(&json)
                .context("could not save the session to the credential store"),
            Backend::File(path) => {
                eprintln!(
                    "note: no system credential store found (common on WSL and minimal \
                     Linux setups), so the session is saved in {}, readable only by you.",
                    path.display()
                );
                write_private(&path, &json)
                    .with_context(|| format!("could not save the session to {}", path.display()))
            }
        }
    }

    pub fn delete() -> Result<()> {
        match Backend::detect()? {
            Backend::Keyring(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(anyhow!(e).context("could not remove the saved session")),
            },
            Backend::File(path) => match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => {
                    Err(anyhow!(e)).with_context(|| format!("could not delete {}", path.display()))
                }
            },
        }
    }
}

/// Write a file that only the current user can read or write.
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        // Tighten an existing file too: `mode` only applies on creation.
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    options.open(path)?.write_all(contents.as_bytes())
}

/// A fresh random passphrase for a new local database.
pub fn new_db_passphrase() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("no system randomness: {e}"))?;
    Ok(hex::encode(bytes))
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("chat", "ngwa", "Ngwa")
        .context("could not determine a data directory for this user")
}

/// Directory for the encrypted local database.
pub fn store_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("store"))
}

/// Fallback session file, used only when there is no credential store.
fn session_file() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join(SESSION_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_file_round_trips_and_is_owner_only() {
        let dir = std::env::temp_dir().join(format!("ngwa-test-{}", std::process::id()));
        let path = dir.join("nested").join(SESSION_FILE);
        write_private(&path, "first").unwrap();
        write_private(&path, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stored_session_serializes() {
        let json = r#"{"homeserver":"https://matrix.org/","db_passphrase":"x",
            "session":{"user_id":"@a:matrix.org","device_id":"DEV","access_token":"t"}}"#;
        let s: StoredSession = serde_json::from_str(json).unwrap();
        assert_eq!(s.session.meta.user_id.as_str(), "@a:matrix.org");
        let back = serde_json::to_string(&s).unwrap();
        assert!(back.contains("\"access_token\":\"t\""));
    }
}
