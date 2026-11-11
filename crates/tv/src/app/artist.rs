//! An artist: their picture behind their name, genres and story, then their videos or concerts in a row.

use crunchy_core::model::Card;

use crate::gfx::{Font, Rect};
use crate::platform::Key;

use super::feed::DETAIL_SHADE;
use super::show::NO_PLAYER;
use super::theme::*;
use super::ui::{Backdrop, HeroSize, HeroView, Look, Ui, row_scroll};
use super::worker::{Command, Event};
use super::{Env, Nav, Page, State};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Tabs,
    List,
}

pub struct ArtistPage {
    card: Card,
    lists: Option<(Vec<Card>, Vec<Card>)>,
    /// 0 videos, 1 concerts
    tab: usize,
    zone: Zone,
    focus: usize,
    scroll: f32,
    backdrop: Backdrop,
}

const EP_W: f32 = 420.0;
const EP_H: f32 = 236.0;
const EP_STEP: f32 = 468.0;

impl ArtistPage {
    pub fn new(card: Card) -> ArtistPage {
        ArtistPage { card, lists: None, tab: 0, zone: Zone::List, focus: 0, scroll: 0.0, backdrop: Backdrop::default() }
    }

    /// The tabs there's something under: (index, label).
    fn tabs(&self) -> Vec<(usize, &'static str)> {
        let Some((videos, concerts)) = &self.lists else { return Vec::new() };
        let mut tabs = Vec::new();
        if !videos.is_empty() {
            tabs.push((0, "Vídeos"));
        }
        if !concerts.is_empty() {
            tabs.push((1, "Conciertos"));
        }
        tabs
    }

    fn list(&self) -> &[Card] {
        match &self.lists {
            Some((videos, concerts)) => {
                if self.tab == 0 {
                    videos
                } else {
                    concerts
                }
            }
            None => &[],
        }
    }
}

impl Page for ArtistPage {
    fn shown(&mut self, env: &mut Env) {
        if self.lists.is_none() {
            env.worker.send(Command::Artist(self.card.id.clone()));
        }
    }

    fn event(&mut self, event: &Event, _env: &mut Env) {
        if let Event::Artist { card, videos, concerts } = event
            && card.id == self.card.id
        {
            self.card = card.clone();
            self.lists = Some((videos.clone(), concerts.clone()));
            self.tab = if videos.is_empty() && !concerts.is_empty() { 1 } else { 0 };
        }
    }

    fn key(&mut self, key: Key, _env: &mut Env) -> Nav {
        let tabs = self.tabs();
        match self.zone {
            Zone::Tabs => match key {
                Key::Left | Key::Right => {
                    if let Some(at) = tabs.iter().position(|(i, _)| *i == self.tab) {
                        let next = if key == Key::Left { at.saturating_sub(1) } else { (at + 1).min(tabs.len() - 1) };
                        if tabs[next].0 != self.tab {
                            self.tab = tabs[next].0;
                            self.focus = 0;
                        }
                    }
                }
                Key::Down => self.zone = Zone::List,
                Key::Back => return Nav::Back,
                _ => {}
            },
            Zone::List => match key {
                Key::Left => self.focus = self.focus.saturating_sub(1),
                Key::Right => self.focus = (self.focus + 1).min(self.list().len().saturating_sub(1)),
                Key::Up if !tabs.is_empty() => self.zone = Zone::Tabs,
                Key::Ok => return Nav::Toast(NO_PLAYER.into()),
                Key::Back => return Nav::Back,
                _ => {}
            },
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, _state: &State, focused: bool) {
        let art = self.card.backdrop.clone().or(self.card.poster.clone()).or(self.card.image.clone());
        let preview = self.card.image.clone();
        ui.backdrop(&mut self.backdrop, art, preview, &DETAIL_SHADE);
        let mut card = self.card.clone();
        card.description = card.description.replace('\n', " ");
        let view = HeroView { card: &card, logo: None, logo_known: true, size: HeroSize::Compact, lines: 4, width: 1000.0 };
        ui.hero(PAD, 96.0, &view, 1.0);

        let tabs_y = 560.0;
        let mut x = PAD;
        for (index, label) in self.tabs() {
            let look = Look::new(focused && self.zone == Zone::Tabs && index == self.tab, index == self.tab);
            x += ui.pill_plain(x, tabs_y, 48.0, label, None, 22.0, look) + 6.0;
        }
        let cards_y = 640.0 + 48.0 + 6.0 + 24.0;
        if self.lists.is_none() {
            for slot in 0..4 {
                ui.skeleton(Rect::new(PAD + slot as f32 * EP_STEP, cards_y, EP_W, EP_H), RADIUS);
            }
            return;
        }
        let list = self.list().to_vec();
        if list.is_empty() {
            ui.text_centered(Font::SemiBold, 28.0, 960.0, cards_y + 80.0, "Vacío", DIM);
            return;
        }
        self.focus = self.focus.min(list.len() - 1);
        self.scroll = row_scroll(self.scroll, self.focus, 2, EP_STEP, ui.dt);
        for (index, card) in list.iter().enumerate() {
            let x = PAD + index as f32 * EP_STEP - self.scroll;
            if x < -EP_STEP || x > 1920.0 {
                continue;
            }
            ui.episode(card, Rect::new(x, cards_y, EP_W, EP_H), focused && self.zone == Zone::List && index == self.focus, 1.0);
        }
    }
}
