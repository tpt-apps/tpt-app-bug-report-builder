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
