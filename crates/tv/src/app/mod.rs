//! The app: a screen at a time before the catalog (start, sign-in code, profiles), then the catalog: a top bar of
//! tabs over the tab's page. Pages opened from them (a show, an artist, settings from the profiles) stack on top,
//! and a dialog can cover everything. Driven by the remote and by what the worker sends back; drawing is immediate
//! mode, each frame draws the current state.

mod alert;
mod artist;
mod controls;
mod feed;
mod grid;
mod home;
mod images;
mod keyboard;
mod languages;
mod music;
mod player;
mod profile_edit;
mod profiles;
mod settings;
mod show;
mod signin;
mod theme;
mod topbar;
mod ui;
pub mod worker;

use std::collections::HashMap;
use std::time::Instant;

use crunchy_core::account::{Profile, Profiles};
use crunchy_core::model::{Card, Kind};

use crate::gfx::{Font, Rect, Renderer};
use crate::platform::{self, Key};
use alert::{Alert, AlertAction, Choice};
use images::Images;
use settings::Settings;
use theme::*;
use topbar::{BarNav, Tab, TopBar};
use ui::Ui;
use worker::{Command, Event, Worker};

/// What a page asks of the app after a key.
pub enum Nav {
    Stay,
    /// focus goes up to the top bar
    Bar,
    /// the page has nothing to do with Back: the app goes back
    Back,
    /// open what a card shows (a show page, an artist)
    Open(Card),
    /// a page over the current one
    Push(Box<dyn Page>),
    /// the settings changed: save them and let every page know
    SettingsChanged,
    /// back to the profiles
    SwitchProfile,
    SelectProfile(String),
    Alert(Alert),
    Toast(String),
    /// play an episode or movie (from where it was left)
    Play(Card),
}

/// What the account is: its profiles, the one chosen, and the language lists once loaded.
#[derive(Default)]
pub struct Account {
    pub profiles: Profiles,
    pub current: Option<String>,
    pub premium: bool,
    pub languages: Option<(Vec<(String, String)>, Vec<(String, String)>)>,
}

impl Account {
    pub fn profile(&self) -> Option<&Profile> {
        self.current.as_ref().and_then(|id| self.profiles.list.iter().find(|p| &p.id == id))
    }

    /// The audio the profile plays in, as the player asks for it (also what episodes are looked up ahead in).
    pub fn audio(&self) -> Option<String> {
        self.profile().map(|profile| profile.audio_language.clone()).filter(|audio| !audio.is_empty())
    }
}

pub struct State {
    pub settings: Settings,
    pub account: Account,
    /// how many times the player closed after playing: pages showing progress reload when it moves
    pub watched: u64,
}

/// What a page gets to act with.
pub struct Env<'a> {
    pub worker: &'a Worker,
    pub state: &'a mut State,
}

pub trait Page {
    /// Draws the page. `focused` is false while the top bar has the focus.
    fn draw(&mut self, ui: &mut Ui, state: &State, focused: bool);
    fn key(&mut self, key: Key, env: &mut Env) -> Nav;
    fn event(&mut self, _event: &Event, _env: &mut Env) {}
    /// The page became the one shown (to load what it needs the first time).
    fn shown(&mut self, _env: &mut Env) {}
    /// The settings changed.
    fn settings_changed(&mut self, _env: &mut Env) {}
    /// Asked each frame while the top bar has the focus: the page wants it (its content just arrived).
    fn take_focus(&mut self) -> bool {
        false
    }
    /// The page is done (a form saved): the app closes it.
    fn finished(&self) -> bool {
        false
    }
    /// The page shows video under it: the frame starts transparent.
    fn transparent(&self) -> bool {
        false
    }
    /// The page played something (when it closes, progress shown elsewhere is out of date).
    fn played(&self) -> bool {
        false
    }
}

enum Screen {
    Starting,
    SignIn { code: Option<String>, expired: bool },
    Profiles(profiles::ProfilesScreen),
    Main(Box<Main>),
}

struct Main {
    bar: TopBar,
    /// the bar has the focus (else the page does)
    in_bar: bool,
    pages: HashMap<Tab, Box<dyn Page>>,
}

pub struct CrunchyApp {
    worker: Worker,
    images: Images,
    screen: Screen,
    state: State,
    /// pages over the screen, the last one shown
    stack: Vec<Box<dyn Page>>,
    alert: Option<Alert>,
    message: Option<(String, Instant)>,
    last: Instant,
    started: Instant,
    /// when a profile was chosen, to log how long the home's first row took
    profile_at: Option<Instant>,
    frame: u64,
}

impl CrunchyApp {
    pub fn new() -> CrunchyApp {
        let dir = platform::data_dir();
        eprintln!("crunchy: data in {}", dir.display());
        let settings = Settings::load(&dir);
        let (width, height) = settings.surface_size();
        platform::set_surface_size(width, height);
        let worker = Worker::start(dir);
        // CRUNCHY_DEMO=player (simulator): the player on a made-up episode, nothing signed in; player-last: the
        // series' last episode
        let demo_kind = std::env::var("CRUNCHY_DEMO").unwrap_or_default();
        let demo = cfg!(not(target_os = "linux")) && demo_kind.starts_with("player");
        if !demo {
            worker.send(Command::Start);
        }
        let app = CrunchyApp {
            worker,
            images: Images::default(),
            screen: Screen::Starting,
            state: State { settings, account: Account::default(), watched: 0 },
            stack: Vec::new(),
            alert: None,
            message: None,
            last: Instant::now(),
            started: Instant::now(),
            profile_at: None,
            frame: 0,
        };
        #[cfg(not(target_os = "linux"))]
        let app = {
            let mut app = app;
            if demo {
                app.apply(Nav::Push(Box::new(player::PlayerPage::demo(demo_kind == "player-last"))));
            }
            app
        };
        app
    }

    fn toast(&mut self, message: impl Into<String>) {
        self.message = Some((message.into(), Instant::now()));
    }

    fn open_main(&mut self) {
        let mut main = Main { bar: TopBar::new(&self.state), in_bar: false, pages: HashMap::new() };
        let mut env = Env { worker: &self.worker, state: &mut self.state };
        show_tab(&mut main, Tab::Home, &mut env);
        self.screen = Screen::Main(Box::new(main));
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::NeedsSignIn => {
                self.stack.clear();
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
            Event::Profiles(profiles) => {
                if matches!(self.screen, Screen::Starting) {
                    eprintln!("crunchy: profiles {:.2} s after start", self.started.elapsed().as_secs_f32());
                }
                self.state.account.profiles = profiles;
                if matches!(self.screen, Screen::Starting | Screen::SignIn { .. }) {
                    self.screen = Screen::Profiles(profiles::ProfilesScreen::new());
                }
                self.route(&Event::ProfileSaved);
            }
            Event::ProfileReady(id, premium) => {
                self.profile_at = Some(Instant::now());
                self.state.account.current = Some(id);
                self.state.account.premium = premium;
                self.open_main();
            }
            Event::ContentLanguage => {
                // everything shown was in the old language: start the catalog over, staying on the tab
                if let Screen::Main(main) = &mut self.screen {
                    let tab = main.bar.tab;
                    let kept = main.pages.remove(&tab);
                    let mut fresh = Main { bar: TopBar::new(&self.state), in_bar: main.in_bar, pages: HashMap::new() };
                    fresh.bar.select(tab, &self.state);
                    if let Some(page) = kept.filter(|_| tab == Tab::Settings) {
                        fresh.pages.insert(tab, page);
                    } else {
                        let mut env = Env { worker: &self.worker, state: &mut self.state };
                        show_tab(&mut fresh, tab, &mut env);
                    }
                    self.screen = Screen::Main(Box::new(fresh));
                }
            }
            Event::Languages { audio, subtitles } => self.state.account.languages = Some((audio, subtitles)),
            Event::Image { url, width, height, format, pixels } => self.images.arrived(url, width, height, format, pixels),
            Event::ImageFailed(url) => self.images.failed(url),
            Event::Error(message) => self.toast(message),
            Event::Info(message) => self.toast(message),
            other => {
                if let Event::Row(0, _) = &other
                    && let Some(at) = self.profile_at.take()
                {
                    eprintln!("crunchy: the home's first row {:.2} s after the profile", at.elapsed().as_secs_f32());
                }
                self.route(&other)
            }
        }
    }

    /// Hands an event to every page open (each takes what's its own).
    fn route(&mut self, event: &Event) {
        let mut env = Env { worker: &self.worker, state: &mut self.state };
        if let Screen::Main(main) = &mut self.screen {
            for page in main.pages.values_mut() {
                page.event(event, &mut env);
            }
        }
        for page in self.stack.iter_mut() {
            page.event(event, &mut env);
        }
    }

    /// Returns `false` to quit.
    fn handle_key(&mut self, key: Key) -> bool {
        if let Some(alert) = self.alert.as_mut() {
            match alert.key(key) {
                Choice::Open => {}
                Choice::Cancel => self.alert = None,
                Choice::Accept(action) => {
                    self.alert = None;
                    match action {
                        AlertAction::Quit => return false,
                        AlertAction::SignOut => {
                            self.stack.clear();
                            self.screen = Screen::Starting;
                            self.worker.send(Command::SignOut);
                        }
                        AlertAction::DeleteProfile(id) => {
                            self.worker.send(Command::DeleteProfile(id));
                            self.stack.pop();
                        }
                        AlertAction::Close => {}
                    }
                }
            }
            return true;
        }
        let mut env = Env { worker: &self.worker, state: &mut self.state };
        let nav = if let Some(page) = self.stack.last_mut() {
            match page.key(key, &mut env) {
                Nav::Back | Nav::Bar => {
                    if self.stack.pop().is_some_and(|page| page.played()) {
                        env.state.watched += 1;
                    }
                    Nav::Stay
                }
                other => other,
            }
        } else {
            match &mut self.screen {
                Screen::Starting => return key != Key::Back,
                Screen::SignIn { expired, .. } => {
                    match key {
                        Key::Back => return false,
                        Key::Ok if *expired => {
                            self.screen = Screen::SignIn { code: None, expired: false };
                            self.worker.send(Command::RequestCode);
                        }
                        _ => {}
                    }
                    return true;
                }
                Screen::Profiles(screen) => screen.key(key, &mut env),
                Screen::Main(main) => {
                    if main.in_bar {
                        match main.bar.key(key) {
                            BarNav::Stay => Nav::Stay,
                            BarNav::Show(tab) => {
                                main.bar.select(tab, env.state);
                                show_tab(main, tab, &mut env);
                                Nav::Stay
                            }
                            BarNav::Down => {
                                main.in_bar = false;
                                Nav::Stay
                            }
                            BarNav::Back => back_to_home(main, &mut env),
                            BarNav::Nav(nav) => nav,
                        }
                    } else {
                        let tab = main.bar.tab;
                        match main.pages.get_mut(&tab).map(|page| page.key(key, &mut env)).unwrap_or(Nav::Bar) {
                            Nav::Bar => {
                                main.in_bar = true;
                                main.bar.enter();
                                Nav::Stay
                            }
                            Nav::Back => back_to_home(main, &mut env),
                            other => other,
                        }
                    }
                }
            }
        };
        self.apply(nav);
        true
    }

    fn apply(&mut self, nav: Nav) {
        match nav {
            Nav::Stay | Nav::Bar | Nav::Back => {}
            Nav::Open(card) => self.open(card),
            Nav::Push(mut page) => {
                page.shown(&mut Env { worker: &self.worker, state: &mut self.state });
                self.stack.push(page);
            }
            Nav::SettingsChanged => {
                self.state.settings.save();
                let (width, height) = self.state.settings.surface_size();
                platform::set_surface_size(width, height);
                if let Screen::Main(main) = &mut self.screen {
                    main.bar.settings_changed(&self.state);
                    let mut env = Env { worker: &self.worker, state: &mut self.state };
                    for page in main.pages.values_mut() {
                        page.settings_changed(&mut env);
                    }
                }
            }
            Nav::SwitchProfile => {
                self.stack.clear();
                self.screen = Screen::Profiles(profiles::ProfilesScreen::new());
                self.worker.send(Command::Profiles);
            }
            Nav::SelectProfile(id) => {
                self.worker.send(Command::SelectProfile(id.clone()));
                self.state.account.current = Some(id.clone());
                self.state.settings.last_profile = Some(id);
                self.state.settings.save();
                // the skeleton until the profile is the session's
                self.screen = Screen::Starting;
            }
            Nav::Alert(alert) => self.alert = Some(alert),
            Nav::Toast(text) => self.toast(text),
            Nav::Play(card) => self.apply(Nav::Push(Box::new(player::PlayerPage::new(card)))),
        }
    }

    /// Opens what a card stands for.
    fn open(&mut self, card: Card) {
        let page: Box<dyn Page> = match card.kind {
            Kind::Series | Kind::MovieListing => Box::new(show::ShowPage::new(card.id.clone(), card.kind, card, None)),
            Kind::Episode | Kind::Season => match card.series_id.clone() {
                Some(id) => Box::new(show::ShowPage::new(id, Kind::Series, card.clone(), Some(card))),
                None => return,
            },
            Kind::Movie => match card.series_id.clone() {
                Some(id) => Box::new(show::ShowPage::new(id, Kind::MovieListing, card.clone(), Some(card))),
                None => return,
            },
            Kind::Artist => Box::new(artist::ArtistPage::new(card)),
            Kind::MusicVideo | Kind::Concert => {
                self.toast(show::NO_PLAYER);
                return;
            }
        };
        self.apply(Nav::Push(page));
    }
}

/// Back on a tab: from any tab to Inicio, and from Inicio to the profiles.
fn back_to_home(main: &mut Main, env: &mut Env) -> Nav {
    if main.bar.tab == Tab::Home {
        return Nav::SwitchProfile;
    }
    main.bar.select(Tab::Home, env.state);
    show_tab(main, Tab::Home, env);
    main.in_bar = false;
    Nav::Stay
}

/// Makes a tab the one shown, creating its page the first time.
fn show_tab(main: &mut Main, tab: Tab, env: &mut Env) {
    let page = main.pages.entry(tab).or_insert_with(|| new_page(tab));
    page.shown(env);
}

fn new_page(tab: Tab) -> Box<dyn Page> {
    match tab {
        Tab::Home => Box::new(home::HomePage::new()),
        Tab::Music => Box::new(music::MusicPage::new()),
        Tab::Settings => Box::new(settings::SettingsPage::new(false)),
        Tab::MyList => Box::new(grid::GridPage::my_list()),
        Tab::Simulcast => Box::new(grid::GridPage::simulcast()),
        other => Box::new(grid::GridPage::catalog(other)),
    }
}

impl platform::App for CrunchyApp {
    fn frame(&mut self, gfx: &mut Renderer, keys: &[Key]) -> bool {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        // the screen's size decides how big pictures are asked for
        let (_, fb_height) = gfx.framebuffer();
        let scale = fb_height as f32 / 1080.0;
        if (scale - crunchy_core::model::display_scale()).abs() > 0.01 || self.frame == 0 {
            crunchy_core::model::set_display_scale(scale);
            self.images.set_scale(scale);
        }
        self.frame += 1;
        self.images.next_frame(gfx);
        while let Ok(event) = self.worker.events.try_recv() {
            self.handle_event(event);
        }
        for key in keys {
            if !self.handle_key(*key) {
                return false;
            }
        }
        if self.stack.last().is_some_and(|page| page.finished())
            && self.stack.pop().is_some_and(|page| page.played())
        {
            self.state.watched += 1;
        }

        let video = self.stack.last().is_some_and(|page| page.transparent());
        gfx.begin(if video { crate::gfx::Color(0.0, 0.0, 0.0, 0.0) } else { BG });
        let t = (now - self.started).as_secs_f32();
        let premium = self.state.account.premium;
        let mut ui = Ui { gfx, images: &mut self.images, worker: &self.worker, dt, t, premium };
        if let Some(page) = self.stack.last_mut() {
            page.draw(&mut ui, &self.state, self.alert.is_none());
        } else {
            match &mut self.screen {
                Screen::Starting => signin::draw_starting(&mut ui),
                Screen::SignIn { code, expired } => signin::draw_sign_in(&mut ui, code.as_deref(), *expired),
                Screen::Profiles(screen) => screen.draw(&mut ui, &self.state),
                Screen::Main(main) => {
                    let tab = main.bar.tab;
                    if main.in_bar && main.bar.idle() && main.pages.get_mut(&tab).is_some_and(|page| page.take_focus()) {
                        main.in_bar = false;
                    }
                    if let Some(page) = main.pages.get_mut(&tab) {
                        page.draw(&mut ui, &self.state, !main.in_bar && self.alert.is_none());
                    }
                    main.bar.draw(&mut ui, &self.state, main.in_bar && self.alert.is_none());
                }
            }
        }
        if let Some(alert) = &self.alert {
            alert.draw(&mut ui);
        }
        if let Some((message, at)) = &self.message {
            if now.duration_since(*at).as_secs_f32() < 3.5 {
                let w = (ui.gfx.measure(Font::Regular, 24.0, message) + 64.0).min(1920.0 - 96.0);
                let r = Rect::new((1920.0 - w) / 2.0, 1080.0 - 120.0, w, 64.0);
                ui.gfx.rect(r, 32.0, SURFACE.alpha(0.96));
                ui.gfx.text(Font::Regular, 24.0, r.x + 32.0, r.y + 17.0, message, TEXT, Some(r.w - 64.0));
            } else {
                self.message = None;
            }
        }
        ui.gfx.end();
        true
    }
}
