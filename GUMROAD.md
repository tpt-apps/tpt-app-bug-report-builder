# Gumroad listing

Everything needed to create/update the Gumroad product. Source copy is
`apps.md` §T2-8 in the `tpt-electrician-nz-2` repo — if that changes, update
here too (and vice versa).

## File to upload

```
release\TPT-Bug-Report-Builder-Pro.zip
```

Doesn't exist yet? Run `just package` from the repo root first — see
`PUBLISH.md` for the full build/publish sequence. Re-run it and re-upload
before every price/content change to the Pro build.

## Product name

```
TPT Bug Report Builder — From Screenshots to a Finished Bug Report
```

## Price

**$15 USD, one-time purchase** (no subscription).

## Short description / summary field

```
Capture a sequence, annotate, and get a formatted bug report ready to send — built for QA, helpdesk, and freelance devs.
```

## Bullets / feature list

```
Auto-assembled reports
Multi-screenshot sequences
Markdown/PDF export
Global hotkeys
One-time purchase
```

## Full description (long-form field)

```
TPT Bug Report Builder captures a sequence of annotated screenshots and
auto-assembles them into a formatted bug report — so you're handing off a
report, not a folder of loose PNGs.

Who it's for: QA testers, IT helpdesk, and freelance developers who report
bugs constantly and need the finished report, not just the raw screenshot.

Pro edition (this download):
- Multi-screenshot sequences — capture the whole repro, not one frame
- Scrolling capture — stitch a tall page/panel into one image
- Auto-generated report — Markdown or PDF, ready to paste into a tracker or
  send as-is
- Arrow, box, text and redaction (blur) annotation on every screenshot
- Global hotkeys for capture, so you don't break flow to reproduce a bug
- Runs fully offline — no account, no cloud upload of your screenshots
- One-time purchase, yours to keep

Try the free single-screenshot edition first, right in your browser, no
install: tptsolutions.co.nz/tools/bug-report-builder

Requirements: Windows 10 (20H2+) or Windows 11. Uses the WebView2 runtime,
which is preinstalled on both — nothing extra to download.
```

## Category / tags

- Category: **Software > Developer Tools** (or Gumroad's closest equivalent)
- Tags: `bug-report`, `qa`, `screenshot`, `annotation`, `developer-tools`,
  `windows`

## Cover image / thumbnail

Not created yet. A screenshot of the Pro capture-sequence view (`src/pro_ui.rs`
— the timeline + annotated preview) makes the most honest cover: run
`release\TPT Bug Report Builder Pro\tpt-bug-report-builder-pro.exe` and grab
one. No stock/AI art — a real screenshot of the tool doing its job is more
convincing for a $15 dev-tool sale anyway.

## After publishing

1. Copy the resulting product URL (`https://gum.co/...`).
2. Paste it into the `gumroadUrl` field of the `bug-report-builder` entry in
   `tpt-electrician-nz-2\src\lib\tpt-apps-registry.ts` (uncomment the line).
3. Flip that entry's `status` from `'coming-soon'` to `'live'`.
4. Commit and push the registry repo.
