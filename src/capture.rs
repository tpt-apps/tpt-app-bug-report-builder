//! Screenshot ingestion for the free tier: file upload and clipboard paste.
//!
//! Both paths decode into the engine's [`RgbaImage`] via an off-screen
//! `<canvas>` (`drawImage` + `getImageData`) rather than a PNG decoder — the
//! engine only *writes* PNGs ([`tpt_bugreport_engine::png::encode`]), so the
//! browser's own image decoder is the simplest route for arbitrary
//! JPEG/PNG/WebP uploads.

use std::rc::Rc;

use tpt_bugreport_engine::RgbaImage;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    CanvasRenderingContext2d, Element, HtmlCanvasElement, HtmlImageElement, HtmlInputElement,
};

/// Screenshots arrive at arbitrary DPI; clamp so a pathological upload can't
/// blow the canvas/PNG pipeline's memory.
const MAX_DIMENSION: u32 = 4096;

/// Appends a `<input type=file accept=image/*>` into `container` and wires it
/// to decode the chosen file, calling `on_ready(image, source_label)` once
/// decoding finishes (decoding is async — `HtmlImageElement::onload`).
pub fn mount_upload_input(
    container: &Element,
    on_ready: Rc<dyn Fn(RgbaImage, String)>,
) -> Result<(), JsValue> {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let input: HtmlInputElement = document.create_element("input")?.dyn_into()?;
    input.set_attribute("type", "file")?;
    input.set_attribute("accept", "image/*")?;
    input.set_attribute("class", "brb-file-input")?;
    container.append_child(&input)?;

    let input_ref = input.clone();
    let onchange = Closure::<dyn FnMut()>::new(move || {
        let Some(files) = input_ref.files() else {
            return;
        };
        let Some(file) = files.get(0) else {
            return;
        };
        let name = file.name();
        let blob: web_sys::Blob = file.unchecked_into();
        let on_ready = on_ready.clone();
        let _ = decode_blob(blob, Rc::new(move |image| {
            on_ready(image, format!("upload: {name}"));
        }));
    });
    input.set_onchange(Some(onchange.as_ref().unchecked_ref()));
    onchange.forget();
    Ok(())
}

/// Listens for a browser-wide clipboard paste of an image and decodes it the
/// same way as an upload. Ctrl+V after a screenshot tool copy is the common
/// case this covers.
pub fn mount_paste_listener(on_ready: Rc<dyn Fn(RgbaImage, String)>) -> Result<(), JsValue> {
    let window = web_sys::window().expect("no window");
    let closure = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(move |event: web_sys::ClipboardEvent| {
        let Some(data) = event.clipboard_data() else {
            return;
        };
        let items = data.items();
        for i in 0..items.length() {
            let Some(item) = items.get(i) else { continue };
            if item.kind() != "file" || !item.type_().starts_with("image/") {
                continue;
            }
            if let Ok(Some(file)) = item.get_as_file() {
                event.prevent_default();
                let blob: web_sys::Blob = file.unchecked_into();
                let on_ready = on_ready.clone();
                let _ = decode_blob(blob, Rc::new(move |image| on_ready(image, "paste".to_string())));
                break;
            }
        }
    });
    window.add_event_listener_with_callback("paste", closure.as_ref().unchecked_ref())?;
    closure.forget();
    Ok(())
}

/// Decodes an image `Blob` (upload or clipboard) into an [`RgbaImage`] via a
/// hidden canvas, then hands it to `on_ready`. Async: the callback fires from
/// the image's `onload` event, not before this function returns.
fn decode_blob(blob: web_sys::Blob, on_ready: Rc<dyn Fn(RgbaImage)>) -> Result<(), JsValue> {
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let img = HtmlImageElement::new()?;

    let img_for_load = img.clone();
    let url_for_load = url.clone();
    let onload = Closure::<dyn FnMut()>::new(move || {
        let decoded = rasterize(&img_for_load);
        // The object URL has done its job once the image has decoded; drop it
        // so the blob isn't pinned in memory for the rest of the session.
        let _ = web_sys::Url::revoke_object_url(&url_for_load);
        if let Ok(image) = decoded {
            on_ready(image);
        }
    });
    img.set_onload(Some(onload.as_ref().unchecked_ref()));
    onload.forget();

    let url_for_error = url.clone();
    let onerror = Closure::<dyn FnMut()>::new(move || {
        let _ = web_sys::Url::revoke_object_url(&url_for_error);
    });
    img.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onerror.forget();

    img.set_src(&url);
    Ok(())
}

/// Pro-only screen and scrolling capture, built on the browser/webview's own
/// Screen Capture API (`getDisplayMedia`) — no native capture backend needed
/// (see `desktop/src/main.rs`'s module doc). Both the free WASM edition and
/// the Pro desktop shell run the same wasm-bindgen bundle in a webview, so
/// this works identically in either host.
#[cfg(feature = "pro")]
mod pro {
    use std::cell::RefCell;
    use std::rc::Rc;

    use tpt_bugreport_engine::{stitch, RgbaImage};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{
        CanvasRenderingContext2d, Element, EventTarget, HtmlCanvasElement, HtmlVideoElement,
        MediaStream,
    };

    /// Appends a "Capture screen" button that grabs a single frame via
    /// `getDisplayMedia`. Region capture reuses this (full-frame) grab — the
    /// pro viewer's Box tool marks the area of interest on the resulting
    /// image rather than the engine cropping pixels, which keeps this module
    /// small and lets the same button serve both hotkeys.
    pub fn mount_screen_capture_button(
        container: &Element,
        on_ready: Rc<dyn Fn(RgbaImage, String)>,
    ) -> Result<(), JsValue> {
        let document = web_sys::window().expect("no window").document().expect("no document");
        let btn = document.create_element("button")?;
        btn.set_attribute("type", "button")?;
        btn.set_attribute("class", "brb-btn")?;
        btn.set_text_content(Some("Capture screen"));
        container.append_child(&btn)?;

        let closure = Closure::<dyn FnMut()>::new(move || {
            let on_ready = Rc::clone(&on_ready);
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(stream) = request_display_stream().await {
                    if let Ok(image) = capture_frame_from_stream(&stream).await {
                        on_ready(image, "screen".to_string());
                    }
                    stop_stream(&stream);
                }
            });
        });
        btn.dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
        Ok(())
    }

    /// Appends the scrolling-capture controls: start a shared
    /// `getDisplayMedia` session, grab one frame per click (the user scrolls
    /// the target between clicks — the standard scrolling-capture workflow),
    /// then stitch the frames into one tall image via
    /// [`tpt_bugreport_engine::stitch`].
    pub fn mount_scrolling_capture(
        container: &Element,
        on_ready: Rc<dyn Fn(RgbaImage, String)>,
    ) -> Result<(), JsValue> {
        let document = web_sys::window().expect("no window").document().expect("no document");

        let status = document.create_element("span")?;
        status.set_attribute("class", "brb-capture-status")?;
        status.set_text_content(Some("Scrolling capture: not started."));

        let start_btn = document.create_element("button")?;
        start_btn.set_attribute("type", "button")?;
        start_btn.set_attribute("class", "brb-btn")?;
        start_btn.set_text_content(Some("Start scrolling capture"));
        let grab_btn = document.create_element("button")?;
        grab_btn.set_attribute("type", "button")?;
        grab_btn.set_attribute("class", "brb-btn")?;
        grab_btn.set_text_content(Some("Capture frame"));
        let finish_btn = document.create_element("button")?;
        finish_btn.set_attribute("type", "button")?;
        finish_btn.set_attribute("class", "brb-btn")?;
        finish_btn.set_text_content(Some("Finish & stitch"));
        container.append_child(&start_btn)?;
        container.append_child(&grab_btn)?;
        container.append_child(&finish_btn)?;
        container.append_child(&status)?;

        let stream_slot: Rc<RefCell<Option<MediaStream>>> = Rc::new(RefCell::new(None));
        let frames: Rc<RefCell<Vec<RgbaImage>>> = Rc::new(RefCell::new(Vec::new()));

        {
            let stream_slot = Rc::clone(&stream_slot);
            let frames = Rc::clone(&frames);
            let status = status.clone();
            let closure = Closure::<dyn FnMut()>::new(move || {
                let stream_slot = Rc::clone(&stream_slot);
                let frames = Rc::clone(&frames);
                let status = status.clone();
                frames.borrow_mut().clear();
                wasm_bindgen_futures::spawn_local(async move {
                    match request_display_stream().await {
                        Ok(stream) => {
                            *stream_slot.borrow_mut() = Some(stream);
                            status.set_text_content(Some(
                                "Started — click \"Capture frame\" after each scroll.",
                            ));
                        }
                        Err(_) => status.set_text_content(Some("Screen sharing was not granted.")),
                    }
                });
            });
            start_btn
                .dyn_ref::<web_sys::HtmlElement>()
                .expect("button is an HtmlElement")
                .set_onclick(Some(closure.as_ref().unchecked_ref()));
            closure.forget();
        }
        {
            let stream_slot = Rc::clone(&stream_slot);
            let frames = Rc::clone(&frames);
            let status = status.clone();
            let closure = Closure::<dyn FnMut()>::new(move || {
                let Some(stream) = stream_slot.borrow().clone() else {
                    status.set_text_content(Some("Start the capture first."));
                    return;
                };
                let frames = Rc::clone(&frames);
                let status = status.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(image) = capture_frame_from_stream(&stream).await {
                        frames.borrow_mut().push(image);
                        let n = frames.borrow().len();
                        status.set_text_content(Some(&format!(
                            "{n} frame(s) captured. Scroll, then capture again, or finish."
                        )));
                    }
                });
            });
            grab_btn
                .dyn_ref::<web_sys::HtmlElement>()
                .expect("button is an HtmlElement")
                .set_onclick(Some(closure.as_ref().unchecked_ref()));
            closure.forget();
        }
        {
            let stream_slot = Rc::clone(&stream_slot);
            let frames = Rc::clone(&frames);
            let status = status.clone();
            let closure = Closure::<dyn FnMut()>::new(move || {
                let taken = frames.borrow_mut().split_off(0);
                if let Some(stream) = stream_slot.borrow_mut().take() {
                    stop_stream(&stream);
                }
                if taken.is_empty() {
                    status.set_text_content(Some("No frames captured."));
                    return;
                }
                let count = taken.len();
                match stitch::stitch_vertical(&taken, &stitch::StitchOptions::default()) {
                    Ok(image) => {
                        on_ready(image, format!("scroll-stitch ({count} frames)"));
                        status.set_text_content(Some(&format!(
                            "Stitched {count} frame(s) into one image."
                        )));
                    }
                    Err(e) => status.set_text_content(Some(&format!("Could not stitch: {e}"))),
                }
            });
            finish_btn
                .dyn_ref::<web_sys::HtmlElement>()
                .expect("button is an HtmlElement")
                .set_onclick(Some(closure.as_ref().unchecked_ref()));
            closure.forget();
        }
        Ok(())
    }

    async fn request_display_stream() -> Result<MediaStream, JsValue> {
        let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
        let media_devices = window.navigator().media_devices()?;
        let promise = media_devices.get_display_media()?;
        let value = wasm_bindgen_futures::JsFuture::from(promise).await?;
        value.dyn_into::<MediaStream>()
    }

    /// Grabs the current frame of `stream` into an [`RgbaImage`] via a hidden
    /// `<video>` + off-screen canvas — the same decode-through-canvas route
    /// [`super::rasterize`] uses for uploads/paste.
    async fn capture_frame_from_stream(stream: &MediaStream) -> Result<RgbaImage, JsValue> {
        let document = web_sys::window().expect("no window").document().expect("no document");
        let video: HtmlVideoElement = document.create_element("video")?.dyn_into()?;
        video.set_muted(true);
        video.set_src_object(Some(stream));
        wait_for_event(&video, "loadedmetadata").await?;
        let _ = video.play();
        wait_animation_frame().await;

        let width = video.video_width().max(1);
        let height = video.video_height().max(1);
        let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
        canvas.set_width(width);
        canvas.set_height(height);
        let ctx: CanvasRenderingContext2d = canvas
            .get_context("2d")?
            .ok_or_else(|| JsValue::from_str("no 2d context"))?
            .dyn_into()?;
        ctx.draw_image_with_html_video_element_and_dw_and_dh(
            &video,
            0.0,
            0.0,
            width as f64,
            height as f64,
        )?;
        let image_data = ctx.get_image_data(0.0, 0.0, width as f64, height as f64)?;
        RgbaImage::from_rgba(width, height, image_data.data().0)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    fn stop_stream(stream: &MediaStream) {
        let tracks = stream.get_tracks();
        for i in 0..tracks.length() {
            if let Ok(track) = tracks.get(i).dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
    }

    /// Resolves once `event` fires on `target` — used to wait for the
    /// `<video>`'s metadata (so `video_width`/`video_height` are populated)
    /// before drawing a frame.
    async fn wait_for_event(target: &EventTarget, event: &str) -> Result<(), JsValue> {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let closure = Closure::once_into_js(move || {
                let _ = resolve.call0(&JsValue::NULL);
            });
            let _ = target.add_event_listener_with_callback(event, closure.unchecked_ref());
        });
        wasm_bindgen_futures::JsFuture::from(promise).await?;
        Ok(())
    }

    /// Resolves on the next animation frame — gives the `<video>` a moment to
    /// actually paint a frame after `play()` before it is drawn to canvas.
    async fn wait_animation_frame() {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let window = web_sys::window().expect("no window");
            let _ = window.request_animation_frame(&resolve);
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }
}

#[cfg(feature = "pro")]
pub use pro::{mount_scrolling_capture, mount_screen_capture_button};

/// Draws a decoded `<img>` onto an off-screen canvas sized to (clamped)
/// natural dimensions and reads the pixels back as an [`RgbaImage`].
fn rasterize(img: &HtmlImageElement) -> Result<RgbaImage, JsValue> {
    let width = img.natural_width().clamp(1, MAX_DIMENSION);
    let height = img.natural_height().clamp(1, MAX_DIMENSION);

    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");
    let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
    canvas.set_width(width);
    canvas.set_height(height);
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into()?;
    // Scale into the (possibly clamped) canvas size rather than drawing at
    // intrinsic size, so an oversized screenshot is downsampled, not cropped.
    ctx.draw_image_with_html_image_element_and_dw_and_dh(
        img,
        0.0,
        0.0,
        width as f64,
        height as f64,
    )?;
    let image_data = ctx.get_image_data(0.0, 0.0, width as f64, height as f64)?;
    RgbaImage::from_rgba(width, height, image_data.data().0)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
