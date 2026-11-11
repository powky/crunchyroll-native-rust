//! The account's profiles and what can change about them: name, avatar, the three languages, adding and deleting
//! one. Also the language lists Crunchyroll offers for audio and subtitles.

use anyhow::Result;
use crunchyroll_rs::Crunchyroll;
use serde_json::Value;

use crate::api::{self, data, str};

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// avatar file name ("gear5avatar6.png")
    pub avatar: String,
    pub username: String,
    pub is_primary: bool,
    /// titles and descriptions ("es-419")
    pub content_language: String,
    pub audio_language: String,
    /// "off" when none
    pub subtitle_language: String,
}

impl Profile {
    pub fn avatar_url(&self) -> Option<String> {
        avatar_url(&self.avatar)
    }
}

pub fn avatar_url(avatar: &str) -> Option<String> {
    (!avatar.is_empty()).then(|| format!("https://static.crunchyroll.com/assets/avatar/510x510/{avatar}"))
}

#[derive(Clone, Debug, Default)]
pub struct Profiles {
    pub list: Vec<Profile>,
    /// whether another profile fits the plan
    pub can_add: bool,
}

pub async fn profiles(crunchy: &Crunchyroll) -> Result<Profiles> {
    let answer = api::get(crunchy, "/accounts/v1/me/multiprofile", &[]).await?;
    let list: Vec<Profile> = answer
        .get("profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|profile| Profile {
            id: str(profile, "profile_id").to_string(),
            name: str(profile, "profile_name").to_string(),
            avatar: str(profile, "avatar").to_string(),
            username: str(profile, "username").to_string(),
            is_primary: profile.get("is_primary").and_then(Value::as_bool).unwrap_or(false),
            content_language: str(profile, "preferred_communication_language").to_string(),
            audio_language: str(profile, "preferred_content_audio_language").to_string(),
            subtitle_language: Some(str(profile, "preferred_content_subtitle_language"))
                .filter(|s| !s.is_empty())
                .unwrap_or("off")
                .to_string(),
        })
        .collect();
    let max = answer
        .get("tier_max_profiles")
        .or(answer.get("max_profiles"))
        .and_then(Value::as_u64)
        .unwrap_or(list.len() as u64) as usize;
    Ok(Profiles { can_add: list.len() < max, list })
}

/// Changes some of a profile's fields (`profile_name`, `avatar`, `preferred_communication_language`,
/// `preferred_content_audio_language`, `preferred_content_subtitle_language`, …).
pub async fn update_profile(crunchy: &Crunchyroll, id: &str, fields: Value) -> Result<()> {
    api::patch(crunchy, &format!("/accounts/v1/me/multiprofile/{id}"), &[], fields).await?;
    Ok(())
}

/// A new profile with these fields (`profile_name`, `avatar`, `username`, the languages).
pub async fn create_profile(crunchy: &Crunchyroll, fields: Value) -> Result<()> {
    api::post(crunchy, "/accounts/v1/me/multiprofile", &[], fields).await?;
    Ok(())
}

pub async fn delete_profile(crunchy: &Crunchyroll, id: &str) -> Result<()> {
    api::delete(crunchy, &format!("/accounts/v1/me/multiprofile/{id}"), &[]).await?;
    Ok(())
}

/// Usernames the account can give a new profile.
pub async fn usernames(crunchy: &Crunchyroll) -> Result<Vec<String>> {
    let answer = api::get(crunchy, "/accounts/v1/usernames", &[]).await?;
    let list = answer.get("usernames").or(answer.get("data")).and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(list.iter().filter_map(|name| name.as_str().map(String::from)).collect())
}

#[derive(Clone, Debug, Default)]
pub struct Album {
    pub title: String,
    /// avatar file names
    pub avatars: Vec<String>,
}

pub async fn avatars(crunchy: &Crunchyroll, language: &str) -> Result<Vec<Album>> {
    let answer = api::get(crunchy, &format!("/assets/v2/{language}/avatar"), &[]).await?;
    Ok(data(&answer)
        .iter()
        .map(|album| Album {
            title: str(album, "title").to_string(),
            avatars: album
                .get("assets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|asset| str(asset, "id").to_string())
                .filter(|id| !id.is_empty())
                .collect(),
        })
        .filter(|album| !album.avatars.is_empty())
        .collect())
}

/// Languages Crunchyroll has audio or subtitles in: (code, name in that language), in Crunchyroll's order.
pub async fn languages(http: &reqwest::Client, subtitles: bool) -> Result<Vec<(String, String)>> {
    let kind = if subtitles { "timed_text_languages" } else { "audio_languages" };
    let text = http
        .get(format!("https://static.crunchyroll.com/config/i18n/v3/{kind}.json"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    // keep the file's order: serde_json's map would sort the keys
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_end_matches(',');
        if let Some((code, name)) = line.split_once(':') {
            let code = code.trim().trim_matches('"');
            let name = name.trim().trim_matches('"');
            if !code.is_empty() && !name.is_empty() {
                out.push((code.to_string(), name.to_string()));
            }
        }
    }
    Ok(out)
}
