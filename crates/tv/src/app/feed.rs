//! A feed: a hero for the focused card over its art, and under it a shelf showing one row at a time (Up/Down slide
//! the next row in). The home and the music tab are feeds.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crunchy_core::content::Hero;
use crunchy_core::model::{BACKDROP_WIDTH, Card, Kind};

use crate::gfx::{Color, Font, Rect};
use crate::platform::Key;

use super::theme::*;
use super::ui::{Backdrop, CARD_PICTURE, HeroSize, HeroView, Ui, row_scroll};

pub struct FeedRow {
    pub title: String,
    pub cards: Option<Vec<Card>>,
    pub requested: bool,
    pub focus: usize,
    pub scroll: f32,
}

impl FeedRow {
    pub fn new(title: String) -> FeedRow {
        FeedRow { title, cards: None, requested: false, focus: 0, scroll: 0.0 }
    }
}

pub enum FeedNav {
    Stay,
    /// Up from the first row
    Up,
    Open(Card),
    Back,
}

pub struct Feed {
    pub rows: Vec<FeedRow>,
    pub row: usize,
    /// vertical position, in rows, eased toward `row`
    row_pos: f32,
    backdrop: Backdrop,
    /// what the hero shows, and the focus waiting to settle into it
    hero: Option<Card>,
    pending: Option<(Card, Instant)>,
    /// where the hero's text starts
    hero_top: f32,
}

const SHELF_TOP: f32 = 680.0;
const ROW_H: f32 = 400.0;
const CARD_X: f32 = 48.0;

impl Feed {
    pub fn new(rows: Vec<FeedRow>, hero_top: f32) -> Feed {
        Feed { rows, row: 0, row_pos: 0.0, backdrop: Backdrop::default(), hero: None, pending: None, hero_top }
    }

    pub fn focused(&self) -> Option<&Card> {
        let row = self.rows.get(self.row)?;
        row.cards.as_ref()?.get(row.focus)
    }

    /// Rows near the one in view that should load now (marked as asked).
    pub fn wanted(&mut self) -> Vec<usize> {
        let mut out = Vec::new();
        let end = (self.row + 3).min(self.rows.len());
        for index in self.row.saturating_sub(1)..end {
            let row = &mut self.rows[index];
            if !row.requested {
                row.requested = true;
                out.push(index);
            }
        }
        out
    }

    pub fn key(&mut self, key: Key) -> FeedNav {
        let Some(current) = self.rows.get(self.row) else {
            return match key {
                Key::Up => FeedNav::Up,
                Key::Back => FeedNav::Back,
                _ => FeedNav::Stay,
            };
        };
        let len = current.cards.as_ref().map(|c| c.len()).unwrap_or(0);
        match key {
            Key::Left => self.rows[self.row].focus = current.focus.saturating_sub(1),
            Key::Right => self.rows[self.row].focus = (current.focus + 1).min(len.saturating_sub(1)),
            Key::Up | Key::Down => {
                if key == Key::Up && self.row == 0 {
                    return FeedNav::Up;
                }
                let target = if key == Key::Up { self.row - 1 } else { self.row + 1 };
                if target >= self.rows.len() {
                    return FeedNav::Stay;
                }
                // the card at the same place on screen in the other row
                let slot = current.focus as f32 - current.scroll / CARD_STEP;
                let other = &mut self.rows[target];
                let other_len = other.cards.as_ref().map(|c| c.len()).unwrap_or(0);
                let first = (other.scroll / CARD_STEP).round();
                other.focus = ((first + slot.round()).max(0.0) as usize).min(other_len.saturating_sub(1));
                self.row = target;
            }
            Key::Ok => {
                if let Some(card) = self.focused() {
                    return FeedNav::Open(card.clone());
                }
            }
            Key::Back => {
                if current.focus > 0 {
                    self.rows[self.row].focus = 0;
                } else if self.row > 0 {
                    self.row = 0;
                } else {
                    return FeedNav::Back;
                }
            }
            _ => {}
        }
        FeedNav::Stay
    }

    /// The id whose keyart a card uses: its series for an episode.
    fn art_id(card: &Card) -> Option<&str> {
        match card.kind {
            Kind::Series | Kind::MovieListing => Some(&card.id),
            Kind::Episode | Kind::Movie | Kind::Season => card.series_id.as_deref(),
            _ => None,
        }
    }

    pub fn draw(&mut self, ui: &mut Ui, heroes: &HashMap<String, Hero>, focused: bool) {
        // the hero follows the focus once it has rested a moment (at once the first time)
        let now = Instant::now();
        if let Some(card) = self.focused().cloned() {
            match &self.pending {
                _ if self.hero.is_none() => self.hero = Some(card),
                Some((waiting, _)) if waiting.id == card.id => {}
                _ if self.hero.as_ref().is_some_and(|h| h.id == card.id) => self.pending = None,
                _ => self.pending = Some((card, now)),
            }
        }
        if let Some((card, at)) = &self.pending
            && now.duration_since(*at) >= Duration::from_millis(250)
        {
            self.hero = Some(card.clone());
            self.pending = None;
            self.prefetch_neighbours(ui, heroes);
        }

        let hero = self.hero.clone();
        let art = hero.as_ref().and_then(|card| Self::art_id(card).and_then(|id| heroes.get(id)));
        // a show's own hero art; while its facts are on the way the backdrop stays as it was (the card's picture
        // has the logo printed on it, so it's no backdrop), and things with no show use their own picture
        let wanted = hero.as_ref().and_then(|card| match Self::art_id(card) {
            Some(_) => art.map(|a| a.hero.clone().or(a.card.backdrop.clone()).or(card.backdrop.clone()))?,
            None => card.backdrop.clone().or(card.image.clone()),
        });
        let preview = art.and_then(|a| a.preview.clone());
        ui.backdrop(&mut self.backdrop, wanted, preview, 1.0);
        shade_home(ui);

        match &hero {
            Some(card) => {
                // a series card gets its facts (rating, counts) from the keyart call
                let facts = if matches!(card.kind, Kind::Series | Kind::MovieListing) { art.map(|a| &a.card) } else { None };
                let shown = facts.unwrap_or(card);
                let view = HeroView {
                    card: shown,
                    logo: art.and_then(|a| a.logo.as_deref()),
                    logo_known: art.is_some() || Self::art_id(card).is_none(),
                    size: HeroSize::Home,
                    lines: 3,
                    width: 940.0,
                };
                let bottom = ui.hero(PAD, self.hero_top, &view, 1.0);
                ui.resume_line(card, PAD, bottom + 22.0, 220.0, 1.0);
            }
            None => {
                let top = self.hero_top + 40.0;
                ui.skeleton(Rect::new(PAD, top, 560.0, 72.0), 12.0);
                ui.skeleton(Rect::new(PAD, top + 98.0, 420.0, 26.0), 8.0);
                for (index, w) in [780.0, 740.0, 520.0].iter().enumerate() {
                    ui.skeleton(Rect::new(PAD, top + 152.0 + index as f32 * 36.0, *w, 22.0), 6.0);
                }
            }
        }

        // the shelf: one row in view
        self.row_pos = approach(self.row_pos, self.row as f32, ui.dt, 14.0);
        ui.gfx.clip(Some(Rect::new(0.0, SHELF_TOP, 1920.0, ROW_H)));
        for (index, row) in self.rows.iter_mut().enumerate() {
            let y = SHELF_TOP + (index as f32 - self.row_pos) * ROW_H;
            if y <= SHELF_TOP - ROW_H || y >= SHELF_TOP + ROW_H {
                continue;
            }
            let title_color = Color(0.96, 0.96, 0.97, 0.92);
            ui.gfx.text(Font::SemiBold, 28.0, PAD, y, &row.title, title_color, Some(1400.0));
            let cards_y = y + 60.0;
            row.scroll = row_scroll(row.scroll, row.focus, 3, CARD_STEP, ui.dt);
            match &row.cards {
                None => {
                    for slot in 0..5 {
                        let x = CARD_X + slot as f32 * CARD_STEP;
                        ui.skeleton(Rect::new(x, cards_y, CARD_W, CARD_H), RADIUS);
                        ui.skeleton(Rect::new(x, cards_y + CARD_H + 22.0, CARD_W * 0.72, 20.0), 6.0);
                        ui.skeleton(Rect::new(x, cards_y + CARD_H + 54.0, CARD_W * 0.46, 16.0), 6.0);
                    }
                }
                Some(cards) if cards.is_empty() => {
                    ui.gfx.text(Font::SemiBold, 28.0, PAD, cards_y + 80.0, "Vacío", DIM, None);
                }
                Some(cards) => {
                    let first = (row.scroll / CARD_STEP) as usize;
                    for (slot, card) in cards.iter().enumerate().skip(first.saturating_sub(1)).take(7) {
                        let x = CARD_X + slot as f32 * CARD_STEP - row.scroll;
                        let is_focus = focused && index == self.row && slot == row.focus;
                        ui.card(card, Rect::new(x, cards_y, CARD_W, CARD_H), is_focus, 1.0);
                    }
                    // pictures a card ahead load before they slide in
                    for card in cards.iter().skip(first + 5).take(2) {
                        if let Some(url) = &card.image {
                            ui.images.prefetch(ui.worker, url, CARD_PICTURE);
                        }
                    }
                }
            }
        }
        ui.gfx.clip(None);
    }

    /// The art of the cards beside the focus, so the hero has it when the focus moves there.
    fn prefetch_neighbours(&self, ui: &mut Ui, heroes: &HashMap<String, Hero>) {
        let Some(row) = self.rows.get(self.row) else { return };
        let Some(cards) = &row.cards else { return };
        for index in [row.focus + 1, row.focus.wrapping_sub(1)] {
            let Some(card) = cards.get(index) else { continue };
            let art = Self::art_id(card).and_then(|id| heroes.get(id));
            if let Some(url) = art.and_then(|a| a.preview.clone()) {
                ui.images.prefetch(ui.worker, &url, 640);
            }
            if let Some(url) = art.and_then(|a| a.hero.clone()) {
                ui.images.prefetch(ui.worker, &url, BACKDROP_WIDTH);
            }
            if let Some(url) = art.and_then(|a| a.logo.clone()) {
                ui.images.prefetch(ui.worker, &url, 900);
            }
        }
    }
}

/// The home backdrop's shading: dark on the left for the text, dark at the bottom for the shelf.
pub fn shade_home(ui: &mut Ui) {
    // 90deg: bg 92% at 0, 72% at 32%, 15% at 62%, 0 at 80%
    let stops = [(0.0, 0.92), (0.32, 0.72), (0.62, 0.15), (0.80, 0.0)];
    for pair in stops.windows(2) {
        let (x0, a0) = pair[0];
        let (x1, a1) = pair[1];
        ui.gfx.gradient(Rect::new(x0 * 1920.0, 0.0, (x1 - x0) * 1920.0, 1080.0), BG.alpha(a0), BG.alpha(a1), false);
    }
    // 180deg: 55% at 0, 0 at 18%, 0 at 40%, 86% at 60%, 100% at 76%
    let stops = [(0.0, 0.55), (0.18, 0.0), (0.40, 0.0), (0.60, 0.86), (0.76, 1.0), (1.0, 1.0)];
    for pair in stops.windows(2) {
        let (y0, a0) = pair[0];
        let (y1, a1) = pair[1];
        if a0 > 0.0 || a1 > 0.0 {
            ui.gfx.gradient(Rect::new(0.0, y0 * 1080.0, 1920.0, (y1 - y0) * 1080.0), BG.alpha(a0), BG.alpha(a1), true);
        }
    }
}

/// The detail backdrop's shading (`dim` for the audio view).
pub fn shade_detail(ui: &mut Ui, dim: bool) {
    if dim {
        let stops = [(0.0, 0.78), (0.45, 0.90), (0.80, 1.0), (1.0, 1.0)];
        for pair in stops.windows(2) {
            let (y0, a0) = pair[0];
            let (y1, a1) = pair[1];
            ui.gfx.gradient(Rect::new(0.0, y0 * 1080.0, 1920.0, (y1 - y0) * 1080.0), BG.alpha(a0), BG.alpha(a1), true);
        }
        return;
    }
    let stops = [(0.0, 0.95), (0.34, 0.78), (0.66, 0.20), (0.82, 0.0)];
    for pair in stops.windows(2) {
        let (x0, a0) = pair[0];
        let (x1, a1) = pair[1];
        ui.gfx.gradient(Rect::new(x0 * 1920.0, 0.0, (x1 - x0) * 1920.0, 1080.0), BG.alpha(a0), BG.alpha(a1), false);
    }
    let stops = [(0.0, 0.35), (0.16, 0.0), (0.36, 0.0), (0.56, 0.90), (0.68, 1.0), (1.0, 1.0)];
    for pair in stops.windows(2) {
        let (y0, a0) = pair[0];
        let (y1, a1) = pair[1];
        if a0 > 0.0 || a1 > 0.0 {
            ui.gfx.gradient(Rect::new(0.0, y0 * 1080.0, 1920.0, (y1 - y0) * 1080.0), BG.alpha(a0), BG.alpha(a1), true);
        }
    }
}
