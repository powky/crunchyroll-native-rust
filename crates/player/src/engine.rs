//! Playback of one stream: segments are fetched ahead on the async runtime and turned into what the pipeline takes
//! (demuxed, rewritten, through the CDM), then fed in presentation order from a thread of its own, which also
//! answers the controls and follows the pipeline's events.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::cdm::{Encrypted, Scheme, Widevine};
use crate::mp4::{self, Codec, Sample, Track};
use crate::starfish::{EVENT_POSITION, Fed, Pipeline};
use crate::{State, Status, Subsample, es};

/// Pipeline event types (webOS 5 on).
const EVENT_ERROR: i32 = 18;
const EVENT_END_OF_STREAM: i32 = 28;
/// Segments prepared ahead of the one being fed.
const AHEAD: usize = 2;
/// Video fed before playing starts (unless the pipeline is full first).
const PREROLL: Duration = Duration::from_secs(2);
/// How far past the position the pipeline is fed (it takes far more, and then plays out a minute before asking again).
const FEED_AHEAD: Duration = Duration::from_secs(20);

/// One media segment: a byte range of a file (or the whole file).
#[derive(Clone, Debug)]
pub struct Segment {
    pub url: String,
    pub range: Option<(u64, u64)>,
    pub duration: Duration,
}

/// A track and its segments, the init segment already parsed into `track`.
pub struct Rendition {
    pub track: Track,
    pub segments: Vec<Segment>,
}

pub struct Media {
    pub video: Rendition,
    pub audio: Rendition,
}

impl Media {
    pub fn duration(&self) -> Duration {
        self.video.segments.iter().map(|segment| segment.duration).sum()
    }

    /// The segment holding `time`, and where it starts.
    fn segment_at(&self, time: Duration) -> (usize, Duration) {
        let mut start = Duration::ZERO;
        for (index, segment) in self.video.segments.iter().enumerate() {
            if time < start + segment.duration {
                return (index, start);
            }
            start += segment.duration;
        }
        (self.video.segments.len().saturating_sub(1), start)
    }
}

enum Control {
    Pause,
    Resume,
    Seek(Duration),
    Stop,
}

/// What `Player::start` needs besides the media.
pub struct Setup {
    pub app_id: String,
    /// the exported video window the picture goes to
    pub window_id: String,
    pub from: Duration,
}

/// A playing stream; dropping it stops playback.
pub struct Player {
    control: Sender<Control>,
    status: Arc<Mutex<Status>>,
}

impl Player {
    pub fn start(media: Media, cdm: Widevine, setup: Setup, runtime: tokio::runtime::Handle) -> Result<Player> {
        let media = Arc::new(media);
        let duration = media.duration();
        let from = setup.from.min(duration.saturating_sub(Duration::from_secs(5)));
        let status = Arc::new(Mutex::new(Status { state: State::Loading, position: from, duration }));
        let mut pipeline = Pipeline::new()?;
        let events = pipeline.load(&load_payload(&media, &setup, from))?;
        let (control, controls) = channel();
        let fetcher = Fetcher { media, cdm: Arc::new(Mutex::new(cdm)), http: reqwest::Client::new(), runtime };
        let thread_status = status.clone();
        std::thread::Builder::new().name("player".into()).spawn(move || {
            let mut feeder = Feeder::new(pipeline, events, fetcher, thread_status);
            feeder.run(from, controls);
        })?;
        Ok(Player { control, status })
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn pause(&self) {
        let _ = self.control.send(Control::Pause);
    }

    pub fn resume(&self) {
        let _ = self.control.send(Control::Resume);
    }

    pub fn seek(&self, to: Duration) {
        let _ = self.control.send(Control::Seek(to));
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
    }
}

/// The TV's Chromium sends this for Crunchyroll (captured with ls-monitor), the video window and start included.
fn load_payload(media: &Media, setup: &Setup, from: Duration) -> String {
    let track = &media.video.track;
    serde_json::json!({"args": [{
        "mediaTransportType": "BUFFERSTREAM",
        "option": {
            "adaptiveStreaming": {"maxFrameRate": 60, "maxHeight": track.height.max(1080), "maxWidth": track.width.max(1920)},
            "appId": setup.app_id,
            "externalStreamingInfo": {
                "contents": {
                    "codec": {"audio": "AAC", "video": "H264"},
                    "esInfo": {"ptsToDecode": from.as_nanos() as u64, "seperatedPTS": true},
                    "format": "RAW",
                    "provider": "Chrome"
                },
                "streamQualityInfo": true
            },
            "queryPosition": true,
            "windowId": setup.window_id
        }
    }]})
    .to_string()
}

/// One access unit ready to feed.
struct Unit {
    data: Vec<u8>,
    pts: u64,
    video: bool,
}

/// The units of one video segment and the audio segment of the same index.
struct Chunk {
    video: Vec<Unit>,
    audio: Vec<Unit>,
}

#[derive(Clone)]
struct Fetcher {
    media: Arc<Media>,
    cdm: Arc<Mutex<Widevine>>,
    http: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl Fetcher {
    /// Prepares segments from `index` on, a few ahead; `None` arrives… never: the channel closes at the end.
    fn start(&self, index: usize) -> (mpsc::Receiver<Result<Chunk>>, JoinHandle<()>) {
        let (sender, receiver) = mpsc::channel(AHEAD);
        let fetcher = self.clone();
        let task = self.runtime.spawn(async move {
            let count = fetcher.media.video.segments.len().max(fetcher.media.audio.segments.len());
            for index in index..count {
                let chunk = fetcher.chunk(index).await;
                let failed = chunk.is_err();
                if sender.send(chunk).await.is_err() || failed {
                    return;
                }
            }
        });
        (receiver, task)
    }

    async fn chunk(&self, index: usize) -> Result<Chunk> {
        let video = self.media.video.segments.get(index);
        let audio = self.media.audio.segments.get(index);
        let (video, audio) = tokio::join!(self.fetch(video), self.fetch(audio));
        let (video, audio) = (video?, audio?);
        let fetcher = self.clone();
        tokio::task::spawn_blocking(move || {
            let cdm = fetcher.cdm.lock().unwrap();
            Ok(Chunk {
                video: units(&cdm, &video, &fetcher.media.video.track)?,
                audio: units(&cdm, &audio, &fetcher.media.audio.track)?,
            })
        })
        .await?
    }

    async fn fetch(&self, segment: Option<&Segment>) -> Result<Vec<u8>> {
        let Some(segment) = segment else { return Ok(Vec::new()) };
        let mut last = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
            }
            let mut request = self.http.get(&segment.url);
            if let Some((start, end)) = segment.range {
                request = request.header("Range", format!("bytes={start}-{end}"));
            }
            match request.send().await.and_then(|response| response.error_for_status()) {
                Ok(response) => match response.bytes().await {
                    Ok(bytes) => return Ok(bytes.to_vec()),
                    Err(error) => last = Some(error),
                },
                Err(error) => last = Some(error),
            }
        }
        Err(last.map(anyhow::Error::from).unwrap_or_else(|| anyhow::anyhow!("fetch failed"))).context("fetching a segment")
    }
}

/// A segment's samples as the pipeline takes them.
fn units(cdm: &Widevine, segment: &[u8], track: &Track) -> Result<Vec<Unit>> {
    if segment.is_empty() {
        return Ok(Vec::new());
    }
    let video = track.kind == mp4::Kind::Video;
    let mut units = Vec::new();
    for sample in mp4::parse_fragments(segment, track)? {
        let mut subsamples = sample.subsamples.clone();
        let data = match &track.codec {
            Codec::Avc { nal_length, sps, pps } => {
                let prefix = if sample.keyframe { es::parameter_sets(sps, pps) } else { Vec::new() };
                es::annex_b(sample.data(segment), *nal_length, &prefix, &mut subsamples)?
            }
            Codec::Aac(config) => es::adts(sample.data(segment), *config, !sample.iv.is_empty(), &mut subsamples),
            Codec::Unknown(kind) => bail!("unsupported codec {}", String::from_utf8_lossy(kind)),
        };
        let data = if sample.iv.is_empty() { data } else { cdm.decrypt(&encrypted(&data, &subsamples, &sample, track))? };
        units.push(Unit { data, pts: sample.pts_ns(track.timescale), video });
    }
    Ok(units)
}

fn encrypted<'a>(data: &'a [u8], subsamples: &'a [Subsample], sample: &'a Sample, track: &'a Track) -> Encrypted<'a> {
    let protection = track.protection.as_ref();
    Encrypted {
        data,
        scheme: if protection.is_some_and(|p| &p.scheme == b"cbcs") { Scheme::Cbcs } else { Scheme::Cenc },
        key_id: protection.map_or(&[][..], |p| &p.key_id[..]),
        iv: &sample.iv,
        subsamples,
        pattern: protection.map_or((0, 0), |p| (p.crypt_blocks as u32, p.skip_blocks as u32)),
        timestamp_us: (sample.pts_ns(track.timescale) / 1000) as i64,
    }
}

struct Feeder {
    pipeline: Pipeline,
    events: Receiver<crate::starfish::Event>,
    fetcher: Fetcher,
    status: Arc<Mutex<Status>>,
    chunks: Option<mpsc::Receiver<Result<Chunk>>>,
    task: Option<JoinHandle<()>>,
    video: VecDeque<Unit>,
    audio: VecDeque<Unit>,
    /// play once the preroll is fed (false while paused by the user)
    play_when_ready: bool,
    started: bool,
    /// first video pts fed since the start or the last seek
    first_video: Option<u64>,
    end_sent: bool,
    /// units fed (video, audio), for the log
    fed: (u64, u64),
    /// the position last read, when it last changed, when it was read
    position: i64,
    moved_at: Instant,
    polled_at: Instant,
    position_logged: Instant,
    /// playing was asked for before the load completed
    play_pending: bool,
}

impl Feeder {
    fn new(pipeline: Pipeline, events: Receiver<crate::starfish::Event>, fetcher: Fetcher, status: Arc<Mutex<Status>>) -> Feeder {
        Feeder {
            pipeline,
            events,
            fetcher,
            status,
            chunks: None,
            task: None,
            video: VecDeque::new(),
            audio: VecDeque::new(),
            play_when_ready: true,
            started: false,
            first_video: None,
            end_sent: false,
            fed: (0, 0),
            position: -1,
            moved_at: Instant::now(),
            polled_at: Instant::now(),
            position_logged: Instant::now(),
            play_pending: false,
        }
    }

    fn run(&mut self, from: Duration, controls: Receiver<Control>) {
        eprintln!("player: from {from:?}");
        self.fetch_from(from);
        loop {
            match controls.try_recv() {
                Ok(Control::Stop) | Err(TryRecvError::Disconnected) => break,
                Ok(Control::Pause) => {
                    self.play_when_ready = false;
                    if self.started {
                        self.pipeline.pause();
                    }
                    self.set_state(State::Paused);
                }
                Ok(Control::Resume) => {
                    self.play_when_ready = true;
                    if self.started {
                        self.play_now();
                    } else {
                        self.set_state(State::Loading);
                    }
                }
                Ok(Control::Seek(to)) => self.seek(to),
                Err(TryRecvError::Empty) => {}
            }
            if !self.pipeline_events() {
                break;
            }
            match self.feed_one() {
                Ok(true) => {}
                Ok(false) => std::thread::sleep(Duration::from_millis(15)),
                Err(error) => {
                    eprintln!("player: {error:#}");
                    self.set_state(State::Failed(format!("{error:#}")));
                    break;
                }
            }
            self.poll_position();
            // playing, yet the position stood still this long: the pipeline is stuck
            if self.started && self.play_when_ready && !self.end_sent && self.moved_at.elapsed() > Duration::from_secs(20) {
                self.set_state(State::Failed("the video stopped".into()));
                break;
            }
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.pipeline.unload();
    }

    fn fetch_from(&mut self, time: Duration) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let (index, _) = self.fetcher.media.segment_at(time);
        let (chunks, task) = self.fetcher.start(index);
        self.chunks = Some(chunks);
        self.task = Some(task);
    }

    fn seek(&mut self, to: Duration) {
        let duration = self.status.lock().unwrap().duration;
        let to = to.min(duration.saturating_sub(Duration::from_secs(3)));
        self.video.clear();
        self.audio.clear();
        self.end_sent = false;
        self.started = false;
        self.first_video = None;
        self.position = -1;
        self.pipeline.seek(to.as_millis() as u64);
        self.fetch_from(to);
        let mut status = self.status.lock().unwrap();
        status.position = to;
        if status.state != State::Paused {
            status.state = State::Loading;
        }
    }

    /// Reads the position a few times a second (once playing).
    fn poll_position(&mut self) {
        if self.polled_at.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.polled_at = Instant::now();
        if self.play_pending && self.play_when_ready {
            self.play_now();
        }
        let position = self.pipeline.position();
        if position != self.position {
            self.position = position;
            self.moved_at = Instant::now();
        }
        if !self.started || self.status.lock().unwrap().state == State::Paused {
            self.moved_at = Instant::now();
        }
        if self.position_logged.elapsed() > Duration::from_secs(60) {
            self.position_logged = Instant::now();
            eprintln!("player: position {position}, fed {:?}", self.fed);
        }
        if self.started && position >= 0 {
            self.status.lock().unwrap().position = Duration::from_nanos(position as u64);
        }
    }

    /// Follows the pipeline; `false` when it failed.
    fn pipeline_events(&mut self) -> bool {
        while let Ok(event) = self.events.try_recv() {
            match event.kind {
                EVENT_POSITION => {
                    if self.started && event.number >= 0 {
                        self.position = event.number;
                        self.moved_at = Instant::now();
                        self.status.lock().unwrap().position = Duration::from_nanos(event.number as u64);
                    }
                }
                EVENT_END_OF_STREAM => self.set_state(State::Ended),
                EVENT_ERROR => {
                    eprintln!("player: pipeline error {} {}", event.number, event.text);
                    self.set_state(State::Failed(format!("{} ({})", event.text, event.number)));
                    return false;
                }
                kind => eprintln!("player: event {kind} {} {}", event.number, event.text),
            }
        }
        true
    }

    /// Feeds the next unit; `false` when there was nothing to do (or the pipeline is full).
    fn feed_one(&mut self) -> Result<bool> {
        self.receive()?;
        let more_coming = self.chunks.is_some();
        let take_video = match (self.video.front(), self.audio.front()) {
            (Some(video), Some(audio)) => video.pts <= audio.pts,
            // one queue ran dry while more is coming: wait for it instead of running ahead with the other
            (Some(_), None) | (None, Some(_)) if more_coming => return Ok(false),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => {
                if !more_coming && !self.end_sent {
                    self.pipeline.end_of_stream();
                    self.end_sent = true;
                    self.start_playing();
                }
                return Ok(false);
            }
        };
        let queue = if take_video { &mut self.video } else { &mut self.audio };
        let unit = queue.front().unwrap();
        if self.started && self.position > 0 && unit.pts > self.position as u64 + FEED_AHEAD.as_nanos() as u64 {
            return Ok(false);
        }
        match self.pipeline.feed(&unit.data, unit.pts, unit.video) {
            Fed::Ok => {
                if unit.video { self.fed.0 += 1 } else { self.fed.1 += 1 }
                if unit.video && self.first_video.is_none() {
                    self.first_video = Some(unit.pts);
                }
                let pts = unit.pts;
                let video = unit.video;
                queue.pop_front();
                if !self.started
                    && video
                    && self.first_video.is_some_and(|first| pts.saturating_sub(first) >= PREROLL.as_nanos() as u64)
                {
                    self.start_playing();
                }
                Ok(true)
            }
            Fed::Full => {
                self.start_playing();
                Ok(false)
            }
            Fed::Other(reply) => bail!("the pipeline refused data: {reply}"),
        }
    }

    /// Moves a prepared chunk into the queues when they run low.
    fn receive(&mut self) -> Result<()> {
        if self.video.len() > 10 && self.audio.len() > 10 {
            return Ok(());
        }
        let Some(chunks) = self.chunks.as_mut() else { return Ok(()) };
        match chunks.try_recv() {
            Ok(chunk) => {
                let chunk = chunk?;
                self.video.extend(chunk.video);
                self.audio.extend(chunk.audio);
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => self.chunks = None,
        }
        Ok(())
    }

    fn start_playing(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        eprintln!(
            "player: starting after {} video / {} audio units, first video at {:?}",
            self.fed.0,
            self.fed.1,
            self.first_video.map(Duration::from_nanos)
        );
        if self.play_when_ready {
            self.play_now();
        }
    }

    /// `Play` once loading completed; until then it's asked again on every poll.
    fn play_now(&mut self) {
        self.play_pending = !self.pipeline.load_completed();
        if !self.play_pending {
            let played = self.pipeline.play();
            eprintln!("player: play {played}");
        }
        self.set_state(State::Playing);
    }

    fn set_state(&self, state: State) {
        self.status.lock().unwrap().state = state;
    }
}
