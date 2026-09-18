//! End-to-end validation of the capture → annotate → assemble pipeline.
//!
//! These tests exercise the crate the way a host does (through the public API
//! only) and are the acceptance evidence for the engine phase of `todo.md`:
//! a sample sequence assembles into a **correctly ordered, readable** report,
//! the edition's capture budget is enforced, the pixels that reach the report
//! are the annotated ones, and the whole assembly is deterministic.

use tpt_bugreport_engine::base64;
use tpt_bugreport_engine::image::RgbaImage;
use tpt_bugreport_engine::markdown;
use tpt_bugreport_engine::model::{
    Annotation, Capture, Environment, Rect, Report, ReportError, Severity,
};
// The engine's PNG writer, aliased so the `png` decoder crate stays reachable
// for the round-trip check below.
use tpt_bugreport_engine::png as engine_png;

/// A screenshot-like image: a light UI panel with a few darker rows.
fn screenshot(width: u32, height: u32, accent: u8) -> RgbaImage {
    let mut image = RgbaImage::filled(width, height, [246, 247, 249, 255]).expect("image");
    for y in (0..height as i32).step_by(9) {
        image.fill_rect(Rect::new(0, y, width as i32, 1), [215, 219, 226, 255], 255);
    }
    image.fill_rect(Rect::new(6, 6, 40, 10), [accent, 60, 90, 255], 255);
    image
}

/// One screenshot of the sample walkthrough, already marked up.
fn sample_capture(id: u32, taken_at_ms: f64, accent: u8) -> Capture {
    let mut capture = Capture::new(id, screenshot(160, 120, accent))
        .with_title(format!("Step {id} screenshot"))
        .with_source(format!("screen: region {id}"))
        .with_taken_at_ms(taken_at_ms);
    capture.note = format!("Observed at step {id}.");
    capture.annotations.push(Annotation::Box {
        rect: Rect::new(6, 6, 40, 10),
        color: [211, 47, 47],
        width: 2.0,
    });
    capture
}

/// A filled-in report with `captures` screenshots taken one second apart.
fn sample_report(captures: usize) -> Report {
    let mut report = Report::new("Toolbar loses focus after saving");
    report.meta.summary = "After saving, the toolbar stops responding to clicks.".to_string();
    report.meta.severity = Severity::High;
    report.meta.reporter = "Field Support".to_string();
    report.meta.environment = Environment {
        app_name: "Acme Invoicer".to_string(),
        app_version: "7.2".to_string(),
        os: "Windows 11 24H2".to_string(),
        browser: "Edge 122".to_string(),
        device: "Dell Latitude 5540".to_string(),
    };
    report.meta.steps = vec![
        "Open an invoice".to_string(),
        "Click Save".to_string(),
        "Try to use the toolbar".to_string(),
    ];
    for index in 0..captures {
        report.push(sample_capture(
            (index + 1) as u32,
            1_700_000_000_000.0 + index as f64 * 1000.0,
            (index * 40) as u8,
        ));
    }
    report
}

#[test]
fn a_report_at_this_editions_limit_assembles() {
    let limit = tpt_bugreport_engine::edition_max_captures();
    let report = sample_report(limit);
    assert_eq!(report.validate(), Ok(()), "the edition's own limit is legal");

    let markdown = markdown::assemble(&report).expect("assembles");
    assert!(markdown.starts_with("# Toolbar loses focus after saving"));
    assert!(markdown.contains("**Severity:** High"));
    assert!(markdown.contains("| Application | Acme Invoicer |"));
    assert!(markdown.contains("3. Try to use the toolbar"));
    for index in 0..limit {
        assert!(
            markdown.contains(&format!("### {} of {} ", index + 1, limit)),
            "screenshot {} is numbered in the report",
            index + 1
        );
    }
    assert_eq!(markdown.matches("data:image/png;base64,").count(), limit);
}

#[test]
fn the_edition_budget_is_enforced() {
    let limit = tpt_bugreport_engine::edition_max_captures();
    let report = sample_report(limit + 1);
    let issues = report.validate().expect_err("over budget");
    match issues.first() {
        Some(ReportError::TooManyCaptures { count, max }) => {
            assert_eq!(*count, limit + 1);
            assert_eq!(*max, limit);
        }
        other => panic!("expected a capture-budget error, got {other:?}"),
    }
    assert!(markdown::assemble(&report).is_err());
    // The message must tell the user how to proceed, not just that it failed.
    let text = ReportError::TooManyCaptures {
        count: limit + 1,
        max: limit,
    }
    .to_string();
    assert!(text.contains(&format!("{}", limit + 1)));
    if limit == 1 {
        assert!(text.contains("Pro edition"), "the upsell text is in the error");
    }
}

#[test]
fn screenshots_are_ordered_chronologically_not_by_insertion() {
    let mut report = Report::new("Out of order");
    report.push(sample_capture(7, 3000.0, 0));
    report.push(sample_capture(2, 1000.0, 40));
    report.push(sample_capture(5, 2000.0, 80));
    let order: Vec<u32> = report.ordered().iter().map(|capture| capture.id).collect();
    assert_eq!(order, vec![2, 5, 7], "both exports use capture order");
}

#[test]
fn the_report_embeds_the_annotated_pixels() {
    let report = sample_report(1);
    let markdown = markdown::assemble(&report).expect("assembles");
    let capture = &report.captures[0];

    // The embedded image is exactly the annotated composite, PNG-encoded and
    // base64-armoured — no sidecar files, no unmarked original.
    let expected = base64::png_data_uri(&engine_png::encode(&capture.annotated()));
    assert!(
        markdown.contains(&expected),
        "the report embeds the marked-up image"
    );

    // ...and that image really is marked up: a box pixel is annotation red.
    let annotated = capture.annotated();
    assert_eq!(annotated.get(6, 6), [211, 47, 47, 255]);
    assert_ne!(capture.image.get(6, 6), [211, 47, 47, 255]);
}

#[test]
fn the_markdown_is_self_contained_and_deterministic() {
    let report = sample_report(1);
    let first = markdown::assemble(&report).expect("assembles");
    let second = markdown::assemble(&report).expect("assembles");
    assert_eq!(first, second, "assembly has no hidden state or clock");
    assert!(!first.contains("http://") && !first.contains("https://"));
    assert_eq!(
        markdown::file_name(&report),
        "toolbar-loses-focus-after-saving.md"
    );
}

#[test]
fn the_annotated_composite_survives_a_png_round_trip() {
    let capture = sample_capture(1, 1000.0, 60);
    let annotated = capture.annotated();
    let png_bytes = engine_png::encode(&annotated);

    let decoder = png::Decoder::new(png_bytes.as_slice());
    let mut reader = decoder.read_info().expect("png header");
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).expect("png data");
    assert_eq!((info.width, info.height), (annotated.width, annotated.height));
    assert_eq!(&buffer[..info.buffer_size()], &annotated.data[..]);
}