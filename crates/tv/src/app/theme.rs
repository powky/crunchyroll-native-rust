//! Colours, sizes and the Spanish text the screens share.

use crunchy_core::catalog::RowSpec;
use crunchy_core::model::{Card, Kind};

use crate::gfx::Color;

pub const BG: Color = Color::rgb(0x0b0b0e);
pub const TEXT: Color = Color::rgb(0xf5f5f7);
pub const MUTED: Color = Color(0.92, 0.92, 0.96, 0.6);
pub const SURFACE: Color = Color::rgb(0x1c1c21);
pub const ACCENT: Color = Color::rgb(0xff5e00);

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
                parts.push(if seasons == 1 { "1 temporada".to_string() } else { format!("{seasons} temporadas") });
            }
            parts.join(" · ")
        }
        Kind::MovieListing | Kind::Movie => "Película".into(),
        Kind::Season => String::new(),
    }
}

/// Moves `current` toward `target`, frame-rate independent (`speed` per second).
pub fn approach(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    let t = 1.0 - (-dt * speed).exp();
    let next = current + (target - current) * t;
    if (target - next).abs() < 0.5 { target } else { next }
}
