# Crunchy native: feasibility (2026-11-11)

Goal: a Crunchyroll client for the LG OLED55C1AUB (webOS 6.5.3, rooted) that draws its UI straight on the GPU and
hands video to the TV's own player, with no Chromium, web view or JavaScript — the approach
[PlxNative](https://github.com/GLinnik21/plx-native) takes for Plex (Rust + SDL2 + OpenGL ES 2, video fed to Starfish,
GPL-3.0). Unlike Plex, every Crunchyroll stream is DRM-protected; that is the whole risk.

## The TV

- Kernel aarch64, **userland 32-bit ARM** (armv7, `armv7-unknown-linux-gnueabi`), glibc 2.28, 4 cores, 2.6 GB RAM,
  ~590 MB free on the app partition.
- Graphics: `libEGL`, `libGLESv2` (Mali), `libwayland-client`, `libwayland-egl`, `libwayland-webos-client`. No SDL2
  (bundle it, or talk to Wayland directly).
- Text and images: `libfreetype`, `libharfbuzz`, `libpng16`, `libjpeg`, `libwebp`. Network: `libcurl`, OpenSSL 1.0.2.
  No FFmpeg, no libass.
- Media: `libNDL_directmedia` (LG's elementary-stream decode API), `libumedia_api`, uMediaServer
  (`luna://com.webos.media`), `starfish-media-pipeline`, GStreamer.
- DRM: `luna://com.webos.service.drm` (load, sendDrmMessage, getRightsError, getDrmMessage, acquireRightsAsync…),
  `libplayready-4.0.so`, `libwidevine_ce_cdm_shared.so.16`, `libwvcdm_shared.so.16`.

## What was proven on the TV

1. **The native player plays DASH.** `com.webos.media/load` with a public clear DASH URL (both SegmentTemplate and
   on-demand/SegmentBase profiles) plays: track info, decoder acquired, `currentTime` advancing. No window needed.
2. **The DRM service loads both systems.** `drmType: "playready"` (documented by LG) and `"widevine"` (undocumented)
   both return a `clientId`.
3. **A real Crunchyroll PlayReady license is acquired natively.** The content's PlayReady header (WRMHEADER, from the
   manifest's `mspr:pro`) is sent as a `PlayReadyInitiator` `LicenseAcquisition` with `LA_URL` pointed at a local
   proxy; the TV's PlayReady client posts its SOAP challenge there, the proxy adds `Authorization`,
   `X-Cr-Content-Id`, `X-Cr-Video-Token` and `SOAPAction` and forwards it to
   `cr-license-proxy.prd.crunchyrollsvc.com/v1/license/playReady`: **200, 10.8 KB license, `DRM_RESULT 0`**.
4. **Crunchyroll's streams** (`cr-play-service …/v1/{id}/tv/lg/play`, with the web app's Chrome user agent — another
   one gets `Outdated Token`): DASH `isoff-on-demand`, CENC (`cenc`), PlayReady **and** Widevine PSSH, H.264
   (avc1.4D401E / 64001F / 640028) + AAC 192k; the manifest needs the bearer token, the segments (absolute BaseURLs on
   `vod-gcp.crunchyrollcdn.com`, signed query) don't. Streams are released with `DELETE /v1/token/{id}/{token}` (204).

## What blocked, and how video plays now (2026-11-11)

**The player's URI mode doesn't play Crunchyroll's encrypted DASH.** Served the manifest through a local proxy, it
either sat idle (the CDN's HTTPS) or fetched the init segments and `sidx` and then **the pipeline process died**
(`errorCode 1000, pipeline died abnormally`), with PlayReady and Widevine alike. URI mode is a dead end.

**Buffer feed works, the way the TV's Chromium does it** (`crates/player`):

- Chromium's own load payload for Crunchyroll, captured with `ls-monitor` (it's in `setResourcePolicyExtraInfo`):
  `mediaTransportType: BUFFERSTREAM`, H264/AAC, `format: RAW`, `esInfo.ptsToDecode` (the start), and `windowId`, an
  exported Wayland window (`wl_webos_foreign.export_element`, type video). The pipeline imports that window and the
  compositor punches the video through it.
- EME runs in-process: `libwvcdm_shared.so` is a Chromium library CDM (interface 10: `CreateCdmInstance`,
  `InitializeCdmModule_10`). We host it like Chromium (`Initialize(false, false, false)`, the license server's service
  certificate, a temporary CENC session); the license comes from Crunchyroll's proxy with the stream's token.
  Initialized that way, **`Decrypt` hands back clear samples** (SW_SECURE_CRYPTO, what the web app asks for too), so
  the pipeline is loaded without `drm`/`svpVersion` and fed plain elementary streams.
- Samples: fMP4 demuxed (`trun`, `senc`), H.264 rewritten to Annex B with SPS/PPS before keyframes, AAC behind ADTS
  headers; subsamples adjusted so the encrypted runs keep their offsets.
- `StarfishMediaAPIs` (libplayerAPIs) through its mangled symbols: constructed with no id (the argument is the media
  server connection's id, not the app's), `Load` with a callback context, `Feed` answering `{"returnValue":"Ok"}` /
  `"BufferFull"`, and **`Play` only once `isLoadCompleted()`** (earlier it's ignored: the first frame shows and the
  clock never starts). With `queryPosition` there are no position events: `getCurrentPlaytime()` is polled. The
  pipeline takes over a minute of data, so feeding stops 20 s past the position.
- Gotchas: the pipeline needs the app's luna role (from an ssh shell `LSRegister` fails and the library exits) and a
  foreground window (else the resources wait as background and time out after 20 s).

Hardsubbed streams (Crunchyroll offers one per subtitle language) give subtitles without an ASS renderer for now.
`crates/player/examples/spike.rs` checks the DRM side from an ssh shell (stream, license, decrypt), as the app's user.

## Everything else is low risk

UI on GLES2 at 60 fps, the Crunchyroll API (auth, profiles, home feed, CMS, play, keep-alive, playheads), images and
caching, ASS subtitles (bundle libass or draw them), packaging as an IPK, and the dev loop over SSH all follow PlxNative's
proven path on an older TV than this one.

`tools/spikes/native-drm-spike.js` is the URI-mode experiment above (Node 8 on the TV; it reads a bearer token from a file the
web app's license request provides and never prints it).
