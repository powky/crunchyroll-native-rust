//! The signed-in session: restored from disk at launch, created from a device-code sign-in, switched between
//! profiles. Crunchyroll rotates the refresh token on every sign-in, so the new one is saved each time.

use anyhow::{Context, Result};
use crunchyroll_rs::auth::{DeviceIdentifier, SessionToken};
use crunchyroll_rs::profile::Profile;
use crunchyroll_rs::{Crunchyroll, Locale};

use crate::platform::Platform;
use crate::store::{Saved, Store};

pub struct Session {
    pub crunchy: Crunchyroll,
    platform: Platform,
    locale: Locale,
    store: Store,
    saved: Saved,
}

/// How the app introduces itself: as an Android TV, like the credentials crunchyroll-rs signs in with.
pub fn device_identifier(saved: &Saved) -> DeviceIdentifier {
    DeviceIdentifier {
        device_id: saved.device_id.clone(),
        device_type: "ANDROIDTV".to_string(),
        device_name: Some("LG webOS TV".to_string()),
    }
}

impl Session {
    /// The session saved last time, or `None` when there's none (or it no longer works).
    pub async fn restore(store: Store, platform: Platform, locale: Locale) -> Result<Option<Session>> {
        let saved = store.try_load().context("reading the saved session")?;
        let Some(refresh_token) = saved.refresh_token.clone() else {
            return Ok(None);
        };
        let device = device_identifier(&saved);
        let builder = platform.builder().locale(locale.clone());
        let login = match saved.profile_id.clone() {
            Some(profile_id) => builder.login_with_refresh_token_profile_id(refresh_token, profile_id, device).await,
            None => builder.login_with_refresh_token(refresh_token, device).await,
        };
        match login {
            Ok(crunchy) => {
                let mut session = Session { crunchy, platform, locale, store, saved };
                session.persist().await?;
                Ok(Some(session))
            }
            Err(error) => {
                let error = anyhow::Error::from(error);
                if crate::platform::is_inactive_client(&error) {
                    return Err(error.context("the app credentials were switched off"));
                }
                // expired or revoked: sign in again
                Ok(None)
            }
        }
    }

    /// After the code was entered (see [`crate::auth`]).
    pub async fn from_refresh_token(
        store: Store,
        platform: Platform,
        locale: Locale,
        refresh_token: String,
    ) -> Result<Session> {
        let mut saved = store.try_load().context("reading the saved session")?;
        let crunchy = platform
            .builder()
            .locale(locale.clone())
            .login_with_refresh_token(refresh_token, device_identifier(&saved))
            .await
            .context("signing in with the new token")?;
        saved.profile_id = None;
        let mut session = Session { crunchy, platform, locale, store, saved };
        session.persist().await?;
        Ok(session)
    }

    /// The device identity to sign in with, the same one a restored session will use.
    pub fn device_for(store: &Store) -> Result<DeviceIdentifier> {
        let saved = store.try_load().context("reading the saved session")?;
        // keep the device id from the first launch: tokens are bound to it
        store.save(&saved).context("saving the device id")?;
        Ok(device_identifier(&saved))
    }

    pub async fn profiles(&self) -> Result<Vec<Profile>> {
        Ok(self.crunchy.profiles().await?.profiles)
    }

    /// Continues as this profile (its watch history, list and languages).
    pub async fn select_profile(&mut self, profile_id: &str) -> Result<()> {
        let refresh_token = self.refresh_token().await.context("no refresh token")?;
        self.crunchy = self
            .platform
            .builder()
            .locale(self.locale.clone())
            .login_with_refresh_token_profile_id(refresh_token, profile_id.to_string(), device_identifier(&self.saved))
            .await
            .context("switching profile")?;
        self.saved.profile_id = Some(profile_id.to_string());
        self.persist().await
    }

    pub fn profile_id(&self) -> Option<&str> {
        self.saved.profile_id.as_deref()
    }

    pub fn sign_out(self) -> Result<()> {
        let saved = Saved { device_id: self.saved.device_id.clone(), ..Default::default() };
        self.store.save(&saved).context("forgetting the session")
    }

    async fn refresh_token(&self) -> Option<String> {
        match self.crunchy.session_token().await {
            SessionToken::RefreshToken(token) => Some(token),
            _ => None,
        }
    }

    /// Saves the current (rotated) refresh token.
    pub async fn persist(&mut self) -> Result<()> {
        if let Some(token) = self.refresh_token().await {
            self.saved.refresh_token = Some(token);
        }
        self.store.save(&self.saved).context("saving the session")
    }
}
