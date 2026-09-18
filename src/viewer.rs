//! The annotation canvas: draws the captured screenshot on a `<canvas>` and
//! lets the user drag out arrow/box/text/blur mark-up, held as engine
//! [`Annotation`] values (image-pixel coordinates, so they survive any CSS
//! scaling of the canvas).
//!
//! Like `tpt-app-fea-lite`'s mesh viewer, the canvas is raw `web-sys` — the
//! DOM backend has no canvas node kind — appended into a placeholder
//! container by [`mount`]. Committed mark-up is rasterised through
//! [`tpt_bugreport_engine::annotate::render_image`], the same function the
//! Markdown export uses, so what you see here is exactly what ships.

use std::cell::RefCell;
use std::rc::Rc;

use tpt_bugreport_engine::model::Rgb;
use tpt_bugreport_engine::{annotate, Annotation, Rect, RgbaImage};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{CanvasRenderingContext2d, Element, HtmlCanvasElement, HtmlSelectElement, MouseEvent};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Arrow,
    Box,
    Text,
    Blur,
}

impl Tool {
    fn from_slug(slug: &str) -> Tool {
        match slug {
            "box" => Tool::Box,
            "text" => Tool::Text,
            "blur" => Tool::Blur,
            _ => Tool::Arrow,
        }
    }
}

/// The named colours the engine's report legend recognises by name (see
/// `model::rgb_name`) — keeping the palette in step means annotations read
/// as "red" rather than "rgb(211, 47, 47)" in the exported report.
const PALETTE: [(&str, Rgb); 4] = [
    ("red", [211, 47, 47]),
    ("amber", [245, 165, 0]),
    ("green", [46, 160, 67]),
    ("blue", [25, 118, 210]),
];

struct ViewerState {
    image: Option<RgbaImage>,
    annotations: Vec<Annotation>,
    tool: Tool,
    color: Rgb,
    stroke_width: f64,
    blur_strength: u8,
    drag_from: Option<(f64, f64)>,
}

impl ViewerState {
    fn new() -> Self {
        ViewerState {
            image: None,
            annotations: Vec::new(),
            tool: Tool::Arrow,
            color: PALETTE[0].1,
            stroke_width: 4.0,
            blur_strength: 12,
            drag_from: None,
        }
    }
}

/// A mounted annotation canvas plus enough of a handle to read/replace its
/// image and read its committed annotations from `app.rs`.
#[derive(Clone)]
pub struct Viewer {
    state: Rc<RefCell<ViewerState>>,
    canvas: HtmlCanvasElement,
}

impl Viewer {
    /// Loads a freshly captured screenshot, discarding any prior image's
    /// annotations (a new capture is a new drawing surface).
    pub fn set_image(&self, image: RgbaImage) {
        {
            let mut state = self.state.borrow_mut();
            let (w, h) = (image.width, image.height);
            state.image = Some(image);
            state.annotations.clear();
            state.drag_from = None;
            self.canvas.set_width(w);
            self.canvas.set_height(h);
        }
        redraw(&self.canvas, &self.state.borrow());
    }

    pub fn image(&self) -> Option<RgbaImage> {
        self.state.borrow().image.clone()
    }

    pub fn annotations(&self) -> Vec<Annotation> {
        self.state.borrow().annotations.clone()
    }
}

/// Builds the toolbar + canvas inside `container` and wires mouse mark-up.
/// `container` should be an otherwise-empty placeholder div.
pub fn mount(container: &Element) -> Result<Viewer, JsValue> {
    let document = web_sys::window()
        .expect("no window")
        .document()
        .expect("no document");

    let state = Rc::new(RefCell::new(ViewerState::new()));

    let toolbar = document.create_element("div")?;
    toolbar.set_attribute("class", "brb-toolbar")?;
    container.append_child(&toolbar)?;

    let canvas: HtmlCanvasElement = document.create_element("canvas")?.dyn_into()?;
    canvas.set_attribute("class", "brb-canvas")?;
    canvas.set_width(1);
    canvas.set_height(1);
    let canvas_holder = document.create_element("div")?;
    canvas_holder.set_attribute("class", "brb-canvas-holder")?;
    canvas_holder.append_child(&canvas)?;
    container.append_child(&canvas_holder)?;

    // Tool buttons.
    let tool_group = document.create_element("div")?;
    tool_group.set_attribute("class", "brb-tool-group")?;
    toolbar.append_child(&tool_group)?;
    for (slug, label) in [("arrow", "Arrow"), ("box", "Box"), ("text", "Text"), ("blur", "Blur")] {
        let btn = document.create_element("button")?;
        btn.set_attribute("type", "button")?;
        btn.set_attribute("class", "brb-btn brb-tool-btn")?;
        btn.set_attribute("data-tool", slug)?;
        btn.set_text_content(Some(label));
        tool_group.append_child(&btn)?;
        let state = Rc::clone(&state);
        let tool_group_el = tool_group.clone();
        let slug_owned = slug.to_string();
        let closure = Closure::<dyn FnMut()>::new(move || {
            state.borrow_mut().tool = Tool::from_slug(&slug_owned);
            mark_active(&tool_group_el, "data-tool", &slug_owned);
        });
        btn.dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }
    mark_active(&tool_group, "data-tool", "arrow");

    // Colour select.
    let color_select: HtmlSelectElement = document.create_element("select")?.dyn_into()?;
    color_select.set_attribute("class", "brb-input brb-color")?;
    for (name, _) in PALETTE {
        let opt = document.create_element("option")?;
        opt.set_attribute("value", name)?;
        opt.set_text_content(Some(name));
        color_select.append_child(&opt)?;
    }
    toolbar.append_child(&color_select)?;
    {
        let state = Rc::clone(&state);
        let select_ref = color_select.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let value = select_ref.value();
            if let Some((_, rgb)) = PALETTE.iter().find(|(name, _)| *name == value) {
                state.borrow_mut().color = *rgb;
            }
        });
        color_select.set_onchange(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // Stroke width (arrow/box outline thickness).
    append_range_control(
        &document,
        &toolbar,
        "brb-width",
        "Width",
        1.0,
        40.0,
        1.0,
        4.0,
        {
            let state = Rc::clone(&state);
            move |value| state.borrow_mut().stroke_width = value
        },
    )?;

    // Blur block size.
    append_range_control(
        &document,
        &toolbar,
        "brb-blur",
        "Blur",
        2.0,
        64.0,
        1.0,
        12.0,
        {
            let state = Rc::clone(&state);
            move |value| state.borrow_mut().blur_strength = value.round().clamp(2.0, 64.0) as u8
        },
    )?;

    // Undo / clear.
    let undo = document.create_element("button")?;
    undo.set_attribute("type", "button")?;
    undo.set_attribute("class", "brb-btn")?;
    undo.set_text_content(Some("Undo"));
    toolbar.append_child(&undo)?;
    {
        let state = Rc::clone(&state);
        let canvas_ref = canvas.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            state.borrow_mut().annotations.pop();
            redraw(&canvas_ref, &state.borrow());
        });
        undo.dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    let clear = document.create_element("button")?;
    clear.set_attribute("type", "button")?;
    clear.set_attribute("class", "brb-btn")?;
    clear.set_text_content(Some("Clear mark-up"));
    toolbar.append_child(&clear)?;
    {
        let state = Rc::clone(&state);
        let canvas_ref = canvas.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            state.borrow_mut().annotations.clear();
            redraw(&canvas_ref, &state.borrow());
        });
        clear.dyn_ref::<web_sys::HtmlElement>()
            .expect("button is an HtmlElement")
            .set_onclick(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    wire_mouse(&canvas, &state);

    Ok(Viewer { state, canvas })
}

fn append_range_control(
    document: &web_sys::Document,
    toolbar: &Element,
    class: &str,
    label: &str,
    min: f64,
    max: f64,
    step: f64,
    default: f64,
    on_change: impl Fn(f64) + 'static,
) -> Result<(), JsValue> {
    let wrap = document.create_element("label")?;
    wrap.set_attribute("class", "brb-range-label")?;
    wrap.set_text_content(Some(label));
    let input: web_sys::HtmlInputElement = document.create_element("input")?.dyn_into()?;
    input.set_attribute("type", "range")?;
    input.set_attribute("class", &format!("brb-input {class}"))?;
    input.set_attribute("min", &min.to_string())?;
    input.set_attribute("max", &max.to_string())?;
    input.set_attribute("step", &step.to_string())?;
    input.set_attribute("value", &default.to_string())?;
    wrap.append_child(&input)?;
    toolbar.append_child(&wrap)?;

    let input_ref = input.clone();
    let closure = Closure::<dyn FnMut()>::new(move || {
        if let Ok(value) = input_ref.value().parse::<f64>() {
            on_change(value);
        }
    });
    input.set_oninput(Some(closure.as_ref().unchecked_ref()));
    closure.forget();
    Ok(())
}

/// Toggles a `brb-active` class so the one button matching `attr=value`
/// stands out.
fn mark_active(group: &Element, attr: &str, value: &str) {
    let Ok(children) = group.query_selector_all("button") else {
        return;
    };
    for i in 0..children.length() {
        if let Some(node) = children.item(i) {
            if let Some(el) = node.dyn_ref::<Element>() {
                let is_active = el.get_attribute(attr).as_deref() == Some(value);
                let _ = el.set_attribute("class", if is_active { "brb-btn brb-tool-btn brb-active" } else { "brb-btn brb-tool-btn" });
            }
        }
    }
}

/// Redraws `canvas` from `state`'s image plus its committed annotations
/// (drag-in-progress previews are not persisted, so nothing extra to do here
/// beyond what [`wire_mouse`]'s move handler layers on top).
fn redraw(canvas: &HtmlCanvasElement, state: &ViewerState) {
    let Some(image) = &state.image else {
        return;
    };
    let composed = annotate::render_image(image, &state.annotations);
    let _ = paint(canvas, &composed);
}

fn paint(canvas: &HtmlCanvasElement, image: &RgbaImage) -> Result<(), JsValue> {
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into()?;
    let clamped = wasm_bindgen::Clamped(image.data.as_slice());
    let image_data =
        web_sys::ImageData::new_with_u8_clamped_array(clamped, image.width)?;
    ctx.put_image_data(&image_data, 0.0, 0.0)
}

/// Maps a mouse event's client coordinates to image-pixel coordinates,
/// accounting for the canvas's CSS display size differing from its backing
/// pixel size (the canvas is `width:100%` in CSS but keeps 1:1 pixels
/// internally, so annotations stay accurate at any zoom).
fn image_coords(canvas: &HtmlCanvasElement, event: &MouseEvent) -> (f64, f64) {
    let rect = canvas.get_bounding_client_rect();
    let (rw, rh) = (rect.width().max(1.0), rect.height().max(1.0));
    let scale_x = canvas.width() as f64 / rw;
    let scale_y = canvas.height() as f64 / rh;
    let x = (event.client_x() as f64 - rect.left()) * scale_x;
    let y = (event.client_y() as f64 - rect.top()) * scale_y;
    (x, y)
}

fn wire_mouse(canvas: &HtmlCanvasElement, state: &Rc<RefCell<ViewerState>>) {
    // mousedown: start a drag (arrow/box/blur) or prompt for text (text tool
    // commits immediately — there's nothing to drag).
    {
        let state = Rc::clone(state);
        let canvas_ref = canvas.clone();
        let closure = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            if state.borrow().image.is_none() {
                return;
            }
            let (x, y) = image_coords(&canvas_ref, &event);
            let tool = state.borrow().tool;
            if tool == Tool::Text {
                let text = web_sys::window()
                    .and_then(|w| w.prompt_with_message("Annotation text:").ok())
                    .flatten()
                    .unwrap_or_default();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    let color = state.borrow().color;
                    state.borrow_mut().annotations.push(Annotation::Text {
                        at: [x, y],
                        text,
                        size: 18.0,
                        color,
                    });
                    redraw(&canvas_ref, &state.borrow());
                }
            } else {
                state.borrow_mut().drag_from = Some((x, y));
            }
        });
        canvas
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("canvas is an HtmlElement")
            .set_onmousedown(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // mousemove: live preview of the in-progress drag, drawn on top of the
    // committed-annotation redraw (cheap enough at screenshot resolution).
    {
        let state = Rc::clone(state);
        let canvas_ref = canvas.clone();
        let closure = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            let from = state.borrow().drag_from;
            let Some(from) = from else { return };
            let (x, y) = image_coords(&canvas_ref, &event);
            redraw(&canvas_ref, &state.borrow());
            let _ = preview(&canvas_ref, &state.borrow(), from, (x, y));
        });
        canvas
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("canvas is an HtmlElement")
            .set_onmousemove(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // mouseup: commit the drag as an annotation (dropping degenerate ones the
    // engine would reject anyway — a zero-length arrow or empty box/blur).
    {
        let state = Rc::clone(state);
        let canvas_ref = canvas.clone();
        let closure = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            let from = state.borrow_mut().drag_from.take();
            let Some((fx, fy)) = from else { return };
            let (x, y) = image_coords(&canvas_ref, &event);
            let (tool, color, stroke_width, blur_strength) = {
                let s = state.borrow();
                (s.tool, s.color, s.stroke_width, s.blur_strength)
            };
            let annotation = match tool {
                Tool::Arrow => {
                    if (fx - x).abs() < 1.0 && (fy - y).abs() < 1.0 {
                        None
                    } else {
                        Some(Annotation::Arrow {
                            from: [fx, fy],
                            to: [x, y],
                            color,
                            width: stroke_width,
                        })
                    }
                }
                Tool::Box => {
                    let rect = Rect::from_drag(fx as i32, fy as i32, x as i32, y as i32);
                    (!rect.is_empty()).then_some(Annotation::Box {
                        rect,
                        color,
                        width: stroke_width,
                    })
                }
                Tool::Blur => {
                    let rect = Rect::from_drag(fx as i32, fy as i32, x as i32, y as i32);
                    (!rect.is_empty()).then_some(Annotation::Blur {
                        rect,
                        strength: blur_strength,
                    })
                }
                Tool::Text => None,
            };
            if let Some(annotation) = annotation {
                state.borrow_mut().annotations.push(annotation);
            }
            redraw(&canvas_ref, &state.borrow());
        });
        canvas
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("canvas is an HtmlElement")
            .set_onmouseup(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }

    // mouseleave: cancel an in-progress drag rather than leaving a stale one
    // that commits on the next unrelated mouseup.
    {
        let state = Rc::clone(state);
        let canvas_ref = canvas.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            if state.borrow_mut().drag_from.take().is_some() {
                redraw(&canvas_ref, &state.borrow());
            }
        });
        canvas
            .dyn_ref::<web_sys::HtmlElement>()
            .expect("canvas is an HtmlElement")
            .set_onmouseleave(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
    }
}

/// Draws a cheap, non-committed preview of the shape currently being dragged
/// directly with the 2D API (no engine round-trip needed for a preview).
fn preview(
    canvas: &HtmlCanvasElement,
    state: &ViewerState,
    from: (f64, f64),
    to: (f64, f64),
) -> Result<(), JsValue> {
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into()?;
    let [r, g, b] = state.color;
    ctx.set_stroke_style_str(&format!("rgba({r}, {g}, {b}, 0.9)"));
    ctx.set_line_width(state.stroke_width);
    match state.tool {
        Tool::Arrow => {
            ctx.begin_path();
            ctx.move_to(from.0, from.1);
            ctx.line_to(to.0, to.1);
            ctx.stroke();
        }
        Tool::Box | Tool::Blur => {
            let x = from.0.min(to.0);
            let y = from.1.min(to.1);
            let w = (to.0 - from.0).abs();
            let h = (to.1 - from.1).abs();
            ctx.stroke_rect(x, y, w, h);
        }
        Tool::Text => {}
    }
    Ok(())
}
