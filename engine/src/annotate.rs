//! Rasterises a capture's annotations into its pixels.
//!
//! This is the offline twin of the on-screen canvas viewer: the browser and the
//! Pro desktop bundle bake mark-up with the platform's own 2D canvas (crisper
//! text, hardware-accelerated), while this module does the same job with the
//! engine's raster primitives so a report can be assembled headlessly and the
//! result can be asserted pixel-by-pixel in unit tests.

use crate::image::{Rgba, RgbaImage};
use crate::model::{Annotation, Capture, Rgb};

/// Opaque RGBA for an annotation colour.
fn rgba(color: Rgb) -> Rgba {
    [color[0], color[1], color[2], 255]
}

/// A dark outline colour that reads on light *and* dark screenshots.
fn outline(color: Rgb) -> Rgba {
    let luma = (color[0] as u32 * 299 + color[1] as u32 * 587 + color[2] as u32 * 114) / 1000;
    if luma > 140 {
        [16, 16, 16, 230]
    } else {
        [245, 245, 245, 230]
    }
}

/// `capture.image` with every annotation drawn in — what the report embeds.
pub fn render(capture: &Capture) -> RgbaImage {
    render_image(&capture.image, &capture.annotations)
}

/// `image` with `annotations` drawn in. The input is not modified.
pub fn render_image(image: &RgbaImage, annotations: &[Annotation]) -> RgbaImage {
    let mut out = image.clone();
    for annotation in annotations {
        draw(&mut out, annotation);
    }
    out
}

/// Draws one annotation onto `image` in place.
pub fn draw(image: &mut RgbaImage, annotation: &Annotation) {
    match annotation {
        Annotation::Arrow {
            from,
            to,
            color,
            width,
        } => {
            // A thin contrasting halo keeps the arrow visible on any background.
            image.arrow(
                (from[0], from[1]),
                (to[0], to[1]),
                outline(*color),
                width + 2.0,
            );
            image.arrow((from[0], from[1]), (to[0], to[1]), rgba(*color), *width);
        }
        Annotation::Box {
            rect,
            color,
            width,
        } => {
            let thickness = width.round().max(1.0) as i32;
            image.stroke_rect(rect.padded(-thickness / 2), outline(*color), thickness + 2);
            image.stroke_rect(*rect, rgba(*color), thickness);
        }
        Annotation::Text {
            at,
            text,
            size,
            color,
        } => {
            let x = at[0].round() as i32;
            let y = at[1].round() as i32;
            let halo = outline(*color);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                image.text(x + dx, y + dy, text, *size, halo);
            }
            image.text(x, y, text, *size, rgba(*color));
        }
        Annotation::Blur { rect, strength } => {
            image.mosaic(*rect, *strength as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::WHITE;
    use crate::model::Rect;

    const RED: Rgb = [211, 47, 47];
    const BLUE: Rgb = [25, 118, 210];

    fn canvas() -> RgbaImage {
        RgbaImage::filled(120, 80, WHITE).expect("image")
    }

    fn capture(annotations: Vec<Annotation>) -> Capture {
        let mut capture = Capture::new(1, canvas());
        capture.annotations = annotations;
        capture
    }

    #[test]
    fn draws_an_arrow_with_a_visible_halo() {
        let capture = capture(vec![Annotation::Arrow {
            from: [10.0, 10.0],
            to: [100.0, 60.0],
            color: RED,
            width: 3.0,
        }]);
        let rendered = capture.annotated();
        // A pixel on the shaft is inked, never left white.
        assert_ne!(rendered.get(55, 35), WHITE);
        // The untouched corner keeps the original pixels.
        assert_eq!(rendered.get(2, 76), WHITE);
        // The source capture is not mutated by rendering.
        assert_eq!(capture.image.get(55, 35), WHITE);
    }

    #[test]
    fn draws_box_edges() {
        let capture = capture(vec![Annotation::Box {
            rect: Rect::new(20, 20, 60, 30),
            color: BLUE,
            width: 2.0,
        }]);
        let rendered = capture.annotated();
        assert_eq!(rendered.get(20, 20), [25, 118, 210, 255]);
        assert_eq!(rendered.get(50, 20), [25, 118, 210, 255]);
        assert_eq!(rendered.get(50, 35), WHITE, "box interior stays clear");
    }

    #[test]
    fn draws_text_with_ink_around_its_origin() {
        let capture = capture(vec![Annotation::Text {
            at: [12.0, 12.0],
            text: "OOPS".to_string(),
            size: 14.0,
            color: [24, 24, 24],
        }]);
        let rendered = capture.annotated();
        let mut changed = 0;
        for y in 0..20 {
            for x in 0..40 {
                if rendered.get(x, y) != WHITE {
                    changed += 1;
                }
            }
        }
        assert!(
            changed > 20,
            "text should ink a few dozen pixels, got {changed}"
        );
    }

    #[test]
    fn redacts_by_mosaic() {
        let mut image = RgbaImage::new(40, 40).expect("image");
        for y in 0..40 {
            for x in 0..40 {
                image.set(x, y, [(x * 6) as u8, (y * 6) as u8, 40, 255]);
            }
        }
        let mut capture = Capture::new(1, image);
        capture.annotations.push(Annotation::Blur {
            rect: Rect::new(8, 8, 16, 16),
            strength: 8,
        });
        let rendered = capture.annotated();
        // Inside one block every pixel is identical...
        let first = rendered.get(8, 8);
        assert_eq!(rendered.get(12, 12), first, "block is uniform");
        // ...and different blocks carry different averages.
        assert_ne!(first, rendered.get(16, 16));
        // Outside the rect the source detail is intact.
        assert_eq!(rendered.get(0, 0), [0, 0, 40, 255]);
    }

    #[test]
    fn render_image_leaves_the_source_untouched() {
        let image = canvas();
        let rendered = render_image(
            &image,
            &[Annotation::Box {
                rect: Rect::new(0, 0, 10, 10),
                color: RED,
                width: 2.0,
            }],
        );
        assert_ne!(rendered, image);
        assert_eq!(image, canvas());
    }
}