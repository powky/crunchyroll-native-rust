# Icons

- `open-icons.json` — [Material Icons](https://github.com/google/material-design-icons) (Round style; `info` is the
  Outlined one) by Google, licensed under the Apache License 2.0 (`LICENSE-material-icons.txt`). Taken as SVG from
  [`@material-design-icons/svg`](https://www.npmjs.com/package/@material-design-icons/svg) 0.14.15 and stored as path
  data, scaled to the app's 230-unit icon box. This is the set every distributed build uses.
- `sf-symbols.json` (not in git) — Apple's SF Symbols, which Apple's license only allows in apps for Apple platforms.
  A local build uses them when the file is there; `CRUNCHY_ICONS=open` (and `tools/release.sh`) builds without them.
- `crunchyroll-logo.svg` — Crunchyroll's wordmark, a trademark of Crunchyroll, LLC.
