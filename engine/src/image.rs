//! A dependency-free 32-bit RGBA pixel buffer plus the raster primitives the
//! annotation renderer and the PDF redaction pass need (lines, arrows, boxes,
//! bitmap text, mosaic and crop).
//!
//! All coordinates are `i32` image pixels with the origin at the top-left, and
//! every drawing call is bounds-checked — drawing partly (or fully) off the
//! image is safe, which is what lets annotations be dragged past an edge.

use std::fmt;

/// Straight (non-premultiplied) 8-bit RGBA.
pub type Rgba = [u8; 4];

/// Opaque white — the default canvas background.
pub const WHITE: Rgba = [255, 255, 255, 255];
/// Fully transparent.
pub const TRANSPARENT: Rgba = [0, 0, 0, 0];

/// Errors from constructing or combining buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// Zero width or height.
    BadDimensions { width: u32, height: u32 },
    /// The supplied byte length does not match `width * height * 4`.
    BufferLength { expected: usize, got: usize },
    /// Two images that must line up have different widths.
    WidthMismatch { expected: u32, got: u32 },
    /// No images were supplied where at least one was required.
    NoInputs,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::BadDimensions { width, height } => {
                write!(f, "image dimensions {width} x {height} are not usable")
            }
            ImageError::BufferLength { expected, got } => write!(
                f,
                "pixel buffer is {got} bytes, expected {expected} (width x height x 4)"
            ),
            ImageError::WidthMismatch { expected, got } => write!(
                f,
                "images must share a width: expected {expected} px, got {got} px"
            ),
            ImageError::NoInputs => write!(f, "no images supplied"),
        }
    }
}

impl std::error::Error for ImageError {}

/// A row-major RGBA8 buffer. `data.len()` is always `width * height * 4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// A fully transparent buffer.
    pub fn new(width: u32, height: u32) -> Result<Self, ImageError> {
        Self::filled(width, height, TRANSPARENT)
    }

    /// A buffer filled with one colour.
    pub fn filled(width: u32, height: u32, color: Rgba) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::BadDimensions { width, height });
        }
        let pixels = width as usize * height as usize;
        let mut data = Vec::with_capacity(pixels * 4);
        for _ in 0..pixels {
            data.extend_from_slice(&color);
        }
        Ok(RgbaImage {
            width,
            height,
            data,
        })
    }

    /// Wraps an existing RGBA8 byte buffer (e.g. straight from a canvas
    /// `getImageData`), validating its length.
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::BadDimensions { width, height });
        }
        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            return Err(ImageError::BufferLength {
                expected,
                got: data.len(),
            });
        }
        Ok(RgbaImage {
            width,
            height,
            data,
        })
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some((y as usize * self.width as usize + x as usize) * 4)
    }

    /// Reads a pixel; out-of-bounds reads return transparent black.
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        match self.index(x, y) {
            Some(at) => [
                self.data[at],
                self.data[at + 1],
                self.data[at + 2],
                self.data[at + 3],
            ],
            None => TRANSPARENT,
        }
    }

    /// Replaces a pixel (no blending).
    pub fn set(&mut self, x: i32, y: i32, color: Rgba) {
        if let Some(at) = self.index(x, y) {
            self.data[at..at + 4].copy_from_slice(&color);
        }
    }

    /// Source-over blends `color` at `coverage` (0-255) over the existing
    /// pixel. Opaque coverage with an opaque source is an exact replacement,
    /// so rasterised output is deterministic.
    pub fn blend(&mut self, x: i32, y: i32, color: Rgba, coverage: u8) {
        let Some(at) = self.index(x, y) else {
            return;
        };
        let alpha = (color[3] as u32 * coverage as u32 + 127) / 255;
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.data[at..at + 4].copy_from_slice(&color);
            return;
        }
        let keep = 255 - alpha;
        for channel in 0..3 {
            let dst = self.data[at + channel] as u32;
            let src = color[channel] as u32;
            self.data[at + channel] = ((src * alpha + dst * keep + 127) / 255) as u8;
        }
        let dst_alpha = self.data[at + 3] as u32;
        self.data[at + 3] = (alpha + (dst_alpha * keep + 127) / 255).min(255) as u8;
    }

    /// Fills a rectangle. The rect is clipped to the image, so negative or
    /// overhanging coordinates are fine.
    pub fn fill_rect(&mut self, rect: crate::model::Rect, color: Rgba, coverage: u8) {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.width as i32);
        let y1 = (rect.y + rect.h).min(self.height as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, color, coverage);
            }
        }
    }

    /// Strokes a rectangle outline of `thickness` pixels, drawn *inside* the
    /// rect's bounds.
    pub fn stroke_rect(&mut self, rect: crate::model::Rect, color: Rgba, thickness: i32) {
        let t = thickness.max(1).min(((rect.w.min(rect.h)) + 1).max(1) / 2);
        for i in 0..t {
            // Top and bottom edges.
            self.fill_rect(crate::model::Rect::new(rect.x, rect.y + i, rect.w, 1), color, 255);
            self.fill_rect(
                crate::model::Rect::new(rect.x, rect.y + rect.h - 1 - i, rect.w, 1),
                color,
                255,
            );
            // Left and right edges.
            self.fill_rect(crate::model::Rect::new(rect.x + i, rect.y, 1, rect.h), color, 255);
            self.fill_rect(
                crate::model::Rect::new(rect.x + rect.w - 1 - i, rect.y, 1, rect.h),
                color,
                255,
            );
        }
    }

    /// Stamps an anti-aliased filled disc — the building block for thick
    /// lines.
    pub fn fill_disc(&mut self, cx: f64, cy: f64, radius: f64, color: Rgba) {
        if radius <= 0.0 || !radius.is_finite() {
            return;
        }
        let x0 = (cx - radius - 1.0).floor() as i32;
        let y0 = (cy - radius - 1.0).floor() as i32;
        let x1 = (cx + radius + 1.0).ceil() as i32;
        let y1 = (cy + radius + 1.0).ceil() as i32;
        let max_x = self.width as i32;
        let max_y = self.height as i32;
        for y in y0.max(0)..y1.min(max_y) {
            for x in x0.max(0)..x1.min(max_x) {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let coverage = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend(x, y, color, (coverage * 255.0).round() as u8);
                }
            }
        }
    }

    /// Draws a line of `width` pixels, anti-aliased, with round caps.
    pub fn line(&mut self, from: (f64, f64), to: (f64, f64), color: Rgba, width: f64) {
        let radius = width.max(1.0) / 2.0;
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        // Half-pixel steps keep the stroke solid at any angle.
        let steps = (length * 2.0).ceil().max(1.0) as i32;
        for step in 0..=steps {
            let t = step as f64 / steps as f64;
            self.fill_disc(from.0 + dx * t, from.1 + dy * t, radius, color);
        }
    }

    /// Fills a triangle given its three corners (used for arrow heads).
    pub fn fill_triangle(
        &mut self,
        a: (f64, f64),
        b: (f64, f64),
        c: (f64, f64),
        color: Rgba,
    ) {
        let min_y = a.1.min(b.1).min(c.1).floor() as i32;
        let max_y = a.1.max(b.1).max(c.1).ceil() as i32;
        let edges = [(a, b), (b, c), (c, a)];
        for y in min_y.max(0)..=max_y.min(self.height as i32 - 1) {
            let scan = y as f64 + 0.5;
            let mut crossings: Vec<f64> = Vec::with_capacity(3);
            for (p, q) in edges {
                if (p.1 <= scan && q.1 > scan) || (q.1 <= scan && p.1 > scan) {
                    let t = (scan - p.1) / (q.1 - p.1);
                    crossings.push(p.0 + t * (q.0 - p.0));
                }
            }
            if crossings.len() < 2 {
                continue;
            }
            let left = crossings.iter().cloned().fold(f64::INFINITY, f64::min);
            let right = crossings
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max);
            for x in left.floor() as i32..=right.ceil() as i32 {
                let lo = (x as f64).max(left);
                let hi = ((x + 1) as f64).min(right);
                let coverage = (hi - lo).clamp(0.0, 1.0);
                if coverage > 0.0 {
                    self.blend(x, y, color, (coverage * 255.0).round() as u8);
                }
            }
        }
    }

    /// Draws a shaft plus a solid triangular head at `to`.
    pub fn arrow(&mut self, from: (f64, f64), to: (f64, f64), color: Rgba, width: f64) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt();
        if length < 1e-6 {
            return;
        }
        let (ux, uy) = (dx / length, dy / length);
        let head = (width * 3.5).max(9.0).min(length);
        let half = head * 0.42;
        let base = (to.0 - ux * head, to.1 - uy * head);
        // Stop the shaft at the head's base so the joins never look lumpy.
        self.line(from, base, color, width);
        let (nx, ny) = (-uy, ux);
        let left = (base.0 + nx * half, base.1 + ny * half);
        let right = (base.0 - nx * half, base.1 - ny * half);
        self.fill_triangle(to, left, right, color);
    }

    /// Draws one line of text with the built-in 5x7 bitmap font, scaled so the
    /// glyph height is ~`size` pixels. `(x, y)` is the top-left corner.
    pub fn text(&mut self, x: i32, y: i32, text: &str, size: f64, color: Rgba) {
        let scale = ((size / crate::font::GLYPH_H as f64).round() as i32).max(1);
        let mut cursor = x;
        for ch in text.chars() {
            let bits = crate::font::glyph(ch);
            for (row, byte) in bits.iter().enumerate() {
                for col in 0..crate::font::GLYPH_W {
                    let on = (byte >> (crate::font::GLYPH_W - 1 - col)) & 1 == 1;
                    if on {
                        self.fill_rect(
                            crate::model::Rect::new(
                                cursor + col as i32 * scale,
                                y + row as i32 * scale,
                                scale,
                                scale,
                            ),
                            color,
                            255,
                        );
                    }
                }
            }
            cursor += (crate::font::GLYPH_W as i32 + 1) * scale;
        }
    }

    /// Advances a text cursor by one rendered character.
    pub fn text_advance(size: f64) -> i32 {
        let scale = ((size / crate::font::GLYPH_H as f64).round() as i32).max(1);
        (crate::font::GLYPH_W as i32 + 1) * scale
    }

    /// The bounding box a [`RgbaImage::text`] call would cover, useful for
    /// drawing a backing plate so the label stays readable on any screenshot.
    pub fn text_bounds(x: i32, y: i32, text: &str, size: f64) -> crate::model::Rect {
        let scale = ((size / crate::font::GLYPH_H as f64).round() as i32).max(1);
        let chars = text.chars().count() as i32;
        crate::model::Rect::new(
            x,
            y,
            crate::font::GLYPH_W as i32 * scale + (chars - 1).max(0) * (crate::font::GLYPH_W as i32 + 1) * scale,
            crate::font::GLYPH_H as i32 * scale,
        )
    }

    /// Pixelates a rectangle into `block`-sized cells — the redaction primitive.
    /// Each block takes the average colour of the pixels it covers.
    pub fn mosaic(&mut self, rect: crate::model::Rect, block: i32) {
        if rect.is_empty() {
            return;
        }
        let block = block.max(2);
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.width as i32);
        let y1 = (rect.y + rect.h).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        // Snapshot the source so blocks read unmodified pixels.
        let source = self.clone();
        let mut by = y0;
        while by < y1 {
            let mut bx = x0;
            let by_end = (by + block).min(y1);
            while bx < x1 {
                let bx_end = (bx + block).min(x1);
                let mut sum = [0u32; 4];
                let mut count = 0u32;
                for y in by..by_end {
                    for x in bx..bx_end {
                        let px = source.get(x, y);
                        for channel in 0..4 {
                            sum[channel] += px[channel] as u32;
                        }
                        count += 1;
                    }
                }
                if count > 0 {
                    let avg = [
                        (sum[0] / count) as u8,
                        (sum[1] / count) as u8,
                        (sum[2] / count) as u8,
                        (sum[3] / count) as u8,
                    ];
                    let cell =
                        crate::model::Rect::new(bx, by, bx_end - bx, by_end - by);
                    self.fill_rect(cell, avg, 255);
                }
                bx = bx_end;
            }
            by = by_end;
        }
    }

    /// Copies a sub-rectangle out into a new buffer. The rect is clipped to the
    /// image; an empty intersection is an error.
    pub fn crop(&self, rect: crate::model::Rect) -> Result<RgbaImage, ImageError> {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w).min(self.width as i32);
        let y1 = (rect.y + rect.h).min(self.height as i32);
        if x1 <= x0 || y1 <= y0 {
            return Err(ImageError::BadDimensions {
                width: rect.w.max(0) as u32,
                height: rect.h.max(0) as u32,
            });
        }
        let (width, height) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut out = RgbaImage::new(width, height)?;
        for y in 0..height {
            let src_start = ((y0 as u32 + y) as usize * self.width as usize + x0 as usize) * 4;
            let dst_start = (y as usize * width as usize) * 4;
            let len = width as usize * 4;
            out.data[dst_start..dst_start + len]
                .copy_from_slice(&self.data[src_start..src_start + len]);
        }
        Ok(out)
    }

    /// Flattens onto white and drops the alpha channel — the layout a PDF
    /// `/DeviceRGB` image XObject needs.
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixel_count() * 3);
        for pixel in self.data.chunks_exact(4) {
            let alpha = pixel[3] as u32;
            for channel in 0..3 {
                let value = if alpha == 255 {
                    pixel[channel]
                } else {
                    ((pixel[channel] as u32 * alpha + 255 * (255 - alpha) + 127) / 255) as u8
                };
                out.push(value);
            }
        }
        out
    }

    // __IMAGE_PART5__
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;

    #[test]
    fn rejects_bad_buffers() {
        assert_eq!(
            RgbaImage::new(0, 4),
            Err(ImageError::BadDimensions {
                width: 0,
                height: 4
            })
        );
        assert_eq!(
            RgbaImage::from_rgba(2, 2, vec![0; 15]),
            Err(ImageError::BufferLength {
                expected: 16,
                got: 15
            })
        );
        assert_eq!(RgbaImage::from_rgba(2, 2, vec![0; 16]).map(|i| i.width), Ok(2));
    }

    #[test]
    fn clips_fills_to_the_image() {
        let mut image = RgbaImage::filled(4, 4, WHITE).expect("image");
        image.fill_rect(Rect::new(-2, -2, 4, 4), [0, 0, 0, 255], 255);
        // The 2x2 top-left corner is black; the rest stays white.
        assert_eq!(image.get(0, 0), [0, 0, 0, 255]);
        assert_eq!(image.get(1, 1), [0, 0, 0, 255]);
        assert_eq!(image.get(2, 2), WHITE);
        assert_eq!(image.get(3, 3), WHITE);
    }

    #[test]
    fn strokes_inside_the_rect() {
        let mut image = RgbaImage::filled(10, 10, WHITE).expect("image");
        image.stroke_rect(Rect::new(2, 2, 6, 6), [255, 0, 0, 255], 2);
        assert_eq!(image.get(2, 2), [255, 0, 0, 255]);
        assert_eq!(image.get(7, 7), [255, 0, 0, 255]);
        assert_eq!(image.get(4, 4), WHITE, "interior stays untouched");
    }

    #[test]
    fn draws_arrow_shaft_and_head_inside_the_image() {
        let mut image = RgbaImage::filled(100, 100, WHITE).expect("image");
        image.arrow((10.0, 10.0), (90.0, 90.0), [0, 0, 0, 255], 3.0);
        // Somewhere along the shaft:
        assert_ne!(image.get(50, 50), WHITE);
        // Near the tip (the head is wide there):
        assert_ne!(image.get(88, 88), WHITE);
        // Opposite corner untouched:
        assert_eq!(image.get(95, 5), WHITE);
    }

    #[test]
    fn mosaic_makes_each_block_uniform() {
        let mut image = RgbaImage::new(8, 8).expect("image");
        for y in 0..8 {
            for x in 0..8 {
                image.set(x, y, [x as u8 * 10, y as u8 * 10, 0, 255]);
            }
        }
        image.mosaic(Rect::new(0, 0, 8, 8), 4);
        for block_y in [0, 4] {
            for block_x in [0, 4] {
                let first = image.get(block_x, block_y);
                for y in block_y..block_y + 4 {
                    for x in block_x..block_x + 4 {
                        assert_eq!(image.get(x, y), first, "block ({block_x},{block_y}) uniform");
                    }
                }
            }
        }
    }

    #[test]
    fn crops_a_sub_rectangle() {
        let mut image = RgbaImage::filled(6, 6, WHITE).expect("image");
        image.fill_rect(Rect::new(2, 2, 2, 2), [1, 2, 3, 255], 255);
        let crop = image.crop(Rect::new(2, 2, 2, 2)).expect("crop");
        assert_eq!((crop.width, crop.height), (2, 2));
        assert_eq!(crop.get(0, 0), [1, 2, 3, 255]);
        // Fully outside is an error, partly outside clips.
        assert!(image.crop(Rect::new(50, 50, 2, 2)).is_err());
        assert_eq!(image.crop(Rect::new(5, 5, 4, 4)).map(|c| c.width), Ok(1));
    }

    #[test]
    fn flattens_transparency_onto_white() {
        let image = RgbaImage::filled(1, 1, [0, 0, 0, 0]).expect("image");
        assert_eq!(image.to_rgb(), vec![255, 255, 255]);
        let half = RgbaImage::from_rgba(1, 1, vec![0, 0, 0, 128]).expect("image");
        assert_eq!(half.to_rgb(), vec![127, 127, 127]);
    }

    #[test]
    fn text_bounds_covers_the_advance() {
        let bounds = RgbaImage::text_bounds(4, 5, "AB", 14.0);
        // scale = round(14 / 7) = 2, advance 12 px per glyph.
        assert_eq!((bounds.x, bounds.y), (4, 5));
        assert_eq!(bounds.w, 5 * 2 + 12);
        assert_eq!(bounds.h, 14);
    }
}