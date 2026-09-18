//! Free WASM edition UI (tpt-appfront-dom), single screenshot.
//!
//! Rendering architecture mirrors `tpt-app-fea-lite`'s proven split:
//! - the **form (title/summary/severity/environment/steps)** is mounted once
//!   (`tpt_appfront_dom::mount`) — typing never loses focus; field values are
//!   read straight from the DOM when Export is pressed;
//! - the **status section** (validation issues / export confirmation)
//!   re-renders fine-grained via `tpt_appfront_dom::render` driven by a
//!   `Signal`;
//! - **capture + viewer** are raw `web-sys` widgets (file input, canvas) —
//!   the DOM backend has no canvas node kind — appended into placeholder
//!   containers by [`capture::mount_upload_input`]/[`viewer::mount`].

use std::cell::RefCell;
use std::rc::Rc;

use tpt_appfront_core::{Signal, UITree};
use tpt_bugreport_engine::{markdown, Environment, Report, ReportMeta, Severity};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::capture;
use crate::viewer::{self, Viewer};

#[derive(Debug, Clone)]
enum Msg {
    Export,
    /// `.md` + sidecar `.png` files instead of one self-contained file —
    /// GitHub/GitLab/Jira all strip `data:` URIs from a pasted comment, so
    /// embedding the screenshot doesn't survive being pasted into the tools
    /// this report is actually meant for.
    ExportForTracker,
}

#[derive(Clone)]
enum Status {
    Idle,
    Issues(Vec<String>),
    Exported { file_name: String, summary: String },
}

const BASE_CSS: &str = r#"
.brb-app{--brb-bg:#ffffff;--brb-panel:#f6f7f9;--brb-fg:#1f2430;--brb-muted:#667085;--brb-border:#d9dee7;--brb-accent:#a4243b;--brb-accent-fg:#ffffff;--brb-err:#b3261e;--brb-err-bg:#fbeeec;--brb-ok:#0e7c6b;--brb-ok-bg:#eaf6f3;max-width:960px;color-scheme:light dark;background:var(--brb-bg);color:var(--brb-fg);font-size:15px;line-height:1.5}
@media(prefers-color-scheme:dark){.brb-app{--brb-bg:#10151f;--brb-panel:#161d2a;--brb-fg:#e6e9ef;--brb-muted:#98a2b3;--brb-border:#2a3342;--brb-accent:#e0576f;--brb-accent-fg:#241017;--brb-err:#f2b8b5;--brb-err-bg:#2c1a1c;--brb-ok:#34b39a;--brb-ok-bg:#122622}}
.brb-app *{box-sizing:border-box}
.brb-app h2{font-size:1.3rem;margin:0 0 2px}
.brb-app h3{font-size:1rem;margin:0 0 6px}
.brb-sub{color:var(--brb-muted);margin:0 0 14px;font-size:.9rem}
.brb-card{background:var(--brb-panel);border:1px solid var(--brb-border);border-radius:10px;padding:14px 16px;margin:0 0 14px}
.brb-hint{color:var(--brb-muted);font-size:.82rem;margin:0 0 8px}
.brb-row{display:flex;gap:14px;flex-wrap:wrap;margin-top:10px}
.brb-field{display:flex;flex-direction:column;gap:4px;flex:1;min-width:160px}
.brb-label{font-size:.78rem;color:var(--brb-muted)}
.brb-input,.brb-app input,.brb-app textarea,.brb-app select{width:100%;padding:7px 9px;border:1px solid var(--brb-border);border-radius:8px;background:var(--brb-bg);color:var(--brb-fg);font:inherit}
.brb-app textarea{min-height:64px;resize:vertical}
.brb-input:focus,.brb-app input:focus,.brb-app textarea:focus,.brb-app select:focus{outline:2px solid var(--brb-accent);outline-offset:1px}
.brb-actions{display:flex;gap:8px;flex-wrap:wrap;margin-top:14px;align-items:center}
.brb-btn{padding:8px 16px;border-radius:8px;border:1px solid var(--brb-border);background:var(--brb-bg);color:var(--brb-fg);font:inherit;cursor:pointer}
.brb-btn:hover{border-color:var(--brb-accent)}
.brb-primary{background:var(--brb-accent);border-color:var(--brb-accent);color:var(--brb-accent-fg);font-weight:600}
.brb-toolbar{display:flex;gap:10px;flex-wrap:wrap;align-items:center;margin-bottom:10px}
.brb-tool-group{display:flex;gap:4px}
.brb-tool-btn{padding:6px 12px}
.brb-active{background:var(--brb-accent);border-color:var(--brb-accent);color:var(--brb-accent-fg)}
.brb-color{width:auto;padding:6px 8px}
.brb-range-label{display:flex;align-items:center;gap:6px;font-size:.8rem;color:var(--brb-muted)}
.brb-canvas-holder{border:1px solid var(--brb-border);border-radius:10px;overflow:auto;background:repeating-conic-gradient(#00000008 0% 25%,transparent 0% 50%) 50%/16px 16px}
.brb-canvas{display:block;max-width:100%;height:auto}
.brb-file-input{margin-bottom:10px}
.brb-capture-status{font-size:.82rem;color:var(--brb-muted);margin:4px 0 10px}
.brb-status-view{margin:4px 0 2px}
.brb-issues{padding:10px 12px;background:var(--brb-err-bg);border:1px solid var(--brb-err);border-radius:8px}
.brb-issue{color:var(--brb-err);font-size:.88rem;margin:2px 0}
.brb-ok{padding:10px 12px;background:var(--brb-ok-bg);border:1px solid var(--brb-ok);border-radius:8px;color:var(--brb-ok);font-size:.9rem}
.brb-upsell{border:1px dashed var(--brb-border);border-radius:10px;padding:12px 16px;color:var(--brb-muted);font-size:.88rem;margin-top:4px}
"#;

/// Mounts the whole free-edition app into `container` (the hub's container
/// div, or the standalone `#tpt-appfront-root` marker).
pub fn mount_app(container: &web_sys::Element) -> Result<(), JsValue> {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    upsert_style(&document, "brb-styles", BASE_CSS);

    // Scope BASE_CSS's custom properties to this mount without clobbering
    // whatever class the hub container already carries.
    let existing_class = container.get_attribute("class").unwrap_or_default();
    let scoped_class = if existing_class.trim().is_empty() {
        "brb-app".to_string()
    } else {
        format!("{existing_class} brb-app")
    };
    let _ = container.set_attribute("class", &scoped_class);

    let status: Signal<Status> = Signal::new(Status::Idle);
    let capture_source: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    // 1. Static shell — mounted exactly once so typing never loses focus.
    let shell = shell_tree();
    let dispatch_container = container.clone();
    let dispatch_status = status.clone();
    let viewer_slot: Rc<RefCell<Option<Viewer>>> = Rc::new(RefCell::new(None));
    let dispatch_viewer = Rc::clone(&viewer_slot);
    let dispatch_source = Rc::clone(&capture_source);
    let dispatch: Rc<dyn Fn(Msg)> = Rc::new(move |msg| match msg {
        Msg::Export => export(
            &dispatch_container,
            &dispatch_viewer,
            &dispatch_source,
            &dispatch_status,
            ExportKind::SelfContained,
        ),
        Msg::ExportForTracker => export(
            &dispatch_container,
            &dispatch_viewer,
            &dispatch_source,
            &dispatch_status,
            ExportKind::Sidecars,
        ),
    });
    let root = tpt_appfront_dom::mount(container, &shell, dispatch)?;
    std::mem::forget(root);

    // 2. Reactive status section into its placeholder.
    let status_el = container
        .query_selector(".brb-status")?
        .ok_or_else(|| JsValue::from_str("status placeholder missing"))?;
    let status_for_render = status.clone();
    let handle = tpt_appfront_dom::render(
        &status_el,
        Rc::new(move || status_view(status_for_render.clone())),
        Rc::new(|_: Msg| {}),
    )?;
    std::mem::forget(handle);

    // 3. Capture (upload + paste) and viewer (canvas) — raw DOM widgets.
    let capture_holder = container
        .query_selector(".brb-capture")?
        .ok_or_else(|| JsValue::from_str("capture placeholder missing"))?;
    let capture_status_el = document.create_element("p")?;
    capture_status_el.set_attribute("class", "brb-capture-status")?;
    capture_status_el.set_text_content(Some("No screenshot loaded yet — upload a file or paste (Ctrl+V) one."));
    capture_holder.append_child(&capture_status_el)?;

    let viewer_holder = container
        .query_selector(".brb-viewer")?
        .ok_or_else(|| JsValue::from_str("viewer placeholder missing"))?;
    let viewer = viewer::mount(&viewer_holder)?;
    *viewer_slot.borrow_mut() = Some(viewer.clone());

    let on_ready: Rc<dyn Fn(tpt_bugreport_engine::RgbaImage, String)> = {
        let viewer = viewer.clone();
        let capture_source = Rc::clone(&capture_source);
        let capture_status_el = capture_status_el.clone();
        Rc::new(move |image, source: String| {
            let (w, h) = (image.width, image.height);
            viewer.set_image(image);
            *capture_source.borrow_mut() = Some(source.clone());
            capture_status_el.set_text_content(Some(&format!(
                "Loaded {w} x {h} px ({source}). Drag on the canvas to mark it up."
            )));
        })
    };
    capture::mount_upload_input(&capture_holder, on_ready.clone())?;
    capture::mount_paste_listener(on_ready.clone())?;

    // Pro's capture-sequence timeline, preview and Markdown/PDF export choice
    // — a pure addition into the `.brb-pro` placeholder `shell_tree` left for
    // it; nothing above this line changes between editions.
    #[cfg(feature = "pro")]
    {
        let pro_container = container
            .query_selector(".brb-pro")?
            .ok_or_else(|| JsValue::from_str("pro placeholder missing"))?;
        crate::pro_ui::mount(
            container,
            &pro_container,
            &capture_holder,
            viewer,
            on_ready,
            capture_source,
        )?;
    }

    Ok(())
}

/// The mounted-once shell: header, form card, screenshot card, status
/// placeholder, upsell.
fn shell_tree() -> UITree<Msg> {
    UITree::container(|c| {
        c.heading(2, "Bug Report Builder");
        c.text(
            "Capture one screenshot, mark it up, and export a self-contained Markdown \
             bug report — no server, nothing leaves your browser.",
        )
        .class("brb-sub");

        c.container(|card| {
            card.heading(3, "1 \u{b7} Report details");
            card.container(|row| {
                row.container(|field| {
                    field.text("Title").class("brb-label");
                    field.input("").class("brb-input brb-title");
                })
                .class("brb-field");
                row.container(|field| {
                    field.text("Severity").class("brb-label");
                    field
                        .select(
                            Severity::ALL.map(|s| (s.slug(), s.label())),
                            Severity::Medium.slug(),
                        )
                        .class("brb-input brb-severity");
                })
                .class("brb-field");
                row.container(|field| {
                    field.text("Reported by").class("brb-label");
                    field.input("").class("brb-input brb-reporter");
                })
                .class("brb-field");
            })
            .class("brb-row");
            card.container(|field| {
                field.text("Summary").class("brb-label");
                field.textarea("").class("brb-input brb-summary");
            })
            .class("brb-field");
            card.container(|field| {
                field.text("Steps to reproduce (one per line)").class("brb-label");
                field.textarea("").class("brb-input brb-steps");
            })
            .class("brb-field");
        })
        .class("brb-card");

        c.container(|card| {
            card.heading(3, "2 \u{b7} Environment (optional)");
            card.container(|row| {
                row.container(|field| {
                    field.text("Application").class("brb-label");
                    field.input("").class("brb-input brb-env-app");
                })
                .class("brb-field");
                row.container(|field| {
                    field.text("Version").class("brb-label");
                    field.input("").class("brb-input brb-env-version");
                })
                .class("brb-field");
            })
            .class("brb-row");
            card.container(|row| {
                row.container(|field| {
                    field.text("Operating system").class("brb-label");
                    field.input("").class("brb-input brb-env-os");
                })
                .class("brb-field");
                row.container(|field| {
                    field.text("Browser").class("brb-label");
                    field.input("").class("brb-input brb-env-browser");
                })
                .class("brb-field");
                row.container(|field| {
                    field.text("Device").class("brb-label");
                    field.input("").class("brb-input brb-env-device");
                })
                .class("brb-field");
            })
            .class("brb-row");
        })
        .class("brb-card");

        c.container(|card| {
            card.heading(3, "3 \u{b7} Screenshot");
            card.text("The free edition exports one marked-up screenshot per report.")
                .class("brb-hint");
            // The upload input + status line are appended here by mount_app.
            card.container(|_| {}).class("brb-capture");
            card.container(|row| {
                row.container(|field| {
                    field.text("Caption").class("brb-label");
                    field.input("").class("brb-input brb-cap-title");
                })
                .class("brb-field");
                row.container(|field| {
                    field.text("Note").class("brb-label");
                    field.input("").class("brb-input brb-cap-note");
                })
                .class("brb-field");
            })
            .class("brb-row");
            // The annotation toolbar + canvas are appended here by mount_app.
            card.container(|_| {}).class("brb-viewer");
        })
        .class("brb-card");

        c.container(|actions| {
            actions
                .button("Export Markdown report")
                .class("brb-btn brb-primary")
                .on_click(Msg::Export);
            actions
                .button("Export for a tracker (.md + .png)")
                .class("brb-btn")
                .on_click(Msg::ExportForTracker);
        })
        .class("brb-actions");

        // The reactive status section mounts into this placeholder.
        c.container(|_| {}).class("brb-status");

        // The free-tier upsell doesn't apply once Pro's own capture-sequence
        // card (appended by `pro_ui::mount`) is already on the page.
        #[cfg(not(feature = "pro"))]
        c.container(|upsell| {
            upsell.heading(3, "Need more than one screenshot?");
            upsell.text(
                "The Pro desktop edition adds multi-screenshot sequences, auto-generated \
                 Markdown/PDF reports, scrolling capture that stitches a long page into one \
                 image, and global hotkeys so you never leave the app you're reporting on.",
            );
        })
        .class("brb-upsell");

        // Placeholder for `pro_ui::mount`'s card (sequence timeline, preview,
        // PDF export, hotkeys) — appended once by `mount_app`.
        #[cfg(feature = "pro")]
        c.container(|_| {}).class("brb-pro");
    })
}

/// Builds the status section. Shape is invariant (one child, either the
/// issues list or the success line) so the positional reconciler updates in
/// place.
fn status_view(status: Signal<Status>) -> UITree<Msg> {
    let status = status.get();
    let mut root_builder = tpt_appfront_core::ContainerBuilder::<Msg>::new();
    root_builder
        .container(|c| match &status {
            Status::Idle => {}
            Status::Issues(issues) => {
                c.container(|list| {
                    for issue in issues {
                        list.text(format!("\u{2022} {issue}")).class("brb-issue");
                    }
                })
                .class("brb-issues");
            }
            Status::Exported { file_name, summary } => {
                c.text(format!("Exported {file_name} \u{2014} {summary}"))
                    .class("brb-ok");
            }
        })
        .class("brb-status-view");
    root_builder
        .into_only_child()
        .expect("status view builds exactly one root container")
}

/// Which of [`markdown::assemble`]'s two output shapes to produce.
#[derive(Clone, Copy)]
enum ExportKind {
    /// One self-contained `.md`, image embedded as a `data:` URI.
    SelfContained,
    /// `.md` with a relative link, plus a sidecar `.png` download — the
    /// shape that survives being pasted into a tracker.
    Sidecars,
}

/// Reads the form + viewer, validates, and either reports issues or
/// assembles the report and triggers the browser download(s) `kind` calls for.
fn export(
    container: &web_sys::Element,
    viewer_slot: &Rc<RefCell<Option<Viewer>>>,
    capture_source: &Rc<RefCell<Option<String>>>,
    status: &Signal<Status>,
    kind: ExportKind,
) {
    let mut report = Report {
        meta: read_meta(container),
        captures: Vec::new(),
    };

    let viewer = viewer_slot.borrow();
    if let Some(viewer) = viewer.as_ref() {
        if let Some(image) = viewer.image() {
            let mut capture = tpt_bugreport_engine::Capture::new(1, image)
                .with_title(read_input(container, ".brb-cap-title"))
                .with_source(
                    capture_source
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| "upload".to_string()),
                )
                .with_taken_at_ms(js_sys::Date::now());
            capture.note = read_input(container, ".brb-cap-note");
            capture.annotations = viewer.annotations();
            report.push(capture);
        }
    }

    if let Err(issues) = report.validate() {
        status.set(Status::Issues(
            issues.iter().map(|issue| issue.to_string()).collect(),
        ));
        return;
    }

    let result = match kind {
        ExportKind::SelfContained => markdown::assemble(&report).map(|text| (text, Vec::new())),
        ExportKind::Sidecars => markdown::assemble_with_sidecars(&report),
    };
    match result {
        Ok((text, images)) => {
            let file_name = markdown::file_name(&report);
            trigger_download(&file_name, &text);
            for (image_name, png_bytes) in &images {
                trigger_download_bytes(image_name, "image/png", png_bytes);
            }
            status.set(Status::Exported {
                file_name,
                summary: markdown::summary(&report),
            });
        }
        Err(issues) => {
            status.set(Status::Issues(
                issues.iter().map(|issue| issue.to_string()).collect(),
            ));
        }
    }
}

fn read_meta(container: &web_sys::Element) -> ReportMeta {
    let mut meta = ReportMeta::new(read_input(container, ".brb-title"));
    meta.summary = read_input(container, ".brb-summary");
    meta.severity = Severity::from_slug(&read_input(container, ".brb-severity"));
    meta.reporter = read_input(container, ".brb-reporter");
    meta.environment = Environment {
        app_name: read_input(container, ".brb-env-app"),
        app_version: read_input(container, ".brb-env-version"),
        os: read_input(container, ".brb-env-os"),
        browser: read_input(container, ".brb-env-browser"),
        device: read_input(container, ".brb-env-device"),
    };
    meta.steps = read_input(container, ".brb-steps")
        .lines()
        .map(|line| line.to_string())
        .collect();
    meta
}

fn read_input(container: &web_sys::Element, selector: &str) -> String {
    container
        .query_selector(selector)
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|input| input.value())
        .or_else(|| {
            container
                .query_selector(selector)
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                .map(|ta| ta.value())
        })
        .or_else(|| {
            container
                .query_selector(selector)
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlSelectElement>().ok())
                .map(|select| select.value())
        })
        .unwrap_or_default()
}

/// Blob -> object URL -> temporary anchor click, same pattern as the Pro
/// PDF export in `tpt-app-fea-lite`.
fn trigger_download(file_name: &str, contents: &str) {
    let array = js_sys::Array::new();
    array.push(&JsValue::from_str(contents));
    let bits: JsValue = array.into();
    if let Ok(blob) = web_sys::Blob::new_with_str_sequence(&bits) {
        download_blob(file_name, &blob);
    }
}

/// Same as [`trigger_download`] for binary content (a sidecar `.png`).
fn trigger_download_bytes(file_name: &str, mime_type: &str, bytes: &[u8]) {
    let array = js_sys::Array::new();
    array.push(&js_sys::Uint8Array::from(bytes).into());
    let bits: JsValue = array.into();
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime_type);
    if let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&bits, &options) {
        download_blob(file_name, &blob);
    }
}

fn download_blob(file_name: &str, blob: &web_sys::Blob) {
    let url = match web_sys::Url::create_object_url_with_blob(blob) {
        Ok(url) => url,
        Err(_) => return,
    };
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(anchor) = document.create_element("a") {
            let _ = anchor.set_attribute("href", &url);
            let _ = anchor.set_attribute("download", file_name);
            let _ = anchor
                .dyn_ref::<web_sys::HtmlAnchorElement>()
                .map(|a| a.click());
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// Creates or updates the `<style id>` in the document head.
fn upsert_style(document: &web_sys::Document, id: &str, css: &str) {
    if let Ok(Some(existing)) = document.query_selector(&format!("style#{id}")) {
        existing.set_text_content(Some(css));
        return;
    }
    let Some(head) = document.head() else {
        return;
    };
    if let Ok(style_el) = document.create_element("style") {
        let _ = style_el.set_attribute("id", id);
        style_el.set_text_content(Some(css));
        let _ = head.append_child(&style_el);
    }
}
