//! Report model: metadata, captures (screenshots), annotations, validation and
//! ordering.
//!
//! Everything here is plain data — no UI, no wasm — so the whole
//! capture → annotate → assemble pipeline is host-unit-testable and shared
//! verbatim between the free WASM edition and the Pro desktop build.

use std::fmt;

use crate::image::RgbaImage;

/// Re-exported so callers only need `tpt_bugreport_engine::Rgb`.
pub type Rgb = [u8; 3];

/// Free edition: a single marked-up screenshot.
pub const FREE_MAX_CAPTURES: usize = 1;
/// Pro edition: a full multi-screenshot sequence.
pub const PRO_MAX_CAPTURES: usize = 64;

/// How many screenshots the compiled edition accepts per report. The free WASM
/// build carries `FREE_MAX_CAPTURES`; `--features pro` carries the sequence
/// budget. Reported by [`Report::max_captures`] and enforced by
/// [`Report::validate`].
pub fn edition_max_captures() -> usize {
    if cfg!(feature = "pro") {
        PRO_MAX_CAPTURES
    } else {
        FREE_MAX_CAPTURES
    }
}

/// The edition this build of the engine represents (`"free"` or `"pro"`).
pub fn edition_name() -> &'static str {
    if cfg!(feature = "pro") {
        "pro"
    } else {
        "free"
    }
}

/// Bug severity, ordered least to most serious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// All severities, least to most serious — the order the UI lists them in.
    pub const ALL: [Severity; 4] = [
        Severity::Low,
        Severity::Medium,
        Severity::High,
        Severity::Critical,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Severity::Low => "Low",
            Severity::Medium => "Medium",
            Severity::High => "High",
            Severity::Critical => "Critical",
        }
    }

    /// Stable identifier used by DOM `<select>` values.
    pub fn slug(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// Parses a [`Severity::slug`], defaulting to [`Severity::Medium`].
    pub fn from_slug(slug: &str) -> Severity {
        match slug.trim().to_ascii_lowercase().as_str() {
            "low" => Severity::Low,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => Severity::Medium,
        }
    }
}

/// Where the bug was seen. Every field is free text so field engineers can put
/// whatever build/OS string their process uses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Environment {
    pub app_name: String,
    pub app_version: String,
    pub os: String,
    pub browser: String,
    pub device: String,
}

impl Environment {
    /// `(label, value)` pairs for the non-empty fields, in a stable order —
    /// used by both the Markdown and PDF writers so the two agree.
    pub fn rows(&self) -> Vec<(&'static str, &str)> {
        let mut rows = Vec::new();
        for (label, value) in [
            ("Application", &self.app_name),
            ("Version", &self.app_version),
            ("Operating system", &self.os),
            ("Browser", &self.browser),
            ("Device", &self.device),
        ] {
            if !value.trim().is_empty() {
                rows.push((label, value.trim()));
            }
        }
        rows
    }
}

/// Report header: what was reported, how bad it is, where it happened and how
/// to reproduce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportMeta {
    pub title: String,
    pub summary: String,
    pub severity: Severity,
    pub environment: Environment,
    /// Reproduction steps, in order. Blank steps are dropped by the writers.
    pub steps: Vec<String>,
    pub reporter: String,
}

impl ReportMeta {
    pub fn new(title: impl Into<String>) -> Self {
        ReportMeta {
            title: title.into(),
            summary: String::new(),
            severity: Severity::Medium,
            environment: Environment::default(),
            steps: Vec::new(),
            reporter: String::new(),
        }
    }

    /// Reproduction steps with blank lines removed, keeping input order.
    pub fn clean_steps(&self) -> Vec<&str> {
        self.steps
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// An integer pixel rectangle. Drawing coordinates are clamped by the raster
/// helpers, so a partly off-image rect is legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }

    /// Normalises a drag (which may run right-to-left / bottom-to-top) into a
    /// rect with a non-negative width and height.
    pub fn from_drag(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Rect {
            x: x0.min(x1),
            y: y0.min(y1),
            w: (x1 - x0).abs(),
            h: (y1 - y0).abs(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// True when the rect overlaps `0..width x 0..height` at all.
    pub fn intersects(&self, width: u32, height: u32) -> bool {
        self.x < width as i32 && self.y < height as i32 && self.x + self.w > 0 && self.y + self.h > 0
    }

    /// Inflates the rect by `pad` pixels on every side.
    pub fn padded(&self, pad: i32) -> Rect {
        Rect {
            x: self.x - pad,
            y: self.y - pad,
            w: self.w + 2 * pad,
            h: self.h + 2 * pad,
        }
    }
}

/// One mark-up on a screenshot. Coordinates are in *image pixels* (origin
/// top-left), so annotations survive any display zoom the host applies.
#[derive(Debug, Clone, PartialEq)]
pub enum Annotation {
    /// A line from `from` to `to` with a solid triangular head at `to`.
    Arrow {
        from: [f64; 2],
        to: [f64; 2],
        color: Rgb,
        width: f64,
    },
    /// A rectangle outline.
    Box {
        rect: Rect,
        color: Rgb,
        width: f64,
    },
    /// A single line of text; `at` is the top-left corner, `size` the cap
    /// height in pixels.
    Text {
        at: [f64; 2],
        text: String,
        size: f64,
        color: Rgb,
    },
    /// Redaction: the rect is pixelated to hide sensitive content. `strength`
    /// is the mosaic block size in pixels.
    Blur { rect: Rect, strength: u8 },
}

impl Annotation {
    /// Stable identifier used by the UI tool buttons and by the report legend.
    pub fn kind(&self) -> &'static str {
        match self {
            Annotation::Arrow { .. } => "arrow",
            Annotation::Box { .. } => "box",
            Annotation::Text { .. } => "text",
            Annotation::Blur { .. } => "blur",
        }
    }

    /// Human-readable one-liner used in the report's annotation index.
    pub fn describe(&self) -> String {
        match self {
            Annotation::Arrow {
                from,
                to,
                color,
                width,
            } => format!(
                "Arrow ({:.0}, {:.0}) -> ({:.0}, {:.0}), {} px, {}",
                from[0],
                from[1],
                to[0],
                to[1],
                fmt_width(*width),
                rgb_name(*color)
            ),
            Annotation::Box { rect, color, width } => format!(
                "Box at ({}, {}), {} x {}, {} px, {}",
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                fmt_width(*width),
                rgb_name(*color)
            ),
            Annotation::Text {
                at, text, size, ..
            } => format!(
                "Text \"{}\" at ({:.0}, {:.0}), {:.0} px",
                text.trim(),
                at[0],
                at[1],
                size
            ),
            Annotation::Blur { rect, strength } => format!(
                "Redaction (mosaic) at ({}, {}), {} x {}, block {} px",
                rect.x, rect.y, rect.w, rect.h, strength
            ),
        }
    }
}

/// One screenshot plus its mark-up.
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    /// Stable id, unique within a report. Also the tie-breaker when two
    /// captures share a timestamp.
    pub id: u32,
    /// Caption shown under the image and in the report legend.
    pub title: String,
    /// Free-text note included with the image in the report.
    pub note: String,
    /// Where the pixels came from (`"upload: crash.png"`, `"screen: region"`,
    /// `"paste"`, `"scroll-stitch (4 frames)"`, …). Provenance matters in a bug
    /// report, so it is carried into the output verbatim.
    pub source: String,
    /// Capture time, milliseconds since the Unix epoch (as
    /// `js_sys::Date::now()` returns). Drives report ordering.
    pub taken_at_ms: f64,
    /// The (annotated) pixels. Hosts bake the vector mark-up into this buffer
    /// before assembly; [`crate::annotate::render`] does the same job offline.
    pub image: RgbaImage,
    pub annotations: Vec<Annotation>,
}

impl Capture {
    pub fn new(id: u32, image: RgbaImage) -> Self {
        Capture {
            id,
            title: String::new(),
            note: String::new(),
            source: String::new(),
            taken_at_ms: 0.0,
            image,
            annotations: Vec::new(),
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = source.into();
        self
    }

    pub fn with_taken_at_ms(mut self, taken_at_ms: f64) -> Self {
        self.taken_at_ms = taken_at_ms;
        self
    }

    /// The pixels with every annotation rasterised in — what the report
    /// embeds.
    pub fn annotated(&self) -> RgbaImage {
        crate::annotate::render(self)
    }
}

/// A complete bug report: metadata plus an ordered set of screenshots.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub meta: ReportMeta,
    pub captures: Vec<Capture>,
}

impl Report {
    pub fn new(title: impl Into<String>) -> Self {
        Report {
            meta: ReportMeta::new(title),
            captures: Vec::new(),
        }
    }

    pub fn with_capture(mut self, capture: Capture) -> Self {
        self.captures.push(capture);
        self
    }

    pub fn push(&mut self, capture: Capture) {
        self.captures.push(capture);
    }

    /// The next unused capture id (max + 1).
    pub fn next_capture_id(&self) -> u32 {
        self.captures.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    pub fn annotation_count(&self) -> usize {
        self.captures.iter().map(|c| c.annotations.len()).sum()
    }

    /// Screenshots in the order they were taken: by timestamp, then by id for
    /// ties. This is the order every writer emits, so a sequence gathered over
    /// several capture sessions still reads chronologically.
    pub fn ordered(&self) -> Vec<&Capture> {
        let mut ordered: Vec<&Capture> = self.captures.iter().collect();
        ordered.sort_by(|a, b| {
            a.taken_at_ms
                .partial_cmp(&b.taken_at_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        ordered
    }

    /// How many screenshots this edition accepts (see
    /// [`edition_max_captures`]).
    pub fn max_captures(&self) -> usize {
        edition_max_captures()
    }

    /// True when the report has content the shipped edition can assemble.
    pub fn is_exportable(&self) -> bool {
        self.validate().is_ok()
    }

    /// Checks the whole report and returns *every* problem, so the UI can list
    /// them at once instead of one-at-a-time.
    pub fn validate(&self) -> Result<(), Vec<ReportError>> {
        let mut issues: Vec<ReportError> = Vec::new();

        if self.meta.title.trim().is_empty() {
            issues.push(ReportError::EmptyTitle);
        }
        if self.captures.is_empty() {
            issues.push(ReportError::NoCaptures);
        }
        let max = self.max_captures();
        if self.captures.len() > max {
            issues.push(ReportError::TooManyCaptures {
                count: self.captures.len(),
                max,
            });
        }

        let mut seen: Vec<u32> = Vec::with_capacity(self.captures.len());
        for capture in &self.captures {
            if seen.contains(&capture.id) {
                issues.push(ReportError::DuplicateCaptureId { id: capture.id });
            }
            seen.push(capture.id);

            if capture.image.width == 0
                || capture.image.height == 0
                || capture.image.data.is_empty()
            {
                issues.push(ReportError::EmptyImage { capture: capture.id });
            }

            for annotation in &capture.annotations {
                if let Some(issue) = check_annotation(capture.id, annotation, &capture.image) {
                    issues.push(issue);
                }
            }
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

/// Validates one annotation against the image it is drawn on.
fn check_annotation(capture: u32, annotation: &Annotation, image: &RgbaImage) -> Option<ReportError> {
    let (width, height) = (image.width as i32, image.height as i32);
    match annotation {
        Annotation::Arrow {
            from,
            to,
            width: w,
            ..
        } => {
            if !from[0].is_finite()
                || !from[1].is_finite()
                || !to[0].is_finite()
                || !to[1].is_finite()
            {
                return Some(ReportError::AnnotationOutOfBounds {
                    capture,
                    detail: "arrow endpoint is not a finite number".to_string(),
                });
            }
            if (from[0] - to[0]).abs() < 1.0 && (from[1] - to[1]).abs() < 1.0 {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: "arrow has zero length".to_string(),
                });
            }
            if !(*w >= 1.0 && *w <= 40.0) {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: format!("arrow width {w} is outside 1-40 px"),
                });
            }
        }
        Annotation::Box { rect, width: w, .. } => {
            if rect.is_empty() {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: "box has zero width or height".to_string(),
                });
            }
            if !(*w >= 1.0 && *w <= 40.0) {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: format!("box width {w} is outside 1-40 px"),
                });
            }
            if !rect.intersects(image.width, image.height) {
                return Some(ReportError::AnnotationOutOfBounds {
                    capture,
                    detail: format!(
                        "box at ({}, {}) {} x {} falls outside the {} x {} image",
                        rect.x, rect.y, rect.w, rect.h, width, height
                    ),
                });
            }
        }
        Annotation::Text { at, text, size, .. } => {
            if text.trim().is_empty() {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: "text annotation is empty".to_string(),
                });
            }
            if !(*size >= 6.0 && *size <= 200.0) {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: format!("text size {size} is outside 6-200 px"),
                });
            }
            if !at[0].is_finite() || !at[1].is_finite() {
                return Some(ReportError::AnnotationOutOfBounds {
                    capture,
                    detail: "text position is not a finite number".to_string(),
                });
            }
        }
        Annotation::Blur { rect, strength } => {
            if rect.is_empty() {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: "redaction box has zero width or height".to_string(),
                });
            }
            if !(*strength >= 2 && *strength <= 64) {
                return Some(ReportError::BadAnnotation {
                    capture,
                    detail: format!("mosaic block size {strength} is outside 2-64 px"),
                });
            }
            if !rect.intersects(image.width, image.height) {
                return Some(ReportError::AnnotationOutOfBounds {
                    capture,
                    detail: format!(
                        "redaction box at ({}, {}) {} x {} falls outside the {} x {} image",
                        rect.x, rect.y, rect.w, rect.h, width, height
                    ),
                });
            }
        }
    }
    None
}

/// Everything that can stop a report being assembled, with user-facing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    /// The report has no title.
    EmptyTitle,
    /// No screenshots yet.
    NoCaptures,
    /// More screenshots than this edition accepts (free: one, pro: a sequence).
    TooManyCaptures { count: usize, max: usize },
    /// Two captures share an id.
    DuplicateCaptureId { id: u32 },
    /// A capture carries no pixels.
    EmptyImage { capture: u32 },
    /// An annotation is structurally wrong (empty text, bad width, …).
    BadAnnotation { capture: u32, detail: String },
    /// An annotation lies (entirely) off the image it belongs to.
    AnnotationOutOfBounds { capture: u32, detail: String },
    /// The scrolling-capture stitcher could not join the frames.
    Stitch(String),
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReportError::EmptyTitle => write!(f, "give the report a title before exporting"),
            ReportError::NoCaptures => write!(f, "add at least one screenshot before exporting"),
            ReportError::TooManyCaptures { count, max } if *max == FREE_MAX_CAPTURES => write!(
                f,
                "this report has {count} screenshots but the free edition exports one — \
                 the Pro edition assembles multi-screenshot sequences"
            ),
            ReportError::TooManyCaptures { count, max } => {
                write!(f, "this report has {count} screenshots, over the limit of {max}")
            }
            ReportError::DuplicateCaptureId { id } => write!(f, "screenshot #{id} is duplicated"),
            ReportError::EmptyImage { capture } => {
                write!(f, "screenshot #{capture} has no pixels")
            }
            ReportError::BadAnnotation { capture, detail } => {
                write!(f, "screenshot #{capture}: {detail}")
            }
            ReportError::AnnotationOutOfBounds { capture, detail } => {
                write!(f, "screenshot #{capture}: {detail}")
            }
            ReportError::Stitch(detail) => write!(f, "scrolling capture: {detail}"),
        }
    }
}

impl std::error::Error for ReportError {}

/// Renders an error list the way the UI shows it.
pub fn describe_issues(issues: &[ReportError]) -> String {
    issues
        .iter()
        .map(|issue| issue.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_width(width: f64) -> String {
    if width.fract().abs() < f64::EPSILON {
        format!("{width:.0}")
    } else {
        format!("{width:.1}")
    }
}

/// Names the handful of colours the UI offers, so a report legend reads "red"
/// rather than "rgb(211, 47, 47)".
pub fn rgb_name(color: Rgb) -> String {
    const NAMED: [(&str, Rgb); 6] = [
        ("red", [211, 47, 47]),
        ("amber", [245, 165, 0]),
        ("green", [46, 160, 67]),
        ("blue", [25, 118, 210]),
        ("black", [24, 24, 24]),
        ("white", [255, 255, 255]),
    ];
    for (name, candidate) in NAMED {
        if candidate == color {
            return name.to_string();
        }
    }
    format!("rgb({}, {}, {})", color[0], color[1], color[2])
}

/// Formats an epoch-millisecond timestamp as `YYYY-MM-DD HH:MM:SS.mmm UTC`.
///
/// Implemented with the days-from-civil algorithm so the engine stays
/// dependency-free and behaves identically on wasm and native hosts.
pub fn format_utc(epoch_ms: f64) -> String {
    if !epoch_ms.is_finite() {
        return "unknown time".to_string();
    }
    // Floor to whole milliseconds, then to seconds (toward negative infinity so
    // pre-1970 timestamps behave).
    let total_ms = epoch_ms.floor();
    let seconds = (total_ms / 1000.0).floor();
    let millis = (total_ms - seconds * 1000.0).round() as i64;
    let days = (seconds / 86_400.0).floor() as i64;
    let secs_of_day = (seconds - days as f64 * 86_400.0).round() as i64;

    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{millis:03} UTC")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 -> (y, m, d).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> RgbaImage {
        RgbaImage::filled(8, 6, [255, 255, 255, 255]).expect("image")
    }

    #[test]
    fn validates_a_minimal_report() {
        let mut report = Report::new("Crash on export");
        report.push(Capture::new(1, image()));
        assert_eq!(report.validate(), Ok(()));
        assert!(report.is_exportable());
    }

    #[test]
    fn rejects_an_untitled_empty_report() {
        let report = Report::new("   ");
        let issues = report.validate().expect_err("must fail");
        assert!(issues.contains(&ReportError::EmptyTitle));
        assert!(issues.contains(&ReportError::NoCaptures));
    }

    #[test]
    fn orders_by_timestamp_then_id() {
        let mut report = Report::new("Ordering");
        report.push(Capture::new(7, image()).with_taken_at_ms(300.0));
        report.push(Capture::new(2, image()).with_taken_at_ms(100.0));
        report.push(Capture::new(3, image()).with_taken_at_ms(100.0));
        let order: Vec<u32> = report.ordered().iter().map(|c| c.id).collect();
        assert_eq!(order, vec![2, 3, 7]);
    }

    #[test]
    fn reports_every_problem_at_once() {
        let mut report = Report::new("");
        report.push(Capture::new(1, image()));
        let mut duplicate = Capture::new(1, image());
        duplicate.annotations.push(Annotation::Text {
            at: [0.0, 0.0],
            text: "   ".to_string(),
            size: 14.0,
            color: [0, 0, 0],
        });
        report.push(duplicate);
        let issues = report.validate().expect_err("must fail");
        assert!(issues.contains(&ReportError::EmptyTitle));
        assert!(issues.contains(&ReportError::DuplicateCaptureId { id: 1 }));
        assert!(issues
            .iter()
            .any(|issue| matches!(issue, ReportError::BadAnnotation { .. })));
    }

    #[test]
    fn rejects_annotations_off_the_image() {
        let mut report = Report::new("Off image");
        let mut capture = Capture::new(1, image());
        capture.annotations.push(Annotation::Box {
            rect: Rect::new(500, 500, 20, 20),
            color: [0, 0, 0],
            width: 2.0,
        });
        report.push(capture);
        let issues = report.validate().expect_err("must fail");
        assert!(matches!(
            issues.first(),
            Some(ReportError::AnnotationOutOfBounds { .. })
        ));
    }

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(format_utc(0.0), "1970-01-01 00:00:00.000 UTC");
        assert_eq!(format_utc(1_700_000_000_000.0), "2023-11-14 22:13:20.000 UTC");
        assert_eq!(
            format_utc(1_700_000_000_123.0),
            "2023-11-14 22:13:20.123 UTC"
        );
        assert_eq!(format_utc(f64::NAN), "unknown time");
    }

    #[test]
    fn names_the_palette_colours() {
        assert_eq!(rgb_name([211, 47, 47]), "red");
        assert_eq!(rgb_name([1, 2, 3]), "rgb(1, 2, 3)");
    }

    #[test]
    fn edition_limits_match_the_feature() {
        let max = edition_max_captures();
        if cfg!(feature = "pro") {
            assert_eq!(max, PRO_MAX_CAPTURES);
            assert_eq!(edition_name(), "pro");
        } else {
            assert_eq!(max, FREE_MAX_CAPTURES);
            assert_eq!(edition_name(), "free");
        }
    }
}