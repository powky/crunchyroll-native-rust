//! What the app keeps between launches: its device id and the session's refresh token. Written readable by the
//! app's user only, since the token signs in to the account.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Saved {
    /// Crunchyroll ties a refresh token to the device id it was issued for, so it never changes once made.
    pub device_id: String,
    pub refresh_token: Option<String>,
    /// The profile picked last, to open on it next time.
    pub profile_id: Option<String>,
}

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self { path: dir.as_ref().join("session.json") }
    }

    /// The saved state, or a fresh one with a new device id when there's none (or it's unreadable garbage). A file
    /// that's there but can't be opened is an error: starting over would overwrite the session in it.
    pub fn try_load(&self) -> std::io::Result<Saved> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let saved: Option<Saved> = bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Ok(match saved {
            Some(saved) if !saved.device_id.is_empty() => saved,
            _ => Saved { device_id: uuid::Uuid::new_v4().to_string(), ..Default::default() },
        })
    }

    /// [`Store::try_load`], with an unopenable file treated as no session (nothing is saved over it unless the caller
    /// saves).
    pub fn load(&self) -> Saved {
        self.try_load().unwrap_or_else(|_| Saved { device_id: uuid::Uuid::new_v4().to_string(), ..Default::default() })
    }

    /// Replaces the file atomically (a power cut mid-write leaves the old one).
    pub fn save(&self, saved: &Saved) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&tmp)?;
            file.write_all(&serde_json::to_vec_pretty(saved)?)?;
            file.sync_all()?;
        }
        fs::rename(tmp, &self.path)
    }
}
