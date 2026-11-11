//! Everything that waits on the network runs here, on a Tokio runtime in its own thread: the session, the sign-in
//! code, profiles, rows and pictures. The UI thread sends [`Command`]s and picks up [`Event`]s once a frame, so a slow
//! request never holds a frame.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Context, Result};
use crunchy_core::auth::{self, Poll};
use crunchy_core::catalog::{self, RowSpec};
use crunchy_core::crunchyroll_rs::Locale;
use crunchy_core::model::Card;
use crunchy_core::platform::Platform;
use crunchy_core::session::Session;
use crunchy_core::store::Store;
use tokio::sync::{Mutex, Semaphore, mpsc};

pub enum Command {
    Start,
    RequestCode,
    SelectProfile(String),
    LoadHome,
    LoadRow(usize, RowSpec),
    /// a picture, decoded to at most this width
    Image(String, u32),
    SignOut,
}

#[derive(Clone, Debug)]
pub struct ProfileInfo {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}

pub enum Event {
    NeedsSignIn,
    Code { user_code: String },
    CodeExpired,
    Profiles(Vec<ProfileInfo>),
    Home(Vec<RowSpec>),
    Row(usize, Vec<Card>),
    Image { url: String, width: u32, height: u32, rgba: Vec<u8> },
    ImageFailed(String),
    Error(String),
}

pub struct Worker {
    commands: mpsc::UnboundedSender<Command>,
    pub events: Receiver<Event>,
}

struct Shared {
    dir: PathBuf,
    session: Mutex<Option<Session>>,
    platform: Mutex<Option<Platform>>,
    http: reqwest::Client,
    events: Sender<Event>,
    /// pictures decoded at once (decoding is CPU work on a 4-core TV)
    decoding: Semaphore,
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
                        http: reqwest::Client::builder().build().expect("http client"),
                        events,
                        decoding: Semaphore::new(2),
                    });
                    while let Some(command) = receiver.recv().await {
                        let shared = shared.clone();
                        tokio::spawn(async move {
                            if let Err(error) = handle(&shared, command).await {
                                eprintln!("crunchy: {error:#}");
                                let _ = shared.events.send(Event::Error(format!("{error:#}")));
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

async fn send_profiles(shared: &Shared) -> Result<()> {
    let guard = shared.session.lock().await;
    let session = guard.as_ref().context("not signed in")?;
    let profiles = session
        .profiles()
        .await?
        .into_iter()
        .map(|profile| ProfileInfo {
            id: profile.profile_id,
            name: profile.profile_name,
            avatar: (!profile.avatar.is_empty())
                .then(|| format!("https://static.crunchyroll.com/assets/avatar/510x510/{}", profile.avatar)),
        })
        .collect();
    let _ = shared.events.send(Event::Profiles(profiles));
    Ok(())
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
            let code = auth::request_code(&client, &platform).await?;
            let _ = shared.events.send(Event::Code { user_code: code.user_code.clone() });
            let deadline = tokio::time::Instant::now() + code.expires_in;
            loop {
                tokio::time::sleep(code.interval).await;
                if tokio::time::Instant::now() > deadline {
                    let _ = shared.events.send(Event::CodeExpired);
                    break;
                }
                match auth::poll(&client, &platform, &device, &code).await? {
                    Poll::Pending => continue,
                    Poll::Expired => {
                        let _ = shared.events.send(Event::CodeExpired);
                        break;
                    }
                    Poll::Authorized { refresh_token } => {
                        let session = Session::from_refresh_token(store, platform, locale(), refresh_token).await?;
                        *shared.session.lock().await = Some(session);
                        send_profiles(shared).await?;
                        break;
                    }
                }
            }
        }
        Command::SelectProfile(id) => {
            {
                let mut guard = shared.session.lock().await;
                let session = guard.as_mut().context("not signed in")?;
                if session.profile_id() != Some(id.as_str()) {
                    session.select_profile(&id).await?;
                }
            }
            Box::pin(handle(shared, Command::LoadHome)).await?;
        }
        Command::LoadHome => {
            let guard = shared.session.lock().await;
            let session = guard.as_ref().context("not signed in")?;
            let specs = catalog::home(&session.crunchy, false).await?;
            let _ = shared.events.send(Event::Home(specs));
        }
        Command::LoadRow(index, spec) => {
            // the session lock is only held to clone the client handle (crunchyroll-rs sessions are cheap to clone)
            let crunchy = shared.session.lock().await.as_ref().context("not signed in")?.crunchy.clone();
            let cards = catalog::load_row(&crunchy, &spec).await?;
            let _ = shared.events.send(Event::Row(index, cards));
        }
        Command::Image(url, width) => {
            match picture(shared, &url, width).await {
                Ok((w, h, rgba)) => {
                    let _ = shared.events.send(Event::Image { url, width: w, height: h, rgba });
                }
                Err(error) => {
                    eprintln!("crunchy: picture {url}: {error:#}");
                    let _ = shared.events.send(Event::ImageFailed(url));
                }
            }
        }
        Command::SignOut => {
            if let Some(session) = shared.session.lock().await.take() {
                session.sign_out()?;
            }
            let _ = shared.events.send(Event::NeedsSignIn);
        }
    }
    Ok(())
}

/// A picture from the disk cache or the network, decoded and shrunk to `max_width`.
async fn picture(shared: &Shared, url: &str, max_width: u32) -> Result<(u32, u32, Vec<u8>)> {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let path = shared.dir.join("images").join(format!("{:016x}", hasher.finish()));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = shared
                .http
                .get(url)
                .header("Accept", "image/webp,image/*;q=0.8")
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?
                .to_vec();
            let _ = tokio::fs::create_dir_all(path.parent().expect("images dir")).await;
            let _ = tokio::fs::write(&path, &bytes).await;
            bytes
        }
    };
    let _permit = shared.decoding.acquire().await?;
    tokio::task::spawn_blocking(move || -> Result<(u32, u32, Vec<u8>)> {
        let mut image = image::load_from_memory(&bytes).context("decoding")?;
        if image.width() > max_width {
            let height = (image.height() as u64 * max_width as u64 / image.width() as u64) as u32;
            image = image.resize_exact(max_width, height.max(1), image::imageops::FilterType::Triangle);
        }
        let rgba = image.to_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    })
    .await?
}
