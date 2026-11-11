//! Colours, sizes and the Spanish text the screens share.

use crunchy_core::catalog::RowSpec;
use crunchy_core::model::{Card, Kind};

use crate::gfx::Color;

pub const BG: Color = Color::rgb(0x0b0b0e);
pub const TEXT: Color = Color::rgb(0xf5f5f7);
pub const MUTED: Color = Color(0.92, 0.92, 0.96, 0.6);
pub const DIM: Color = Color(0.92, 0.92, 0.96, 0.42);
pub const SURFACE: Color = Color::rgb(0x1c1c21);
pub const ACCENT: Color = Color::rgb(0xff5e00);
pub const GOLD: Color = Color::rgb(0xffc94d);
/// A control at rest: a light veil over whatever is behind.
pub const VEIL: Color = Color(1.0, 1.0, 1.0, 0.12);
pub const VEIL_STRONG: Color = Color(1.0, 1.0, 1.0, 0.22);

/// Side margin of every screen.
pub const PAD: f32 = 48.0;
pub const CARD_W: f32 = 384.0;
pub const CARD_H: f32 = 216.0;
/// Card plus the gap to the next one.
pub const CARD_STEP: f32 = 432.0;
pub const RADIUS: f32 = 14.0;

pub fn row_title(spec: &RowSpec) -> String {
    match spec {
        RowSpec::ContinueWatching => "Seguir viendo".into(),
        RowSpec::Watchlist => "Mi lista".into(),
        RowSpec::Recommendations => "Recomendado para ti".into(),
        RowSpec::Series { title, .. } => title.clone(),
        RowSpec::Browse { .. } => "Explorar".into(),
        RowSpec::SimilarTo { title, .. } => title.clone(),
        RowSpec::Featured(_) => "Destacados".into(),
    }
}

/// The line under a card's title: "T1 · E5 — Name", "2023 · 2 temporadas", "Película".
pub fn card_subtitle(card: &Card) -> String {
    match card.kind {
        Kind::Episode => match &card.episode {
            Some(episode) => {
                let mut out = format!("T{}", episode.season);
                if !episode.number.is_empty() {
                    out.push_str(&format!(" · E{}", episode.number));
                }
                if !episode.name.is_empty() {
                    out.push_str(&format!(" — {}", episode.name));
                }
                out
            }
            None => String::new(),
        },
        Kind::Series => {
            let mut parts = Vec::new();
            if let Some(year) = card.year {
                parts.push(year.to_string());
            }
            if let Some(seasons) = card.season_count.filter(|count| *count > 0) {
                parts.push(seasons_text(seasons));
            }
            parts.join(" · ")
        }
        Kind::MovieListing | Kind::Movie => match card.year {
            Some(year) => format!("{year} · Película"),
            None => "Película".into(),
        },
        Kind::MusicVideo | Kind::Concert => {
            let minutes = card.episode.as_ref().map(|e| e.duration_secs).filter(|d| *d > 0).map(|d| format!("{} min", ((d as f32 / 60.0).round() as u32).max(1)));
            [card.artist.clone(), minutes].into_iter().flatten().collect::<Vec<_>>().join(" · ")
        }
        Kind::Artist => "Artista".into(),
        Kind::Season => String::new(),
    }
}

pub fn seasons_text(count: u32) -> String {
    if count == 1 { "1 temporada".into() } else { format!("{count} temporadas") }
}

pub fn episodes_text(count: u32) -> String {
    if count == 1 { "1 episodio".into() } else { format!("{count} episodios") }
}

/// "Sub | Dob", "Sub", "Dob".
pub fn audio_text(card: &Card) -> Option<&'static str> {
    match (card.subbed, card.dubbed) {
        (true, true) => Some("Sub | Dob"),
        (true, false) => Some("Sub"),
        (false, true) => Some("Dob"),
        (false, false) => None,
    }
}

/// "4,8": Spanish decimals.
pub fn rating_text(rating: f32) -> String {
    format!("{rating:.1}").replace('.', ",")
}

/// What kind of show it is, for the eyebrow over a hero.
pub fn eyebrow(kind: Kind) -> &'static str {
    match kind {
        Kind::MovieListing | Kind::Movie => "PELÍCULA",
        Kind::MusicVideo | Kind::Concert => "MÚSICA",
        Kind::Artist => "ARTISTA",
        _ => "SERIES",
    }
}

/// Moves `current` toward `target`, frame-rate independent (`speed` per second).
pub fn approach(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let t = 1.0 - (-dt * speed).exp();
    let next = current + (target - current) * t;
    if (target - next).abs() < 0.5 { target } else { next }
}
