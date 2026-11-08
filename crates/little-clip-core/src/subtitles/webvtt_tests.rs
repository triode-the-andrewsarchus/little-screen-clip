use super::*;
fn parse(source: &str) -> Result<Document, String> {
    Document::parse(source, Format::WebVtt, &AtomicBool::new(false))
}
#[test]
fn webvtt_metadata_stays_passive_and_identifiers_markup_and_settings_survive() {
    let source = "\u{feff}WEBVTT Study notes\nX-TIMESTAMP-MAP=LOCAL:00:00.000,MPEGTS:900000\n\nNOTE editorial annotation\nDo not display as a cue.\n\nSTYLE\n::cue { color: red; }\n\nREGION\nid:lower\nwidth:50%\n\n  opening title  \n00:01.125 --> 00:03.000 region:lower align:start\n<v narrator><b>Garden</b> &amp; sky\nSecond line\n\nNOTE\n00:00.000 --> 00:09.000\nAn annotation, not a third cue.\n\nNOTEBOOK\n00:00:02.500 --> 00:00:04.000 line:80%\nTwo voices overlap.\n";
    let doc = parse(source).unwrap();
    assert_eq!(doc.cues.len(), 2);
    assert_eq!(doc.cues[0].identifier, "  opening title  ");
    assert_eq!(
        doc.cues[0].text,
        "<v narrator><b>Garden</b> &amp; sky\nSecond line"
    );
    assert_eq!(doc.cues[0].settings, "region:lower align:start");
    assert_eq!(doc.cues[0].start_ms, 1125); // Local cue clock, no external mapping applied.
    assert_eq!(
        doc.cues[0].source_line,
        source[..source.find("00:01.125").unwrap()].lines().count() + 1
    );
    assert_eq!(doc.cues[1].identifier, "NOTEBOOK");
    assert_eq!(doc.cues[1].end_ms, 4000);
    assert_eq!(doc.timeline.peak_concurrency, 2);
}
#[test]
fn webvtt_supports_all_line_endings_and_header_only_documents() {
    let source = "WEBVTT\n\none\n00:01.000 --> 01:02:03.004\nA\nB\n";
    let expected = parse(source).unwrap();
    assert_eq!(expected.cues[0].source_line, 4);
    assert_eq!(expected.timeline.duration_ms, 3_723_004);
    for separator in ["\r", "\r\n"] {
        assert_eq!(
            parse(&source.replace('\n', separator)).unwrap().cues,
            expected.cues
        );
    }
    assert!(parse("WEBVTT\n\n").unwrap().cues.is_empty());
    assert_eq!(
        Format::from_path(Path::new("captions.VTT")),
        Some(Format::WebVtt)
    );
    assert!(is_subtitle_path(Path::new("captions.sRt")));
    assert!(!is_subtitle_path(Path::new("captions.txt")));
}
#[test]
fn webvtt_invalid_headers_and_timings_fail_without_becoming_empty_previews() {
    for source in [
        "",
        "webvtt\n\n",
        "WEBVTTbad\n\n",
        "WEBVTT --> invalid\n\n",
        "WEBVTT\n00:01.000 --> 00:02.000\nmissing separator",
        "WEBVTT\n\n00:01,000 --> 00:02,000\nwrong fraction separator",
        "WEBVTT\n\n00:01.000 --> 00:01.000\nzero duration",
        "WEBVTT\n\n00:02.000 --> 00:01.000\nreversed",
        "WEBVTT\n\n00:60.000 --> 01:01.000\nseconds out of range",
        "WEBVTT\n\n60:00.000 --> 61:00.000\nminutes out of range",
        "WEBVTT\n\n1:00:01.000 --> 01:00:02.000\nshort hours",
        "WEBVTT\n\n00:01.0000 --> 00:02.000\nlong fraction",
        "WEBVTT\n\n999999999999999999:00:01.000 --> 999999999999999999:00:02.000\noverflow",
    ] {
        assert!(parse(source).is_err(), "{source}");
    }
    assert!(Document::parse("WEBVTT\n\n", Format::WebVtt, &AtomicBool::new(true)).is_err());
    assert!(
        parse(&format!(
            "WEBVTT\n\n{}\n00:01.000 --> 00:02.000\nx",
            "x".repeat(4097)
        ))
        .unwrap_err()
        .contains("4096")
    );
    assert!(
        parse(&" ".repeat(MAX_BYTES + 1))
            .unwrap_err()
            .contains("8 MiB")
    );
}
#[test]
fn webvtt_cue_limit_stops_before_unbounded_timeline_work() {
    let source = format!(
        "WEBVTT\n\n{}",
        "00:00.000 --> 00:00.001\nx\n\n".repeat(MAX_CUES + 1)
    );
    assert!(parse(&source).unwrap_err().contains("100,000"));
}
