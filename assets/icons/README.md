# Icons

- `open-icons.json` — [Framework7 Icons](https://github.com/framework7io/framework7-icons) 5.0.5 by Vladimir
  Kharlampidi, licensed under the MIT License (`LICENSE-framework7-icons.txt`): iOS-style icons drawn after Apple's
  SF Symbols. `tools/open-icons.py` writes it from the npm package's SVGs (which icon stands for which, the sign-out
  arrow flipped to point out of the box, and a light stroke that gives the line icons SF Symbols' semibold weight).
  Every build uses it.
- `sf-symbols.json` (not in git) — Apple's SF Symbols, which Apple's license only allows in apps for Apple platforms.
  A local build takes them with `CRUNCHY_ICONS=sf` when the file is there; `tools/release.sh` never does.
- `crunchyroll-logo.svg` — Crunchyroll's wordmark, a trademark of Crunchyroll, LLC.
