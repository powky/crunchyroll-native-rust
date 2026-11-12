//! Everything that waits on the network runs here, on a Tokio runtime in its own thread: the session, the sign-in
//! code, profiles, the catalog and pictures. The UI thread sends [`Command`]s and picks up [`Event`]s once a frame,
//! so a slow request never holds a frame.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Context, Result};
use crunchy_core::account::{self, Album, Profiles};
use crunchy_core::auth::{self, Poll};
use crunchy_core::catalog::{self, RowSpec};
use crunchy_core::content::{Browse, Ctx, Hero, MusicRow, Named, Page, Show, SkipEvent};
use crunchy_core::crunchyroll_rs::Locale;
use crunchy_core::model::{Card, Kind};
use crunchy_core::platform::Platform;
use crunchy_core::session::Session;
use crunchy_core::store::Store;
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore, mpsc};

use crate::gfx::PixelFormat;
use crate::player::{self, Playback, Wanted};

use super::i18n::{self, Text};

/// Sign-in codes shown one after another (5 minutes each) before the screen asks for OK.
const CODE_RENEWALS: usize = 6;
/// The picture cache on disk is trimmed to this at launch, least recently used first.
const IMAGE_CACHE_BYTES: u64 = 300 * 1024 * 1024;
/// How long a show's hero facts (logo, art, rating) are kept before asking again: keyart rarely changes.
const HERO_DAYS: u64 = 14;
/// Shows whose hero facts are kept on disk at most.
const HERO_MAX: usize = 1500;

pub enum Command {
    Start,
    RequestCode,
    /// the profiles again, to switch
    Profiles,
    SelectProfile(String),
    /// the home rows; `continue_first` puts "continue watching" first
    LoadHome { continue_first: bool },
    LoadRow(usize, RowSpec),
    /// a show's page (series or movie listing)
    Show(String),
    /// a season's episodes, or a movie listing's movies
    Episodes(String, Kind),
    SetWatchlist(String, bool),
    Rate(String, Kind, u8),
    /// marks a season (then sends its episodes again) or a whole series watched
    MarkWatched { id: String, season: bool },
    /// a page of a grid; `key` comes back with it so a screen only takes its own answers
    Grid { key: u64, query: Query, start: usize, count: usize },
    Categories,
    SeasonalTags,
    MusicFeed,
    MusicRow(usize, MusicRow),
    /// a show's songs
    Featured(String),
    /// a show's genres
    Genres(String),
    Artist(String),
    UpdateProfile { id: String, fields: Value },
    CreateProfile { fields: Value },
    DeleteProfile(String),
    Avatars,
    Usernames,
    Languages,
    /// a picture, decoded to at most this width; `soon` when it's on screen (else it's for later)
    Image { url: String, width: u32, soon: bool },
    /// pictures to have on disk, most wanted first, in place of what was asked before (only what's near the focus
    /// now is worth the network): two at a time; ones already downloading finish
    Download(Vec<String>),
    /// an episode or movie to play: its id, and the audio, subtitles, height and start wanted
    Play { id: String, wanted: Wanted },
    /// where an episode or movie was left, in seconds
    SavePlayhead { id: String, seconds: u32 },
    /// the episode after (or before) this one
    Adjacent { id: String, forward: bool },
    SkipEvents(String),
    /// a stream's seek thumbnails (a BIF file at `url`), kept under `key`; its frames are then pictures
    /// `bif:{key}:{index}`
    Bif { key: String, url: String },
    /// an episode a play button would start, looked up ahead in the version `audio` wants
    Warm { id: String, audio: Option<String> },
    EpisodeRating(String),
    /// "up", "down", or `None` to take the thumb back
    RateEpisode { id: String, rating: Option<String> },
    SignOut,
}

/// What a grid shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Query {
    Browse(Browse),
    /// the watchlist, as its series and movies
    MyList,
    MusicSearch(String),
    Similar(String),
}

pub enum Event {
    NeedsSignIn,
    Code { user_code: String },
    CodeExpired,
    Profiles(Profiles),
    /// the profile chosen is the session's now: the catalog can load (and whether the account pays)
    ProfileReady(String, bool),
    Home(Vec<RowSpec>),
    Row(usize, Vec<Card>),
    /// logos, art and facts for the hero, by show id
    Heroes(HashMap<String, Hero>),
    Show(Show),
    ShowFailed(String),
    Episodes(String, Vec<Card>),
    Watchlist(String, bool),
    MarkedWatched(String),
    Grid { key: u64, start: usize, page: Page },
    GridFailed(u64),
    Categories(Vec<Named>),
    SeasonalTags(Vec<Named>),
    MusicFeed(Vec<MusicRow>),
    MusicRow(usize, Vec<Card>),
    Featured(String, Vec<Card>),
    Genres(String, Vec<String>),
    Artist { card: Card, videos: Vec<Card>, concerts: Vec<Card> },
    /// a profile change went through; the profiles follow
    ProfileSaved,
    Avatars(Vec<Album>),
    Usernames(Vec<String>),
    Languages { audio: Vec<(String, String)>, subtitles: Vec<(String, String)> },
    /// the profile shown changed its content language: the catalog reloads
    ContentLanguage,
    Image { url: String, width: u32, height: u32, format: PixelFormat, pixels: Vec<u8> },
    ImageFailed(String),
    Error(Text),
    /// a toast that isn't an error
    Info(Text),
    /// the video asked for is playing (its id, the playback to control)
    Playback(String, Arc<Playback>),
    /// it couldn't be played (its id, why)
    PlaybackFailed(String, Text),
    /// the episode after (or before) the one with this id, if there's one
    Adjacent { id: String, forward: bool, card: Option<Card> },
    SkipEvents(String, Vec<SkipEvent>),
    /// a BIF loaded: its key and when each frame is, in milliseconds
    Bif { key: String, times: Vec<u32> },
    EpisodeRating(String, Option<String>),
}

pub struct Worker {
    commands: mpsc::UnboundedSender<Command>,
    pub events: Receiver<Event>,
}

struct Shared {
    dir: PathBuf,
    session: Mutex<Option<Session>>,
    platform: Mutex<Option<Platform>>,
    /// requests for the catalog, made once a profile is chosen
    ctx: Mutex<Option<Ctx>>,
    /// hero facts asked for already
    heroes_asked: Mutex<HashSet<String>>,
    /// hero facts kept on disk (`heroes.json`), in the content language they're in
    hero_store: Mutex<Option<HeroStore>>,
    http: reqwest::Client,
    /// pictures: HTTP/2 from the first byte, so a row's pictures asked at once share one connection (over ALPN
    /// each would make its own TLS handshake first: a row took 1-2 s instead of a third of one)
    pictures: reqwest::Client,
    events: Sender<Event>,
    /// pictures on screen decoded at once (decoding is CPU work on a 4-core TV; the UI keeps a core)
    decoding: Semaphore,
    /// pictures asked ahead of time get one core of their own, so they never hold up what's on screen
    background: Semaphore,
    /// the last [`Command::Download`]: an earlier one stops when it sees it changed
    downloads: AtomicU64,
    /// the seek thumbnails of the stream playing, and their key
    bif: std::sync::Mutex<Option<(String, Arc<Bif>)>>,
}

impl Worker {
    pub fn start(dir: PathBuf) -> Worker {
        let (commands, mut receiver) = mpsc::unbounded_channel::<Command>();
        let (events, events_rx) = channel();
        std::thread::Builder::new()
            .name("crunchy-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                runtime.block_on(async move {
                    let shared = Arc::new(Shared {
                        dir,
                        session: Mutex::new(None),
                        platform: Mutex::new(None),
                        ctx: Mutex::new(None),
                        heroes_asked: Mutex::new(HashSet::new()),
                        hero_store: Mutex::new(None),
                        http: reqwest::Client::builder().build().expect("http client"),
                        pictures: reqwest::Client::builder()
                            .http2_prior_knowledge()
                            .http2_keep_alive_interval(std::time::Duration::from_secs(30))
                            .http2_keep_alive_while_idle(true)
                            .pool_idle_timeout(std::time::Duration::from_secs(600))
                            .build()
                            .expect("picture client"),
                        events,
                        decoding: Semaphore::new(3),
                        background: Semaphore::new(1),
                        downloads: AtomicU64::new(0),
                        bif: std::sync::Mutex::new(None),
                    });
                    let images = shared.dir.join("images");
                    tokio::task::spawn_blocking(move || trim_images(&images));
                    while let Some(command) = receiver.recv().await {
                        let shared = shared.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle(&shared, command).await {
                                eprintln!("crunchy: {error:#}");
                                let _ = shared.events.send(Event::Error(short_error(&error)));
                            }
                        });
                    }
                });
            })
            .expect("worker thread");
        Worker { commands, events: events_rx }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

/// An error for a toast: its first line, without the request's details. One of ours ([`Text`]) shows in the
/// interface's language; the rest as they came.
fn short_error(error: &anyhow::Error) -> Text {
    if let Some(text) = error.downcast_ref::<Text>() {
        return text.clone();
    }
    let text = format!("{error}");
    let text: String = text.lines().next().unwrap_or("").chars().take(140).collect();
    if text.is_empty() { Text::Key("Something went wrong") } else { Text::Plain(text) }
}

fn locale() -> Locale {
    Locale::es_419
}

async fn platform(shared: &Shared, refresh: bool) -> Result<Platform> {
    let mut slot = shared.platform.lock().await;
    if refresh || slot.is_none() {
        *slot = Some(Platform::load(&shared.dir, refresh).await?);
    }
    Ok(slot.clone().expect("platform"))
}

async fn crunchy(shared: &Shared) -> Result<crunchy_core::crunchyroll_rs::Crunchyroll> {
    Ok(shared.session.lock().await.as_ref().context("not signed in")?.crunchy.clone())
}

/// The catalog context of the signed-in profile, in its content language.
async fn ctx(shared: &Shared) -> Result<Ctx> {
    let mut slot = shared.ctx.lock().await;
    if let Some(ctx) = slot.as_ref() {
        return Ok(ctx.clone());
    }
    let (crunchy, profile_id) = {
        let guard = shared.session.lock().await;
        let session = guard.as_ref().context("not signed in")?;
        (session.crunchy.clone(), session.profile_id().map(String::from))
    };
    let language = match account::profiles(&crunchy).await {
        Ok(profiles) => profiles
            .list
            .iter()
            .find(|p| Some(&p.id) == profile_id.as_ref())
            .or(profiles.list.first())
            .map(|p| p.content_language.clone())
            .filter(|l| !l.is_empty()),
        Err(_) => None,
    };
    let ctx = Ctx::new(crunchy, language.unwrap_or_else(|| locale().to_string())).await?;
    *slot = Some(ctx.clone());
    Ok(ctx)
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct HeroStore {
    locale: String,
    /// show id -> (when it was asked, seconds since 1970; the trimmed object)
    items: HashMap<String, (u64, Value)>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The shows behind a row's cards that the hero hasn't got facts for yet: from the disk when they're recent,
/// else asked for (and kept).
async fn send_heroes(shared: &Shared, cards: &[Card]) -> Result<()> {
    let ids: Vec<String> = {
        let mut asked = shared.heroes_asked.lock().await;
        let mut ids = Vec::new();
        for card in cards {
            let id = match card.kind {
                Kind::Series | Kind::MovieListing => Some(card.id.clone()),
                Kind::Episode | Kind::Movie | Kind::Season => card.series_id.clone(),
                _ => None,
            };
            if let Some(id) = id
                && asked.insert(id.clone())
            {
                ids.push(id);
            }
        }
        ids
    };
    if ids.is_empty() {
        return Ok(());
    }
    let ctx = ctx(shared).await?;
    let path = shared.dir.join("heroes.json");
    let mut known = HashMap::new();
    let mut missing = Vec::new();
    {
        let mut slot = shared.hero_store.lock().await;
        if slot.as_ref().is_none_or(|store| store.locale != ctx.locale) {
            let saved: Option<HeroStore> =
                tokio::fs::read(&path).await.ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
            *slot = Some(saved.filter(|store| store.locale == ctx.locale).unwrap_or(HeroStore { locale: ctx.locale.clone(), ..Default::default() }));
        }
        let store = slot.as_ref().expect("store");
        let fresh_after = now_secs().saturating_sub(HERO_DAYS * 86400);
        for id in ids {
            match store.items.get(&id).filter(|(at, _)| *at > fresh_after).and_then(|(_, object)| Hero::from_object(object)) {
                Some(hero) => {
                    known.insert(id, hero);
                }
                None => missing.push(id),
            }
        }
    }
    if !known.is_empty() {
        let _ = shared.events.send(Event::Heroes(known));
    }
    if missing.is_empty() {
        return Ok(());
    }
    let objects = ctx.hero_objects(&missing).await?;
    let heroes: HashMap<String, Hero> =
        objects.iter().filter_map(Hero::from_object).map(|hero| (hero.card.id.clone(), hero)).collect();
    let _ = shared.events.send(Event::Heroes(heroes));
    let json = {
        let mut slot = shared.hero_store.lock().await;
        let store = slot.as_mut().expect("store");
        let now = now_secs();
        for object in objects {
            if let Some(id) = object.get("id").and_then(Value::as_str) {
                store.items.insert(id.to_string(), (now, object));
            }
        }
        if store.items.len() > HERO_MAX {
            let mut by_age: Vec<(u64, String)> = store.items.iter().map(|(id, (at, _))| (*at, id.clone())).collect();
            by_age.sort();
            for (_, id) in by_age.into_iter().take(store.items.len() - HERO_MAX) {
                store.items.remove(&id);
            }
        }
        serde_json::to_vec(&*store)?
    };
    let tmp = path.with_extension("json.tmp");
    if tokio::fs::write(&tmp, json).await.is_ok() {
        let _ = tokio::fs::rename(tmp, path).await;
    }
    Ok(())
}

async fn send_profiles(shared: &Shared) -> Result<()> {
    let crunchy = crunchy(shared).await?;
    let profiles = account::profiles(&crunchy).await?;
    let _ = shared.events.send(Event::Profiles(profiles));
    Ok(())
}

/// Forgets what belongs to the profile shown: its catalog context and the hero facts asked for.
async fn forget_profile(shared: &Shared) {
    *shared.ctx.lock().await = None;
    shared.heroes_asked.lock().await.clear();
}

async fn handle(shared: &Arc<Shared>, command: Command) -> Result<()> {
    match command {
        Command::Start => {
            let store = Store::new(&shared.dir);
            let mut restored = Session::restore(store, platform(shared, false).await?, locale()).await;
            if restored.is_err() {
                // the app credentials were switched off: fetch the current ones once and retry
                restored = Session::restore(Store::new(&shared.dir), platform(shared, true).await?, locale()).await;
            }
            match restored? {
                Some(session) => {
                    *shared.session.lock().await = Some(session);
                    send_profiles(shared).await?;
                }
                None => {
                    let _ = shared.events.send(Event::NeedsSignIn);
                }
            }
        }
        Command::RequestCode => {
            let platform = platform(shared, false).await?;
            let client = platform.http_client()?;
            let store = Store::new(&shared.dir);
            let device = Session::device_for(&store)?;
            // a code lasts 5 minutes: show a fresh one when it runs out, and only after half an hour ask for OK
            for _ in 0..CODE_RENEWALS {
                let code = auth::request_code(&client, &platform).await?;
                let _ = shared.events.send(Event::Code { user_code: code.user_code.clone() });
                let deadline = tokio::time::Instant::now() + code.expires_in;
                while tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(code.interval).await;
                    match auth::poll(&client, &platform, &device, &code).await? {
                        Poll::Pending => continue,
                        Poll::Expired => break,
                        Poll::Authorized { refresh_token } => {
                            let session = Session::from_refresh_token(store, platform, locale(), refresh_token).await?;
                            *shared.session.lock().await = Some(session);
                            return send_profiles(shared).await;
                        }
                    }
                }
            }
            let _ = shared.events.send(Event::CodeExpired);
        }
        Command::Profiles => send_profiles(shared).await?,
        Command::SelectProfile(id) => {
            {
                let mut guard = shared.session.lock().await;
                let session = guard.as_mut().context("not signed in")?;
                if session.profile_id() != Some(id.as_str())
                    && let Err(error) = session.select_profile(&id).await
                {
                    // back to the list the screen left for the skeleton
                    drop(guard);
                    send_profiles(shared).await?;
                    return Err(error);
                }
            }
            // another profile has its own lists, progress and languages
            forget_profile(shared).await;
            let premium = ctx(shared).await.map(|ctx| ctx.premium).unwrap_or(true);
            let _ = shared.events.send(Event::ProfileReady(id, premium));
        }
        Command::LoadHome { continue_first } => {
            let crunchy = crunchy(shared).await?;
            let specs = catalog::home(&crunchy, continue_first).await?;
            let _ = shared.events.send(Event::Home(specs));
        }
        Command::LoadRow(index, spec) => {
            let started = std::time::Instant::now();
            let fast = match spec {
                RowSpec::ContinueWatching => ctx(shared).await?.continue_watching().await.inspect_err(|error| {
                    eprintln!("crunchy: continue watching from the discover history: {error:#}");
                }),
                _ => Err(anyhow::anyhow!("no quick way")),
            };
            let cards = match fast {
                Ok(cards) => cards,
                Err(_) => catalog::load_row(&crunchy(shared).await?, &spec).await?,
            };
            eprintln!("crunchy: row {index} in {:.2} s", started.elapsed().as_secs_f32());
            let _ = shared.events.send(Event::Row(index, cards.clone()));
            send_heroes(shared, &cards).await?;
        }
        Command::Show(id) => match ctx(shared).await?.show(&id).await {
            Ok(show) => {
                let _ = shared.events.send(Event::Show(show));
            }
            Err(error) => {
                let _ = shared.events.send(Event::ShowFailed(id));
                return Err(error);
            }
        },
        Command::Episodes(id, kind) => {
            let ctx = ctx(shared).await?;
            let cards = if kind == Kind::MovieListing { ctx.movies(&id).await? } else { ctx.episodes(&id).await? };
            let _ = shared.events.send(Event::Episodes(id, cards));
        }
        Command::SetWatchlist(id, on) => {
            let result = ctx(shared).await?.set_watchlist(&id, on).await;
            // the page changed it already; on failure it goes back
            let _ = shared.events.send(Event::Watchlist(id, if result.is_ok() { on } else { !on }));
            result?;
        }
        Command::Rate(id, kind, stars) => {
            ctx(shared).await?.rate(&id, kind, Some(stars)).await?;
        }
        Command::MarkWatched { id, season } => {
            let ctx = ctx(shared).await?;
            ctx.mark_watched(&id).await?;
            if season {
                let cards = ctx.episodes(&id).await?;
                let _ = shared.events.send(Event::Episodes(id.clone(), cards));
            } else {
                let _ = shared.events.send(Event::Info(Text::Key("Marked as watched")));
            }
            let _ = shared.events.send(Event::MarkedWatched(id));
        }
        Command::Grid { key, query, start, count } => {
            let result = grid(shared, &query, start, count).await;
            match result {
                Ok(page) => {
                    let _ = shared.events.send(Event::Grid { key, start, page });
                }
                Err(error) => {
                    let _ = shared.events.send(Event::GridFailed(key));
                    return Err(error);
                }
            }
        }
        Command::Categories => {
            let list = ctx(shared).await?.categories().await?;
            let _ = shared.events.send(Event::Categories(list));
        }
        Command::SeasonalTags => {
            let list = ctx(shared).await?.seasonal_tags().await?;
            let _ = shared.events.send(Event::SeasonalTags(list));
        }
        Command::MusicFeed => {
            let rows = ctx(shared).await?.music_feed().await?;
            let _ = shared.events.send(Event::MusicFeed(rows));
        }
        Command::MusicRow(index, row) => {
            let cards = ctx(shared).await?.music(row.kind, &row.ids).await?;
            let _ = shared.events.send(Event::MusicRow(index, cards));
        }
        Command::Featured(id) => {
            let cards = ctx(shared).await?.featured_music(&id).await?;
            let _ = shared.events.send(Event::Featured(id, cards));
        }
        Command::Genres(id) => {
            let genres = ctx(shared).await?.genres_of(&id).await?;
            let _ = shared.events.send(Event::Genres(id, genres));
        }
        Command::Artist(id) => {
            let (card, videos, concerts) = ctx(shared).await?.artist(&id).await?;
            let _ = shared.events.send(Event::Artist { card, videos, concerts });
        }
        Command::UpdateProfile { id, fields } => {
            let crunchy = crunchy(shared).await?;
            account::update_profile(&crunchy, &id, fields.clone()).await?;
            let current = shared.session.lock().await.as_ref().and_then(|s| s.profile_id().map(String::from));
            if let Some(language) = fields.get("preferred_communication_language").and_then(Value::as_str)
                && current.as_deref() == Some(id.as_str())
            {
                forget_profile(shared).await;
                let _ = language;
                let _ = shared.events.send(Event::ContentLanguage);
            }
            let _ = shared.events.send(Event::ProfileSaved);
            send_profiles(shared).await?;
        }
        Command::CreateProfile { fields } => {
            let crunchy = crunchy(shared).await?;
            account::create_profile(&crunchy, fields).await?;
            let _ = shared.events.send(Event::ProfileSaved);
            send_profiles(shared).await?;
        }
        Command::DeleteProfile(id) => {
            let crunchy = crunchy(shared).await?;
            account::delete_profile(&crunchy, &id).await?;
            let _ = shared.events.send(Event::ProfileSaved);
            send_profiles(shared).await?;
        }
        Command::Avatars => {
            let crunchy = crunchy(shared).await?;
            // the albums' names in the interface's language
            let albums = account::avatars(&crunchy, i18n::lang().locale()).await?;
            let _ = shared.events.send(Event::Avatars(albums));
        }
        Command::Usernames => {
            let crunchy = crunchy(shared).await?;
            let names = account::usernames(&crunchy).await?;
            let _ = shared.events.send(Event::Usernames(names));
        }
        Command::Languages => {
            let (audio, subtitles) =
                futures_util::join!(account::languages(&shared.http, false), account::languages(&shared.http, true));
            let _ = shared.events.send(Event::Languages { audio: audio?, subtitles: subtitles? });
        }
        Command::Download(urls) => {
            let generation = shared.downloads.fetch_add(1, Ordering::SeqCst) + 1;
            let queue = Arc::new(std::sync::Mutex::new(VecDeque::from(urls)));
            let runners: Vec<_> = (0..2)
                .map(|_| {
                    let (shared, queue) = (shared.clone(), queue.clone());
                    tokio::spawn(async move {
                        while shared.downloads.load(Ordering::SeqCst) == generation {
                            let Some(url) = queue.lock().unwrap().pop_front() else { break };
                            if let Err(error) = picture_bytes(&shared, &url).await {
                                eprintln!("crunchy: download {url}: {error:#}");
                            }
                        }
                    })
                })
                .collect();
            for runner in runners {
                let _ = runner.await;
            }
        }
        Command::Image { url, width, soon } => match picture(shared, &url, width, soon).await {
            Ok((w, h, format, pixels)) => {
                let _ = shared.events.send(Event::Image { url, width: w, height: h, format, pixels });
            }
            Err(error) => {
                eprintln!("crunchy: picture {url}: {error:#}");
                let _ = shared.events.send(Event::ImageFailed(url));
            }
        },
        Command::Play { id, wanted } => {
            let crunchy = crunchy(shared).await?;
            let event = match player::open(&crunchy, &id, &wanted).await {
                Ok(playback) => Event::Playback(id, Arc::new(playback)),
                Err(error) => {
                    eprintln!("crunchy: playing {id}: {error:#}");
                    Event::PlaybackFailed(id, short_error(&error))
                }
            };
            let _ = shared.events.send(event);
        }
        Command::Warm { id, audio } => player::warm(&crunchy(shared).await?, &id, audio.as_deref()).await,
        // the player's own requests go on quietly when they fail: the video plays on without them
        Command::SavePlayhead { id, seconds } => {
            if let Err(error) = ctx(shared).await?.save_playhead(&id, seconds).await {
                eprintln!("crunchy: saving where {id} was left: {error:#}");
            }
        }
        Command::Adjacent { id, forward } => {
            let card = ctx(shared).await?.adjacent_episode(&id, forward).await.unwrap_or_else(|error| {
                eprintln!("crunchy: the episode next to {id}: {error:#}");
                None
            });
            let _ = shared.events.send(Event::Adjacent { id, forward, card });
        }
        Command::SkipEvents(id) => {
            let events = ctx(shared).await?.skip_events(&id).await.unwrap_or_else(|error| {
                eprintln!("crunchy: skip events of {id}: {error:#}");
                Vec::new()
            });
            let _ = shared.events.send(Event::SkipEvents(id, events));
        }
        Command::Bif { key, url } => {
            let started = std::time::Instant::now();
            let bytes = match fetch(&shared.pictures, &url).await {
                Ok(bytes) => bytes,
                Err(_) => fetch(&shared.http, &url).await?,
            };
            let bif = tokio::task::spawn_blocking(move || Bif::parse(bytes)).await??;
            eprintln!("crunchy: seek thumbnails: {} frames in {:.1} s", bif.frames.len(), started.elapsed().as_secs_f32());
            let times = bif.frames.iter().map(|frame| frame.0).collect();
            *shared.bif.lock().unwrap() = Some((key.clone(), Arc::new(bif)));
            let _ = shared.events.send(Event::Bif { key, times });
        }
        Command::EpisodeRating(id) => match ctx(shared).await?.episode_rating(&id).await {
            Ok(rating) => {
                let _ = shared.events.send(Event::EpisodeRating(id, rating));
            }
            Err(error) => eprintln!("crunchy: the rating of {id}: {error:#}"),
        },
        Command::RateEpisode { id, rating } => {
            let ctx = ctx(shared).await?;
            if let Err(error) = ctx.rate_episode(&id, rating.as_deref()).await {
                // the thumb shown goes back to what Crunchyroll has
                let rating = ctx.episode_rating(&id).await.unwrap_or(None);
                let _ = shared.events.send(Event::EpisodeRating(id, rating));
                return Err(error);
            }
        }
        Command::SignOut => {
            forget_profile(shared).await;
            if let Some(session) = shared.session.lock().await.take() {
                session.sign_out()?;
            }
            let _ = shared.events.send(Event::NeedsSignIn);
        }
    }
    Ok(())
}

async fn grid(shared: &Shared, query: &Query, start: usize, count: usize) -> Result<Page> {
    let ctx = ctx(shared).await?;
    Ok(match query {
        Query::Browse(browse) => ctx.browse(browse, start, count).await?,
        Query::MyList => {
            let page = ctx.watchlist(start, count).await?;
            let cards = ctx.shows_of(&page.cards).await?;
            Page { total: page.total, cards }
        }
        Query::MusicSearch(text) => {
            let cards = ctx.music_search(text, count).await?;
            Page { total: cards.len(), cards }
        }
        Query::Similar(id) => {
            let cards = ctx.similar(id, count).await?;
            Page { total: cards.len(), cards }
        }
    })
}

/// Keeps the picture cache under its size, dropping the files used longest ago.
fn trim_images(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let used = meta.accessed().or_else(|_| meta.modified()).ok()?;
            Some((used, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    if total <= IMAGE_CACHE_BYTES {
        return;
    }
    files.sort();
    for (_, size, path) in files {
        if total <= IMAGE_CACHE_BYTES * 8 / 10 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

/// A picture from the disk cache or the network, decoded and shrunk to `max_width`.
async fn picture(shared: &Shared, url: &str, max_width: u32, soon: bool) -> Result<(u32, u32, PixelFormat, Vec<u8>)> {
    let started = std::time::Instant::now();
    let (bytes, cached) = picture_bytes(shared, url).await?;
    let fetched = started.elapsed();
    let _permit = if soon { shared.decoding.acquire().await? } else { shared.background.acquire().await? };
    let waited = started.elapsed();
    let url_tail: String = url.chars().rev().take(48).collect::<String>().chars().rev().collect();
    tokio::task::spawn_blocking(move || -> Result<(u32, u32, PixelFormat, Vec<u8>)> {
        let t0 = std::time::Instant::now();
        let mut image = image::load_from_memory(&bytes).context("decoding")?;
        let started = fetched;
        let decoded = t0.elapsed();
        let (w0, h0) = (image.width(), image.height());
        // the GPU shrinks a picture up to half again its size well enough; past that, a quick averaging pass
        // (a filtered resize costs seconds on this CPU)
        if image.width() > max_width * 3 / 2 {
            let height = (image.height() as u64 * max_width as u64 / image.width() as u64) as u32;
            image = image.thumbnail_exact(max_width, height.max(1));
        }
        let (width, height) = (image.width(), image.height());
        // big opaque pictures (backdrops) go to the GPU at 2 bytes a pixel, dithered so their gradients don't band
        let (format, pixels) = if !image.color().has_alpha() && width * height >= 1280 * 720 {
            (PixelFormat::Rgb565, rgb565_dithered(&image.into_rgb8()))
        } else {
            (PixelFormat::Rgba, image.into_rgba8().into_raw())
        };
        // only the slow ones are worth a line
        if started + t0.elapsed() > std::time::Duration::from_millis(600) {
            eprintln!(
            "crunchy: picture {}KB {} fetch {:?} queue {:?} decode {w0}x{h0} {:?} resize->{} {:?} …{url_tail}",
            bytes.len() / 1024,
            if cached { "disk" } else { "net" },
            fetched,
            waited - fetched,
            decoded,
            width,
            t0.elapsed() - decoded,
            );
        }
        Ok((width, height, format, pixels))
    })
    .await?
}

/// A picture's file: from the disk cache (true), or downloaded into it. A seek thumbnail (`bif:{key}:{index}`)
/// comes out of the stream's BIF, in memory.
async fn picture_bytes(shared: &Shared, url: &str) -> Result<(Vec<u8>, bool)> {
    if let Some(frame) = url.strip_prefix("bif:") {
        let (key, index) = frame.rsplit_once(':').context("a seek thumbnail's link")?;
        let index: usize = index.parse()?;
        let bif = shared.bif.lock().unwrap().clone().filter(|(held, _)| held == key).context("no such seek thumbnails")?;
        return Ok((bif.1.frame(index).context("no such seek thumbnail")?.to_vec(), true));
    }
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let path = shared.dir.join("images").join(format!("{:016x}", hasher.finish()));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return Ok((bytes, true));
    }
    let bytes = match fetch(&shared.pictures, url).await {
        Ok(bytes) => bytes,
        // a host that doesn't speak HTTP/2 straight away
        Err(_) => fetch(&shared.http, url).await?,
    };
    let _ = tokio::fs::create_dir_all(path.parent().expect("images dir")).await;
    // whole or not at all: a file cut short by the app closing would be read back as a broken picture
    let partial = path.with_extension("part");
    if tokio::fs::write(&partial, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&partial, &path).await;
    }
    Ok((bytes, false))
}

/// A file over HTTP, as a picture would be asked for.
async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client.get(url).header("Accept", "image/webp,image/*;q=0.8").send().await?;
    Ok(response.error_for_status()?.bytes().await?.to_vec())
}

/// Seek thumbnails in Roku's BIF format: a 64-byte header (magic, version, frame count, milliseconds per
/// timestamp unit), then an index of (timestamp, offset) pairs ending with 0xffffffff, then the JPEGs.
pub struct Bif {
    data: Vec<u8>,
    /// each frame's time in milliseconds, and where its JPEG starts and ends
    frames: Vec<(u32, usize, usize)>,
}

impl Bif {
    fn parse(data: Vec<u8>) -> Result<Bif> {
        const MAGIC: [u8; 8] = [0x89, b'B', b'I', b'F', 0x0d, 0x0a, 0x1a, 0x0a];
        anyhow::ensure!(data.len() >= 64 && data[..8] == MAGIC, "not a BIF file");
        let word = |at: usize| -> Option<u32> { Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?)) };
        let count = word(12).context("BIF header")? as usize;
        let unit = match word(16).context("BIF header")? {
            0 => 1000,
            unit => unit,
        };
        let mut frames = Vec::with_capacity(count);
        for index in 0..count {
            let at = 64 + index * 8;
            let (Some(time), Some(start), Some(end)) = (word(at), word(at + 4), word(at + 12)) else { break };
            let (start, end) = (start as usize, end as usize);
            if time == u32::MAX || start >= end || end > data.len() {
                break;
            }
            frames.push((time.saturating_mul(unit), start, end));
        }
        anyhow::ensure!(!frames.is_empty(), "a BIF file with no frames");
        Ok(Bif { data, frames })
    }

    fn frame(&self, index: usize) -> Option<&[u8]> {
        let (_, start, end) = *self.frames.get(index)?;
        self.data.get(start..end)
    }
}

/// RGB 5-6-5 (native-endian 16-bit words, as GL takes them), with a 4x4 ordered dither: at 4K its pattern is too
/// fine to see, and it keeps dark gradients from banding.
fn rgb565_dithered(rgb: &image::RgbImage) -> Vec<u8> {
    const BAYER: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
    let mut out = Vec::with_capacity(rgb.width() as usize * rgb.height() as usize * 2);
    for (y, row) in rgb.rows().enumerate() {
        let thresholds = &BAYER[(y & 3) * 4..(y & 3) * 4 + 4];
        for (x, pixel) in row.enumerate() {
            let threshold = thresholds[x & 3] * 255 / 16 + 8;
            let r = ((pixel[0] as u32 * 31 + threshold) / 255).min(31);
            let g = ((pixel[1] as u32 * 63 + threshold) / 255).min(63);
            let b = ((pixel[2] as u32 * 31 + threshold) / 255).min(31);
            out.extend_from_slice(&((r << 11 | g << 5 | b) as u16).to_ne_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Bif;

    /// A BIF with `frames` (timestamp, bytes), 10-second units, laid out as Roku's spec has it.
    fn bif(frames: &[(u32, &[u8])]) -> Vec<u8> {
        let mut out = vec![0x89, b'B', b'I', b'F', 0x0d, 0x0a, 0x1a, 0x0a];
        out.extend(0u32.to_le_bytes());
        out.extend((frames.len() as u32).to_le_bytes());
        out.extend(10_000u32.to_le_bytes());
        out.resize(64, 0);
        let mut offset = 64 + (frames.len() as u32 + 1) * 8;
        for (time, data) in frames {
            out.extend(time.to_le_bytes());
            out.extend(offset.to_le_bytes());
            offset += data.len() as u32;
        }
        out.extend(u32::MAX.to_le_bytes());
        out.extend(offset.to_le_bytes());
        for (_, data) in frames {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn frames_come_with_their_times() {
        let bif = Bif::parse(bif(&[(0, b"first"), (1, b"second!"), (2, b"third")])).unwrap();
        assert_eq!(bif.frames.iter().map(|frame| frame.0).collect::<Vec<_>>(), [0, 10_000, 20_000]);
        assert_eq!(bif.frame(1), Some(&b"second!"[..]));
        assert_eq!(bif.frame(3), None);
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(Bif::parse(b"not a bif file at all, just some bytes that are long enough to be read".to_vec()).is_err());
        assert!(Bif::parse(bif(&[])).is_err());
    }
}
