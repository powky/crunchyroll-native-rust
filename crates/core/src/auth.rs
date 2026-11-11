//! Signing in the way TV apps do: the screen shows a short code, the user enters it at crunchyroll.com/activate on
//! a phone, and the app, asking every few seconds, gets a refresh token once that's done. No password is typed on
//! the TV. crunchyroll-rs has no such flow, so it's two plain requests with the Android TV app credentials the
//! session is then issued with ([`Platform`]).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use crunchyroll_rs::auth::DeviceIdentifier;
use serde::Deserialize;

use crate::platform::Platform;

const BASE: &str = "https://www.crunchyroll.com";

/// Where the user enters the code.
pub const ACTIVATE_URL: &str = "crunchyroll.com/activate";

#[derive(Clone, Debug)]
pub struct DeviceCode {
    /// What the screen shows, e.g. `ABC123`.
    pub user_code: String,
    device_code: String,
    pub interval: Duration,
    pub expires_in: Duration,
}

#[derive(Debug)]
pub enum Poll {
    /// Not entered yet: ask again after [`DeviceCode::interval`].
    Pending,
    /// Entered: sign in with this refresh token (and the same device identifier).
    Authorized { refresh_token: String },
    /// The code ran out: ask for a new one.
    Expired,
}

fn basic_auth(platform: &Platform) -> String {
    format!("Basic {}", platform.basic_auth_token)
}

#[derive(Deserialize)]
struct CodeResponse {
    device_code: String,
    user_code: String,
    #[serde(default = "default_interval")]
    interval: u64,
    #[serde(default = "default_expires")]
    expires_in: u64,
}

fn default_interval() -> u64 {
    5000
}

fn default_expires() -> u64 {
    300
}

/// A new code to show.
pub async fn request_code(client: &reqwest::Client, platform: &Platform) -> Result<DeviceCode> {
    let response = client
        .post(format!("{BASE}/auth/v1/device/code"))
        .header("Authorization", basic_auth(platform))
        .form(&[("grant_type", "client_id")])
        .send()
        .await
        .context("asking for a device code")?;
    let status = response.status();
    if !status.is_success() {
        bail!("device code: {status} {}", response.text().await.unwrap_or_default());
    }
    let code: CodeResponse = response.json().await.context("reading the device code")?;
    Ok(DeviceCode {
        user_code: code.user_code,
        device_code: code.device_code,
        // milliseconds (the web app hands it straight to setInterval)
        interval: Duration::from_millis(code.interval).max(Duration::from_secs(2)),
        expires_in: Duration::from_secs(code.expires_in),
    })
}

#[derive(Deserialize)]
struct TokenResponse {
    refresh_token: Option<String>,
}

/// Whether the code has been entered yet.
pub async fn poll(
    client: &reqwest::Client,
    platform: &Platform,
    device: &DeviceIdentifier,
    code: &DeviceCode,
) -> Result<Poll> {
    let response = client
        .post(format!("{BASE}/auth/v1/device/token"))
        .header("Authorization", basic_auth(platform))
        .json(&serde_json::json!({
            "device_id": device.device_id,
            "device_name": device.device_name,
            "device_type": device.device_type,
            "device_code": code.device_code,
        }))
        .send()
        .await
        .context("polling the device code")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.is_success() {
        // an empty answer while the code hasn't been entered
        if body.trim().is_empty() {
            return Ok(Poll::Pending);
        }
        let token: TokenResponse = serde_json::from_str(&body).context("reading the device token")?;
        return match token.refresh_token {
            Some(refresh_token) => Ok(Poll::Authorized { refresh_token }),
            None => Ok(Poll::Pending),
        };
    }
    if body.contains("expired") || status == reqwest::StatusCode::GONE {
        return Ok(Poll::Expired);
    }
    // 4xx while the code hasn't been entered (authorization_pending and the like)
    if status.is_client_error() {
        return Ok(Poll::Pending);
    }
    bail!("device token: {status} {body}")
}
