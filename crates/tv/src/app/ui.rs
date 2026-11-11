//! The pieces screens are built from: pills, round icon buttons, the three kinds of tile (16:9 card, episode,
//! 2:3 poster), the line of facts under a title, skeletons. Everything draws through [`Ui`], which carries the
//! renderer, the picture cache and the worker for one frame.

use crunchy_core::model::{BACKDROP_WIDTH, Card, Kind};

use crate::gfx::{Color, Font, Icon, Rect, Renderer};

use super::images::Images;
use super::theme::*;
use super::worker::Worker;

/// Width pictures on cards are decoded to.
pub const CARD_PICTURE: u32 = 480;

pub struct Ui<'a> {
    pub gfx: &'a mut Renderer,
    pub images: &'a mut Images,
    pub worker: &'a Worker,
    /// seconds since the last frame
    pub dt: f32,
    /// seconds since launch, for pulses
    pub t: f32,
    /// the account pays: no Premium badges
    pub premium: bool,
}

/// How a control looks: with the focus, chosen (the current tab, the season shown), or neither.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    pub focused: bool,
    pub selected: bool,
}

impl Look {
    pub fn new(focused: bool, selected: bool) -> Look {
        Look { focused, selected }
    }
}

impl Ui<'_> {
    /// A soft pulse for placeholders.
    pub fn pulse(&self) -> f32 {
        0.55 + 0.45 * (0.5 + 0.5 * (self.t * 3.5).sin())
    }

    pub fn skeleton(&mut self, r: Rect, radius: f32) {
        let a = self.pulse();
        self.gfx.rect(r, radius, SURFACE.alpha(a));
    }

    /// A picture filling `r`, or a placeholder until it arrives. Returns whether it drew the picture.
    pub fn picture(&mut self, url: Option<&str>, r: Rect, radius: f32, alpha: f32, width: u32) -> bool {
        match url.and_then(|url| self.images.get(self.worker, url, width)) {
            Some(texture) => {
                self.gfx.image(&texture, r, radius, alpha);
                true
            }
            None => {
                self.gfx.rect(r, radius, SURFACE.alpha(alpha));
                false
            }
        }
    }

    /// A picture kept at its own aspect inside `max` (logos), anchored at the box's left and bottom. Returns the
    /// rectangle drawn, or `None` while it loads.
    pub fn contained(&mut self, url: &str, max: Rect, width: u32, alpha: f32) -> Option<Rect> {
        let texture = self.images.get(self.worker, url, width)?;
        let aspect = texture.width as f32 / texture.height.max(1) as f32;
        let (mut w, mut h) = (max.w, max.w / aspect);
        if h > max.h {
            h = max.h;
            w = h * aspect;
        }
        let r = Rect::new(max.x, max.y + max.h - h, w, h);
        self.gfx.image(&texture, r, 0.0, alpha);
        Some(r)
    }

    pub fn text_centered(&mut self, font: Font, size: f32, cx: f32, y: f32, text: &str, color: Color) {
        let w = self.gfx.measure(font, size, text);
        self.gfx.text(font, size, cx - w / 2.0, y, text, color, None);
    }

    /// A rounded pill with a label (and maybe an icon before it), `h` tall. Returns its width.
    pub fn pill(&mut self, x: f32, y: f32, h: f32, label: &str, icon: Option<Icon>, size: f32, look: Look) -> f32 {
        let pad = h * 0.42;
        let icon_w = if icon.is_some() { size * 1.1 + 12.0 } else { 0.0 };
        let w = pad * 2.0 + icon_w + self.gfx.measure(Font::SemiBold, size, label);
        self.pill_box(Rect::new(x, y, w, h), label, icon, size, look, true);
        w
    }

    /// A pill without a resting fill (text on the background until it's chosen or focused).
    pub fn pill_plain(&mut self, x: f32, y: f32, h: f32, label: &str, icon: Option<Icon>, size: f32, look: Look) -> f32 {
        let pad = h * 0.42;
        let icon_w = if icon.is_some() { size * 1.1 + 12.0 } else { 0.0 };
        let w = pad * 2.0 + icon_w + self.gfx.measure(Font::SemiBold, size, label);
        self.pill_box(Rect::new(x, y, w, h), label, icon, size, look, false);
        w
    }

    fn pill_box(&mut self, r: Rect, label: &str, icon: Option<Icon>, size: f32, look: Look, filled: bool) {
        let (fill, ink) = if look.focused {
            (Some(TEXT), BG)
        } else if look.selected {
            (Some(VEIL_STRONG), TEXT)
        } else if filled {
            (Some(VEIL), TEXT)
        } else {
            (None, MUTED)
        };
        let r = if look.focused { r.scaled(1.04) } else { r };
        if let Some(fill) = fill {
            self.gfx.rect(r, r.h / 2.0, fill);
        }
        let mut x = r.x + r.h * 0.42;
        let ascent_fix = size * 0.62;
        if let Some(icon) = icon {
            self.gfx.icon(icon, x, r.y + (r.h - size * 1.1) / 2.0, size * 1.1, ink);
            x += size * 1.1 + 12.0;
        }
        self.gfx.text(Font::SemiBold, size, x, r.y + r.h / 2.0 - ascent_fix, label, ink, None);
    }

    /// A round button with an icon, centred on `cx`, `cy`.
    pub fn circle_button(&mut self, cx: f32, cy: f32, d: f32, icon: Icon, look: Look) {
        let d = if look.focused { d * 1.08 } else { d };
        let (fill, ink) = if look.focused {
            (TEXT, BG)
        } else if look.selected {
            (VEIL_STRONG, TEXT)
        } else {
            (VEIL, TEXT)
        };
        self.gfx.rect(Rect::new(cx - d / 2.0, cy - d / 2.0, d, d), d / 2.0, fill);
        self.gfx.icon_centered(icon, cx, cy, d * 0.44, ink);
    }

    /// The facts under a hero's title: a maturity badge, then year · seasons · episodes · Sub | Dob · ★ 4,8.
    /// Returns the width drawn.
    pub fn meta_line(&mut self, card: &Card, x: f32, y: f32, size: f32, alpha: f32) -> f32 {
        let mut cx = x;
        let line_h = size * 1.6;
        if let Some(maturity) = card.maturity.as_deref().filter(|m| !m.is_empty()) {
            let label = if maturity.chars().all(|c| c.is_ascii_digit()) { format!("{maturity}+") } else { maturity.to_string() };
            let w = self.gfx.measure(Font::SemiBold, size * 0.82, &label) + 22.0;
            let badge = Rect::new(cx, y, w, line_h);
            self.gfx.rect(badge, 7.0, MUTED.alpha(0.9 * alpha));
            self.gfx.rect(badge.inset(2.0), 5.0, BG.alpha(alpha));
            self.gfx.text(Font::SemiBold, size * 0.82, cx + 11.0, y + line_h / 2.0 - size * 0.52, &label, TEXT.alpha(alpha), None);
            cx += w;
        }
        let mut parts: Vec<String> = Vec::new();
        match card.kind {
            Kind::Episode | Kind::Movie | Kind::MusicVideo | Kind::Concert => {
                if let Some(year) = card.year.or_else(|| card.date.as_ref().and_then(|d| d.get(0..4)?.parse().ok())) {
                    parts.push(year.to_string());
                }
                if let Some(duration) = card.episode.as_ref().map(|e| e.duration_secs).filter(|d| *d > 0) {
                    parts.push(format!("{} min", ((duration as f32 / 60.0).round() as u32).max(1)));
                }
            }
            _ => {
                if let Some(year) = card.year {
                    parts.push(year.to_string());
                }
                if let Some(seasons) = card.season_count.filter(|n| *n > 0 && card.kind == Kind::Series) {
                    parts.push(seasons_text(seasons));
                }
                if let Some(episodes) = card.episode_count.filter(|n| *n > 0) {
                    parts.push(episodes_text(episodes));
                }
            }
        }
        if let Some(audio) = audio_text(card) {
            parts.push(audio.to_string());
        }
        let text_y = y + line_h / 2.0 - size * 0.62;
        for part in &parts {
            if cx > x {
                cx += self.dot(cx, y + line_h / 2.0, alpha);
            }
            cx += self.gfx.text(Font::Regular, size, cx, text_y, part, MUTED.alpha(alpha), None);
        }
        if let Some(rating) = card.rating {
            if cx > x {
                cx += self.dot(cx, y + line_h / 2.0, alpha);
            }
            self.gfx.icon(Icon::Star, cx, y + (line_h - size) / 2.0, size, GOLD.alpha(alpha));
            cx += size + 8.0;
            cx += self.gfx.text(Font::SemiBold, size, cx, text_y, &rating_text(rating), GOLD.alpha(alpha), None);
        }
        cx - x
    }

    fn dot(&mut self, x: f32, cy: f32, alpha: f32) -> f32 {
        self.gfx.rect(Rect::new(x + 14.0, cy - 2.5, 5.0, 5.0), 2.5, DIM.alpha(alpha));
        33.0
    }

    /// The orange eyebrow over a hero ("SERIES"). Returns its height.
    pub fn eyebrow(&mut self, text: &str, x: f32, y: f32, alpha: f32) -> f32 {
        let mut cx = x;
        for ch in text.chars() {
            cx += self.gfx.text(Font::Bold, 19.0, cx, y, &ch.to_string(), ACCENT.alpha(alpha), None) + 2.2;
        }
        28.0
    }

    /// A 16:9 tile with its title (and a second line) under it: home rows.
    pub fn card(&mut self, card: &Card, r: Rect, focused: bool, alpha: f32) {
        let art = if focused { r.scaled(1.08) } else { r };
        if focused {
            self.gfx.rect(art.inset(-4.0), RADIUS + 4.0, TEXT.alpha(alpha));
        }
        self.picture(card.image.as_deref(), art, RADIUS, alpha, CARD_PICTURE);
        self.progress(card, art, alpha);
        if card.premium_only && !self.premium {
            self.premium_badge(art, alpha);
        }
        let text_y = r.y + r.h + 18.0;
        let title_color = if focused { TEXT } else { Color(0.96, 0.96, 0.97, 0.92) };
        self.gfx.text(Font::SemiBold, 22.0, r.x, text_y, &card.title, title_color.alpha(alpha), Some(r.w));
        let subtitle = card_subtitle(card);
        if !subtitle.is_empty() {
            self.gfx.text(Font::Regular, 19.0, r.x, text_y + 32.0, &subtitle, DIM.alpha(alpha), Some(r.w));
        }
    }

    fn premium_badge(&mut self, art: Rect, alpha: f32) {
        let w = self.gfx.measure(Font::Bold, 16.0, "Premium") + 20.0;
        let badge = Rect::new(art.x + 12.0, art.y + 12.0, w, 28.0);
        self.gfx.rect(badge, 8.0, Color(0.0, 0.0, 0.0, 0.66 * alpha));
        self.gfx.text(Font::Bold, 16.0, badge.x + 10.0, badge.y + 4.0, "Premium", GOLD.alpha(alpha), None);
    }

    fn progress(&mut self, card: &Card, art: Rect, alpha: f32) {
        if let Some(progress) = card.progress.filter(|p| *p > 0.0) {
            let bar = Rect::new(art.x + 16.0, art.y + art.h - 20.0, art.w - 32.0, 6.0);
            self.gfx.rect(bar, 3.0, Color(1.0, 1.0, 1.0, 0.3 * alpha));
            self.gfx.rect(Rect::new(bar.x, bar.y, (bar.w * progress).max(6.0), bar.h), 3.0, ACCENT.alpha(alpha));
        }
    }

    /// An episode or movie: its thumbnail (duration, watched check, progress), then "1 Name" and two lines of
    /// synopsis.
    pub fn episode(&mut self, card: &Card, r: Rect, focused: bool, alpha: f32) {
        let art = if focused { r.scaled(1.08) } else { r };
        if focused {
            self.gfx.rect(art.inset(-4.0), RADIUS + 4.0, TEXT.alpha(alpha));
        }
        self.picture(card.image.as_deref(), art, RADIUS, alpha, CARD_PICTURE);
        let episode = card.episode.clone().unwrap_or_default();
        if episode.duration_secs > 0 {
            let label = format!("{} min", ((episode.duration_secs as f32 / 60.0).round() as u32).max(1));
            let w = self.gfx.measure(Font::SemiBold, 16.0, &label) + 16.0;
            let badge = Rect::new(art.x + art.w - w - 14.0, art.y + art.h - 30.0 - 24.0, w, 24.0);
            self.gfx.rect(badge, 6.0, Color(0.0, 0.0, 0.0, 0.66 * alpha));
            self.gfx.text(Font::SemiBold, 16.0, badge.x + 8.0, badge.y + 2.0, &label, TEXT.alpha(alpha), None);
        }
        if card.watched {
            let d = 34.0;
            let c = Rect::new(art.x + art.w - d - 12.0, art.y + 12.0, d, d);
            self.gfx.rect(c, d / 2.0, Color(0.0, 0.0, 0.0, 0.62 * alpha));
            self.gfx.icon_centered(Icon::Check, c.x + d / 2.0, c.y + d / 2.0, 20.0, TEXT.alpha(alpha));
        }
        if card.premium_only && !self.premium {
            self.premium_badge(art, alpha);
        }
        self.progress(card, art, alpha);
        let text_y = r.y + r.h + 18.0;
        let mut x = r.x;
        let number = if card.kind == Kind::Episode && episode.number != "0" { episode.number.as_str() } else { "" };
        if !number.is_empty() {
            x += self.gfx.text(Font::SemiBold, 22.0, x, text_y, number, DIM.alpha(alpha), None) + 10.0;
        }
        let name = if episode.name.is_empty() { card.title.clone() } else { episode.name.clone() };
        let title_color = if focused { TEXT } else { Color(0.96, 0.96, 0.97, 0.92) };
        self.gfx.text(Font::SemiBold, 22.0, x, text_y, &name, title_color.alpha(alpha), Some(r.x + r.w - x));
        let synopsis = card.description.replace('\n', " ");
        self.gfx.paragraph(Font::Regular, 18.0, r.x, text_y + 36.0, &synopsis, DIM.alpha(alpha), r.w, 2, 25.0);
    }

    /// A full-screen backdrop that crossfades when it changes. Until the full picture is in, `preview` (a smaller
    /// one already loaded, like the card's own) stands in for it.
    pub fn backdrop(&mut self, fade: &mut Backdrop, wanted: Option<String>, preview: Option<String>, alpha: f32) {
        if let Some(url) = wanted
            && fade.next.as_ref().map(|(next, _)| next) != Some(&url)
        {
            if fade.amount >= 1.0 {
                fade.current = fade.next.take();
            }
            fade.next = Some((url, preview));
            fade.amount = 0.0;
        }
        let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        if let Some(layer) = fade.current.clone()
            && let Some(texture) = self.layer(&layer)
        {
            self.gfx.image(&texture, screen, 0.0, alpha);
        }
        if let Some(layer) = fade.next.clone()
            && let Some(texture) = self.layer(&layer)
        {
            fade.amount = (fade.amount + self.dt * 2.5).min(1.0);
            self.gfx.image(&texture, screen, 0.0, fade.amount * alpha);
        }
    }

    fn layer(&mut self, (url, preview): &(String, Option<String>)) -> Option<crate::gfx::Texture> {
        self.images.get(self.worker, url, BACKDROP_WIDTH).or_else(|| preview.as_ref().and_then(|p| self.images.get(self.worker, p, 640)))
    }
}

/// The backdrop shown and the one fading in over it, each with the picture that stands in while it loads.
#[derive(Default)]
pub struct Backdrop {
    current: Option<(String, Option<String>)>,
    next: Option<(String, Option<String>)>,
    amount: f32,
}

/// A horizontal list that keeps the focus in place until `keep` items in, then slides.
pub fn row_scroll(current: f32, focus: usize, keep: usize, step: f32, dt: f32) -> f32 {
    approach(current, focus.saturating_sub(keep) as f32 * step, dt, 16.0)
}

/// The three sizes of the block of text over art (the web app's HeroInfo).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HeroSize {
    Home,
    Detail,
    Compact,
}

/// What the hero shows.
pub struct HeroView<'a> {
    pub card: &'a Card,
    /// the show's logo, when it has one
    pub logo: Option<&'a str>,
    /// false while it isn't known yet whether there's a logo (the title waits, so it doesn't flash)
    pub logo_known: bool,
    pub size: HeroSize,
    pub lines: usize,
    pub width: f32,
}

impl Ui<'_> {
    /// Draws the hero block from its top-left; returns the y under it.
    pub fn hero(&mut self, x: f32, y: f32, view: &HeroView, alpha: f32) -> f32 {
        let card = view.card;
        let compact = view.size == HeroSize::Compact;
        let episode_name = match card.kind {
            Kind::Episode => card.episode.as_ref().map(|e| e.name.clone()).filter(|n| !n.is_empty() && *n != card.title),
            _ => None,
        };
        let mut y = y;
        let logo_url = view.logo.filter(|url| !self.images.is_failed(url));
        let show_logo = logo_url.is_some() || !view.logo_known;
        if !show_logo && view.size != HeroSize::Home {
            let mut text = eyebrow(card.kind).to_string();
            if card.kind == Kind::Episode {
                text = "EPISODIO".into();
            }
            y += self.eyebrow(&text, x, y, alpha) - 6.0;
        }
        let (logo_h, logo_w) = match (view.size, episode_name.is_some()) {
            (HeroSize::Home, true) => (130.0, 640.0),
            (HeroSize::Home, false) => (170.0, 640.0),
            (HeroSize::Detail, _) => (190.0, 720.0),
            (HeroSize::Compact, _) => (110.0, 440.0),
        };
        if show_logo {
            y += 14.0;
            if let Some(url) = logo_url {
                self.contained(url, Rect::new(x, y, logo_w, logo_h), 900, alpha);
            }
            y += logo_h;
        } else {
            let size = match (view.size, episode_name.is_some()) {
                (HeroSize::Home, true) => 60.0,
                (HeroSize::Home, false) => 68.0,
                (HeroSize::Detail, _) => 76.0,
                (HeroSize::Compact, _) => 52.0,
            };
            y += if compact { 8.0 } else { 12.0 };
            let lines = self.gfx.wrap(Font::Bold, size, &card.title, view.width, 2);
            for line in &lines {
                self.gfx.text(Font::Bold, size, x, y, line, TEXT.alpha(alpha), Some(view.width));
                y += size * 1.04;
            }
        }
        let mut lines = view.lines;
        if let Some(name) = &episode_name {
            let size = if compact { 25.0 } else { 30.0 };
            y += 12.0;
            y += self.gfx.paragraph(Font::SemiBold, size, x, y, name, TEXT.alpha(alpha), view.width.min(900.0), 2, size * 1.25);
            lines = lines.saturating_sub(1).max(1);
        }
        let subtitle = match card.kind {
            Kind::Episode => card.episode.as_ref().map(|e| {
                if e.number.is_empty() { format!("T{}", e.season) } else { format!("T{} · E{}", e.season, e.number) }
            }),
            Kind::MusicVideo | Kind::Concert => card.artist.clone(),
            _ => None,
        };
        if let Some(subtitle) = subtitle {
            if card.kind == Kind::Episode {
                y += 4.0;
                self.gfx.text(Font::Regular, 24.0, x, y, &subtitle, MUTED.alpha(alpha), Some(view.width));
                y += 32.0;
            } else {
                y += 12.0;
                self.gfx.text(Font::SemiBold, 28.0, x, y, &subtitle, TEXT.alpha(0.86 * alpha), Some(view.width));
                y += 36.0;
            }
        }
        let meta_size = if compact { 20.0 } else { 22.0 };
        y += if compact { 14.0 } else { 20.0 };
        self.meta_line(card, x, y, meta_size, alpha);
        y += meta_size * 1.6;
        if !card.description.is_empty() && lines > 0 {
            let (size, line_h) = if compact { (22.0, 31.0) } else { (25.0, 36.0) };
            y += if compact { 14.0 } else { 20.0 };
            let text = card.description.replace('\n', " ");
            y += self.gfx.paragraph(Font::Regular, size, x, y, &text, MUTED.alpha(alpha), view.width, lines, line_h);
        }
        y
    }

    /// "▬▬▬ Quedan 12 min" under a hero, for something half watched.
    pub fn resume_line(&mut self, card: &Card, x: f32, y: f32, width: f32, alpha: f32) -> bool {
        let Some(progress) = card.progress.filter(|p| *p > 0.0 && *p < 0.98) else { return false };
        let Some(duration) = card.episode.as_ref().map(|e| e.duration_secs).filter(|d| *d > 0) else { return false };
        let left = ((duration as f32 / 60.0) * (1.0 - progress)).round().max(1.0);
        self.gfx.rect(Rect::new(x, y + 9.0, width, 6.0), 3.0, Color(1.0, 1.0, 1.0, 0.24 * alpha));
        self.gfx.rect(Rect::new(x, y + 9.0, width * progress, 6.0), 3.0, TEXT.alpha(alpha));
        self.gfx.text(Font::SemiBold, 21.0, x + width + 16.0, y, &format!("Quedan {left} min"), MUTED.alpha(alpha), None);
        true
    }
}
