//! Checks the DRM side of playback on the TV from an ssh shell, outside the app: `spike <episode id> [--info]` gets the
//! stream, a Widevine license through the TV's CDM and decrypts a few samples; `spike history` lists recent episodes.
//! (Feeding the pipeline needs the app's luna role and a foreground window, so that's tried in the app.) Uses the
//! app's saved session (close the app first: the refresh token rotates) and turns into the app's user before touching
//! it, so the session stays readable by the app. Prints sizes and statuses, never tokens.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use crunchy_core::crunchyroll_rs::media::{Episode, MediaStreamDRMType, Stream};
use crunchy_core::crunchyroll_rs::{Crunchyroll, Locale};
use crunchy_core::platform::Platform;
use crunchy_core::session::Session;
use crunchy_core::store::Store;
use crunchy_player::cdm::{Encrypted, Scheme, Widevine};
use crunchy_player::mp4::{self, Codec, Init, Kind, Sample, Track};
use crunchy_player::{Subsample, es};

/// The license server's Widevine service certificate (public; the web app ships it too).
const SERVICE_CERTIFICATE: &str = "CrsCCAMSEKDc0WAwLAQT1SB2ogyBJEwYv4Tx7gUijgIwggEKAoIBAQC8Xc/GTRwZDtlnBThq8V382D1oJAM0F/YgCQtNDLz7vTWJ+QskNGi5Dd2qzO4s48Cnx5BLvL4H0xCRSw2Ed6ekHSdrRUwyoYOE+M/t1oIbccwlTQ7o+BpV1X6TB7fxFyx1jsBtRsBWphU65w121zqmSiwzZzJ4xsXVQCJpQnNI61gzHO42XZOMuxytMm0F6puNHTTqhyY3Z290YqvSDdOB+UY5QJuXJgjhvOUD9+oaLlvT+vwmV2/NJWxKqHBKdL9JqvOnNiQUF0hDI7Wf8Wb63RYSXKE27Ky31hKgx1wuq7TTWkA+kHnJTUrTEfQxfPR4dJTquE+IDLAi5yeVVxzbAgMBAAE6DGNhc3RsYWJzLmNvbUABEoADMmGXpXg/0qxUuwokpsqVIHZrJfu62ar+BF8UVUKdK5oYQoiTZd9OzK3kr29kqGGk3lSgM0/p499p/FUL8oHHzgsJ7Hajdsyzn0Vs3+VysAgaJAkXZ+k+N6Ka0WBiZlCtcunVJDiHQbz1sF9GvcePUUi2fM/h7hyskG5ZLAyJMzTvgnV3D8/I5Y6mCFBPb/+/Ri+9bEvquPF3Ff9ip3yEHu9mcQeEYCeGe9zR/27eI5MATX39gYtCnn7dDXVxo4/rCYK0A4VemC3HRai2X3pSGcsKY7+6we7h4IycjqtuGtYg8AbaigovcoURAZcr1d/G0rpREjLdVLG0Gjqk63Gx688W5gh3TKemsK3R1jV0dOfj3e6uV/kTpsNRL9KsD0v7ysBQVdUXEbJotcFz71tI5qc3jwr6GjYIPA3VzusD17PN6AGQniMwxJV12z/EgnUopcFB13osydpD2AaDsgWo5RWJcNf+fzCgtUQx/0Au9+xVm5LQBdv8Ja4f2oiHN3dw";
const DATA_DIR: &str = "/media/developer/apps/usr/palm/data/com.crunchyroll.webos.native";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let id = args.first().context("usage: spike <episode id | history> [--info] [--root]")?.clone();
    let dir = PathBuf::from(std::env::var("CRUNCHY_DATA_DIR").unwrap_or(DATA_DIR.into()));
    if !args.iter().any(|a| a == "--root") {
        become_owner_of(&dir)?;
    }
    log(&format!("uid {} gid {}", unsafe { libc::getuid() }, unsafe { libc::getgid() }));

    let platform = Platform::load(&dir, false).await?;
    let session = Session::restore(Store::new(&dir), platform, Locale::es_419).await?.context("not signed in")?;
    let crunchy = session.crunchy.clone();
    if id == "history" {
        let ctx = crunchy_core::content::Ctx::new(crunchy.clone(), "es-419".to_string()).await?;
        for card in ctx.history(1, 8).await?.cards {
            let episode = card.episode.map(|e| format!("T{} E{} {}", e.season, e.number, e.name)).unwrap_or_default();
            println!("{} {} {episode}", card.id, card.title);
        }
        return Ok(());
    }
    let episode: Episode = crunchy.media_from_id(&id).await.context("episode")?;
    log(&format!("episode: {} E{} {}", episode.series_title, episode.episode_number.unwrap_or(0), episode.title));
    let stream = episode.stream().await.context("stream")?;
    log(&format!(
        "stream: drm {} at {}, audio {}, {} versions, {} subtitles, limits {}",
        stream.drm.name,
        stream.drm.drm_url.split('?').next().unwrap_or(""),
        stream.audio_locale,
        stream.versions.len(),
        stream.subtitles.len(),
        stream.session.uses_stream_limits
    ));
    let mut hard: Vec<String> = stream.hard_subs.keys().map(|l| l.to_string()).collect();
    hard.sort();
    log(&format!("hardsubs: {}", hard.join(" ")));
    for sub in &stream.subtitles {
        log(&format!("subtitle {} {}", sub.locale, sub.format));
    }
    if args.iter().any(|a| a == "--info") {
        let _ = stream.invalidate().await;
        return Ok(());
    }
    let result = run(&crunchy, &stream, &id).await;
    let _ = stream.invalidate().await;
    log("stream released");
    result
}

async fn run(crunchy: &Crunchyroll, stream: &Stream, id: &str) -> Result<()> {
    let data = stream.stream_data(None).await?.context("no stream data")?;
    for video in &data.video {
        log(&format!("video {}x{} {} {}kbps", video.resolution.width, video.resolution.height, video.codecs, video.bandwidth / 1000));
    }
    for audio in &data.audio {
        log(&format!("audio {} {}Hz {}kbps", audio.codecs, audio.sampling_rate, audio.bandwidth / 1000));
    }
    let video = data
        .video
        .iter()
        .filter(|video| video.resolution.height <= 1080)
        .max_by_key(|video| video.bandwidth)
        .context("no video")?;
    let audio = data.audio.iter().max_by_key(|audio| audio.bandwidth).context("no audio")?;
    let video_segments = video.segments().await?;
    let audio_segments = audio.segments().await?;
    log(&format!("segments: video {}, audio {}", video_segments.len(), audio_segments.len()));

    let video_init = Init::parse(&video_segments[0].data().await?)?;
    let audio_init = Init::parse(&audio_segments[0].data().await?)?;
    let video_track = video_init.track(Kind::Video).context("no video track")?.clone();
    let audio_track = audio_init.track(Kind::Audio).context("no audio track")?.clone();
    describe(&video_track);
    describe(&audio_track);

    // Widevine init data: the init segment's pssh boxes, else the manifest's
    let mut pssh = video_init.pssh_for(&mp4::WIDEVINE);
    if pssh.is_empty() {
        for drm in video.drm.iter().flat_map(|drm| &drm.types) {
            if let MediaStreamDRMType::Widevine { pssh: boxes } = drm {
                for b in boxes {
                    pssh.extend(B64.decode(b.trim())?);
                }
            }
        }
    }
    log(&format!("pssh: {} bytes", pssh.len()));

    log(&format!("cdm version {}", Widevine::version()?));
    let started = Instant::now();
    let mut cdm = Widevine::new()?;
    log(&format!("cdm ready in {:?}", started.elapsed()));
    cdm.set_server_certificate(&B64.decode(SERVICE_CERTIFICATE)?)?;
    log("service certificate set");
    let challenge = cdm.license_request(&pssh)?;
    log(&format!("license request: {} bytes", challenge.len()));
    let license = fetch_license(crunchy, &stream.drm.drm_url, id, &stream.token, challenge).await?;
    log(&format!("license: {} bytes", license.len()));
    let keys = cdm.update(&license)?;
    for key in &keys {
        log(&format!("key {} status {}", hex(&key.id), key.status));
    }

    let first = video_segments[1].data().await?;
    let samples = mp4::parse_fragments(&first, &video_track)?;
    log(&format!("first video segment: {} bytes, {} samples", first.len(), samples.len()));
    for sample in samples.iter().take(3) {
        let es = video_es(&first, sample, &video_track)?;
        let out = cdm.decrypt(&encrypted(&es, sample, &video_track))?;
        let changed = es.0.iter().zip(&out).filter(|(a, b)| a != b).count();
        let first_change = es.0.iter().zip(&out).position(|(a, b)| a != b);
        log(&format!(
            "video pts {} key {} in {} ({:?}) out {}, {changed} bytes changed from {first_change:?}",
            sample.pts_ns(video_track.timescale) / 1_000_000,
            sample.keyframe,
            es.0.len(),
            es.1,
            out.len(),
        ));
    }
    let first_audio = audio_segments[1].data().await?;
    let audio_samples = mp4::parse_fragments(&first_audio, &audio_track)?;
    for sample in audio_samples.iter().take(2) {
        let es = audio_es(&first_audio, sample, &audio_track)?;
        let out = cdm.decrypt(&encrypted(&es, sample, &audio_track))?;
        log(&format!("audio in {} out {} head {}", es.0.len(), out.len(), hex(&out[..out.len().min(24)])));
    }
    Ok(())
}

/// The sample as H.264 Annex B (parameter sets before keyframes) and its adjusted subsamples.
fn video_es(segment: &[u8], sample: &Sample, track: &Track) -> Result<(Vec<u8>, Vec<Subsample>)> {
    let Codec::Avc { nal_length, sps, pps } = &track.codec else { bail!("not H.264: {:?}", track.codec) };
    let prefix = if sample.keyframe { es::parameter_sets(sps, pps) } else { Vec::new() };
    let mut subsamples = sample.subsamples.clone();
    let data = es::annex_b(sample.data(segment), *nal_length, &prefix, &mut subsamples)?;
    Ok((data, subsamples))
}

fn audio_es(segment: &[u8], sample: &Sample, track: &Track) -> Result<(Vec<u8>, Vec<Subsample>)> {
    let Codec::Aac(config) = &track.codec else { bail!("not AAC: {:?}", track.codec) };
    let mut subsamples = sample.subsamples.clone();
    let data = es::adts(sample.data(segment), *config, !sample.iv.is_empty(), &mut subsamples);
    Ok((data, subsamples))
}

fn encrypted<'a>(es: &'a (Vec<u8>, Vec<Subsample>), sample: &'a Sample, track: &'a Track) -> Encrypted<'a> {
    let protection = track.protection.as_ref();
    let scheme = match protection.map(|p| &p.scheme) {
        _ if sample.iv.is_empty() => Scheme::Clear,
        Some(b"cbcs") => Scheme::Cbcs,
        _ => Scheme::Cenc,
    };
    Encrypted {
        data: &es.0,
        scheme,
        key_id: protection.map_or(&[][..], |p| &p.key_id[..]),
        iv: &sample.iv,
        subsamples: &es.1,
        pattern: protection.map_or((0, 0), |p| (p.crypt_blocks as u32, p.skip_blocks as u32)),
        timestamp_us: (sample.pts_ns(track.timescale) / 1000) as i64,
    }
}

async fn fetch_license(crunchy: &Crunchyroll, url: &str, id: &str, token: &str, challenge: Vec<u8>) -> Result<Vec<u8>> {
    let url = if url.is_empty() { "https://cr-license-proxy.prd.crunchyrollsvc.com/v1/license/widevine" } else { url };
    let response = crunchy
        .client()
        .post(url)
        .bearer_auth(crunchy.access_token().await)
        .header("Content-Type", "application/octet-stream")
        .header("X-Cr-Content-Id", id)
        .header("X-Cr-Video-Token", token)
        .body(challenge)
        .send()
        .await?;
    let status = response.status();
    let body = response.bytes().await?;
    if !status.is_success() {
        bail!("license server {status}: {}", String::from_utf8_lossy(&body[..body.len().min(200)]));
    }
    // Crunchyroll wraps Widevine licenses in JSON
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body)
        && let Some(license) = json.get("license").and_then(|l| l.as_str())
    {
        return Ok(B64.decode(license)?);
    }
    Ok(body.to_vec())
}

fn describe(track: &Track) {
    let codec = match &track.codec {
        Codec::Avc { nal_length, sps, pps } => format!("avc nal {nal_length} sps {} pps {}", sps.len(), pps.len()),
        Codec::Aac(config) => format!("aac {config:?}"),
        Codec::Unknown(kind) => format!("unknown {}", String::from_utf8_lossy(kind)),
    };
    let protection = track.protection.as_ref().map(|p| {
        format!("{} iv {} kid {} pattern {}/{}", String::from_utf8_lossy(&p.scheme), p.iv_size, hex(&p.key_id), p.crypt_blocks, p.skip_blocks)
    });
    log(&format!(
        "track {} {:?} {}x{} {}Hz timescale {} {codec} {}",
        track.id,
        track.kind,
        track.width,
        track.height,
        track.sample_rate,
        track.timescale,
        protection.unwrap_or("clear".into())
    ));
}

/// Runs as whoever owns the app's data folder, as the app does.
fn become_owner_of(dir: &PathBuf) -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    let meta = std::fs::metadata(dir).context("the app's data folder")?;
    unsafe {
        if libc::setgid(meta.gid()) != 0 || libc::setuid(meta.uid()) != 0 {
            bail!("couldn't become uid {}", meta.uid());
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn log(line: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    eprintln!("{}.{:03} {line}", now.as_secs() % 100_000, now.subsec_millis());
}
