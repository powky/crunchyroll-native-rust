//! The app credentials the session is issued with. Crunchyroll rotates them and turns the old ones off
//! (`client_inactive`), so instead of the ones bundled in crunchyroll-rs the app uses crunchy-labs' published
//! current set, kept on disk for a day and fetched again when Crunchyroll refuses them.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use crunchyroll_rs::auth::platform_credentials::{PlatformCredentials, get_platform_credentials};
use crunchyroll_rs::auth::{CrunchyrollBuilder, DevicePlatform};

const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The Android TV app's credentials and matching user agent.
#[derive(Clone, Debug)]
pub struct Platform {
    pub basic_auth_token: String,
    pub user_agent: String,
}

impl Platform {
    fn from_credentials(credentials: &PlatformCredentials) -> Platform {
        Platform {
            basic_auth_token: credentials.android_tv.basic_auth_token.clone(),
            user_agent: credentials.android_tv_user_agent(),
        }
    }

    /// The cached set if it's recent, else a fresh one (falling back to a stale cache when offline).
    pub async fn load(dir: impl AsRef<Path>, force_refresh: bool) -> Result<Platform> {
        let path = cache_path(dir.as_ref());
        let cached: Option<PlatformCredentials> =
            std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let fresh = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age < MAX_AGE);
        if let (Some(credentials), true, false) = (&cached, fresh, force_refresh) {
            return Ok(Platform::from_credentials(credentials));
        }
        match get_platform_credentials().await {
            Ok(credentials) => {
                if let Ok(bytes) = serde_json::to_vec(&credentials) {
                    let _ = std::fs::create_dir_all(dir.as_ref());
                    let _ = std::fs::write(&path, bytes);
                }
                Ok(Platform::from_credentials(&credentials))
            }
            Err(error) => cached
                .map(|credentials| Platform::from_credentials(&credentials))
                .ok_or(error)
                .context("fetching the current app credentials"),
        }
    }

    /// A session builder issuing Android TV sessions with these credentials.
    pub fn builder(&self) -> CrunchyrollBuilder {
        CrunchyrollBuilder::default().platform(
            DevicePlatform::TvAndroid,
            self.basic_auth_token.clone(),
            Some(self.user_agent.clone()),
        )
    }

    /// An http client with this platform's user agent, for the requests crunchyroll-rs doesn't make.
    pub fn http_client(&self) -> Result<reqwest::Client> {
        CrunchyrollBuilder::predefined_client_builder()
            .user_agent(&self.user_agent)
            .build()
            .context("building the http client")
    }
}

fn cache_path(dir: &Path) -> PathBuf {
    dir.join("platform.json")
}

/// Whether a sign-in failed because the app credentials were switched off (fetch them again and retry).
pub fn is_inactive_client(error: &anyhow::Error) -> bool {
    let text = format!("{error:#}");
    text.contains("client_inactive") || text.contains("invalid_client")
}
