//! TPT Bug Report Builder Pro — desktop shell (paid edition).
//!
//! A native host binary that opens the OS webview (WebView2 on Windows) and
//! serves the `trunk build --release --features pro` output of the appfront DOM
//! bundle from `dist/`. The same DOM bundle ships to the hub for the free WASM
//! edition; the Pro difference at runtime is that the bundle is built with
//! `--features pro`, which compiles in multi-screenshot sequences, scrolling
//! capture, the Markdown/PDF report assembler and the hotkey bindings.
//!
//! ## OS-level capture and global hotkeys
//!
//! Screen/window/region capture inside the shell uses the WebView2 Screen
//! Capture API (`navigator.mediaDevices.getDisplayMedia`), which the bundled
//! DOM code drives exactly as it does in a browser — no native capture backend
//! is needed for the shipped build. Region capture is a crop of the captured
//! frame; scrolling capture is a stitched sequence of frames (`engine::stitch`).
//!
//! Global hotkeys are registered *natively* through
//! [`AppBuilder::with_shortcut`](tpt_appfront_webview::AppBuilder::with_shortcut)
//! so they fire while another window has focus — the whole point of a
//! screen-capture hotkey. Each press arrives at `on_command` as the action
//! `"shortcut"` with `{ "id": <id> }` in `params` (see
//! `tpt-appfront-webview`'s `manager.rs` event pump — *not* an action string
//! shaped `shortcut:<id>`, despite that being the natural-looking name);
//! this shell logs the id. `tpt-appfront-webview` has no call today to push
//! an event into the hosted page, nor a way for `on_command` to reach the
//! window/webview handle to focus it (`AppBuilder` exposes no such method) —
//! a real gap in that crate for this use case, out of scope for this shell
//! to work around. So a *global* press (window unfocused) is logged but does
//! not yet trigger a capture; the combo works today whenever the window
//! already has focus, caught by the bundle's own in-page keydown listener
//! (`pro_ui::mount` in the UI crate) exactly as it does in a plain browser
//! tab running the standalone Pro build. Wiring true background-press ->
//! capture needs an upstream addition to `tpt-appfront-webview` (a window
//! handle on `on_command`, or a push-to-page call) — noted here rather than
//! guessed at.
//!
//! Resolve order for the bundle directory:
//! 1. `TPT_BUGREPORT_DIST` environment variable,
//! 2. `./dist` next to the current working directory,
//! 3. `dist` next to the executable.

use std::path::PathBuf;

use tpt_appfront_webview::{AppBuilder, WindowConfig};

const APP_ID: &str = "nz.co.tptsolutions.bug-report-builder-pro";

/// Global hotkeys registered with the OS for the Pro edition. The ids match the
/// `shortcut:<id>` events the bundle listens for.
const HOTKEYS: [(&str, &str); 4] = [
    ("capture-screen", "Ctrl+Shift+1"),
    ("capture-region", "Ctrl+Shift+2"),
    ("capture-scrolling", "Ctrl+Shift+3"),
    ("capture-paste", "Ctrl+Shift+4"),
];

fn main() {
    let dist_dir = resolve_dist_dir();
    if !dist_dir.join("index.html").exists() {
        eprintln!(
            "tpt-bug-report-builder-pro: no index.html under {} — run `trunk build \
             --release --features pro` first (or set TPT_BUGREPORT_DIST)",
            dist_dir.display()
        );
        std::process::exit(1);
    }

    let mut builder = AppBuilder::new(APP_ID)
        .with_window(WindowConfig {
            id: "main".to_string(),
            title: "TPT Bug Report Builder Pro".to_string(),
            width: 1360,
            height: 940,
            dist_dir,
        })
        .with_single_instance(true);
    for (id, spec) in HOTKEYS {
        builder = builder.with_shortcut(id.to_string(), spec.to_string());
    }

    let result = builder.run(|action, params| {
        // The shortcut manager reports the action as the literal string
        // "shortcut" with the pressed id in `params.id` (see
        // `tpt-appfront-webview`'s `manager.rs`: `on_command("shortcut",
        // json!({ "id": id }))`) — not `shortcut:<id>` as the id might
        // suggest. Everything else is the bundle's own IPC. Logging the
        // press is enough here to prove registration worked on a real
        // machine; the capture flow itself runs in-page (see the module doc
        // above for why a background press can't trigger it directly yet).
        if action == "shortcut" {
            let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            eprintln!("tpt-bug-report-builder-pro: global hotkey `{id}` pressed");
        }
        Ok(())
    });

    if let Err(error) = result {
        eprintln!("tpt-bug-report-builder-pro: {error:#}");
        std::process::exit(1);
    }
}

fn resolve_dist_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("TPT_BUGREPORT_DIST") {
        return PathBuf::from(from_env);
    }
    let cwd_dist = PathBuf::from("dist");
    if cwd_dist.join("index.html").exists() {
        return cwd_dist;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let exe_dist = exe_dir.join("dist");
            if exe_dist.join("index.html").exists() {
                return exe_dist;
            }
        }
    }
    cwd_dist
}