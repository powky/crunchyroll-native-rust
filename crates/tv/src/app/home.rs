//! Inicio: Crunchyroll's home feed as a [`Feed`], "continue watching" first or second as the settings say.

use std::collections::HashMap;

use crunchy_core::catalog::RowSpec;
use crunchy_core::content::Hero;

use crate::platform::Key;

use super::feed::{Feed, FeedNav, FeedRow};
use super::theme::row_title;
use super::ui::Ui;
use super::worker::{Command, Event};
use super::{Env, Nav, Page, State};

pub struct HomePage {
    feed: Option<Feed>,
    specs: Vec<RowSpec>,
    heroes: HashMap<String, Hero>,
    requested: bool,
    /// the order the rows were asked in, to reload when the setting changes
    continue_first: bool,
    /// the focus comes down from the bar once the first row arrives
    grab: bool,
}

impl HomePage {
    pub fn new() -> HomePage {
        HomePage {
            feed: None,
            specs: Vec::new(),
            heroes: HashMap::new(),
            requested: false,
            continue_first: false,
            grab: true,
        }
    }
}

impl Page for HomePage {
    fn shown(&mut self, env: &mut Env) {
        let continue_first = env.state.settings.continue_first;
        if !self.requested || continue_first != self.continue_first {
            self.requested = true;
            self.continue_first = continue_first;
            self.feed = None;
            env.worker.send(Command::LoadHome { continue_first });
        }
        self.grab = true;
    }

    fn take_focus(&mut self) -> bool {
        let ready = self.feed.as_ref().is_some_and(|feed| feed.rows.first().is_some_and(|row| row.cards.is_some()));
        if self.grab && ready {
            self.grab = false;
            return true;
        }
        false
    }

    fn event(&mut self, event: &Event, _env: &mut Env) {
        match event {
            Event::Home(specs) => {
                self.specs = specs.clone();
                let rows = specs.iter().map(|spec| FeedRow::new(row_title(spec))).collect();
                self.feed = Some(Feed::new(rows, 170.0));
            }
            Event::Row(index, cards) => {
                if let Some(row) = self.feed.as_mut().and_then(|feed| feed.rows.get_mut(*index)) {
                    row.cards = Some(cards.clone());
                }
            }
            Event::Heroes(heroes) => self.heroes.extend(heroes.iter().map(|(id, hero)| (id.clone(), hero.clone()))),
            _ => {}
        }
    }

    fn key(&mut self, key: Key, _env: &mut Env) -> Nav {
        self.grab = false;
        let Some(feed) = self.feed.as_mut() else {
            return match key {
                Key::Up => Nav::Bar,
                Key::Back => Nav::Back,
                _ => Nav::Stay,
            };
        };
        match feed.key(key) {
            FeedNav::Stay => Nav::Stay,
            FeedNav::Up => Nav::Bar,
            FeedNav::Back => Nav::Back,
            FeedNav::Open(card) => Nav::Open(card),
        }
    }

    fn draw(&mut self, ui: &mut Ui, _state: &State, focused: bool) {
        match self.feed.as_mut() {
            Some(feed) => {
                for index in feed.wanted() {
                    if let Some(spec) = self.specs.get(index) {
                        ui.worker.send(Command::LoadRow(index, spec.clone()));
                    }
                }
                feed.draw(ui, &self.heroes, focused);
            }
            None => {
                let mut feed = Feed::new(Vec::new(), 170.0);
                feed.draw(ui, &self.heroes, false);
                for slot in 0..5 {
                    let x = 48.0 + slot as f32 * super::theme::CARD_STEP;
                    ui.skeleton(crate::gfx::Rect::new(48.0, 684.0, 260.0, 28.0), 8.0);
                    ui.skeleton(crate::gfx::Rect::new(x, 740.0, super::theme::CARD_W, super::theme::CARD_H), super::theme::RADIUS);
                }
            }
        }
    }
}
