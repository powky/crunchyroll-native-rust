//! What the screens draw: plain data, no crunchyroll-rs types, so the UI never waits on or reaches into the API.
//! Text is kept as numbers and names; the UI formats it in its own language.

use std::sync::atomic::{AtomicU32, Ordering};

use crunchyroll_rs::MediaCollection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::images;

/// Picture width cards ask for, in UI pixels (cards are 384-420 wide on the 1920x1080 UI).
pub const CARD_IMAGE_WIDTH: u32 = 420;
/// Picture width for a full-screen backdrop, in UI pixels.
pub const BACKDROP_WIDTH: u32 = 1920;
/// Picture width for a 2:3 poster in a grid, in UI pixels.
pub const POSTER_WIDTH: u32 = 320;
/// Picture width for a show's logo, in UI pixels (the widest logo box).
pub const LOGO_WIDTH: u32 = 720;

/// Screen pixels per UI pixel, in hundredths: 200 on a 4K TV, whose pictures must be twice as wide to stay sharp.
static SCALE: AtomicU32 = AtomicU32::new(100);

/// Tells the catalog how big the screen is (its height over the UI's 1080), before pictures are chosen.
pub fn set_display_scale(scale: f32) {
    SCALE.store((scale.clamp(0.5, 4.0) * 100.0).round() as u32, Ordering::Relaxed);
}

pub fn display_scale() -> f32 {
    SCALE.load(Ordering::Relaxed) as f32 / 100.0
}

/// imgsrv options for a show's logo: transparent, so WebP.
pub fn logo_params() -> String {
    format!("fit=contain,width={},trim=border,format=auto,quality=85", px(LOGO_WIDTH))
}

/// imgsrv options for keyart behind a hero, `width` UI pixels wide (16:9): JPEG, the fastest to decode.
pub fn hero_params(width: u32) -> String {
    format!("fit=cover,width={},height={},format=jpeg,quality=85", px(width), px(width) * 9 / 16)
}

/// A width in UI pixels, in screen pixels.
pub fn px(width: u32) -> u32 {
    (width as f32 * display_scale()).round() as u32
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    #[default]
    Series,
    Season,
    Episode,
    MovieListing,
    Movie,
    MusicVideo,
    Concert,
    Artist,
}

#[derive(Clone, Debug, Default)]
pub struct Card {
    pub id: String,
    pub kind: Kind,
    /// The show's name (for an episode, its series).
    pub title: String,
    /// The series an episode or season belongs to, to open its page.
    pub series_id: Option<String>,
    pub episode: Option<EpisodeInfo>,
    pub year: Option<u32>,
    pub season_count: Option<u32>,
    pub episode_count: Option<u32>,
    pub description: String,
    /// 16:9 artwork, card size.
    pub image: Option<String>,
    /// The same artwork, screen size.
    pub backdrop: Option<String>,
    /// 2:3 artwork, for grids.
    pub poster: Option<String>,
    /// Watched share, 0..=1, for a progress bar.
    pub progress: Option<f32>,
    /// Seen to the end.
    pub watched: bool,
    /// "TV-14" and the like.
    pub maturity: Option<String>,
    pub subbed: bool,
    pub dubbed: bool,
    /// Average stars, 1..=5.
    pub rating: Option<f32>,
    pub premium_only: bool,
    /// When it came out or was last updated (RFC 3339), for grids sorted by date.
    pub date: Option<String>,
    /// Artist of a music video or concert.
    pub artist: Option<String>,
    /// The show's title logo, when the answer had keyart.
    pub logo: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct EpisodeInfo {
    pub season: u32,
    pub season_id: String,
    /// As Crunchyroll numbers it ("12", "12.5", "SP"); empty for some specials.
    pub number: String,
    pub name: String,
    pub duration_secs: u32,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub title: String,
    pub cards: Vec<Card>,
}

impl Card {
    /// A card for anything the API lists, or `None` for kinds the app doesn't show (music, artists).
    pub fn from_collection(collection: &MediaCollection) -> Option<Card> {
        let card = match collection {
            MediaCollection::Series(series) => Card {
                id: series.id.clone(),
                kind: Kind::Series,
                title: series.title.clone(),
                series_id: Some(series.id.clone()),
                year: series.series_launch_year,
                season_count: Some(series.season_count),
                episode_count: Some(series.episode_count),
                description: series.description.clone(),
                image: images::pick(&series.images.poster_wide, px(CARD_IMAGE_WIDTH)),
                backdrop: images::pick(&series.images.poster_wide, px(BACKDROP_WIDTH)),
                poster: images::pick(&series.images.poster_tall, px(POSTER_WIDTH)),
                maturity: series.maturity_ratings.first().cloned(),
                subbed: series.is_subbed,
                dubbed: series.is_dubbed,
                ..Default::default()
            },
            MediaCollection::Season(season) => Card {
                id: season.id.clone(),
                kind: Kind::Season,
                title: season.title.clone(),
                series_id: Some(season.series_id.clone()),
                description: season.description.clone(),
                ..Default::default()
            },
            MediaCollection::Episode(episode) => Card {
                id: episode.id.clone(),
                kind: Kind::Episode,
                title: episode.series_title.clone(),
                series_id: Some(episode.series_id.clone()),
                episode: Some(EpisodeInfo {
                    season: episode.season_number,
                    season_id: episode.season_id.clone(),
                    number: episode.episode.clone(),
                    name: episode.title.clone(),
                    duration_secs: episode.duration.num_seconds().max(0) as u32,
                }),
                description: episode.description.clone(),
                image: images::pick(&episode.images, px(CARD_IMAGE_WIDTH)),
                backdrop: images::pick(&episode.images, px(BACKDROP_WIDTH)),
                maturity: episode.maturity_ratings.first().cloned(),
                subbed: episode.is_subbed,
                dubbed: episode.is_dubbed,
                premium_only: episode.is_premium_only,
                date: Some(episode.episode_air_date.to_rfc3339()),
                ..Default::default()
            },
            MediaCollection::MovieListing(listing) => Card {
                id: listing.id.clone(),
                kind: Kind::MovieListing,
                title: listing.title.clone(),
                year: Some(listing.movie_release_year).filter(|year| *year > 0),
                description: listing.description.clone(),
                image: images::pick(&listing.images.poster_wide, px(CARD_IMAGE_WIDTH)),
                backdrop: images::pick(&listing.images.poster_wide, px(BACKDROP_WIDTH)),
                poster: images::pick(&listing.images.poster_tall, px(POSTER_WIDTH)),
                maturity: listing.maturity_ratings.first().cloned(),
                subbed: listing.is_subbed,
                dubbed: listing.is_dubbed,
                ..Default::default()
            },
            MediaCollection::Movie(movie) => Card {
                id: movie.id.clone(),
                kind: Kind::Movie,
                title: movie.title.clone(),
                description: movie.description.clone(),
                image: images::pick(&movie.images.thumbnail, px(CARD_IMAGE_WIDTH)),
                backdrop: images::pick(&movie.images.thumbnail, px(BACKDROP_WIDTH)),
                ..Default::default()
            },
            _ => return None,
        };
        Some(card)
    }

    /// A card from the API's JSON for a series, movie listing, episode, movie, season, music video or concert
    /// (a `panel` of a list entry is one of these too). `None` for anything else.
    pub fn from_json(item: &Value) -> Option<Card> {
        let s = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64).map(|n| n as u32);
        let b = |value: &Value, key: &str| value.get(key).and_then(Value::as_bool).unwrap_or(false);
        let maturity = |value: &Value| {
            value.get("maturity_ratings").and_then(Value::as_array).and_then(|list| list.first()).and_then(Value::as_str).map(String::from)
        };
        let images = item.get("images");
        let wide = |width| images::pick_json(images, &["poster_wide", "thumbnail"], px(width));
        let id = s(item, "id");
        if id.is_empty() {
            return None;
        }
        let mut card = Card {
            id: id.clone(),
            title: s(item, "title"),
            description: s(item, "description"),
            image: wide(CARD_IMAGE_WIDTH),
            backdrop: wide(BACKDROP_WIDTH),
            poster: images::pick_json(images, &["poster_tall"], px(POSTER_WIDTH)),
            rating: item
                .get("rating")
                .and_then(|rating| rating.get("average"))
                .and_then(|average| average.as_str().and_then(|text| text.parse().ok()).or(average.as_f64().map(|f| f as f32)))
                .filter(|average: &f32| *average > 0.0),
            date: Some(s(item, "last_public")).filter(|date| !date.is_empty()),
            logo: images::localized(item, "content_logo_polychromatic", &logo_params()),
            ..Default::default()
        };
        // the cms episode and movie lists give their fields flat, with no type
        let kind = match s(item, "type") {
            kind if !kind.is_empty() => kind,
            _ if item.get("season_id").is_some() && item.get("series_id").is_some() => "episode".to_string(),
            _ if item.get("movie_listing_id").is_some() => "movie".to_string(),
            _ => String::new(),
        };
        match kind.as_str() {
            "series" => {
                let meta = item.get("series_metadata").unwrap_or(&Value::Null);
                card.kind = Kind::Series;
                card.series_id = Some(id);
                card.year = n(meta, "series_launch_year");
                card.season_count = n(meta, "season_count");
                card.episode_count = n(meta, "episode_count");
                card.maturity = maturity(meta);
                card.subbed = b(meta, "is_subbed");
                card.dubbed = b(meta, "is_dubbed");
            }
            "movie_listing" => {
                let meta = item.get("movie_listing_metadata").unwrap_or(&Value::Null);
                card.kind = Kind::MovieListing;
                card.year = n(meta, "movie_release_year").filter(|year| *year > 0);
                card.maturity = maturity(meta);
                card.subbed = b(meta, "is_subbed");
                card.dubbed = b(meta, "is_dubbed");
                card.premium_only = b(meta, "is_premium_only");
            }
            "episode" => {
                let meta = item.get("episode_metadata").unwrap_or(item);
                card.kind = Kind::Episode;
                card.title = s(meta, "series_title");
                card.series_id = Some(s(meta, "series_id")).filter(|id| !id.is_empty());
                card.episode = Some(EpisodeInfo {
                    season: n(meta, "season_number").unwrap_or(0),
                    season_id: s(meta, "season_id"),
                    number: s(meta, "episode"),
                    name: s(item, "title"),
                    duration_secs: (meta.get("duration_ms").and_then(Value::as_u64).unwrap_or(0) / 1000) as u32,
                });
                card.maturity = maturity(meta);
                card.subbed = b(meta, "is_subbed");
                card.dubbed = b(meta, "is_dubbed");
                card.premium_only = b(meta, "is_premium_only");
                card.date = Some(s(meta, "episode_air_date")).filter(|date| !date.is_empty()).or(card.date);
            }
            "movie" => {
                let meta = item.get("movie_metadata").unwrap_or(item);
                card.kind = Kind::Movie;
                card.series_id = Some(s(meta, "movie_listing_id")).filter(|id| !id.is_empty());
                card.episode = Some(EpisodeInfo {
                    name: card.title.clone(),
                    duration_secs: (meta.get("duration_ms").and_then(Value::as_u64).unwrap_or(0) / 1000) as u32,
                    ..Default::default()
                });
                card.title = Some(s(meta, "movie_listing_title")).filter(|t| !t.is_empty()).unwrap_or(card.title);
                card.maturity = maturity(meta);
                card.premium_only = b(meta, "is_premium_only");
            }
            "season" => {
                card.kind = Kind::Season;
                card.series_id = Some(s(item, "series_id")).filter(|id| !id.is_empty());
            }
            "musicArtist" | "artist" => {
                card.kind = Kind::Artist;
                card.title = s(item, "name");
                card.poster = card.poster.or(images::pick_json(images, &["poster_wide"], px(POSTER_WIDTH)));
            }
            "music_video" | "musicVideo" | "concert" | "musicConcert" => {
                card.kind = if kind.contains("oncert") { Kind::Concert } else { Kind::MusicVideo };
                card.artist = item.get("artist").map(|artist| s(artist, "name")).filter(|name| !name.is_empty());
                card.episode = Some(EpisodeInfo {
                    name: card.title.clone(),
                    duration_secs: (item.get("durationMs").or(item.get("duration_ms")).and_then(Value::as_u64).unwrap_or(0)
                        / 1000) as u32,
                    ..Default::default()
                });
                card.image = images::pick_json(images, &["thumbnail", "poster_wide"], px(CARD_IMAGE_WIDTH));
                card.backdrop = images::pick_json(images, &["thumbnail", "poster_wide"], px(BACKDROP_WIDTH));
                card.date = Some(s(item, "originalRelease")).filter(|date| !date.is_empty()).or(card.date);
            }
            _ => return None,
        }
        Some(card)
    }

    /// With the watched share from a playhead (seconds into it).
    pub fn with_playhead(mut self, playhead: u32, fully_watched: bool) -> Card {
        let duration = self.episode.as_ref().map(|episode| episode.duration_secs).unwrap_or(0);
        self.watched = fully_watched;
        self.progress = if fully_watched {
            Some(1.0)
        } else if duration > 0 && playhead > 0 {
            Some((playhead as f32 / duration as f32).clamp(0.0, 1.0))
        } else {
            None
        };
        self
    }
}
