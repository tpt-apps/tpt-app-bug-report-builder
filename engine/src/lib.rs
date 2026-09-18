//! TPT Bug Report Builder — engine.
//!
//! Takes screenshots plus their mark-up and turns them into a bug report. There
//! are no UI dependencies anywhere in this crate, so the whole assembly
//! pipeline is host-unit-testable and is shared verbatim between the free WASM
//! edition, the Pro desktop build and the test suite.
//!
//! ## Pipeline
//!
//! ```text
//!   capture (upload / paste / screen grab / scroll-stitch)
//!        |
//!        v
//!   model::Capture { image, annotations }        <- plain data, validated
//!        |
//!        +--> annotate::render(capture)          <- pixels with mark-up burnt in
//!        |
//!        +--> markdown::assemble(report)         <- self-contained .md
//!        +--> markdown::assemble_with_sidecars(report) <- .md + PNG files, for pasting into a tracker
//!        |
//!        +--> pdf::assemble(report)              <- pro only: vector-over-image PDF
//! ```
//!
//! ## Editions
//!
//! The free edition builds without default features and assembles a
//! single-screenshot report ([`model::FREE_MAX_CAPTURES`]). `--features pro`
//! raises the capture budget to a full sequence ([`model::PRO_MAX_CAPTURES`]),
//! compiles in the [`pdf`] writer and the [`stitch`] scrolling-capture joiner.
//! [`model::validate`](Report::validate) is the single gate every writer goes
//! through, so an edition can never emit a report it cannot render.
//!
//! ## Encoding
//!
//! Report images are PNGs produced by [`png`], which sits on a hand-written
//! DEFLATE writer ([`deflate`]) — the same zlib stream the PDF writer uses for
//! its `/FlateDecode` image XObjects. `tests/encoding.rs` inflates every stream
//! back through `flate2` and decodes every PNG through the `png` crate, so the
//! compressor is pinned to reference implementations without shipping either
//! dependency.

pub mod annotate;
pub mod base64;
pub mod deflate;
pub mod font;
pub mod image;
pub mod markdown;
pub mod model;
pub mod png;

#[cfg(feature = "pro")]
pub mod pdf;
#[cfg(feature = "pro")]
pub mod stitch;

pub use image::{ImageError, Rgba, RgbaImage, TRANSPARENT, WHITE};
pub use model::{
    describe_issues, edition_max_captures, edition_name, format_utc, Annotation, Capture,
    Environment, Rect, Report, ReportError, ReportMeta, Severity, FREE_MAX_CAPTURES,
    PRO_MAX_CAPTURES,
};

/// The engine's version, surfaced in the UI footer so a bug report can name the
/// tool that produced it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A one-line description of what this build can do, for the UI and the upsell
/// card.
pub fn edition_capabilities() -> &'static str {
    if cfg!(feature = "pro") {
        "Pro edition: multi-screenshot sequences, scrolling capture, hotkeys, \
         Markdown and PDF report export"
    } else {
        "Free edition: one screenshot with arrow, box, text and redaction \
         mark-up, exported as a single-image Markdown report"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_compiled_edition() {
        assert_eq!(edition_name(), if cfg!(feature = "pro") { "pro" } else { "free" });
        assert!(!edition_capabilities().is_empty());
        assert!(!VERSION.is_empty());
        assert_eq!(edition_max_captures().max(1), edition_max_captures());
    }
}