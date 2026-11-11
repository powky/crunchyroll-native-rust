//! Opening an episode or movie for the player: its stream (with subtitles burned in, in the profile's language, when
//! the audio isn't already that language), the best renditions up to 1080p, their init segments, a Widevine license
//! from the TV's own CDM, and the video window, then [`crunchy_player`]'s engine on it. Only the TV plays.

use std::time::Duration;

use anyhow::Result;
use crunchy_core::crunchyroll_rs::Crunchyroll;
use crunchy_core::crunchyroll_rs::media::Stream;
use crunchy_player::Status;

/// A stream being played; dropping it stops playback, then releases the stream.
pub struct Playback {
    #[cfg(target_os = "linux")]
    player: crunchy_player::engine::Player,
    _stream: Release,
}

impl Playback {
    pub fn status(&self) -> Status {
        #[cfg(target_os = "linux")]
        return self.player.status();
        #[cfg(not(target_os = "linux"))]
        unreachable!()
    }

    pub fn pause(&self) {
        #[cfg(target_os = "linux")]
        self.player.pause();
    }

    pub fn resume(&self) {
        #[cfg(target_os = "linux")]
        self.player.resume();
    }

    pub fn seek(&self, to: Duration) {
        #[cfg(target_os = "linux")]
        self.player.seek(to);
        #[cfg(not(target_os = "linux"))]
        let _ = to;
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
pub async fn open(_: &Crunchyroll, _: &str, _: Option<&str>, _: Duration) -> Result<Playback> {
    anyhow::bail!("Los videos se reproducen en la TV")
}

#[cfg(target_os = "linux")]
pub async fn open(crunchy: &Crunchyroll, content_id: &str, subtitles: Option<&str>, from: Duration) -> Result<Playback> {
    use anyhow::Context;
    use crunchy_core::crunchyroll_rs::media::{Episode, Movie};

    let stream = match crunchy.media_from_id::<Episode>(content_id).await {
        Ok(episode) => episode.stream().await,
        Err(_) => crunchy.media_from_id::<Movie>(content_id).await.context("the video")?.stream().await,
    }
    .context("asking for the stream")?;
    let release = Release { stream: Some(stream), runtime: tokio::runtime::Handle::current() };
    let player = tv::start(crunchy, content_id, release.stream.as_ref().unwrap(), subtitles, from).await?;
    Ok(Playback { player, _stream: release })
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
        from: Duration,
    ) -> Result<Player> {
        let started = Instant::now();
        // subtitles burned in, unless the audio is already in their language
        let hardsub = subtitles
            .filter(|wanted| *wanted != stream.audio_locale.to_string())
            .and_then(|wanted| stream.hard_subs.keys().find(|locale| locale.to_string() == wanted).cloned());
        let data = match hardsub {
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
            .filter(|video| video.resolution.height <= 1080)
            .max_by_key(|video| video.bandwidth)
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
        let setup = Setup { app_id: std::env::var("APPID").unwrap_or("com.ian.crunchy".into()), window_id, from };
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
