//! The catalog the screens browse, read as JSON from the endpoints the web app uses: a show's page (seasons,
//! episodes with progress, what to watch next, rating, watchlist), catalog grids, search, the user's lists and the
//! simulcast seasons.

use std::collections::HashMap;

use anyhow::{Context, Result};
use crunchyroll_rs::Crunchyroll;
use futures_util::future::join_all;
use serde_json::{Value, json};

use crate::api::{self, data, str};
use crate::images;
use crate::model::{Card, Kind};

/// Ids the API takes in one list.
const BATCH: usize = 40;

/// What every request needs: the session, the account the lists hang from and the profile's languages.
#[derive(Clone)]
pub struct Ctx {
    pub crunchy: Crunchyroll,
    pub account: String,
    /// for titles and descriptions ("es-419")
    pub locale: String,
    pub premium: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Show {
    pub card: Card,
    /// The show's title logo, transparent, when Crunchyroll has one in this language.
    pub logo: Option<String>,
    /// Art made for a hero (room for text on the left), else the wide poster.
    pub hero: Option<String>,
    /// The same art, small, to show while the big one loads.
    pub preview: Option<String>,
    pub seasons: Vec<SeasonInfo>,
    pub in_watchlist: bool,
    /// The user's own stars.
    pub my_rating: Option<u8>,
    /// What "play" starts: the next episode (with its progress), or the first.
    pub up_next: Option<Card>,
    /// languages it can be heard and read in
    pub audio_locales: Vec<String>,
    pub subtitle_locales: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct SeasonInfo {
    pub id: String,
    pub number: u32,
    /// "1", "2", sometimes text: what the season's pill shows after "Temporada"
    pub display_number: String,
    pub title: String,
    pub episode_count: u32,
}

/// One page of a grid.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub cards: Vec<Card>,
    pub total: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    #[default]
    Popularity,
    NewlyAdded,
    Alphabetical,
}

impl Sort {
    fn param(self) -> &'static str {
        match self {
            Sort::Popularity => "popularity",
            Sort::NewlyAdded => "newly_added",
            Sort::Alphabetical => "alphabetical",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audio {
    #[default]
    All,
    Subbed,
    Dubbed,
}

/// A catalog query, as the grids ask it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Browse {
    /// "series", "movie_listing", or both when empty
    pub media: Option<&'static str>,
    pub sort: Sort,
    pub audio: Audio,
    pub category: Option<String>,
    pub seasonal_tag: Option<String>,
    /// what was typed in the search field (browse matches titles with it)
    pub query: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MusicKind {
    Videos,
    Concerts,
    Artists,
}

#[derive(Clone, Debug)]
pub struct MusicRow {
    pub title: String,
    pub kind: MusicKind,
    pub ids: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Named {
    pub id: String,
    pub title: String,
    pub count: Option<u32>,
}

impl Ctx {
    pub async fn new(crunchy: Crunchyroll, locale: String) -> Result<Ctx> {
        let me = api::get(&crunchy, "/accounts/v1/me", &[]).await.context("reading the account")?;
        let account = str(&me, "account_id").to_string();
        let premium = crunchy.premium().await;
        Ok(Ctx { crunchy, account, locale, premium })
    }

    fn q(&self, extra: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
        let mut query = vec![("locale", self.locale.clone())];
        query.extend(extra.iter().cloned());
        query
    }

    async fn get(&self, path: &str, extra: &[(&'static str, String)]) -> Result<Value> {
        api::get(&self.crunchy, path, &self.q(extra)).await
    }

    /// Full objects (with ratings and keyart) for any ids, in their order; ids it doesn't know are left out.
    pub async fn objects(&self, ids: &[String]) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let answer = self.get(&format!("/content/v2/cms/objects/{}", chunk.join(",")), &[("ratings", "true".into())]).await?;
            out.extend(data(&answer).iter().cloned());
        }
        Ok(out)
    }

    /// What the hero needs of these shows (logo, art, rating, counts), trimmed for keeping, by id.
    pub async fn hero_objects(&self, ids: &[String]) -> Result<Vec<Value>> {
        Ok(self.objects(ids).await?.iter().map(Hero::trim).collect())
    }

    /// A series' or movie listing's page.
    pub async fn show(&self, id: &str) -> Result<Show> {
        let objects = self.objects(&[id.to_string()]).await?;
        let object = objects.first().context("Crunchyroll doesn't know this show")?;
        let hero = Hero::from_object(object).context("not a show")?;
        let kind = hero.card.kind;
        let meta = object.get("series_metadata").or(object.get("movie_listing_metadata"));
        let locales = |key: &str| -> Vec<String> {
            meta.and_then(|m| m.get(key)).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
        };
        let (audio_locales, subtitle_locales) = (locales("audio_locales"), locales("subtitle_locales"));
        let (seasons, in_watchlist, my_rating, up_next) = futures_util::join!(
            async {
                if kind == Kind::Series { self.seasons(id).await } else { Ok(Vec::new()) }
            },
            self.in_watchlist(id),
            self.my_rating(id, kind),
            self.up_next(id),
        );
        Ok(Show {
            card: hero.card,
            logo: hero.logo,
            hero: hero.hero,
            preview: hero.preview,
            seasons: seasons?,
            in_watchlist: in_watchlist.unwrap_or(false),
            my_rating: my_rating.unwrap_or(None),
            up_next: up_next.unwrap_or(None),
            audio_locales,
            subtitle_locales,
        })
    }

    pub async fn seasons(&self, series_id: &str) -> Result<Vec<SeasonInfo>> {
        let answer = self.get(&format!("/content/v2/cms/series/{series_id}/seasons"), &[]).await?;
        let mut seasons: Vec<(f64, SeasonInfo)> = data(&answer)
            .iter()
            .map(|season| {
                let number = season.get("season_number").and_then(Value::as_u64).unwrap_or(0) as u32;
                let display = str(season, "season_display_number");
                (
                    season.get("season_sequence_number").and_then(Value::as_f64).unwrap_or(number as f64),
                    SeasonInfo {
                        id: str(season, "id").to_string(),
                        number,
                        display_number: if display.is_empty() { number.to_string() } else { display.to_string() },
                        title: str(season, "title").to_string(),
                        episode_count: season.get("number_of_episodes").and_then(Value::as_u64).unwrap_or(0) as u32,
                    },
                )
            })
            .collect();
        seasons.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(seasons.into_iter().map(|(_, season)| season).collect())
    }

    /// A season's episodes in order, with how far each was watched.
    pub async fn episodes(&self, season_id: &str) -> Result<Vec<Card>> {
        let answer = self.get(&format!("/content/v2/cms/seasons/{season_id}/episodes"), &[]).await?;
        let mut cards: Vec<Card> = data(&answer).iter().filter_map(Card::from_json).collect();
        self.add_playheads(&mut cards).await;
        Ok(cards)
    }

    /// A movie listing's movies, with progress.
    pub async fn movies(&self, listing_id: &str) -> Result<Vec<Card>> {
        let answer = self.get(&format!("/content/v2/cms/movie_listings/{listing_id}/movies"), &[]).await?;
        let mut cards: Vec<Card> = data(&answer).iter().filter_map(Card::from_json).collect();
        self.add_playheads(&mut cards).await;
        Ok(cards)
    }

    async fn add_playheads(&self, cards: &mut [Card]) {
        let ids: Vec<String> = cards.iter().map(|card| card.id.clone()).collect();
        if let Ok(playheads) = self.playheads(&ids).await {
            for card in cards.iter_mut() {
                if let Some((playhead, watched)) = playheads.get(&card.id) {
                    *card = std::mem::take(card).with_playhead(*playhead, *watched);
                }
            }
        }
    }

    /// Seconds watched and whether it was seen to the end, by content id.
    pub async fn playheads(&self, ids: &[String]) -> Result<HashMap<String, (u32, bool)>> {
        let mut out = HashMap::new();
        let path = format!("/content/v2/{}/playheads", self.account);
        let path = &path;
        let answers = join_all(ids.chunks(BATCH).map(|chunk| async move {
            self.get(path, &[("content_ids", chunk.join(","))]).await
        }))
        .await;
        for answer in answers {
            for item in data(&answer?) {
                let playhead = item.get("playhead").and_then(Value::as_u64).unwrap_or(0) as u32;
                let watched = item.get("fully_watched").and_then(Value::as_bool).unwrap_or(false);
                out.insert(str(item, "content_id").to_string(), (playhead, watched));
            }
        }
        Ok(out)
    }

    /// The episode "play" should start for a series (or the movie of a listing), with its progress.
    pub async fn up_next(&self, show_id: &str) -> Result<Option<Card>> {
        let answer = self.get(&format!("/content/v2/discover/up_next/{show_id}"), &[]).await?;
        Ok(data(&answer).first().and_then(|item| {
            let card = Card::from_json(item.get("panel")?)?;
            let playhead = item.get("playhead").and_then(Value::as_u64).unwrap_or(0) as u32;
            let watched = item.get("fully_watched").and_then(Value::as_bool).unwrap_or(false);
            Some(card.with_playhead(playhead, watched))
        }))
    }

    pub async fn in_watchlist(&self, id: &str) -> Result<bool> {
        let answer = self.get(&format!("/content/v2/{}/watchlist", self.account), &[("content_ids", id.to_string())]).await?;
        Ok(data(&answer).iter().any(|item| str(item, "id") == id))
    }

    pub async fn set_watchlist(&self, id: &str, on: bool) -> Result<()> {
        if on {
            api::post(&self.crunchy, &format!("/content/v2/{}/watchlist", self.account), &self.q(&[]), json!({ "content_id": id }))
                .await?;
        } else {
            api::delete(&self.crunchy, &format!("/content/v2/{}/watchlist/{id}", self.account), &self.q(&[])).await?;
        }
        Ok(())
    }

    fn rating_path(&self, id: &str, kind: Kind) -> String {
        let kind = if kind == Kind::MovieListing { "movie_listing" } else { "series" };
        format!("/content-reviews/v2/user/{}/rating/{kind}/{id}", self.account)
    }

    pub async fn my_rating(&self, id: &str, kind: Kind) -> Result<Option<u8>> {
        let answer = api::get(&self.crunchy, &self.rating_path(id, kind), &[]).await?;
        Ok(str(&answer, "rating").trim_end_matches('s').parse().ok().filter(|stars: &u8| (1..=5).contains(stars)))
    }

    /// Rates a show 1..=5 stars, or takes the rating back with `None`.
    pub async fn rate(&self, id: &str, kind: Kind, stars: Option<u8>) -> Result<()> {
        let path = self.rating_path(id, kind);
        match stars {
            Some(stars) => api::put(&self.crunchy, &path, &[], json!({ "rating": format!("{stars}s") })).await?,
            None => api::delete(&self.crunchy, &path, &[]).await?,
        };
        Ok(())
    }

    /// Marks a season (or a single episode) as watched.
    pub async fn mark_watched(&self, content_id: &str) -> Result<()> {
        api::post(
            &self.crunchy,
            &format!("/content/v2/discover/{}/mark_as_watched/{content_id}", self.account),
            &self.q(&[]),
            Value::Null,
        )
        .await?;
        Ok(())
    }

    /// Episodes in progress, newest first, one per series: the discover history the official apps (and the web app)
    /// fill "continue watching" from, in one request (the full watch history pages through everything ever watched,
    /// and took seconds).
    pub async fn continue_watching(&self) -> Result<Vec<Card>> {
        let path = format!("/content/v2/discover/{}/history", self.account);
        let answer = self.get(&path, &[("n", "30".into()), ("ratings", "true".into())]).await?;
        let mut cards: Vec<Card> = Vec::new();
        for item in data(&answer) {
            if item.get("fully_watched").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let Some(card) = Card::from_json(item.get("panel").unwrap_or(item)) else { continue };
            if card.series_id.is_some() && cards.iter().any(|seen| seen.series_id == card.series_id) {
                continue;
            }
            let playhead = item.get("playhead").and_then(Value::as_u64).unwrap_or(0) as u32;
            cards.push(card.with_playhead(playhead, false));
        }
        Ok(cards)
    }

    /// The episode after (or before) this one, with how far it was watched.
    pub async fn adjacent_episode(&self, id: &str, forward: bool) -> Result<Option<Card>> {
        let which = if forward { "up_next" } else { "previous_episode" };
        let answer = self.get(&format!("/content/v2/discover/{which}/{id}"), &[]).await?;
        Ok(data(&answer).first().and_then(|item| {
            let card = Card::from_json(item.get("panel").unwrap_or(item))?;
            let playhead = item.get("playhead").and_then(Value::as_u64).unwrap_or(0) as u32;
            let watched = item.get("fully_watched").and_then(Value::as_bool).unwrap_or(false);
            Some(card.with_playhead(playhead, watched))
        }))
    }

    /// Where an episode or movie was left, in seconds.
    pub async fn save_playhead(&self, id: &str, seconds: u32) -> Result<()> {
        let path = format!("/content/v2/{}/playheads", self.account);
        api::post(&self.crunchy, &path, &self.q(&[]), json!({ "content_id": id, "playhead": seconds })).await?;
        Ok(())
    }

    fn episode_rating_path(&self, id: &str) -> String {
        format!("/content-reviews/v2/user/{}/rating/episode/{id}", self.account)
    }

    /// The profile's thumb on an episode: "up", "down", or `None`.
    pub async fn episode_rating(&self, id: &str) -> Result<Option<String>> {
        let answer = api::get(&self.crunchy, &self.episode_rating_path(id), &[]).await?;
        Ok(Some(str(&answer, "rating").to_string()).filter(|rating| rating == "up" || rating == "down"))
    }

    /// Thumbs an episode up or down, or takes the thumb back with `None`.
    pub async fn rate_episode(&self, id: &str, rating: Option<&str>) -> Result<()> {
        let path = self.episode_rating_path(id);
        match rating {
            Some(rating) => api::put(&self.crunchy, &path, &[], json!({ "rating": rating })).await?,
            None => api::delete(&self.crunchy, &path, &[]).await?,
        };
        Ok(())
    }

    /// The recap, intro, credits and preview of an episode, where Crunchyroll marked them (none for most movies).
    pub async fn skip_events(&self, id: &str) -> Result<Vec<SkipEvent>> {
        let url = format!("https://static.crunchyroll.com/skip-events/production/{id}.json");
        let response = self.crunchy.client().get(url).send().await.context("asking for the skip events")?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }
        let answer: Value = response.json().await.context("reading the skip events")?;
        let mut events: Vec<SkipEvent> = ["recap", "intro", "credits", "preview"]
            .into_iter()
            .filter_map(|kind| {
                let event = answer.get(kind)?;
                let start = event.get("start").and_then(Value::as_f64)? as f32;
                let end = event.get("end").and_then(Value::as_f64)? as f32;
                (end > start).then(|| SkipEvent { kind: kind.to_string(), start, end })
            })
            .collect();
        events.sort_by(|a, b| a.start.total_cmp(&b.start));
        Ok(events)
    }

    pub async fn browse(&self, browse: &Browse, start: usize, count: usize) -> Result<Page> {
        let mut extra = vec![
            ("n", count.to_string()),
            ("start", start.to_string()),
            ("sort_by", browse.sort.param().to_string()),
            ("ratings", "true".to_string()),
        ];
        if let Some(media) = browse.media {
            extra.push(("type", media.to_string()));
        }
        if let Some(category) = &browse.category {
            extra.push(("categories", category.clone()));
        }
        if let Some(tag) = &browse.seasonal_tag {
            extra.push(("seasonal_tag", tag.clone()));
        }
        if let Some(query) = browse.query.as_ref().filter(|q| !q.trim().is_empty()) {
            extra.push(("q", query.trim().to_string()));
        }
        match browse.audio {
            Audio::All => {}
            Audio::Subbed => extra.push(("is_subbed", "true".to_string())),
            Audio::Dubbed => extra.push(("is_dubbed", "true".to_string())),
        }
        let answer = self.get("/content/v2/discover/browse", &extra).await?;
        Ok(page(&answer, |item| Card::from_json(item)))
    }

    /// Shows and episodes matching what was typed: shows first.
    pub async fn search(&self, text: &str, count: usize) -> Result<Vec<Card>> {
        let answer = self
            .get(
                "/content/v2/discover/search",
                &[("q", text.to_string()), ("n", count.to_string()), ("type", "series,movie_listing".to_string())],
            )
            .await?;
        let mut cards: Vec<Card> = Vec::new();
        for group in data(&answer) {
            for item in group.get("items").and_then(Value::as_array).into_iter().flatten() {
                if let Some(card) = Card::from_json(item)
                    && !cards.iter().any(|seen| seen.id == card.id)
                {
                    cards.push(card);
                }
            }
        }
        Ok(cards)
    }

    pub async fn categories(&self) -> Result<Vec<Named>> {
        let answer = self.get("/content/v2/discover/categories", &[]).await?;
        Ok(data(&answer).iter().map(named).collect())
    }

    /// Simulcast seasons, newest first.
    pub async fn seasonal_tags(&self) -> Result<Vec<Named>> {
        let answer = self.get("/content/v2/discover/seasonal_tags", &[]).await?;
        Ok(data(&answer).iter().map(named).collect())
    }

    pub async fn watchlist(&self, start: usize, count: usize) -> Result<Page> {
        let answer = self
            .get(
                &format!("/content/v2/discover/{}/watchlist", self.account),
                &[("n", count.to_string()), ("start", start.to_string()), ("order", "desc".to_string())],
            )
            .await?;
        Ok(page(&answer, entry_card))
    }

    /// Watched episodes, newest first (`page` from 1).
    pub async fn history(&self, page_number: usize, count: usize) -> Result<Page> {
        let answer = self
            .get(
                &format!("/content/v2/{}/watch-history", self.account),
                &[("page", page_number.to_string()), ("page_size", count.to_string())],
            )
            .await?;
        Ok(page(&answer, entry_card))
    }

    pub async fn custom_lists(&self) -> Result<Vec<Named>> {
        let answer = self.get(&format!("/content/v2/{}/custom-lists", self.account), &[]).await?;
        Ok(data(&answer)
            .iter()
            .map(|list| Named {
                id: str(list, "list_id").to_string(),
                title: str(list, "title").to_string(),
                count: list.get("total").and_then(Value::as_u64).map(|n| n as u32),
            })
            .collect())
    }

    pub async fn custom_list(&self, list_id: &str) -> Result<Page> {
        let answer = self.get(&format!("/content/v2/{}/custom-lists/{list_id}", self.account), &[]).await?;
        Ok(page(&answer, entry_card))
    }

    pub async fn similar(&self, id: &str, count: usize) -> Result<Vec<Card>> {
        let answer = self
            .get(&format!("/content/v2/discover/{}/similar_to/{id}", self.account), &[("n", count.to_string())])
            .await?;
        Ok(data(&answer).iter().filter_map(Card::from_json).collect())
    }

    /// The music tab's rows: titled collections of videos, concerts or artists, in the feed's order.
    pub async fn music_feed(&self) -> Result<Vec<MusicRow>> {
        let answer = self.get(&format!("/content/v2/music/{}/landing_feed", self.account), &[]).await?;
        let mut rows = Vec::new();
        for item in data(&answer) {
            if str(item, "resource_type") != "curated_collection" {
                continue;
            }
            let kind = match str(item, "response_type") {
                "music_video" => MusicKind::Videos,
                "music_concert" => MusicKind::Concerts,
                "artist" => MusicKind::Artists,
                _ => continue,
            };
            let ids: Vec<String> = item
                .get("collection_items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|entry| str(entry, "id").to_string())
                .filter(|id| !id.is_empty())
                .collect();
            if !ids.is_empty() {
                rows.push(MusicRow { title: str(item, "title").to_string(), kind, ids });
            }
        }
        Ok(rows)
    }

    /// Music videos, concerts or artists by id, in that order.
    pub async fn music(&self, kind: MusicKind, ids: &[String]) -> Result<Vec<Card>> {
        let path = match kind {
            MusicKind::Videos => "music_videos",
            MusicKind::Concerts => "concerts",
            MusicKind::Artists => "artists",
        };
        let mut cards = Vec::new();
        for chunk in ids.chunks(BATCH) {
            let answer = self.get(&format!("/content/v2/music/{path}/{}", chunk.join(",")), &[]).await?;
            cards.extend(data(&answer).iter().filter_map(Card::from_json));
        }
        Ok(cards)
    }

    /// The songs of a show (openings, endings), newest first.
    pub async fn featured_music(&self, show_id: &str) -> Result<Vec<Card>> {
        let answer = self.get(&format!("/content/v2/music/featured/{show_id}"), &[]).await?;
        let mut cards: Vec<Card> = data(&answer).iter().filter_map(Card::from_json).collect();
        cards.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(cards)
    }

    pub async fn music_search(&self, text: &str, count: usize) -> Result<Vec<Card>> {
        let answer = self
            .get("/content/v2/discover/search", &[("q", text.to_string()), ("n", count.to_string()), ("type", "music".to_string())])
            .await?;
        Ok(data(&answer)
            .iter()
            .flat_map(|group| group.get("items").and_then(Value::as_array).cloned().unwrap_or_default())
            .filter_map(|item| Card::from_json(&item))
            .collect())
    }

    /// An artist and their videos and concerts.
    pub async fn artist(&self, id: &str) -> Result<(Card, Vec<Card>, Vec<Card>)> {
        let answer = self.get(&format!("/content/v2/music/artists/{id}"), &[]).await?;
        let item = data(&answer).first().cloned().context("Crunchyroll doesn't know this artist")?;
        let card = Card::from_json(&item).context("not an artist")?;
        let ids = |key: &str| -> Vec<String> {
            item.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
        };
        let (video_ids, concert_ids) = (ids("videos"), ids("concerts"));
        let (videos, concerts) =
            futures_util::join!(self.music(MusicKind::Videos, &video_ids), self.music(MusicKind::Concerts, &concert_ids));
        let newest = |mut cards: Vec<Card>| {
            cards.sort_by(|a, b| b.date.cmp(&a.date));
            cards
        };
        Ok((card, newest(videos.unwrap_or_default()), newest(concerts.unwrap_or_default())))
    }

    /// The series or movie listings behind a list's entries (episodes become their series), without repeats.
    pub async fn shows_of(&self, cards: &[Card]) -> Result<Vec<Card>> {
        let mut ids: Vec<String> = Vec::new();
        for card in cards {
            let id = match card.kind {
                Kind::Series | Kind::MovieListing => Some(card.id.clone()),
                _ => card.series_id.clone(),
            };
            if let Some(id) = id
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        Ok(self.objects(&ids).await?.iter().filter_map(Card::from_json).collect())
    }

    /// A show's genres, in the profile's language.
    pub async fn genres_of(&self, id: &str) -> Result<Vec<String>> {
        let answer = self.get("/content/v2/discover/categories", &[("guid", id.to_string())]).await?;
        Ok(data(&answer).iter().map(named).map(|n| n.title).filter(|t| !t.is_empty()).collect())
    }

    /// Raw GET for screens with their own shapes.
    pub async fn raw(&self, path: &str, extra: &[(&'static str, String)]) -> Result<Value> {
        self.get(path, extra).await
    }
}

/// A stretch of an episode the player offers to skip, in seconds from the start.
#[derive(Clone, Debug)]
pub struct SkipEvent {
    /// recap, intro, credits or preview
    pub kind: String,
    pub start: f32,
    pub end: f32,
}

/// A card with what the hero shows above it.
#[derive(Clone, Debug, Default)]
pub struct Hero {
    pub card: Card,
    pub logo: Option<String>,
    /// Art made for a hero, screen size.
    pub hero: Option<String>,
    /// The same art, small: it loads in a moment and stands in until the big one is in.
    pub preview: Option<String>,
}

impl Hero {
    pub fn from_object(object: &Value) -> Option<Hero> {
        let card = Card::from_json(object)?;
        let art = |params: &str| {
            images::localized(object, "hero_background", params).or_else(|| images::localized(object, "tile_artwork", params))
        };
        Some(Hero {
            logo: images::localized(object, "content_logo_polychromatic", &crate::model::logo_params()),
            hero: art(&crate::model::hero_params(1920)),
            preview: art(&crate::model::hero_params(640)),
            card,
        })
    }

    /// The part of an object the hero needs, to keep on disk (a full object is ten times this).
    pub fn trim(object: &Value) -> Value {
        let pick = |value: Option<&Value>, keys: &[&str]| -> Value {
            let mut out = serde_json::Map::new();
            if let Some(Value::Object(map)) = value {
                for key in keys {
                    if let Some(v) = map.get(*key) {
                        out.insert(key.to_string(), v.clone());
                    }
                }
            }
            Value::Object(out)
        };
        let meta = ["series_launch_year", "season_count", "episode_count", "maturity_ratings", "is_subbed", "is_dubbed", "movie_release_year", "is_premium_only"];
        let mut out = pick(Some(object), &["id", "type", "title", "description", "slug_title"]);
        let map = out.as_object_mut().expect("object");
        map.insert("series_metadata".into(), pick(object.get("series_metadata"), &meta));
        map.insert("movie_listing_metadata".into(), pick(object.get("movie_listing_metadata"), &meta));
        map.insert("rating".into(), pick(object.get("rating"), &["average"]));
        map.insert(
            "images_localized".into(),
            pick(object.get("images_localized"), &["hero_background", "tile_artwork", "content_logo_polychromatic"]),
        );
        out
    }
}

/// A list entry's card (`panel`), with its progress.
fn entry_card(item: &Value) -> Option<Card> {
    let card = Card::from_json(item.get("panel").unwrap_or(item))?;
    let playhead = item.get("playhead").and_then(Value::as_u64).unwrap_or(0) as u32;
    let watched = item.get("fully_watched").and_then(Value::as_bool).unwrap_or(false);
    Some(card.with_playhead(playhead, watched))
}

fn page(answer: &Value, card: impl Fn(&Value) -> Option<Card>) -> Page {
    let cards: Vec<Card> = data(answer).iter().filter_map(card).collect();
    let total = answer.get("total").and_then(Value::as_u64).map(|n| n as usize).unwrap_or(cards.len());
    Page { cards, total }
}

fn named(item: &Value) -> Named {
    let title = item
        .get("localization")
        .map(|localization| str(localization, "title"))
        .filter(|title| !title.is_empty())
        .unwrap_or(str(item, "title"));
    Named { id: str(item, "id").to_string(), title: title.to_string(), count: None }
}
