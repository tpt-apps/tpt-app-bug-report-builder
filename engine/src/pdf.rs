//! Pro edition: a dependency-free PDF 1.4 report writer.
//!
//! The layout walks the same ordered captures the Markdown writer uses, so the
//! two exports agree on content and order:
//!
//! * **page 1** — title, summary, the metadata/environment table, numbered
//!   reproduction steps and a screenshot index;
//! * **one page per screenshot** — caption, provenance line, the screenshot
//!   (with redactions burnt into the pixels) and the annotation index.
//!
//! Annotations are *not* rasterised into the image on this path: arrows, boxes
//! and text are emitted as PDF vector operators on top of the image XObject, so
//! a printed report has crisp mark-up at any zoom. Only [`Annotation::Blur`]
//! touches pixels, because a redaction has to be irreversible.
//!
//! Images are embedded as `/DeviceRGB` `/FlateDecode` XObjects using the
//! engine's own zlib writer ([`crate::deflate::zlib_compress`]).

use crate::deflate::zlib_compress;
use crate::model::{format_utc, Annotation, Capture, Report, ReportError, Rgb};

/// Letter portrait, in PDF points.
const PAGE_W: f64 = 612.0;
const PAGE_H: f64 = 792.0;
const MARGIN: f64 = 48.0;
const TEXT_W: f64 = PAGE_W - 2.0 * MARGIN;
/// Space reserved at the bottom of every page for the footer.
const FOOTER_H: f64 = 26.0;
const BODY_SIZE: f64 = 10.0;
const SMALL_SIZE: f64 = 8.6;
/// Baseline-to-baseline distance for body text.
const LINE: f64 = 13.6;
/// Helvetica's average advance width as a fraction of the font size. The
/// built-in fonts are not embedded, so wrapping uses this standard estimate
/// instead of per-glyph metrics.
const AVG_ADVANCE: f64 = 0.5;
const BOLD_ADVANCE: f64 = 0.53;
/// Column split for the two-column metadata tables.
const LABEL_W: f64 = 132.0;

/// Which of the two built-in fonts an op uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Regular,
    Bold,
}

/// One drawing operation on a page, in PDF points with the origin bottom-left.
#[derive(Debug, Clone)]
enum Op {
    Text {
        x: f64,
        y: f64,
        size: f64,
        font: Font,
        color: Rgb,
        text: String,
    },
    Line {
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        width: f64,
        color: Rgb,
    },
    Triangle {
        points: [(f64, f64); 3],
        color: Rgb,
    },
    RectStroke {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        width: f64,
        color: Rgb,
    },
    Image {
        name: String,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
}

/// An image XObject waiting to be numbered.
struct PendingImage {
    name: String,
    width: u32,
    height: u32,
    /// Flattened RGB rows, already zlib-compressed.
    compressed: Vec<u8>,
}

/// Estimated width of `text` in points.
fn text_width(text: &str, size: f64, font: Font) -> f64 {
    let advance = if font == Font::Bold {
        BOLD_ADVANCE
    } else {
        AVG_ADVANCE
    };
    text.chars().count() as f64 * size * advance
}

/// Greedy word wrap to `max_width` points.
fn wrap(text: &str, max_width: f64, size: f64, font: Font) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if text_width(&candidate, size, font) <= max_width || current.is_empty() {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Escapes a string for a PDF literal and transliterates the characters the
/// built-in fonts' WinAnsi encoding cannot carry.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for ch in text.chars() {
        match ch {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            '\u{2014}' | '\u{2013}' => out.push('-'),   // em/en dash
            '\u{2018}' | '\u{2019}' => out.push('\''),  // curly single quotes
            '\u{201c}' | '\u{201d}' => out.push('"'),   // curly double quotes
            '\u{2026}' => out.push_str("..."),
            '\u{2192}' => out.push_str("->"),
            '\u{2190}' => out.push_str("<-"),
            '\u{00b7}' | '\u{2022}' => out.push('\u{b7}'), // middle dot / bullet
            '\u{00d7}' => out.push('x'),                // multiplication sign
            '\t' => out.push(' '),
            c if (c as u32) < 128 => out.push(c),
            c if (c as u32) < 256 => out.push(c), // WinAnsi range
            _ => out.push('?'),
        }
    }
    out
}

/// `r g b` triple in the 0-1 range PDF uses, rounded to three decimals.
fn color_operands(color: Rgb) -> String {
    format!(
        "{:.3} {:.3} {:.3}",
        color[0] as f64 / 255.0,
        color[1] as f64 / 255.0,
        color[2] as f64 / 255.0
    )
}

/// A paginating cursor over a list of pages of drawing ops.
struct Layout {
    pages: Vec<Vec<Op>>,
    /// Current baseline height above the page bottom.
    y: f64,
}

impl Layout {
    fn new() -> Self {
        Layout {
            pages: vec![Vec::new()],
            y: PAGE_H - MARGIN,
        }
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Space between the cursor and the footer.
    fn space_left(&self) -> f64 {
        self.y - MARGIN - FOOTER_H
    }

    fn open_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = PAGE_H - MARGIN;
    }

    /// Starts a new page if `height` points would not fit.
    fn ensure(&mut self, height: f64) {
        if self.space_left() < height {
            self.open_page();
        }
    }

    fn push(&mut self, op: Op) {
        let last = self.pages.len() - 1;
        self.pages[last].push(op);
    }

    fn gap(&mut self, height: f64) {
        self.y -= height;
    }

    /// One line of text without wrapping (headings, captions).
    fn line(&mut self, text: &str, size: f64, font: Font, color: Rgb, indent: f64) {
        self.ensure(LINE);
        self.y -= size;
        self.push(Op::Text {
            x: MARGIN + indent,
            y: self.y,
            size,
            font,
            color,
            text: escape(text),
        });
        self.y -= LINE - size;
    }

    /// A wrapped paragraph.
    fn paragraph(&mut self, text: &str, size: f64, font: Font, color: Rgb, indent: f64) {
        for line in wrap(text, TEXT_W - indent, size, font) {
            self.line(&line, size, font, color, indent);
        }
    }

    fn heading(&mut self, text: &str) {
        self.gap(6.0);
        self.line(text, 13.0, Font::Bold, [24, 24, 24], 0.0);
        self.gap(2.0);
        self.rule([200, 200, 200], 0.5);
        self.gap(4.0);
    }

    fn rule(&mut self, color: Rgb, width: f64) {
        self.ensure(2.0);
        self.y -= 4.0;
        self.push(Op::Line {
            x0: MARGIN,
            y0: self.y,
            x1: PAGE_W - MARGIN,
            y1: self.y,
            width,
            color,
        });
        self.y -= 4.0;
    }

    /// A two-column label/value table with a rule under each row.
    fn table(&mut self, rows: &[(String, String)], label_width: f64) {
        let value_width = TEXT_W - label_width - 8.0;
        for (label, value) in rows {
            let label_lines = wrap(label, label_width, SMALL_SIZE, Font::Bold);
            let value_lines = wrap(value, value_width, SMALL_SIZE, Font::Regular);
            let height = label_lines.len().max(value_lines.len()) as f64 * LINE + 2.0;
            self.ensure(height);
            let top = self.y;
            for line in &label_lines {
                self.line(line, SMALL_SIZE, Font::Bold, [70, 70, 70], 0.0);
            }
            self.y = top;
            for line in &value_lines {
                self.line(line, SMALL_SIZE, Font::Regular, [24, 24, 24], label_width);
            }
            self.y = self.y.min(top - height) - 1.0;
            self.push(Op::Line {
                x0: MARGIN,
                y0: self.y + 2.0,
                x1: PAGE_W - MARGIN,
                y1: self.y + 2.0,
                width: 0.4,
                color: [224, 224, 224],
            });
        }
    }

    /// Places an image op and returns the page-space rect it occupies
    /// (`(x, y_top, width, height)`), which the vector annotation pass needs.
    fn image(&mut self, name: &str, image_w: u32, image_h: u32, reserve_below: f64) -> (f64, f64, f64, f64) {
        let available = (self.space_left() - reserve_below).max(140.0);
        let scale = (TEXT_W / image_w as f64).min(available / image_h as f64);
        let width = image_w as f64 * scale;
        let height = image_h as f64 * scale;
        self.ensure(height + 6.0);
        let x = MARGIN + (TEXT_W - width) / 2.0;
        let y_top = self.y;
        self.push(Op::Image {
            name: name.to_string(),
            x,
            y: y_top - height,
            w: width,
            h: height,
        });
        self.y = y_top - height - 8.0;
        (x, y_top, width, height)
    }
}

/// Assembles the report as PDF bytes, or returns every validation problem.
pub fn assemble(report: &Report) -> Result<Vec<u8>, Vec<ReportError>> {
    assemble_at(report, 0.0)
}

/// Like [`assemble`], but stamps the generation time (`0.0` to omit it).
///
/// The clock is passed in rather than read here so the writer stays pure and
/// its output is reproducible in tests.
pub fn assemble_at(report: &Report, now_ms: f64) -> Result<Vec<u8>, Vec<ReportError>> {
    report.validate()?;
    let ordered = report.ordered();
    let mut layout = Layout::new();
    let mut images: Vec<PendingImage> = Vec::new();

    // ---- page 1: header, metadata, steps, index ----------------------------
    layout.line(&report.meta.title, 20.0, Font::Bold, [17, 17, 17], 0.0);
    layout.gap(3.0);
    let mut subtitle = format!(
        "TPT Bug Report Builder - {} edition",
        crate::model::edition_name()
    );
    if now_ms > 0.0 {
        subtitle.push_str(&format!(" - generated {}", format_utc(now_ms)));
    }
    layout.paragraph(&subtitle, SMALL_SIZE, Font::Regular, [110, 110, 110], 0.0);
    layout.gap(6.0);

    if !report.meta.summary.trim().is_empty() {
        layout.paragraph(report.meta.summary.trim(), BODY_SIZE, Font::Regular, [24, 24, 24], 0.0);
        layout.gap(4.0);
    }

    let mut rows = crate::markdown::meta_rows(&report.meta);
    rows.push(("Screenshots".to_string(), ordered.len().to_string()));
    rows.push((
        "Annotations".to_string(),
        report.annotation_count().to_string(),
    ));
    layout.heading("Report");
    layout.table(&rows, LABEL_W);

    let steps = report.meta.clean_steps();
    if !steps.is_empty() {
        layout.heading("Steps to reproduce");
        for (index, step) in steps.iter().enumerate() {
            layout.paragraph(
                &format!("{}. {}", index + 1, step),
                BODY_SIZE,
                Font::Regular,
                [24, 24, 24],
                0.0,
            );
        }
        layout.gap(4.0);
    }

    layout.heading("Screenshot index");
    let index_rows: Vec<(String, String)> = ordered
        .iter()
        .enumerate()
        .map(|(index, capture)| {
            let caption = if capture.title.trim().is_empty() {
                format!("Screenshot {}", index + 1)
            } else {
                capture.title.trim().to_string()
            };
            let detail = if capture.source.trim().is_empty() {
                caption
            } else {
                format!("{} ({})", caption, capture.source.trim())
            };
            (format!("#{}", index + 1), detail)
        })
        .collect();
    layout.table(&index_rows, 40.0);

    // ---- one page per screenshot ------------------------------------------
    for (index, capture) in ordered.iter().enumerate() {
        layout.open_page();
        let caption = if capture.title.trim().is_empty() {
            format!("Screenshot {}", index + 1)
        } else {
            capture.title.trim().to_string()
        };
        layout.heading(&format!("{}. {}", index + 1, caption));

        let mut provenance = Vec::new();
        if !capture.source.trim().is_empty() {
            provenance.push(format!("Source: {}", capture.source.trim()));
        }
        if capture.taken_at_ms > 0.0 {
            provenance.push(format!("Captured: {}", format_utc(capture.taken_at_ms)));
        }
        provenance.push(format!(
            "Image: {} x {} px",
            capture.image.width, capture.image.height
        ));
        layout.paragraph(
            &provenance.join("  |  "),
            SMALL_SIZE,
            Font::Regular,
            [110, 110, 110],
            0.0,
        );
        layout.gap(4.0);

        // Redactions are burnt into the pixels; every other annotation is drawn
        // as vectors over the image below.
        let mut base = capture.image.clone();
        for annotation in &capture.annotations {
            if let Annotation::Blur { rect, strength } = annotation {
                base.mosaic(*rect, *strength as i32);
            }
        }
        let name = format!("Im{index}");
        let compressed = zlib_compress(&base.to_rgb());
        images.push(PendingImage {
            name: name.clone(),
            width: base.width,
            height: base.height,
            compressed,
        });

        // Leave room for the annotation index under the image.
        let reserve = 40.0 + capture.annotations.len() as f64 * LINE;
        let (x, y_top, width, _height) =
            layout.image(&name, base.width, base.height, reserve);
        let scale = width / base.width as f64;
        draw_vector_annotations(&mut layout, capture, x, y_top, scale);

        if !capture.note.trim().is_empty() {
            layout.gap(2.0);
            layout.paragraph(capture.note.trim(), BODY_SIZE, Font::Regular, [60, 60, 60], 8.0);
        }
        if !capture.annotations.is_empty() {
            layout.gap(2.0);
            layout.line(
                &format!("Mark-up ({}):", capture.annotations.len()),
                SMALL_SIZE,
                Font::Bold,
                [70, 70, 70],
                0.0,
            );
            for annotation in &capture.annotations {
                layout.paragraph(
                    &format!("- {}", annotation.describe()),
                    SMALL_SIZE,
                    Font::Regular,
                    [60, 60, 60],
                    8.0,
                );
            }
        }
    }

    add_footers(&mut layout);
    Ok(serialize(&layout.pages, &images, &report.meta.title))
}

/// Draws arrows, boxes and text as PDF vectors on top of an image op.
///
/// `x0`/`y_top` are the image's top-left corner in page space and `scale`
/// converts image pixels to points, so the mark-up lands exactly where the user
/// drew it however the image was scaled to fit the page.
fn draw_vector_annotations(
    layout: &mut Layout,
    capture: &Capture,
    x0: f64,
    y_top: f64,
    scale: f64,
) {
    for annotation in &capture.annotations {
        match annotation {
            Annotation::Arrow {
                from,
                to,
                color,
                width,
            } => {
                let (fx, fy) = (x0 + from[0] * scale, y_top - from[1] * scale);
                let (tx, ty) = (x0 + to[0] * scale, y_top - to[1] * scale);
                let (dx, dy) = (tx - fx, ty - fy);
                let length = (dx * dx + dy * dy).sqrt();
                if length < 0.5 {
                    continue;
                }
                let (ux, uy) = (dx / length, dy / length);
                let line_width = (width * scale).max(0.7);
                let head = ((width * 3.5).max(9.0) * scale).min(length);
                let half = head * 0.42;
                let (bx, by) = (tx - ux * head, ty - uy * head);
                let (nx, ny) = (-uy, ux);
                layout.push(Op::Line {
                    x0: fx,
                    y0: fy,
                    x1: bx,
                    y1: by,
                    width: line_width,
                    color: *color,
                });
                layout.push(Op::Triangle {
                    points: [
                        (tx, ty),
                        (bx + nx * half, by + ny * half),
                        (bx - nx * half, by - ny * half),
                    ],
                    color: *color,
                });
            }
            Annotation::Box {
                rect,
                color,
                width,
            } => {
                layout.push(Op::RectStroke {
                    x: x0 + rect.x as f64 * scale,
                    y: y_top - (rect.y + rect.h) as f64 * scale,
                    w: rect.w as f64 * scale,
                    h: rect.h as f64 * scale,
                    width: (width * scale).max(0.7),
                    color: *color,
                });
            }
            Annotation::Text {
                at,
                text,
                size,
                color,
            } => {
                let size_pt = (size * scale).max(4.0);
                layout.push(Op::Text {
                    x: x0 + at[0] * scale,
                    // PDF positions text on its baseline; our annotation
                    // coordinate is the top-left of the glyph cell.
                    y: y_top - at[1] * scale - size_pt * 0.8,
                    size: size_pt,
                    font: Font::Regular,
                    color: *color,
                    text: escape(text),
                });
            }
            // Burnt into the pixels before the image was embedded.
            Annotation::Blur { .. } => {}
        }
    }
}

/// Stamps the footer on every page once the total page count is known.
fn add_footers(layout: &mut Layout) {
    let total = layout.page_count();
    let label = format!(
        "TPT Bug Report Builder {} v{}",
        crate::model::edition_name(),
        crate::VERSION
    );
    for (index, ops) in layout.pages.iter_mut().enumerate() {
        ops.push(Op::Text {
            x: MARGIN,
            y: MARGIN - 16.0,
            size: 7.5,
            font: Font::Regular,
            color: [150, 150, 150],
            text: escape(&label),
        });
        let page_label = format!("Page {} of {}", index + 1, total);
        ops.push(Op::Text {
            x: PAGE_W - MARGIN - text_width(&page_label, 7.5, Font::Regular),
            y: MARGIN - 16.0,
            size: 7.5,
            font: Font::Regular,
            color: [150, 150, 150],
            text: page_label,
        });
    }
}

/// Serialises the pages into a complete PDF file with a correct xref table.
///
/// Object numbers are fixed up front so page/resource references can be written
/// before the objects themselves exist:
///
/// | object | contents |
/// | --- | --- |
/// | 1 | catalogue |
/// | 2 | page tree |
/// | 3, 4 | Helvetica, Helvetica-Bold |
/// | 5 + 2i, 6 + 2i | content stream and page dictionary of page *i* |
/// | … | one image XObject per screenshot |
/// | last | document info |
fn serialize(pages: &[Vec<Op>], images: &[PendingImage], title: &str) -> Vec<u8> {
    let page_count = pages.len();
    let content_num = |index: usize| 5 + 2 * index;
    let page_num = |index: usize| 6 + 2 * index;
    let image_base = 5 + 2 * page_count;
    let info_num = image_base + images.len();
    let mut objects: Vec<Vec<u8>> = vec![Vec::new(); info_num];

    objects[0] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
    let kids: Vec<String> = (0..page_count)
        .map(|index| format!("{} 0 R", page_num(index)))
        .collect();
    objects[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        page_count
    )
    .into_bytes();
    objects[2] = font_object("Helvetica");
    objects[3] = font_object("Helvetica-Bold");

    for (index, ops) in pages.iter().enumerate() {
        objects[content_num(index) - 1] = stream_object(&render_ops(ops));
        let xobjects = match images.get(index) {
            Some(image) => format!("/XObject << /{} {} 0 R >>", image.name, image_base + index),
            None => String::new(),
        };
        objects[page_num(index) - 1] = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W:.0} {PAGE_H:.0}] \
             /Resources << /Font << /F1 3 0 R /F2 4 0 R >> {xobjects} >> \
             /Contents {} 0 R >>",
            content_num(index)
        )
        .into_bytes();
    }

    for (index, image) in images.iter().enumerate() {
        objects[image_base + index - 1] = image_object(image);
    }
    objects[info_num - 1] = format!(
        "<< /Producer (TPT Bug Report Builder {} v{}) /Title ({}) >>",
        crate::model::edition_name(),
        crate::VERSION,
        escape(title)
    )
    .into_bytes();

    let mut out: Vec<u8> =
        Vec::with_capacity(4096 + images.iter().map(|i| i.compressed.len()).sum::<usize>());
    // The `%` comment marks the file as binary for transfer tools.
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets = vec![0usize; info_num + 1];
    for (index, body) in objects.iter().enumerate() {
        offsets[index + 1] = out.len();
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", info_num + 1).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for number in 1..=info_num {
        out.extend_from_slice(format!("{:010} 00000 n \n", offsets[number]).as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R /Info {info_num} 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            info_num + 1
        )
        .as_bytes(),
    );
    out
}

/// Renders ops into a PDF content stream.
fn render_ops(ops: &[Op]) -> Vec<u8> {
    let mut out = String::with_capacity(ops.len() * 48);
    for op in ops {
        match op {
            Op::Text {
                x,
                y,
                size,
                font,
                color,
                text,
            } => {
                let name = if *font == Font::Bold { "F2" } else { "F1" };
                out.push_str(&format!(
                    "BT /{name} {size:.2} Tf {} rg {x:.2} {y:.2} Td ({text}) Tj ET\n",
                    color_operands(*color)
                ));
            }
            Op::Line {
                x0,
                y0,
                x1,
                y1,
                width,
                color,
            } => {
                out.push_str(&format!(
                    "{} RG {width:.2} w {x0:.2} {y0:.2} m {x1:.2} {y1:.2} l S\n",
                    color_operands(*color)
                ));
            }
            Op::Triangle { points, color } => {
                let [a, b, c] = points;
                out.push_str(&format!(
                    "{} rg {:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l h f\n",
                    color_operands(*color),
                    a.0,
                    a.1,
                    b.0,
                    b.1,
                    c.0,
                    c.1
                ));
            }
            Op::RectStroke {
                x,
                y,
                w,
                h,
                width,
                color,
            } => {
                out.push_str(&format!(
                    "{} RG {width:.2} w {x:.2} {y:.2} {w:.2} {h:.2} re S\n",
                    color_operands(*color)
                ));
            }
            Op::Image { name, x, y, w, h } => {
                out.push_str(&format!(
                    "q {w:.2} 0 0 {h:.2} {x:.2} {y:.2} cm /{name} Do Q\n"
                ));
            }
        }
    }
    out.into_bytes()
}

fn stream_object(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    out.extend_from_slice(format!("<< /Length {} >>\nstream\n", data.len()).as_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

fn image_object(image: &PendingImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.compressed.len() + 160);
    out.extend_from_slice(
        format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB \
             /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
            image.width,
            image.height,
            image.compressed.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&image.compressed);
    out.extend_from_slice(b"\nendstream");
    out
}

fn font_object(base_font: &str) -> Vec<u8> {
    format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base_font} /Encoding /WinAnsiEncoding >>")
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::RgbaImage;
    use crate::model::{Environment, Rect, Report, Severity};
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    fn inflate(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        ZlibDecoder::new(data)
            .read_to_end(&mut out)
            .expect("flate2 must inflate the image XObject");
        out
    }

    /// A gradient screenshot with a distinctive redaction region.
    fn screenshot(width: u32, height: u32) -> RgbaImage {
        let mut image = RgbaImage::new(width, height).expect("image");
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                image.set(x, y, [(x * 3) as u8, (y * 5) as u8, 90, 255]);
            }
        }
        image
    }

    fn report(captures: usize) -> Report {
        let mut report = Report::new("Save button stays disabled");
        report.meta.summary = "The save button never enables once the form is valid.".to_string();
        report.meta.severity = Severity::Critical;
        report.meta.reporter = "Ops Team".to_string();
        report.meta.environment = Environment {
            app_name: "Acme Invoicer".to_string(),
            app_version: "4.0.1".to_string(),
            os: "Windows 11".to_string(),
            browser: "WebView2 121".to_string(),
            device: "Surface Pro 9".to_string(),
        };
        report.meta.steps = vec![
            "Open a draft invoice".to_string(),
            "Fill in the required fields".to_string(),
        ];
        for index in 0..captures {
            let mut capture = crate::model::Capture::new((index + 1) as u32, screenshot(120, 90))
                .with_title(format!("Step {} screenshot", index + 1))
                .with_source("screen: region")
                .with_taken_at_ms(1_700_000_000_000.0 + index as f64 * 1000.0);
            capture.annotations.push(Annotation::Box {
                rect: Rect::new(10, 12, 60, 30),
                color: [211, 47, 47],
                width: 3.0,
            });
            capture.annotations.push(Annotation::Arrow {
                from: [80.0, 20.0],
                to: [60.0, 28.0],
                color: [25, 118, 210],
                width: 2.0,
            });
            capture.annotations.push(Annotation::Text {
                at: [8.0, 60.0],
                text: "disabled \u{2014} step".to_string(),
                size: 12.0,
                color: [24, 24, 24],
            });
            capture.annotations.push(Annotation::Blur {
                rect: Rect::new(30, 40, 40, 24),
                strength: 8,
            });
            report.push(capture);
        }
        report
    }

    /// Byte-slice substring search (the PDF contains binary stream data, so the
    /// tests never go through `String` for anything they index).
    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || haystack.len() < needle.len() {
            return None;
        }
        haystack.windows(needle.len()).position(|window| window == needle)
    }

    fn number_after(dict: &[u8], key: &[u8]) -> u32 {
        let at = find(dict, key).expect("dictionary key") + key.len();
        let digits: Vec<u8> = dict[at..]
            .iter()
            .copied()
            .take_while(|byte| byte.is_ascii_digit())
            .collect();
        std::str::from_utf8(&digits)
            .expect("ascii digits")
            .parse()
            .expect("number")
    }

    /// Pulls the first image XObject's `(width, height, decoded RGB)` out of the
    /// file by reading the dictionary the writer emitted.
    fn first_image(pdf: &[u8]) -> (u32, u32, Vec<u8>) {
        let image_at = find(pdf, b"/Subtype /Image").expect("image dictionary");
        let dict_end = find(&pdf[image_at..], b">>").expect("dictionary end") + image_at + 2;
        let dict = &pdf[image_at..dict_end];
        let width = number_after(dict, b"/Width ");
        let height = number_after(dict, b"/Height ");
        let length = number_after(dict, b"/Length ") as usize;
        let stream_at =
            find(&pdf[dict_end..], b"stream\n").expect("stream") + dict_end + b"stream\n".len();
        let decoded = inflate(&pdf[stream_at..stream_at + length]);
        (width, height, decoded)
    }

    /// Every in-use entry in the xref table, in object order.
    fn xref_offsets(pdf: &[u8]) -> Vec<usize> {
        let xref_at = find(pdf, b"xref").expect("xref");
        let mut offsets = Vec::new();
        for line in pdf[xref_at..].split(|byte| *byte == b'\n').skip(2) {
            if line.starts_with(b"trailer") {
                break;
            }
            if line.len() == 19 && line.ends_with(b" n ") {
                let offset = std::str::from_utf8(&line[..10])
                    .expect("ascii offset")
                    .trim()
                    .parse()
                    .expect("offset");
                offsets.push(offset);
            }
        }
        offsets
    }

    #[test]
    fn writes_a_multi_page_pdf_with_valid_offsets() {
        let pdf = assemble(&report(3)).expect("assembles");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Count 4"), "title page + 3 screenshots");
        assert!(text.contains("Save button stays disabled"));
        assert!(text.contains("Page 1 of 4"));
        assert!(text.contains("/Filter /FlateDecode"));
        assert!(text.contains(" cm /Im0 Do Q"));
        assert!(text.contains(" cm /Im2 Do Q"));
        // Vector mark-up drawn over the images.
        assert!(text.contains(" re S"), "box stroke");
        assert!(text.contains(" l S"), "arrow shaft");
        assert!(text.contains(" l h f"), "arrow head");
        // The em dash in the annotation text is transliterated for WinAnsi.
        assert!(text.contains("disabled - step"));
        assert!(!text.contains('\u{2014}'), "no raw em dash in the file");
        // Every xref entry must point at its own "N 0 obj" header. (4 fixed
        // objects + 8 page objects for 4 pages + 3 image XObjects + info = 16.)
        let offsets = xref_offsets(&pdf);
        assert_eq!(offsets.len(), 16);
        for (index, offset) in offsets.iter().enumerate() {
            let header = format!("{} 0 obj", index + 1);
            assert!(
                pdf[*offset..].starts_with(header.as_bytes()),
                "xref entry {} points at the wrong byte offset",
                index + 1
            );
        }
    }

    #[test]
    fn embeds_an_image_with_redactions_baked_in() {
        let pdf = assemble(&report(1)).expect("assembles");
        let (width, height, rgb) = first_image(&pdf);
        assert_eq!((width, height), (120, 90));
        assert_eq!(rgb.len(), 120 * 90 * 3);
        let pixel = |x: usize, y: usize| -> [u8; 3] {
            let at = (y * 120 + x) * 3;
            [rgb[at], rgb[at + 1], rgb[at + 2]]
        };
        // The redaction rect (30, 40) 40 x 24 with an 8 px block is uniform
        // inside a block, varies between blocks, and the untouched gradient
        // corner is byte-for-byte the source pixel.
        assert_eq!(pixel(32, 42), pixel(36, 46), "mosaic block is uniform");
        assert_ne!(pixel(32, 42), pixel(40, 50), "neighbouring blocks differ");
        assert_eq!(pixel(2, 2), [6, 10, 90], "unmarked gradient pixel");
    }

    #[test]
    fn escapes_pdf_special_characters() {
        assert_eq!(escape("100% (done) \\ ok"), "100% \\(done\\) \\\\ ok");
        assert_eq!(escape("a\u{2014}b\u{2192}c"), "a-b->c");
        assert_eq!(escape("caf\u{e9}"), "caf\u{e9}");
        assert_eq!(escape("\u{4e2d}"), "?");
    }

    #[test]
    fn wraps_long_text_within_the_page() {
        let lines = wrap(&"word ".repeat(120), TEXT_W, BODY_SIZE, Font::Regular);
        assert!(lines.len() > 4, "long text wraps: {}", lines.len());
        for candidate in &lines {
            assert!(
                text_width(candidate, BODY_SIZE, Font::Regular) <= TEXT_W + 1.0,
                "line too wide: {candidate:?}"
            );
        }
    }

    #[test]
    fn refuses_an_invalid_report() {
        let mut broken = report(1);
        broken.meta.title = String::new();
        let issues = assemble(&broken).expect_err("must fail");
        assert!(issues.contains(&ReportError::EmptyTitle));
    }

    #[test]
    fn a_tall_screenshot_still_fits_one_page() {
        let mut tall = Report::new("Vertical page");
        tall.push(
            crate::model::Capture::new(1, screenshot(200, 1600))
                .with_title("Whole page")
                .with_source("scroll-stitch (4 frames)"),
        );
        let pdf = assemble(&tall).expect("assembles");
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.contains("/Count 2"));
        // The provenance line is a PDF literal, so its parentheses are escaped.
        assert!(text.contains("scroll-stitch \\(4 frames\\)"));
        let (width, height, rgb) = first_image(&pdf);
        assert_eq!((width, height), (200, 1600));
        assert_eq!(rgb.len(), 200 * 1600 * 3);
    }
}