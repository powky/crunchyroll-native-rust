//! What the screens draw: plain data, no crunchyroll-rs types, so the UI never waits on or reaches into the API.
//! Text is kept as numbers and names; the UI formats it in its own language.

use crunchyroll_rs::MediaCollection;

use crate::images;

/// Picture width cards ask for (cards are 384 px wide on the 1920x1080 UI).
pub const CARD_IMAGE_WIDTH: u32 = 480;
/// Picture width for a full-screen backdrop.
pub const BACKDROP_WIDTH: u32 = 1920;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Series,
    Season,
    Episode,
    MovieListing,
    Movie,
}

#[derive(Clone, Debug)]
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
    pub description: String,
    /// 16:9 artwork, card size.
    pub image: Option<String>,
    /// The same artwork, screen size.
    pub backdrop: Option<String>,
    /// Watched share, 0..=1, for a progress bar.
    pub progress: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct EpisodeInfo {
    pub season: u32,
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
                episode: None,
                year: series.series_launch_year,
                season_count: Some(series.season_count),
                description: series.description.clone(),
                image: images::pick(&series.images.poster_wide, CARD_IMAGE_WIDTH),
                backdrop: images::pick(&series.images.poster_wide, BACKDROP_WIDTH),
                progress: None,
            },
            MediaCollection::Season(season) => Card {
                id: season.id.clone(),
                kind: Kind::Season,
                title: season.title.clone(),
                series_id: Some(season.series_id.clone()),
                episode: None,
                year: None,
                season_count: None,
                description: season.description.clone(),
                image: None,
                backdrop: None,
                progress: None,
            },
            MediaCollection::Episode(episode) => Card {
                id: episode.id.clone(),
                kind: Kind::Episode,
                title: episode.series_title.clone(),
                series_id: Some(episode.series_id.clone()),
                episode: Some(EpisodeInfo {
                    season: episode.season_number,
                    number: episode.episode.clone(),
                    name: episode.title.clone(),
                    duration_secs: episode.duration.num_seconds().max(0) as u32,
                }),
                year: None,
                season_count: None,
                description: episode.description.clone(),
                image: images::pick(&episode.images, CARD_IMAGE_WIDTH),
                backdrop: images::pick(&episode.images, BACKDROP_WIDTH),
                progress: None,
            },
            MediaCollection::MovieListing(listing) => Card {
                id: listing.id.clone(),
                kind: Kind::MovieListing,
                title: listing.title.clone(),
                series_id: None,
                episode: None,
                year: None,
                season_count: None,
                description: listing.description.clone(),
                image: images::pick(&listing.images.poster_wide, CARD_IMAGE_WIDTH),
                backdrop: images::pick(&listing.images.poster_wide, BACKDROP_WIDTH),
                progress: None,
            },
            MediaCollection::Movie(movie) => Card {
                id: movie.id.clone(),
                kind: Kind::Movie,
                title: movie.title.clone(),
                series_id: None,
                episode: None,
                year: None,
                season_count: None,
                description: movie.description.clone(),
                image: images::pick(&movie.images.thumbnail, CARD_IMAGE_WIDTH),
                backdrop: images::pick(&movie.images.thumbnail, BACKDROP_WIDTH),
                progress: None,
            },
            _ => return None,
        };
        Some(card)
    }

    /// With the watched share from a playhead (seconds into it).
    pub fn with_playhead(mut self, playhead: u32, fully_watched: bool) -> Card {
        let duration = self.episode.as_ref().map(|episode| episode.duration_secs).unwrap_or(0);
        self.progress = if fully_watched {
            Some(1.0)
        } else if duration > 0 {
            Some((playhead as f32 / duration as f32).clamp(0.0, 1.0))
        } else {
            None
        };
        self
    }
}
