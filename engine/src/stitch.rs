//! Scrolling capture: join overlapping frames of a long page into one image.
//!
//! Pro edition. A scrolling capture is a sequence of screen grabs taken while
//! the user scrolls; consecutive frames overlap, and the overlap is what lets
//! the joiner recover the full page without knowing anything about the
//! scrolling application. The overlap is found by *matching pixels* — the
//! bottom rows of one frame are compared against the top rows of the next over
//! a range of candidate offsets — and the candidate with the smallest mean
//! channel difference wins, provided it is inside
//! [`StitchOptions::tolerance`].
//!
//! This module is deterministic and host-testable: feeding it windows sliced out
//! of a known page reproduces that page exactly.

use std::fmt;

use crate::image::{ImageError, RgbaImage};

/// Tuning for the overlap search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StitchOptions {
    /// Smallest overlap to consider (a few rows is enough to lock on).
    pub min_overlap: u32,
    /// Largest overlap to consider.
    pub max_overlap: u32,
    /// Largest mean channel difference (0-255) that still counts as a match.
    pub tolerance: f64,
    /// Compare every `sample_step`-th row and column — a stride of 2-4 is
    /// plenty and keeps the search fast on retina-sized frames.
    pub sample_step: u32,
}

impl Default for StitchOptions {
    fn default() -> Self {
        StitchOptions {
            min_overlap: 8,
            max_overlap: 512,
            tolerance: 6.0,
            sample_step: 2,
        }
    }
}

/// Why a scrolling capture could not be joined.
#[derive(Debug, Clone, PartialEq)]
pub enum StitchError {
    /// Nothing to join.
    NoFrames,
    /// Frames must come from the same capture width.
    WidthMismatch {
        pair: usize,
        expected: u32,
        got: u32,
    },
    /// No candidate overlap matched within tolerance.
    NoOverlap {
        pair: usize,
        best: f64,
        tolerance: f64,
    },
    /// Buffer construction failed.
    Image(ImageError),
}

impl fmt::Display for StitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StitchError::NoFrames => write!(f, "there are no frames to join"),
            StitchError::WidthMismatch {
                pair,
                expected,
                got,
            } => write!(
                f,
                "frame {} is {got} px wide but the first frame is {expected} px",
                pair + 2
            ),
            StitchError::NoOverlap {
                pair,
                best,
                tolerance,
            } => write!(
                f,
                "frames {} and {} do not overlap (closest match differs by {best:.1}, \
                 over the {tolerance:.1} limit) — scroll a little less between grabs",
                pair + 1,
                pair + 2
            ),
            StitchError::Image(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StitchError {}

impl From<ImageError> for StitchError {
    fn from(error: ImageError) -> Self {
        StitchError::Image(error)
    }
}

/// Mean absolute RGB difference between the last `overlap` rows of `top` and
/// the first `overlap` rows of `bottom`, sampled on a stride.
fn overlap_difference(top: &RgbaImage, bottom: &RgbaImage, overlap: u32, step: u32) -> f64 {
    let step = step.max(1) as i32;
    let width = top.width.min(bottom.width) as i32;
    let top_start = top.height as i32 - overlap as i32;
    let mut total = 0.0f64;
    let mut samples = 0.0f64;
    let mut y = 0i32;
    while y < overlap as i32 {
        let mut x = 0i32;
        while x < width {
            let above = top.get(x, top_start + y);
            let below = bottom.get(x, y);
            for channel in 0..3 {
                total += (above[channel] as f64 - below[channel] as f64).abs();
                samples += 1.0;
            }
            x += step;
        }
        y += step;
    }
    if samples == 0.0 {
        f64::INFINITY
    } else {
        total / samples
    }
}

/// Finds how many rows the bottom of `top` and the top of `bottom` share.
///
/// Returns the candidate offset with the smallest mean difference, provided it
/// is within [`StitchOptions::tolerance`]. Ties (within half a channel step)
/// resolve to the *larger* overlap, which is the safer join.
pub fn find_overlap(top: &RgbaImage, bottom: &RgbaImage, options: &StitchOptions) -> Option<u32> {
    let limit = options
        .max_overlap
        .min(top.height)
        .min(bottom.height);
    if limit == 0 {
        return None;
    }
    let mut best: Option<(f64, u32)> = None;
    for overlap in options.min_overlap.max(1)..=limit {
        let difference = overlap_difference(top, bottom, overlap, options.sample_step);
        let better = match best {
            None => true,
            Some((best_difference, best_overlap)) => {
                if difference + 0.5 < best_difference {
                    true
                } else {
                    // Treat near-equal scores as a tie and prefer more overlap.
                    (difference - best_difference).abs() <= 0.5 && overlap > best_overlap
                }
            }
        };
        if better {
            best = Some((difference, overlap));
        }
    }
    match best {
        Some((difference, overlap)) if difference <= options.tolerance => Some(overlap),
        _ => None,
    }
}

/// Joins frames top-to-bottom, removing the overlapping rows between them.
///
/// One frame is a copy of that frame; the result is identical in size to the
/// page it was captured from when every pair overlaps within tolerance.
pub fn stitch_vertical(
    frames: &[RgbaImage],
    options: &StitchOptions,
) -> Result<RgbaImage, StitchError> {
    if frames.is_empty() {
        return Err(StitchError::NoFrames);
    }
    let width = frames[0].width;
    for (index, frame) in frames.iter().enumerate().skip(1) {
        if frame.width != width {
            return Err(StitchError::WidthMismatch {
                // `pair` is the index of the join this frame belongs to.
                pair: index - 1,
                expected: width,
                got: frame.width,
            });
        }
    }

    // Work out every join before allocating the output.
    let mut overlaps = Vec::with_capacity(frames.len().saturating_sub(1));
    for pair in 0..frames.len().saturating_sub(1) {
        match find_overlap(&frames[pair], &frames[pair + 1], options) {
            Some(overlap) => overlaps.push(overlap),
            None => {
                let best = (options.min_overlap.max(1)..=options
                    .max_overlap
                    .min(frames[pair].height)
                    .min(frames[pair + 1].height))
                    .map(|candidate| {
                        overlap_difference(
                            &frames[pair],
                            &frames[pair + 1],
                            candidate,
                            options.sample_step,
                        )
                    })
                    .fold(f64::INFINITY, f64::min);
                return Err(StitchError::NoOverlap {
                    pair,
                    best,
                    tolerance: options.tolerance,
                });
            }
        }
    }

    let total: u32 = frames
        .iter()
        .map(|frame| frame.height)
        .sum::<u32>()
        .saturating_sub(overlaps.iter().sum::<u32>())
        .max(1);
    let mut out = RgbaImage::new(width, total)?;

    let mut destination_y = 0u32;
    for (index, frame) in frames.iter().enumerate() {
        let skip = if index == 0 { 0 } else { overlaps[index - 1] };
        for row in skip..frame.height {
            if destination_y >= total {
                break;
            }
            let source_start = (row as usize * width as usize) * 4;
            let destination_start = (destination_y as usize * width as usize) * 4;
            let len = width as usize * 4;
            out.data[destination_start..destination_start + len]
                .copy_from_slice(&frame.data[source_start..source_start + len]);
            destination_y += 1;
        }
    }
    Ok(out)
}

/// `(width, height)` of the joined result without building it — used by the UI
/// to show the capture size before committing.
pub fn stitched_height(
    frames: &[RgbaImage],
    options: &StitchOptions,
) -> Result<u32, StitchError> {
    if frames.is_empty() {
        return Err(StitchError::NoFrames);
    }
    let mut total = frames[0].height;
    for pair in 0..frames.len().saturating_sub(1) {
        let overlap = find_overlap(&frames[pair], &frames[pair + 1], options).ok_or(
            StitchError::NoOverlap {
                pair,
                best: f64::NAN,
                tolerance: options.tolerance,
            },
        )?;
        total = total.saturating_add(frames[pair + 1].height.saturating_sub(overlap));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: u32 = 120;
    const HEIGHT: u32 = 700;

    /// A page whose every row is visually distinct, so an overlap search has
    /// exactly one correct answer.
    fn page(seed: u32) -> RgbaImage {
        let mut image = RgbaImage::new(WIDTH, HEIGHT).expect("page");
        for y in 0..HEIGHT as u32 {
            for x in 0..WIDTH as u32 {
                image.set(
                    x as i32,
                    y as i32,
                    [
                        ((y * 5 + x * 3 + seed) % 256) as u8,
                        ((y * y + x * 7 + seed * 3) % 251) as u8,
                        ((y * 11 + x + seed * 17) % 241) as u8,
                        255,
                    ],
                );
            }
        }
        image
    }

    /// Rows `start..end` of a page, as their own frame.
    fn window(source: &RgbaImage, start: u32, end: u32) -> RgbaImage {
        source
            .crop(crate::model::Rect::new(0, start as i32, WIDTH as i32, (end - start) as i32))
            .expect("window")
    }

    fn options() -> StitchOptions {
        StitchOptions {
            sample_step: 1,
            ..StitchOptions::default()
        }
    }

    #[test]
    fn joins_three_overlapping_frames_back_into_the_page() {
        let source = page(0);
        let frames = vec![
            window(&source, 0, 300),
            window(&source, 250, 550),
            window(&source, 500, 700),
        ];
        let joined = stitch_vertical(&frames, &options()).expect("stitches");
        assert_eq!((joined.width, joined.height), (WIDTH, HEIGHT));
        assert_eq!(joined, source, "the join must reproduce the captured page");
    }

    #[test]
    fn finds_the_known_overlap_for_each_pair() {
        let source = page(0);
        let first = window(&source, 0, 300);
        let second = window(&source, 250, 550);
        assert_eq!(find_overlap(&first, &second, &options()), Some(50));
        assert_eq!(stitched_height(&[first, second], &options()), Ok(550));
    }

    #[test]
    fn rejoin_remains_exact_for_many_small_steps() {
        // Sixteen frames scrolled 40 px at a time, 60 px of overlap each: the
        // last frame reaches the bottom of the 700 px page.
        let source = page(0);
        let frames: Vec<RgbaImage> = (0..16)
            .map(|index| {
                let start = index * 40;
                window(&source, start, (start + 100).min(HEIGHT))
            })
            .collect();
        let joined = stitch_vertical(&frames, &options()).expect("stitches");
        assert_eq!(joined.height, HEIGHT);
        assert_eq!(joined, source);
    }

    #[test]
    fn refuses_frames_that_do_not_overlap() {
        let first = window(&page(0), 0, 300);
        let second = window(&page(97), 250, 550);
        let error = stitch_vertical(&[first, second], &options()).expect_err("must fail");
        match &error {
            StitchError::NoOverlap { pair, best, .. } => {
                assert_eq!(*pair, 0);
                assert!(*best > options().tolerance, "reported difference {best}");
            }
            other => panic!("unexpected error: {other}"),
        }
        assert!(error.to_string().contains("do not overlap"));
    }

    #[test]
    fn refuses_frames_of_different_widths() {
        let first = window(&page(0), 0, 300);
        let mut second = window(&page(0), 250, 550);
        second.width += 1;
        second.data.extend_from_slice(&[0, 0, 0, 255]);
        let error = stitch_vertical(&[first, second], &options()).expect_err("must fail");
        assert!(matches!(
            error,
            StitchError::WidthMismatch {
                pair: 0,
                expected: 120,
                got: 121
            }
        ));
    }

    #[test]
    fn a_single_frame_is_its_own_result() {
        let source = page(0);
        let frame = window(&source, 100, 200);
        let joined = stitch_vertical(std::slice::from_ref(&frame), &options()).expect("stitches");
        assert_eq!(joined, frame);
        assert_eq!(stitched_height(std::slice::from_ref(&frame), &options()), Ok(100));
    }

    #[test]
    fn rejects_an_empty_capture() {
        assert_eq!(
            stitch_vertical(&[], &options()),
            Err(StitchError::NoFrames)
        );
        assert_eq!(stitched_height(&[], &options()), Err(StitchError::NoFrames));
    }
}