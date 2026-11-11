//! Opening an episode or movie for the player: the version in the audio wanted, its stream (with subtitles burned in,
//! in the language wanted, when the audio isn't already that language), the best renditions up to the height wanted,
//! their init segments, a Widevine license from the TV's own CDM, and the video window, then [`crunchy_player`]'s
//! engine on it. Only the TV plays; the simulator has a stand-in that only keeps time ([`Playback::simulated`]).

use std::time::Duration;

use anyhow::Result;
use crunchy_core::crunchyroll_rs::Crunchyroll;
use crunchy_core::crunchyroll_rs::media::Stream;
use crunchy_player::Status;

/// What to play: the audio and subtitles wanted, the tallest picture, and where to start.
#[derive(Clone, Debug)]
pub struct Wanted {
    /// plays an episode's version in this language ("ja-JP") when it has one, else its original one
    pub audio: Option<String>,
    /// burned in, unless the audio is already in this language ("off" or `None` for none)
    pub subtitles: Option<String>,
    pub max_height: u32,
    pub from: Duration,
}

/// What a stream offers the player's menus: the audio versions and the subtitles that can be burned in.
#[derive(Clone, Debug, Default)]
pub struct Tracks {
    /// the version playing (its id goes with the license, the skip events and the rating)
    pub id: String,
    pub audio: String,
    /// every version: its audio language and its id
    pub audios: Vec<(String, String)>,
    /// the language burned in, if any
    pub subtitle: Option<String>,
    pub subtitles: Vec<String>,
}

#[cfg(target_os = "linux")]
impl Tracks {
    fn of(id: &str, stream: &Stream, subtitles: Option<&str>) -> Tracks {
        let mut audios: Vec<(String, String)> =
            stream.versions.iter().map(|version| (version.audio_locale.to_string(), version.id.clone())).collect();
        if audios.is_empty() {
            audios.push((stream.audio_locale.to_string(), id.to_string()));
        }
        let mut hard_subs: Vec<String> = stream.hard_subs.keys().map(|locale| locale.to_string()).collect();
        hard_subs.sort();
        Tracks {
            id: id.to_string(),
            audio: stream.audio_locale.to_string(),
            audios,
            subtitle: burned_in(stream, subtitles).map(|locale| locale.to_string()),
            subtitles: hard_subs,
        }
    }
}

/// The subtitles to burn in: the ones wanted, unless the audio is already their language.
#[cfg(target_os = "linux")]
fn burned_in(stream: &Stream, wanted: Option<&str>) -> Option<crunchy_core::crunchyroll_rs::Locale> {
    wanted
        .filter(|wanted| *wanted != stream.audio_locale.to_string())
        .and_then(|wanted| stream.hard_subs.keys().find(|locale| locale.to_string() == wanted).cloned())
}

/// A stream being played; dropping it stops playback, then releases the stream.
pub struct Playback {
    #[cfg(target_os = "linux")]
    player: crunchy_player::engine::Player,
    #[cfg(not(target_os = "linux"))]
    clock: std::sync::Mutex<Clock>,
    pub tracks: Tracks,
    /// where the stream's seek thumbnails are (a BIF file), when it has them
    pub bif: Option<String>,
    _stream: Option<Release>,
}

impl Playback {
    pub fn status(&self) -> Status {
        #[cfg(target_os = "linux")]
        return self.player.status();
        #[cfg(not(target_os = "linux"))]
        self.clock.lock().unwrap().status()
    }

    pub fn pause(&self) {
        #[cfg(target_os = "linux")]
        self.player.pause();
        #[cfg(not(target_os = "linux"))]
        self.clock.lock().unwrap().pause();
    }

    pub fn resume(&self) {
        #[cfg(target_os = "linux")]
        self.player.resume();
        #[cfg(not(target_os = "linux"))]
        self.clock.lock().unwrap().resume();
    }

    pub fn seek(&self, to: Duration) {
        #[cfg(target_os = "linux")]
        self.player.seek(to);
        #[cfg(not(target_os = "linux"))]
        self.clock.lock().unwrap().seek(to);
    }

    /// For the simulator: no picture, a clock that runs like the video would.
    #[cfg(not(target_os = "linux"))]
    pub fn simulated(duration: Duration, from: Duration, tracks: Tracks) -> Playback {
        // the first picture takes a moment, like a stream's
        let until = std::time::Instant::now() + Duration::from_secs(2);
        let clock = Clock { base: from, since: Some(until), duration, loading_until: Some(until) };
        Playback { clock: std::sync::Mutex::new(clock), tracks, bif: None, _stream: None }
    }
}

/// The simulator's stand-in for a video: a position that runs while playing, and half a second of loading after a
/// seek.
#[cfg(not(target_os = "linux"))]
struct Clock {
    base: Duration,
    since: Option<std::time::Instant>,
    duration: Duration,
    loading_until: Option<std::time::Instant>,
}

#[cfg(not(target_os = "linux"))]
impl Clock {
    fn position(&self) -> Duration {
        let now = std::time::Instant::now();
        let run = self.since.map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        (self.base + run).min(self.duration)
    }

    fn status(&self) -> Status {
        use crunchy_player::State;
        let position = self.position();
        let state = if position >= self.duration {
            State::Ended
        } else if self.loading_until.is_some_and(|until| until > std::time::Instant::now()) {
            State::Loading
        } else if self.since.is_some() {
            State::Playing
        } else {
            State::Paused
        };
        Status { state, position, duration: self.duration }
    }

    fn pause(&mut self) {
        self.base = self.position();
        self.since = None;
    }

    fn resume(&mut self) {
        if self.since.is_none() {
            self.since = Some(self.loading_until.unwrap_or_else(std::time::Instant::now));
        }
    }

    fn seek(&mut self, to: Duration) {
        let playing = self.since.is_some();
        let until = std::time::Instant::now() + Duration::from_millis(500);
        self.base = to.min(self.duration);
        self.loading_until = Some(until);
        self.since = playing.then_some(until);
    }
}

/// A stream open at Crunchyroll, which counts how many an account has: released when dropped.
struct Release {
    stream: Option<Stream>,
    runtime: tokio::runtime::Handle,
}

impl Drop for Release {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.runtime.spawn(async move {
                let _ = stream.invalidate().await;
            });
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn open(_: &Crunchyroll, _: &str, _: &Wanted) -> Result<Playback> {
    anyhow::bail!("Los videos se reproducen en la TV")
}

#[cfg(target_os = "linux")]
pub async fn open(crunchy: &Crunchyroll, content_id: &str, wanted: &Wanted) -> Result<Playback> {
    use anyhow::Context;
    use crunchy_core::crunchyroll_rs::media::{Episode, Movie};

    let (id, stream) = match crunchy.media_from_id::<Episode>(content_id).await {
        Ok(episode) => {
            let episode = version(crunchy, episode, wanted.audio.as_deref()).await;
            (episode.id.clone(), episode.stream().await)
        }
        Err(_) => {
            let movie = crunchy.media_from_id::<Movie>(content_id).await.context("the video")?;
            (content_id.to_string(), movie.stream().await)
        }
    };
    let stream = stream.context("asking for the stream")?;
    let tracks = Tracks::of(&id, &stream, wanted.subtitles.as_deref());
    let bif = bif_link(&stream.bifs);
    eprintln!("player: seek thumbnails {}", if bif.is_some() { "in the stream" } else { "none" });
    let release = Release { stream: Some(stream), runtime: tokio::runtime::Handle::current() };
    let stream = release.stream.as_ref().unwrap();
    let player = tv::start(crunchy, &id, stream, wanted.subtitles.as_deref(), wanted.max_height, wanted.from).await?;
    Ok(Playback { player, tracks, bif, _stream: Some(release) })
}

/// The seek thumbnails' link in a play answer: a string, or a list whose first entry is one.
#[cfg(target_os = "linux")]
fn bif_link(value: &serde_json::Value) -> Option<String> {
    let link = value.as_str().or_else(|| value.as_array()?.first()?.as_str())?;
    link.starts_with("http").then(|| link.to_string())
}

/// The episode's version in `audio`, else its original-language one, else the episode as it is.
#[cfg(target_os = "linux")]
async fn version(
    crunchy: &Crunchyroll,
    episode: crunchy_core::crunchyroll_rs::media::Episode,
    audio: Option<&str>,
) -> crunchy_core::crunchyroll_rs::media::Episode {
    let wanted = audio
        .and_then(|audio| episode.versions.iter().find(|version| version.audio_locale.to_string() == audio))
        .or_else(|| episode.versions.iter().find(|version| version.original))
        .map(|version| version.id.clone());
    match wanted {
        Some(id) if id != episode.id => crunchy.media_from_id(&id).await.unwrap_or(episode),
        _ => episode,
    }
}

#[cfg(target_os = "linux")]
mod tv {
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result, bail};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use crunchy_core::crunchyroll_rs::Crunchyroll;
    use crunchy_core::crunchyroll_rs::media::{MediaStreamDRMType, Stream, StreamSegment};
    use crunchy_core::play;
    use crunchy_player::cdm::Widevine;
    use crunchy_player::engine::{Media, Player, Rendition, Segment, Setup};
    use crunchy_player::mp4::{self, Init, Kind};

    pub async fn start(
        crunchy: &Crunchyroll,
        content_id: &str,
        stream: &Stream,
        subtitles: Option<&str>,
        max_height: u32,
        from: Duration,
    ) -> Result<Player> {
        let started = Instant::now();
        let data = match super::burned_in(stream, subtitles) {
            Some(locale) => stream.stream_data(Some(locale)).await?,
            None => None,
        };
        let data = match data {
            Some(data) => data,
            None => stream.stream_data(None).await?.context("no stream data")?,
        };
        let video = data
            .video
            .iter()
            .filter(|video| video.resolution.height <= u64::from(max_height))
            .max_by_key(|video| video.bandwidth)
            .or_else(|| data.video.iter().min_by_key(|video| video.resolution.height))
            .context("no video")?;
        let audio = data.audio.iter().max_by_key(|audio| audio.bandwidth).context("no audio")?;
        eprintln!("player: {}x{} {}kbps, audio {}kbps", video.resolution.width, video.resolution.height, video.bandwidth / 1000, audio.bandwidth / 1000);
        let (video_segments, audio_segments) = tokio::try_join!(video.segments(), audio.segments())?;
        let ((video_rendition, video_init), (audio_rendition, _)) =
            tokio::try_join!(rendition(&video_segments, Kind::Video), rendition(&audio_segments, Kind::Audio))?;

        // Widevine's init data: the init segment's pssh boxes, else the manifest's
        let mut pssh = video_init.pssh_for(&mp4::WIDEVINE);
        if pssh.is_empty() {
            for drm in video.drm.iter().flat_map(|drm| &drm.types) {
                if let MediaStreamDRMType::Widevine { pssh: boxes } = drm {
                    for pssh_box in boxes {
                        pssh.extend(B64.decode(pssh_box.trim())?);
                    }
                }
            }
        }
        let (mut cdm, challenge) = tokio::task::spawn_blocking(move || -> Result<(Widevine, Vec<u8>)> {
            let mut cdm = Widevine::new()?;
            cdm.set_server_certificate(&play::service_certificate())?;
            let challenge = cdm.license_request(&pssh)?;
            Ok((cdm, challenge))
        })
        .await??;
        let license = play::widevine_license(crunchy, &stream.drm.drm_url, content_id, &stream.token, challenge).await?;
        let cdm = tokio::task::spawn_blocking(move || -> Result<Widevine> {
            let keys = cdm.update(&license)?;
            if !keys.is_empty() && keys.iter().all(|key| key.status != 0) {
                bail!("the license's keys can't be used here");
            }
            Ok(cdm)
        })
        .await??;
        eprintln!("player: licensed in {:?}", started.elapsed());

        // the page asked for the window before asking for the video: its id is normally here by now
        let window_id = loop {
            if let Some(id) = crate::platform::video_window() {
                break id;
            }
            if started.elapsed() > Duration::from_secs(15) {
                bail!("no video window");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let media = Media { video: video_rendition, audio: audio_rendition };
        let setup = Setup { app_id: crate::platform::app_id(), window_id, from };
        Player::start(media, cdm, setup, tokio::runtime::Handle::current())
    }

    /// A rendition's track (from its init segment) and media segments.
    async fn rendition(segments: &[StreamSegment], kind: Kind) -> Result<(Rendition, Init)> {
        let (first, rest) = segments.split_first().context("no segments")?;
        let init = Init::parse(&first.data().await?)?;
        let track = init.track(kind).context("no track of that kind")?.clone();
        let segments = rest
            .iter()
            .map(|segment| Segment { url: segment.url.clone(), range: segment.range, duration: segment.length })
            .collect();
        Ok((Rendition { track, segments }, init))
    }
}
