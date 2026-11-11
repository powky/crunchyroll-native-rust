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

    /// The saved state, or a fresh one with a new device id.
    pub fn load(&self) -> Saved {
        let saved: Option<Saved> = fs::read(&self.path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match saved {
            Some(saved) if !saved.device_id.is_empty() => saved,
            _ => Saved { device_id: uuid::Uuid::new_v4().to_string(), ..Default::default() },
        }
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
