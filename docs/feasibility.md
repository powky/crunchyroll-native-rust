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

## What blocks

**The player's URI mode doesn't play Crunchyroll's encrypted DASH.** Served the manifest (through the local proxy):

- With the CDN's own segment URLs, it fetches the manifest and then sits idle (most likely its HTTP stack failing on
  the CDN's HTTPS; never reaches track info).
- With segments proxied over local HTTP, it fetches the video and audio init + `sidx` ranges and then **the pipeline
  process dies** (`errorCode 1000, pipeline died abnormally`) — with PlayReady and with Widevine alike, license or not.

LG documents the native player for PlayReady with URI/Smooth and HLS only; Widevine/DASH is meant for MSE/EME in the
browser. So URI mode is a dead end for this content, short of finding which box crashes the parser.

## Ways forward for video (in order of promise)

1. **Buffer feed (what PlxNative, the browser's MSE and YouTube's Cobalt do):** the app does DASH itself (manifest,
   `sidx`, segment requests, fMP4 demux) and feeds samples to Starfish (`StarfishMediaAPIs::Feed`, the C++ seam PlxNative
   has in `src/starfish.c`), passing the encrypted samples with their CENC data (`senc`/IV/subsamples) and the DRM
   `clientId` so decryption happens in the TV's secure path. Needs reverse-engineering the feed's DRM options (the
   browser on this TV does exactly this for Widevine EME, so the capability is there).
2. **Find the URI-mode crash:** rewrite the init segments or manifest (e.g. drop the Widevine `pssh`, `sidx` →
   SegmentList) until the parser survives. Cheap experiments, uncertain.
3. **Fallback while 1 is built:** native browsing (the slow part today) and hand playback to the existing web app by
   deep link. Not "no Chromium", but usable from day one.

## Everything else is low risk

UI on GLES2 at 60 fps, the Crunchyroll API (auth, profiles, home feed, CMS, play, keep-alive, playheads), images and
caching, ASS subtitles (bundle libass or draw them), packaging as an IPK, and the dev loop over SSH all follow PlxNative's
proven path on an older TV than this one.

`tools/spikes/native-drm-spike.js` is the experiment above (Node 8 on the TV; it reads a bearer token from a file the
web app's license request provides and never prints it).
