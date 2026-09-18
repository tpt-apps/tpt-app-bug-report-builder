//! Pro-only UI surface: capture sequence timeline, report preview and the
//! Markdown/PDF export choice, layered on top of the free-tier mount.
//!
//! Built as raw DOM widgets (`Rc<RefCell<...>>` state + closures), the same
//! style `capture.rs`/`viewer.rs` use, rather than through the UITree/Msg
//! plumbing in `app.rs` — that keeps this module a pure addition: nothing in
//! the free-tier shell, `Msg` enum or dispatch has to change to support it.
//! `app::mount_app` calls [`mount`] once, after the free-tier viewer is
//! wired up, passing it the same capture-ready callback and DOM handles the
//! free tier uses so screen/scroll capture and the annotation canvas are
//! shared, not duplicated.

use std::cell::RefCell;
use std::rc::Rc;

use tpt_bugreport_engine::{markdown, pdf, Capture, Report, ReportMeta};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Element, HtmlInputElement, HtmlTextAreaElement, KeyboardEvent};

use crate::capture;
use crate::viewer::Viewer;

/// Global hotkeys the desktop shell registers natively (`desktop/src/main.rs`
/// `HOTKEYS`). Shown here so the in-page settings match what's actually
/// bound; kept in this one place so the two editions can't drift.
const HOTKEYS: [(&str, &str); 4] = [
    ("Capture screen", "Ctrl+Shift+1"),
    ("Capture region", "Ctrl+Shift+2"),
    ("Scrolling capture", "Ctrl+Shift+3"),
    ("Paste screenshot", "Ctrl+Shift+4"),
];

/// Mounts the Pro card into `container` (a placeholder appended after the
/// free-tier screenshot card by `app::mount_app`).
///
/// `app_root` is the whole mounted app element — read to reuse the same
/// title/summary/environment/caption fields the free tier's `export()` reads.
/// `capture_holder` is the free tier's upload/paste card, where the
/// screen/scrolling capture buttons are appended alongside the existing file
/// input. `viewer` is the shared annotation canvas.
pub fn mount(
    app_root: &Element,
    container: &Element,
    capture_holder: &Element,
    viewer: Viewer,
    on_capture_ready: Rc<dyn Fn(tpt_bugreport_engine::RgbaImage, String)>,
    capture_source: Rc<RefCell<Option<String>>>,
) -> Result<(), JsValue> {
    let document = web_sys::window().expect("no window").document().expect("no document");

    // Screen + scrolling capture share the free tier's upload/paste card and
    // its `on_ready` callback, so a Pro capture lands in the same viewer the
    // free tier annotates with.
    capture::mount_screen_capture_button(capture_holder, on_capture_ready.clone())?;
    capture::mount_scrolling_capture(capture_holder, on_capture_ready)?;

    let card = document.create_element("div")?;
    card.set_attribute("class", "brb-card")?;
    container.append_child(&card)?;

    let heading = document.create_element("h3")?;
    heading.set_text_content(Some("4 \u{b7} Capture sequence (Pro)"));
    card.append_child(&heading)?;

    let hint = document.create_element("p")?;
    hint.set_attribute("class", "brb-hint")?;
    hint.set_text_content(Some(&format!(
        "Add each marked-up screenshot below (up to {} per report), reorder or remove them, \
         then preview and export.",
        tpt_bugreport_engine::PRO_MAX_CAPTURES
    )));
    card.append_child(&hint)?;

    let add_row = document.create_element("div")?;
    add_row.set_attribute("class", "brb-row")?;
    let add_btn = document.create_element("button")?;
    add_btn.set_attribute("type", "button")?;
    add_btn.set_attribute("class", "brb-btn")?;
    add_btn.set_text_content(Some("Add current screenshot to sequence"));
    add_row.append_child(&add_btn)?;
    card.append_child(&add_row)?;

    let list_el = document.create_element("div")?;
    list_el.set_attribute("class", "brb-sequence-list")?;
    card.append_child(&list_el)?;

    let status_el = document.create_element("p")?;
    status_el.set_attribute("class", "brb-capture-status")?;
    card.append_child(&status_el)?;

    // Hotkey settings: what's bound (native registration is desktop-only —
    // see `desktop/src/main.rs` — the bundle just lists it either way).
    let hotkeys_heading = document.create_element("h3")?;
    hotkeys_heading.set_text_content(Some("Hotkeys"));
    card.append_child(&hotkeys_heading)?;
    let hotkeys_list = document.create_element("div")?;
    hotkeys_list.set_attribute("class", "brb-hint")?;
    for (label, spec) in HOTKEYS {
        let row = document.create_element("p")?;
        row.set_text_content(Some(&format!("{label}: {spec}")));
        hotkeys_list.append_child(&row)?;
    }
    card.append_child(&hotkeys_list)?;

    let preview_el = document.create_element("pre")?;
    preview_el.set_attribute("class", "brb-preview")?;
    preview_el.set_attribute("style", "display:none;white-space:pre-wrap;max-height:320px;overflow:auto")?;
    card.append_child(&preview_el)?;

    let export_row = document.create_element("div")?;
    export_row.set_attribute("class", "brb-actions")?;
    let preview_btn = button(&document, "Preview report")?;
    let md_btn = button(&document, "Export Markdown (Pro sequence)")?;
    let tracker_btn = button(&document, "Export for a tracker (.md + .png)")?;
    let pdf_btn = button(&document, "Export PDF")?;
    export_row.append_child(&preview_btn)?;
    export_row.append_child(&md_btn)?;
    export_row.append_child(&tracker_btn)?;
    export_row.append_child(&pdf_btn)?;
    card.append_child(&export_row)?;

    let sequence: Rc<RefCell<Vec<Capture>>> = Rc::new(RefCell::new(Vec::new()));
    let next_id: Rc<RefCell<u32>> = Rc::new(RefCell::new(1));
    let app_root = app_root.clone();

    redraw_sequence(&document, &list_el, &sequence, &status_el)?;

    // Add current screenshot to sequence.
    {
        let sequence = Rc::clone(&sequence);
        let next_id = Rc::clone(&next_id);
        let viewer = viewer.clone();
        let capture_source = Rc::clone(&capture_source);
        let app_root = app_root.clone();
        let list_el = list_el.clone();
        let status_el = status_el.clone();
        let document = document.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let Some(image) = viewer.image() else {
                status_el.set_text_content(Some("Load and mark up a screenshot first."));
                return;
            };
            if sequence.borrow().len() >= tpt_bugreport_engine::PRO_MAX_CAPTURES {
                status_el.set_text_content(Some("Sequence is full for this edition."));
                return;
            }
            let id = {
                let mut next_id = next_id.borrow_mut();
                let id = *next_id;
                *next_id += 1;
                id
            };
            let mut cap = Capture::new(id, image)
                .with_title(read_value(&app_root, ".brb-cap-title"))
                .with_source(
                    capture_source.borrow().clone().unwrap_or_else(|| "capture".to_string()),
                )
                .with_taken_at_ms(js_sys::Date::now());
            cap.note = read_value(&app_root, ".brb-cap-note");
            cap.annotations = viewer.annotations();
            sequence.borrow_mut().push(cap);
            let _ = redraw_sequence(&document, &list_el, &sequence, &status_el);
        });
        add_btn
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // Preview: assembled Markdown shown inline (no download).
    {
        let sequence = Rc::clone(&sequence);
        let app_root = app_root.clone();
        let status_el = status_el.clone();
        let preview_el = preview_el.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let report = build_report(&app_root, &sequence);
            match report.validate() {
                Err(issues) => {
                    status_el.set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues)));
                }
                Ok(()) => match markdown::assemble(&report) {
                    Ok(text) => {
                        preview_el.set_text_content(Some(&text));
                        let _ = preview_el.set_attribute("style", "white-space:pre-wrap;max-height:320px;overflow:auto");
                        status_el.set_text_content(Some("Preview below reflects exactly what Export will produce."));
                    }
                    Err(issues) => {
                        status_el
                            .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues)));
                    }
                },
            }
        });
        preview_btn
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // Export Markdown (sequence version — the free-tier "Export Markdown
    // report" button above still exports just the viewer's current image).
    {
        let sequence = Rc::clone(&sequence);
        let app_root = app_root.clone();
        let status_el = status_el.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let report = build_report(&app_root, &sequence);
            match report.validate() {
                Err(issues) => status_el
                    .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues))),
                Ok(()) => match markdown::assemble(&report) {
                    Ok(text) => {
                        let file_name = markdown::file_name(&report);
                        trigger_download_text(&file_name, &text);
                        status_el.set_text_content(Some(&format!("Exported {file_name}.")));
                    }
                    Err(issues) => status_el
                        .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues))),
                },
            }
        });
        md_btn
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // Export for a tracker: .md with relative links + a sidecar .png per
    // screenshot, instead of one self-contained file — GitHub/GitLab/Jira
    // strip `data:` URIs from pasted comments, so the embedded-image export
    // above doesn't survive being pasted into the tools this report targets.
    {
        let sequence = Rc::clone(&sequence);
        let app_root = app_root.clone();
        let status_el = status_el.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let report = build_report(&app_root, &sequence);
            match report.validate() {
                Err(issues) => status_el
                    .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues))),
                Ok(()) => match markdown::assemble_with_sidecars(&report) {
                    Ok((text, images)) => {
                        let file_name = markdown::file_name(&report);
                        trigger_download_text(&file_name, &text);
                        for (image_name, png_bytes) in &images {
                            trigger_download_bytes(image_name, png_bytes, "image/png");
                        }
                        status_el.set_text_content(Some(&format!(
                            "Exported {file_name} + {} screenshot(s).",
                            images.len()
                        )));
                    }
                    Err(issues) => status_el
                        .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues))),
                },
            }
        });
        tracker_btn
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // Export PDF.
    {
        let sequence = Rc::clone(&sequence);
        let app_root = app_root.clone();
        let status_el = status_el.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let report = build_report(&app_root, &sequence);
            match report.validate() {
                Err(issues) => status_el
                    .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues))),
                Ok(()) => match pdf::assemble(&report) {
                    Ok(bytes) => {
                        let file_name = pdf_file_name(&report);
                        trigger_download_bytes(&file_name, &bytes, "application/pdf");
                        status_el.set_text_content(Some(&format!("Exported {file_name}.")));
                    }
                    Err(issues) => status_el
                        .set_text_content(Some(&tpt_bugreport_engine::describe_issues(&issues))),
                },
            }
        });
        pdf_btn
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // In-page hotkey bindings: mirrors the desktop shell's native shortcuts
    // (`Ctrl+Shift+1..4`) so the same combo works while the page has focus,
    // whether that's this webview or a plain browser tab running the pro
    // bundle standalone. The native shell additionally registers these
    // *globally* (fires even when unfocused) and focuses the window on
    // press — see `desktop/src/main.rs` — but cannot push the key straight
    // into the page (no such call exists in `tpt-appfront-webview` today;
    // noted as a framework gap rather than worked around here), so the
    // global path only gets the window in front of the user, who then
    // presses the combo again or clicks the button. Capture-region (2) and
    // paste (4) have no distinct in-page action yet: region reuses the
    // full-screen grab (mark the area with the Box tool) and paste is
    // already covered by the free tier's browser-wide paste listener.
    {
        let window = web_sys::window().expect("no window");
        let closure = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            if !(event.ctrl_key() && event.shift_key() && !event.alt_key()) {
                return;
            }
            match event.code().as_str() {
                "Digit1" | "Digit2" => {
                    event.prevent_default();
                    add_btn_click_screen_capture();
                }
                "Digit3" => {
                    event.prevent_default();
                    // Scrolling capture is a multi-step session (start, grab
                    // N frames, finish); a single keypress can't safely infer
                    // which step the user wants, so this just surfaces the
                    // control rather than guessing.
                    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                        if let Some(el) = doc.query_selector(".brb-capture-status").ok().flatten() {
                            el.set_text_content(Some(
                                "Scrolling capture: use the Start / Capture frame / Finish buttons.",
                            ));
                        }
                    }
                }
                _ => {}
            }
        });
        window
            .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())?;
        closure.forget();
    }

    Ok(())
}

/// `Digit1`/`Digit2` both click the free tier's "Capture screen" button that
/// [`capture::mount_screen_capture_button`] appended into the capture card —
/// found by class rather than threading another handle through, since the
/// hotkey listener is set up after that button already exists in the DOM.
fn add_btn_click_screen_capture() {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(Some(button)) = document.query_selector(".brb-capture button") else {
        return;
    };
    if let Some(el) = button.dyn_ref::<web_sys::HtmlElement>() {
        el.click();
    }
}

fn build_report(app_root: &Element, sequence: &Rc<RefCell<Vec<Capture>>>) -> Report {
    Report {
        meta: read_meta(app_root),
        captures: sequence.borrow().clone(),
    }
}

fn read_meta(container: &Element) -> ReportMeta {
    let mut meta = ReportMeta::new(read_value(container, ".brb-title"));
    meta.summary = read_value(container, ".brb-summary");
    meta.severity =
        tpt_bugreport_engine::Severity::from_slug(&read_value(container, ".brb-severity"));
    meta.reporter = read_value(container, ".brb-reporter");
    meta.environment = tpt_bugreport_engine::Environment {
        app_name: read_value(container, ".brb-env-app"),
        app_version: read_value(container, ".brb-env-version"),
        os: read_value(container, ".brb-env-os"),
        browser: read_value(container, ".brb-env-browser"),
        device: read_value(container, ".brb-env-device"),
    };
    meta.steps = read_value(container, ".brb-steps")
        .lines()
        .map(|line| line.to_string())
        .collect();
    meta
}

fn read_value(container: &Element, selector: &str) -> String {
    container
        .query_selector(selector)
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
        .or_else(|| {
            container
                .query_selector(selector)
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<HtmlTextAreaElement>().ok())
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

fn button(document: &web_sys::Document, label: &str) -> Result<Element, JsValue> {
    let btn = document.create_element("button")?;
    btn.set_attribute("type", "button")?;
    btn.set_attribute("class", "brb-btn")?;
    btn.set_text_content(Some(label));
    Ok(btn)
}

/// Rebuilds the sequence list from scratch — simplest correct option at the
/// list sizes this edition allows (`PRO_MAX_CAPTURES`), and matches the
/// redraw-the-whole-thing style `viewer.rs` already uses for the canvas.
fn redraw_sequence(
    document: &web_sys::Document,
    list_el: &Element,
    sequence: &Rc<RefCell<Vec<Capture>>>,
    status_el: &Element,
) -> Result<(), JsValue> {
    while let Some(child) = list_el.first_child() {
        let _ = list_el.remove_child(&child);
    }
    let items = sequence.borrow();
    if items.is_empty() {
        let empty = document.create_element("p")?;
        empty.set_attribute("class", "brb-hint")?;
        empty.set_text_content(Some("No screenshots in the sequence yet."));
        list_el.append_child(&empty)?;
        return Ok(());
    }
    for (index, capture) in items.iter().enumerate() {
        let row = document.create_element("div")?;
        row.set_attribute("class", "brb-row")?;

        let label = document.create_element("span")?;
        let title = if capture.title.trim().is_empty() {
            format!("Screenshot #{}", index + 1)
        } else {
            capture.title.clone()
        };
        label.set_text_content(Some(&format!(
            "{}. {title} \u{2014} {}x{} px, {} annotation(s), {}",
            index + 1,
            capture.image.width,
            capture.image.height,
            capture.annotations.len(),
            capture.source
        )));
        row.append_child(&label)?;

        let up = button(document, "\u{2191}")?;
        let down = button(document, "\u{2193}")?;
        let remove = button(document, "Remove")?;
        row.append_child(&up)?;
        row.append_child(&down)?;
        row.append_child(&remove)?;
        list_el.append_child(&row)?;

        wire_move(document, &up, list_el, sequence, status_el, index, -1)?;
        wire_move(document, &down, list_el, sequence, status_el, index, 1)?;
        wire_remove(document, &remove, list_el, sequence, status_el, index)?;
    }
    Ok(())
}

fn wire_move(
    document: &web_sys::Document,
    btn: &Element,
    list_el: &Element,
    sequence: &Rc<RefCell<Vec<Capture>>>,
    status_el: &Element,
    index: usize,
    delta: isize,
) -> Result<(), JsValue> {
    let document = document.clone();
    let list_el = list_el.clone();
    let sequence = Rc::clone(sequence);
    let status_el = status_el.clone();
    let closure = Closure::<dyn FnMut()>::new(move || {
        let target = index as isize + delta;
        let len = sequence.borrow().len() as isize;
        if target < 0 || target >= len {
            return;
        }
        sequence.borrow_mut().swap(index, target as usize);
        let _ = redraw_sequence(&document, &list_el, &sequence, &status_el);
    });
    btn.dyn_ref::<web_sys::HtmlElement>()
        .expect("button is an HtmlElement")
        .set_onclick(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
    Ok(())
}

fn wire_remove(
    document: &web_sys::Document,
    btn: &Element,
    list_el: &Element,
    sequence: &Rc<RefCell<Vec<Capture>>>,
    status_el: &Element,
    index: usize,
) -> Result<(), JsValue> {
    let document = document.clone();
    let list_el = list_el.clone();
    let sequence = Rc::clone(sequence);
    let status_el = status_el.clone();
    let closure = Closure::<dyn FnMut()>::new(move || {
        if index < sequence.borrow().len() {
            sequence.borrow_mut().remove(index);
        }
        let _ = redraw_sequence(&document, &list_el, &sequence, &status_el);
    });
    btn.dyn_ref::<web_sys::HtmlElement>()
        .expect("button is an HtmlElement")
        .set_onclick(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
    Ok(())
}

fn pdf_file_name(report: &Report) -> String {
    let md_name = markdown::file_name(report);
    match md_name.strip_suffix(".md") {
        Some(stem) => format!("{stem}.pdf"),
        None => format!("{md_name}.pdf"),
    }
}

fn trigger_download_text(file_name: &str, contents: &str) {
    let array = js_sys::Array::new();
    array.push(&JsValue::from_str(contents));
    trigger_download(file_name, &array.into(), "text/markdown");
}

fn trigger_download_bytes(file_name: &str, bytes: &[u8], mime: &str) {
    let array = js_sys::Array::new();
    let uint8 = js_sys::Uint8Array::from(bytes);
    array.push(&uint8.into());
    trigger_download(file_name, &array.into(), mime);
}

/// Shared blob -> object URL -> temporary anchor click, for both the text
/// (Markdown) and binary (PDF) export paths.
fn trigger_download(file_name: &str, bits: &JsValue, mime: &str) {
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = match web_sys::Blob::new_with_u8_array_sequence_and_options(bits, &options) {
        Ok(blob) => blob,
        Err(_) => return,
    };
    let url = match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(url) => url,
        Err(_) => return,
    };
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(anchor) = document.create_element("a") {
            let _ = anchor.set_attribute("href", &url);
            let _ = anchor.set_attribute("download", file_name);
            let _ = anchor.dyn_ref::<web_sys::HtmlAnchorElement>().map(|a| a.click());
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}
