# TPT Bug Report Builder — Build Todo

> Hub listing: `tptsolutions.co.nz/tools/bug-report-builder` (registry slug
> `bug-report-builder`, category `developer`). Spec: `apps.md` §T2-8 in the
> tpt-electrician-nz-2 repo. Free version: single screenshot + basic
> annotation. Paid offline edition ($15 USD, Gumroad): multi-screenshot
> sequences, auto-generated Markdown/PDF report, scrolling capture, hotkeys.
> UI on `tpt-appfront`.
>
> Hub contract: `wasm-bindgen --target web` glue exposes `mount(container)`
> (called by `WasmAppRunner` after `default()` init). Standalone `trunk serve`
> mounts into the `#tpt-appfront-root` marker div instead — never
> `document.body` — so the two hosts never double-mount.

## Phase 0 — Scaffold & pipeline proof
- [x] `tpt-appfront init tpt-app-bug-report-builder --target dom` (path deps on the tpt-appfront checkout)
- [x] Restructure scaffold to hub contract: `#[wasm_bindgen] pub fn mount(el)` + `#[wasm_bindgen(start)]` no-op unless `#tpt-appfront-root` present
- [x] `index.html` carries the `#tpt-appfront-root` marker div
- [x] Build pipeline script mirroring fea-lite/cutlist-optimizer's (`--target web` glue → web repo `public/apps/bug-report-builder/`)
- [x] Hello-world build loads through the standalone dev page (`trunk serve`)
- [x] git init + first commit

## Phase 1 — Core engine
- [x] Screenshot capture: browser tier uses uploaded images or a
      screen-capture API where available; desktop tier needs OS-level
      screen/window/region capture and scrolling capture
- [x] Annotation: arrows/boxes/text/blur overlay drawn onto captured images
- [x] Adapter crate `engine/`: capture(s) + annotations in → assembled report
      (Markdown text + embedded images, or PDF) out as plain functions — no
      UI deps, host-unit-testable
- [x] Validation: a sample multi-screenshot sequence assembles into a
      correctly ordered, readable report

## Phase 2 — Free tier
- [x] Single screenshot upload + basic annotation tools (arrow, box, text)
- [x] Free-tier upsell copy pointing at multi-screenshot sequences,
      auto-generated reports, scrolling capture, hotkeys
- [x] Browser-verified: mount, annotate, export a single-image report

## Phase 3 — Paid desktop edition (cargo feature `pro`)
- [x] `pro` gates: multi-screenshot sequences, auto-generated Markdown/PDF
      report assembly, scrolling capture, global hotkeys for capture
      (engine gates were already done; UI/capture gates added in
      `src/pro_ui.rs` and `src/capture.rs`'s `pro` submodule)
- [x] Pro UI: capture sequence timeline, report preview, "Export
      Markdown/PDF", hotkey settings (`src/pro_ui.rs`)
- [x] Desktop shell via `tpt-appfront-webview`; Windows exe build + smoke test
      (OS screen capture + global hotkeys need real-machine verification —
      exe launch, window creation and page mount verified on this machine;
      getDisplayMedia/global-hotkey *behavior* still wants a human click
      through, see report)
- [ ] Gumroad product from apps.md listing copy ($15), exe upload, URL into
      registry — manual step needing Gumroad account access, out of scope
      here; listing copy at apps.md §T2-8

## Phase 4 — Ship & measure
- [x] Registry entry (`developer` category, `wasm: { entry: '/apps/bug-report-builder/bug-report-builder.js' }`)
      per the "Adding a finished app to the hub" runbook at the bottom of apps.md
      (added as `status: 'coming-soon'` — flips to `'live'` once Gumroad exists)
- [x] Verify hub card/detail/sitemap/runner in dev (done against the repo's
      already-running dev server); prod verification after deploy is
      out of scope — nothing has been deployed
- [x] WASM size budget check on the shipped build — no hub-enforced wasm
      size budget was found (apps.md and the runner component have none;
      fea-lite's ~2 MB gzip figure is that app's own FEM-solver budget, not
      a hub policy); this build is ~101 KB gzipped (~260 KB raw), well
      inside any plausible budget
