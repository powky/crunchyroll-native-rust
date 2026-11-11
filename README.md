# Crunchy

A native Crunchyroll client for LG webOS TVs (built and tested on an LG OLED55C1AUB, webOS 6). The UI is drawn
straight with OpenGL ES 2 on a Wayland surface; there's no Chromium, web view or JavaScript. Almost all of it is Rust.
Unofficial; not affiliated with Crunchyroll or LG.

- `crates/core` — the backend on [crunchyroll-rs](https://docs.rs/crunchyroll-rs): a session that signs in with a
  code shown on the TV (crunchyroll.com/activate) and stays signed in, profiles, and the home feed as rows.
- `crates/tv` — the app: a small GPU renderer (`gfx`), the TV and simulator platforms (`platform`), the screens and the
  network worker (`app`).
- `docs/feasibility.md` — what was proven on the TV, including the native DRM path; video comes last.

## Build

Needs Rust (stable, with the `armv7-unknown-linux-gnueabi` target), [Zig](https://ziglang.org) and
`cargo install cargo-zigbuild`; the TV's libraries are loaded at run time, so no SDK.

```sh
cargo run -p crunchy-tv          # simulator on macOS: arrows, Enter, Esc
./tools/build.sh && ./tools/package.sh   # build/com.crunchyroll.webos.native_<version>_arm.ipk
./tools/install.sh               # install on the TV and launch it (rooted TV, over ssh)
./tools/push.sh                  # dev loop: replace the installed binary and relaunch
```

## Credits

Inter (SIL Open Font License, `assets/fonts/Inter-OFL.txt`); webOS Wayland protocol files from
[webosbrew/wayland-protocols](https://github.com/webosbrew/wayland-protocols) (Apache-2.0); approach after
[PlxNative](https://github.com/GLinnik21/plx-native).
