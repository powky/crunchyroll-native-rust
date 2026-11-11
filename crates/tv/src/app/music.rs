//! Música: the music feed (curated collections of videos, concerts and artists) as a [`Feed`] under a search
//! field; with something typed, the matching videos and concerts in a grid instead.

use std::collections::HashMap;

use crunchy_core::content::{Hero, MusicRow};

use crate::platform::Key;

use super::controls::{FieldNav, SearchField};
use super::feed::{Feed, FeedNav, FeedRow};
use super::grid::{CardGrid, GridNav, Subtitle};
use super::ui::Ui;
use super::worker::{Command, Event, Query};
use super::{Env, Nav, Page, State};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Field,
    Feed,
}

pub struct MusicPage {
    feed: Option<Feed>,
    rows: Vec<MusicRow>,
    field: SearchField,
    results: Option<CardGrid>,
    zone: Zone,
    requested: bool,
    grab: bool,
    heroes: HashMap<String, Hero>,
}

impl MusicPage {
    pub fn new() -> MusicPage {
        MusicPage {
            feed: None,
            rows: Vec::new(),
            field: SearchField::new(720.0),
            results: None,
            zone: Zone::Feed,
            requested: false,
            grab: true,
            heroes: HashMap::new(),
        }
    }

    fn search(&mut self) {
        let text = self.field.applied.trim().to_string();
        self.results = (!text.is_empty()).then(|| CardGrid::new(Query::MusicSearch(text), Subtitle::Facts, false));
    }
}

impl Page for MusicPage {
    fn shown(&mut self, env: &mut Env) {
        if !self.requested {
            self.requested = true;
            env.worker.send(Command::MusicFeed);
        }
        self.grab = true;
    }

    fn event(&mut self, event: &Event, env: &mut Env) {
        match event {
            Event::MusicFeed(rows) => {
                self.rows = rows.clone();
                let feed_rows = rows.iter().map(|row| FeedRow::new(row.title.clone())).collect();
                self.feed = Some(Feed::new(feed_rows, 250.0));
            }
            Event::MusicRow(index, cards) => {
                if let Some(row) = self.feed.as_mut().and_then(|feed| feed.rows.get_mut(*index)) {
                    row.cards = Some(cards.clone());
                }
            }
            other => {
                if let Some(grid) = self.results.as_mut() {
                    grid.event(other, env.worker);
                }
            }
        }
    }

    fn take_focus(&mut self) -> bool {
        let ready = self.feed.as_ref().is_some_and(|feed| feed.rows.first().is_some_and(|row| row.cards.is_some()));
        if self.grab && ready {
            self.grab = false;
            self.zone = Zone::Feed;
            return true;
        }
        false
    }

    fn key(&mut self, key: Key, _env: &mut Env) -> Nav {
        self.grab = false;
        if self.field.keyboard.is_some() {
            if let FieldNav::Search = self.field.key(key) {
                self.search();
            }
            return Nav::Stay;
        }
        match self.zone {
            Zone::Field => match key {
                Key::Ok => {
                    self.field.key(Key::Ok);
                }
                Key::Up => return Nav::Bar,
                Key::Down => self.zone = Zone::Feed,
                Key::Back => return Nav::Back,
                _ => {}
            },
            Zone::Feed => {
                if let Some(grid) = self.results.as_mut() {
                    return match grid.key(key) {
                        GridNav::Up => {
                            self.zone = Zone::Field;
                            Nav::Stay
                        }
                        GridNav::Open(card) => Nav::Open(card),
                        GridNav::Stay if key == Key::Back => Nav::Back,
                        GridNav::Stay => Nav::Stay,
                    };
                }
                let Some(feed) = self.feed.as_mut() else {
                    if key == Key::Up {
                        self.zone = Zone::Field;
                    }
                    return if key == Key::Back { Nav::Back } else { Nav::Stay };
                };
                match feed.key(key) {
                    FeedNav::Stay => {}
                    FeedNav::Up => self.zone = Zone::Field,
                    FeedNav::Back => return Nav::Back,
                    FeedNav::Open(card) => return Nav::Open(card),
                }
            }
        }
        Nav::Stay
    }

    fn draw(&mut self, ui: &mut Ui, _state: &State, focused: bool) {
        if self.field.tick() {
            self.search();
        }
        let in_feed = focused && self.zone == Zone::Feed;
        match (self.results.as_mut(), self.feed.as_mut()) {
            (Some(grid), _) => grid.draw(ui, 226.0, 1056.0, in_feed, false),
            (None, Some(feed)) => {
                for index in feed.wanted() {
                    if let Some(row) = self.rows.get(index) {
                        ui.worker.send(Command::MusicRow(index, row.clone()));
                    }
                }
                feed.draw(ui, &self.heroes, in_feed);
            }
            (None, None) => {
                let mut feed = Feed::new(Vec::new(), 250.0);
                feed.draw(ui, &self.heroes, false);
            }
        }
        self.field.draw(ui, 48.0, 136.0, focused && self.zone == Zone::Field);
        self.field.draw_keyboard(ui);
    }
}
