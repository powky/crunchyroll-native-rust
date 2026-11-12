# Third-party notices

Crunchy for webOS is MIT-licensed (`LICENSE`). It includes or builds on the following work, each under its own license.

| Component | Author | License | Where |
|---|---|---|---|
| [crunchyroll-rs](https://github.com/crunchy-labs/crunchyroll-rs) 0.19.1 | crunchy-labs | MIT OR Apache-2.0 | `vendor/crunchyroll-rs` (patched, see `PATCHED.md` there) |
| [Inter](https://rsms.me/inter/) | Rasmus Andersson | SIL Open Font License 1.1 | `assets/fonts` (`Inter-OFL.txt`) |
| [Material Icons](https://github.com/google/material-design-icons) (Round) | Google | Apache-2.0 | `assets/icons/open-icons.json` (`LICENSE-material-icons.txt`) |
| webOS Wayland protocol definitions from [webosbrew/wayland-protocols](https://github.com/webosbrew/wayland-protocols) | LG Electronics / webosbrew | Apache-2.0 | `crates/tv/protocols` |
| Rust crates (tokio, reqwest, rustls, glow, fontdue, tiny-skia, image, wayland-client, khronos-egl, serde and their dependencies) | their authors | mostly MIT and/or Apache-2.0 | `Cargo.lock` (`cargo tree` lists them) |

"Crunchyroll" and the Crunchyroll logo are trademarks of Crunchyroll, LLC. "LG" and "webOS" are trademarks of
LG Electronics. This project is not affiliated with, endorsed by or sponsored by either of them.
