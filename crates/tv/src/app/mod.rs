//! The app: a screen at a time (start, sign-in code, profiles, home), driven by the remote and by what the worker
//! sends back. Drawing is immediate mode: each frame draws the current screen from its state.

mod images;
mod theme;
pub mod worker;

use std::time::Instant;

use crunchy_core::catalog::RowSpec;
use crunchy_core::model::{BACKDROP_WIDTH, Card, Kind};

use crate::gfx::{Font, Rect, Renderer};
use crate::platform::{self, Key};
use images::Images;
use theme::*;
use worker::{Command, Event, ProfileInfo, Worker};

/// Pixel width pictures are decoded to.
const CARD_PICTURE: u32 = 480;
const AVATAR_PICTURE: u32 = 240;

enum Screen {
    Starting,
    SignIn { code: Option<String>, expired: bool },
    Profiles { list: Vec<ProfileInfo>, focus: usize },
    Home(Home),
}

struct Row {
    spec: RowSpec,
    title: String,
    cards: Option<Vec<Card>>,
    requested: bool,
    focus: usize,
    /// horizontal scroll in px, eased toward the focus
    scroll: f32,
}

struct Home {
    rows: Vec<Row>,
    row: usize,
    /// vertical position, in rows, eased toward `row`
    row_pos: f32,
    /// the backdrop shown, and the one fading in
    backdrop: Option<String>,
    backdrop_next: Option<String>,
    fade: f32,
}

pub struct CrunchyApp {
    worker: Worker,
    images: Images,
    screen: Screen,
    message: Option<(String, Instant)>,
    last: Instant,
    started: Instant,
}

impl CrunchyApp {
    pub fn new() -> CrunchyApp {
        let dir = platform::data_dir();
        eprintln!("crunchy: data in {}", dir.display());
        let worker = Worker::start(dir);
        worker.send(Command::Start);
        CrunchyApp {
            worker,
            images: Images::default(),
            screen: Screen::Starting,
            message: None,
            last: Instant::now(),
            started: Instant::now(),
        }
    }

    fn handle_event(&mut self, gfx: &mut Renderer, event: Event) {
        match event {
            Event::NeedsSignIn => {
                self.screen = Screen::SignIn { code: None, expired: false };
                self.worker.send(Command::RequestCode);
            }
            Event::Code { user_code } => {
                if let Screen::SignIn { code, expired } = &mut self.screen {
                    *code = Some(user_code);
                    *expired = false;
                }
            }
            Event::CodeExpired => {
                if let Screen::SignIn { expired, .. } = &mut self.screen {
                    *expired = true;
                }
            }
            Event::Profiles(list) => {
                if list.len() == 1 {
                    self.worker.send(Command::SelectProfile(list[0].id.clone()));
                }
                self.screen = Screen::Profiles { list, focus: 0 };
            }
            Event::Home(specs) => {
                let rows = specs
                    .into_iter()
                    .map(|spec| Row { title: row_title(&spec), spec, cards: None, requested: false, focus: 0, scroll: 0.0 })
                    .collect();
                self.screen = Screen::Home(Home {
                    rows,
                    row: 0,
                    row_pos: 0.0,
                    backdrop: None,
                    backdrop_next: None,
                    fade: 1.0,
                });
            }
            Event::Row(index, cards) => {
                if let Screen::Home(home) = &mut self.screen
                    && let Some(row) = home.rows.get_mut(index)
                {
                    row.cards = Some(cards);
                }
            }
            Event::Image { url, width, height, rgba } => self.images.arrived(gfx, url, width, height, &rgba),
            Event::ImageFailed(url) => self.images.failed(url),
            Event::Error(message) => self.message = Some((message, Instant::now())),
        }
    }

    /// Returns `false` to quit.
    fn handle_key(&mut self, key: Key) -> bool {
        match &mut self.screen {
            Screen::Starting => key != Key::Back,
            Screen::SignIn { expired, .. } => match key {
                Key::Back => false,
                Key::Ok if *expired => {
                    self.screen = Screen::SignIn { code: None, expired: false };
                    self.worker.send(Command::RequestCode);
                    true
                }
                _ => true,
            },
            Screen::Profiles { list, focus } => {
                match key {
                    Key::Left => *focus = focus.saturating_sub(1),
                    Key::Right => *focus = (*focus + 1).min(list.len().saturating_sub(1)),
                    Key::Ok => {
                        if let Some(profile) = list.get(*focus) {
                            self.worker.send(Command::SelectProfile(profile.id.clone()));
                        }
                    }
                    Key::Back => return false,
                    _ => {}
                }
                true
            }
            Screen::Home(home) => {
                match key {
                    Key::Up => home.row = home.row.saturating_sub(1),
                    Key::Down => home.row = (home.row + 1).min(home.rows.len().saturating_sub(1)),
                    Key::Left | Key::Right => {
                        if let Some(row) = home.rows.get_mut(home.row) {
                            let len = row.cards.as_ref().map(|cards| cards.len()).unwrap_or(0);
                            row.focus = if key == Key::Left {
                                row.focus.saturating_sub(1)
                            } else {
                                (row.focus + 1).min(len.saturating_sub(1))
                            };
                        }
                    }
                    Key::Back => {
                        if home.rows.get(home.row).is_some_and(|row| row.focus > 0) {
                            home.rows[home.row].focus = 0;
                        } else if home.row > 0 {
                            home.row = 0;
                        } else {
                            self.screen = Screen::Starting;
                            self.worker.send(Command::Start);
                        }
                    }
                    _ => {}
                }
                true
            }
        }
    }
}

impl platform::App for CrunchyApp {
    fn frame(&mut self, gfx: &mut Renderer, keys: &[Key]) -> bool {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.images.next_frame();
        while let Ok(event) = self.worker.events.try_recv() {
            self.handle_event(gfx, event);
        }
        for key in keys {
            if !self.handle_key(*key) {
                return false;
            }
        }

        gfx.begin(BG);
        let t = (now - self.started).as_secs_f32();
        match &mut self.screen {
            Screen::Starting => draw_starting(gfx, t),
            Screen::SignIn { code, expired } => draw_sign_in(gfx, code.as_deref(), *expired, t),
            Screen::Profiles { list, focus } => draw_profiles(gfx, &mut self.images, &self.worker, list, *focus),
            Screen::Home(home) => draw_home(gfx, &mut self.images, &self.worker, home, dt, t),
        }
        if let Some((message, at)) = &self.message {
            if now.duration_since(*at).as_secs() < 6 {
                let r = Rect::new(PAD, 1080.0 - 110.0, 1920.0 - 2.0 * PAD, 64.0);
                gfx.rect(r, 32.0, SURFACE);
                gfx.text(Font::Regular, 22.0, r.x + 28.0, r.y + 18.0, message, MUTED, Some(r.w - 56.0));
            } else {
                self.message = None;
            }
        }
        gfx.end();
        true
    }
}

fn brand(gfx: &mut Renderer) {
    gfx.text(Font::Bold, 34.0, PAD, 44.0, "crunchyroll", ACCENT, None);
}

/// A soft pulse for placeholders.
fn pulse(t: f32) -> f32 {
    0.55 + 0.45 * (0.5 + 0.5 * (t * 3.5).sin())
}

fn draw_starting(gfx: &mut Renderer, t: f32) {
    brand(gfx);
    let a = pulse(t);
    gfx.rect(Rect::new(PAD, 200.0, 560.0, 64.0), 12.0, SURFACE.alpha(a));
    gfx.rect(Rect::new(PAD, 290.0, 780.0, 24.0), 8.0, SURFACE.alpha(a));
    gfx.rect(Rect::new(PAD, 330.0, 700.0, 24.0), 8.0, SURFACE.alpha(a));
    for index in 0..5 {
        gfx.rect(Rect::new(PAD + index as f32 * CARD_STEP, 690.0, CARD_W, CARD_H), RADIUS, SURFACE.alpha(a));
    }
}

fn draw_sign_in(gfx: &mut Renderer, code: Option<&str>, expired: bool, t: f32) {
    brand(gfx);
    gfx.text(Font::Bold, 64.0, PAD, 220.0, "Inicia sesión", TEXT, None);
    gfx.text(Font::Regular, 30.0, PAD, 330.0, "1. En tu móvil o computadora, entra en", MUTED, None);
    gfx.text(Font::SemiBold, 40.0, PAD, 380.0, crunchy_core::auth::ACTIVATE_URL, TEXT, None);
    gfx.text(Font::Regular, 30.0, PAD, 470.0, "2. Escribe este código:", MUTED, None);
    match code {
        Some(code) if !expired => {
            let mut x = PAD;
            for ch in code.to_uppercase().chars() {
                let r = Rect::new(x, 530.0, 96.0, 120.0);
                gfx.rect(r, 18.0, SURFACE);
                let s = ch.to_string();
                let w = gfx.measure(Font::Bold, 64.0, &s);
                gfx.text(Font::Bold, 64.0, r.x + (r.w - w) / 2.0, r.y + 22.0, &s, TEXT, None);
                x += 112.0;
            }
            gfx.text(Font::Regular, 24.0, PAD, 690.0, "La pantalla avanzará sola en cuanto lo escribas.", MUTED, None);
        }
        Some(_) => {
            gfx.text(Font::SemiBold, 30.0, PAD, 560.0, "El código caducó. Pulsa OK para pedir otro.", TEXT, None);
        }
        None => {
            let a = pulse(t);
            for index in 0..6 {
                gfx.rect(Rect::new(PAD + index as f32 * 112.0, 530.0, 96.0, 120.0), 18.0, SURFACE.alpha(a));
            }
        }
    }
}

fn draw_profiles(gfx: &mut Renderer, images: &mut Images, worker: &Worker, list: &[ProfileInfo], focus: usize) {
    let title = "¿Quién está viendo?";
    let w = gfx.measure(Font::Bold, 56.0, title);
    gfx.text(Font::Bold, 56.0, (1920.0 - w) / 2.0, 260.0, title, TEXT, None);
    let size = if list.len() > 3 { 240.0 } else { 300.0 };
    let step = size + 80.0;
    let total = list.len() as f32 * step - 80.0;
    let mut x = (1920.0 - total) / 2.0;
    for (index, profile) in list.iter().enumerate() {
        let mut r = Rect::new(x, 400.0, size, size);
        if index == focus {
            r = r.scaled(1.08);
            gfx.rect(r.inset(-8.0), size, TEXT);
        }
        match profile.avatar.as_deref().and_then(|url| images.get(worker, url, AVATAR_PICTURE * 2)) {
            Some(texture) => gfx.image(&texture, r, r.w / 2.0, 1.0),
            None => gfx.rect(r, r.w / 2.0, SURFACE),
        }
        let name_w = gfx.measure(Font::SemiBold, 34.0, &profile.name);
        gfx.text(Font::SemiBold, 34.0, x + (size - name_w) / 2.0, 400.0 + size + 44.0, &profile.name, TEXT, Some(step));
        x += step;
    }
}

fn draw_home(gfx: &mut Renderer, images: &mut Images, worker: &Worker, home: &mut Home, dt: f32, t: f32) {
    // rows near the one in view load their cards
    for index in home.row.saturating_sub(1)..(home.row + 3).min(home.rows.len()) {
        let row = &mut home.rows[index];
        if !row.requested {
            row.requested = true;
            worker.send(Command::LoadRow(index, row.spec.clone()));
        }
    }
    home.row_pos = approach(home.row_pos, home.row as f32, dt, 14.0);

    let focused: Option<Card> = home
        .rows
        .get(home.row)
        .and_then(|row| row.cards.as_ref())
        .and_then(|cards| cards.get(home.rows[home.row].focus))
        .cloned();

    // backdrop: the focused card's artwork, crossfading when it changes
    let wanted = focused.as_ref().and_then(|card| card.backdrop.clone().or(card.image.clone()));
    if wanted != home.backdrop_next && wanted.is_some() {
        if home.fade >= 1.0 {
            home.backdrop = home.backdrop_next.take();
        }
        home.backdrop_next = wanted;
        home.fade = 0.0;
    }
    let screen = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    if let Some(url) = home.backdrop.clone()
        && let Some(texture) = images.get(worker, &url, BACKDROP_WIDTH)
    {
        gfx.image(&texture, screen, 0.0, 1.0);
    }
    if let Some(url) = home.backdrop_next.clone()
        && let Some(texture) = images.get(worker, &url, BACKDROP_WIDTH)
    {
        home.fade = (home.fade + dt * 3.0).min(1.0);
        gfx.image(&texture, screen, 0.0, home.fade);
    }
    gfx.gradient(Rect::new(0.0, 0.0, 1300.0, 1080.0), BG.alpha(0.94), BG.alpha(0.0), false);
    gfx.gradient(Rect::new(0.0, 520.0, 1920.0, 260.0), BG.alpha(0.0), BG.alpha(0.96), true);
    gfx.rect(Rect::new(0.0, 780.0, 1920.0, 300.0), 0.0, BG.alpha(0.96));
    brand(gfx);

    // hero: what's focused
    if let Some(card) = &focused {
        gfx.text(Font::Bold, 64.0, PAD, 170.0, &card.title, TEXT, Some(1000.0));
        let mut y = 260.0;
        let subtitle = card_subtitle(card);
        if !subtitle.is_empty() {
            gfx.text(Font::SemiBold, 28.0, PAD, y, &subtitle, TEXT.alpha(0.86), Some(1000.0));
            y += 52.0;
        }
        gfx.paragraph(Font::Regular, 25.0, PAD, y, &card.description, MUTED, 880.0, 3, 36.0);
    } else {
        let a = pulse(t);
        gfx.rect(Rect::new(PAD, 170.0, 560.0, 64.0), 12.0, SURFACE.alpha(a));
        gfx.rect(Rect::new(PAD, 260.0, 700.0, 24.0), 8.0, SURFACE.alpha(a));
    }

    // rows: the current one at the bottom, the next one peeking in as it slides up
    let base_y = 640.0;
    let row_gap = 400.0;
    for (index, row) in home.rows.iter_mut().enumerate() {
        let offset = (index as f32 - home.row_pos) * row_gap;
        let y = base_y + offset;
        if !(-200.0..1080.0).contains(&y) {
            continue;
        }
        let alpha = (1.0 - (index as f32 - home.row_pos).abs()).clamp(0.0, 1.0).max(if index == home.row { 1.0 } else { 0.0 });
        if alpha <= 0.01 {
            continue;
        }
        gfx.text(Font::SemiBold, 30.0, PAD, y, &row.title, TEXT.alpha(alpha), None);
        let cards_y = y + 56.0;
        // the focus stays in place until the fourth slot, then the row slides
        let target = row.focus.saturating_sub(3) as f32 * CARD_STEP;
        row.scroll = approach(row.scroll, target, dt, 16.0);
        match &row.cards {
            None => {
                let a = pulse(t) * alpha;
                for slot in 0..5 {
                    gfx.rect(Rect::new(PAD + slot as f32 * CARD_STEP, cards_y, CARD_W, CARD_H), RADIUS, SURFACE.alpha(a));
                }
            }
            Some(cards) if cards.is_empty() => {
                gfx.text(Font::Regular, 24.0, PAD, cards_y + 20.0, "No hay nada aquí todavía.", MUTED.alpha(alpha), None);
            }
            Some(cards) => {
                let first = (row.scroll / CARD_STEP) as usize;
                for (slot, card) in cards.iter().enumerate().skip(first.saturating_sub(1)).take(7) {
                    let x = PAD + slot as f32 * CARD_STEP - row.scroll;
                    if x > 1920.0 {
                        break;
                    }
                    let is_focus = index == home.row && slot == row.focus;
                    draw_card(gfx, images, worker, card, Rect::new(x, cards_y, CARD_W, CARD_H), is_focus, alpha);
                }
                // pictures one card ahead load before they slide in
                for card in cards.iter().skip(first + 5).take(2) {
                    if let Some(url) = &card.image {
                        images.prefetch(worker, url, CARD_PICTURE);
                    }
                }
            }
        }
    }
}

fn draw_card(gfx: &mut Renderer, images: &mut Images, worker: &Worker, card: &Card, r: Rect, focused: bool, alpha: f32) {
    let art = if focused { r.scaled(1.08) } else { r };
    if focused {
        gfx.rect(art.inset(-6.0), RADIUS + 6.0, TEXT.alpha(alpha));
    }
    match card.image.as_deref().and_then(|url| images.get(worker, url, CARD_PICTURE)) {
        Some(texture) => gfx.image(&texture, art, RADIUS, alpha),
        None => gfx.rect(art, RADIUS, SURFACE.alpha(alpha)),
    }
    if let Some(progress) = card.progress.filter(|p| *p > 0.0 && *p < 1.0) {
        let bar = Rect::new(art.x + 16.0, art.y + art.h - 22.0, art.w - 32.0, 6.0);
        gfx.rect(bar, 3.0, Color(1.0, 1.0, 1.0, 0.35 * alpha));
        gfx.rect(Rect::new(bar.x, bar.y, bar.w * progress, bar.h), 3.0, ACCENT.alpha(alpha));
    }
    let text_y = r.y + r.h + if focused { 30.0 } else { 18.0 };
    gfx.text(Font::SemiBold, 24.0, r.x, text_y, &card.title, TEXT.alpha(alpha), Some(r.w));
    let subtitle = card_subtitle(card);
    if !subtitle.is_empty() && card.kind != Kind::Season {
        gfx.text(Font::Regular, 20.0, r.x, text_y + 34.0, &subtitle, MUTED.alpha(alpha), Some(r.w));
    }
}

use crate::gfx::Color;
