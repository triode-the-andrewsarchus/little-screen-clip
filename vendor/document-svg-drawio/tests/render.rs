use base64::Engine;
use document_svg_drawio::{Limits, render};
use std::io::Write;
const BASIC: &str = include_str!("fixtures/basic.drawio");

fn compressed(source: &str) -> String {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(source.as_bytes()).unwrap();
    base64::engine::general_purpose::STANDARD.encode(encoder.finish().unwrap())
}

#[test]
fn plain_compressed_and_multiple_pages_preserve_shapes_labels_and_ids() {
    let model = &BASIC[BASIC.find("<mxGraphModel>").unwrap()..BASIC.find("</diagram>").unwrap()];
    let body = compressed(model);
    let packed = format!("<mxfile><diagram>{body}</diagram></mxfile>");
    for source in [BASIC, model, packed.as_str()] {
        let report = render(source.as_bytes(), Limits::default()).unwrap();
        assert_eq!(report.pages.len(), 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        for expected in [
            "<path", "Source", "Preview", "renders", "drawio-a", "#DAE8FC",
        ] {
            assert!(report.pages[0].contains(expected), "{expected}");
        }
    }
    let source = format!("<mxfile><diagram>{model}</diagram><diagram>{body}</diagram></mxfile>");
    assert_eq!(
        render(source.as_bytes(), Limits::default())
            .unwrap()
            .pages
            .len(),
        2
    );
}

#[test]
fn budgets_reject_source_inflate_event_page_and_svg_overruns() {
    for limits in [
        Limits {
            source_bytes: 10,
            ..Limits::default()
        },
        Limits {
            xml_events: 2,
            ..Limits::default()
        },
        Limits {
            pages: 0,
            ..Limits::default()
        },
        Limits {
            svg_bytes: 100,
            ..Limits::default()
        },
    ] {
        assert!(render(BASIC.as_bytes(), limits).is_err());
    }
    let body = compressed(&" ".repeat(65536));
    let source = format!("<mxfile><diagram>{body}</diagram></mxfile>");
    let error = render(
        source.as_bytes(),
        Limits {
            inflated_page_bytes: 1024,
            ..Limits::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("expands past"), "{error}");
}

#[test]
fn external_images_are_diagnosed_without_retaining_the_reference() {
    for href in ["file:///etc/passwd", "https://example.com/private.png"] {
        let source = BASIC.replace("rounded=1;", &format!("shape=image;image={href};"));
        let report = render(source.as_bytes(), Limits::default()).unwrap();
        assert!(!report.warnings.is_empty());
        assert!(!report.pages[0].contains("href=\"file:"));
        assert!(!report.pages[0].contains("href=\"https:"));
    }
}

#[test]
fn invalid_containers_and_compressed_data_report_errors() {
    for source in [
        "<xml/>",
        "<mxfile><diagram>broken!</diagram></mxfile>",
        "<mxfile><diagram/></mxfile>",
        "<mxlibrary/>",
    ] {
        assert!(
            render(source.as_bytes(), Limits::default()).is_err(),
            "{source}"
        );
    }
}
