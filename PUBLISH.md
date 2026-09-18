# Publishing this tool

Step-by-step checklist from "code is done" to "live and sellable."

## 0. Where the Gumroad file actually is

Short answer if that's all you need right now:

```
cd "D:\Programming\1PRODUCTION\Open Source\tpt-app-bug-report-builder"
just package
```

This produces **`release\TPT-Bug-Report-Builder-Pro.zip`** — that zip is what
you upload to Gumroad as the digital download.

Why it's easy to lose track of: `release\` (like `dist\` and `target\`) is
gitignored — it's a local build artifact, not something that lives in the
repo or on GitHub. If you don't see it, it's because nobody has run
`just package` since the last clean, not because it's missing from the
project. Running the command above rebuilds it from scratch.

`release\TPT Bug Report Builder Pro\` (the unzipped folder next to the zip)
is what's actually inside: the exe, its `dist\` folder of wasm assets, and a
short `README.txt` for the buyer. The exe reads `dist\` next to itself at
runtime, so don't zip the exe alone.

## 1. Run CI locally first

```
just test    # engine test suite, free + pro
just check   # wasm type-check (free + pro) + desktop shell type-check
```

Same checks GitHub Actions runs on push (`.github/workflows/ci.yml`) — catch
failures here before pushing.

## 2. Build and register the free edition

```
just build
```

- Builds the free WASM edition and copies the wasm-bindgen glue into
  `TPT_WEB_REPO`'s `public\apps\bug-report-builder\` (defaults to the local
  `D:\Programming\2 WIP\TPT Electrician\tpt-electrician-nz-2` checkout if
  that env var isn't set).
- Prints a ready-to-paste registry entry at the end — paste it into
  `TPT_APPS_REGISTRY` in `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts`.
  Leave `status: 'coming-soon'` and the `gumroadUrl` line commented out
  until step 5.
- Verify locally: `npm run dev` in the web repo, visit
  `/tools/bug-report-builder`. Confirm the tool mounts, captures a
  screenshot, annotates it, and exports a Markdown report.

## 3. Push both repos

- Push `tpt-app-bug-report-builder` to its GitHub remote.
- Commit and push the registry change in `tpt-electrician-nz-2`.

## 4. Build and package the Pro edition

```
just package
```

Runs the Pro build (`--features pro`: multi-screenshot sequences, scrolling
capture, Markdown/PDF export, hotkeys) and zips the result — see step 0 for
exactly what you get and where.

Smoke-test the exe standalone before listing it:
`release\TPT Bug Report Builder Pro\tpt-bug-report-builder-pro.exe`
- Upload/paste a few screenshots, add arrow/box/text/blur annotations.
- Export both Markdown and PDF.
- Try a capture sequence beyond the free tier's single-screenshot limit.
- Ctrl+Shift+1 / Ctrl+Shift+2 while the window has focus (screen / region
  capture hotkeys — see `desktop/src/main.rs`'s module doc for the current
  limitation on hotkeys firing while the window is *unfocused*).

## 5. List it on Gumroad

- Create the Gumroad product (price, description — reuse the free-tier
  upsell copy in `src/app.rs` / the registry blurb as a starting point).
- Upload `release\TPT-Bug-Report-Builder-Pro.zip` as the digital download.
- Paste the resulting product URL into the registry entry's `gumroadUrl`
  field (uncomment it), flip `status` to `'live'`, commit, push.

## 6. Tell people it exists

- Live automatically on the `/tools` hub index once `status: 'live'`.
- Consider a relevant subreddit / Show HN post once live.
