use super::*;
use std::{
    io::{Cursor, Read},
    sync::atomic::AtomicBool,
};

fn open<'a>(data: &'a str, filename: &str) -> Reader<Cursor<&'a str>> {
    Reader::open(Cursor::new(data), Path::new(filename)).unwrap()
}

fn assert_value(point: &Point, rows: usize, cols: usize, re: f64, im: f64) {
    let value = point.value(rows, cols).unwrap();
    assert!((value.re - re).abs() < 1e-10, "{value:?}");
    assert!((value.im - im).abs() < 1e-10, "{value:?}");
}

#[test]
fn legacy_two_port_order_and_encoding_keep_frequency_and_matrix_identity() {
    let mut input = open(
        "! Sample\r\n# MHz S RI R 75\r\n1 .1 .01 .2 .02 .3 .03 .4 .04 ! comment\r\n",
        "SAMPLE.S2P",
    );
    assert_eq!(input.metadata().reference_ohms, [75.0, 75.0]);
    let point = input.next_point().unwrap().unwrap();
    assert_eq!(point.frequency_hz, 1.0e6);
    assert_value(&point, 0, 0, 0.1, 0.01);
    assert_value(&point, 1, 0, 0.2, 0.02);
    assert_value(&point, 0, 1, 0.3, 0.03);
    assert_value(&point, 1, 1, 0.4, 0.04);
    assert!(input.next_point().unwrap().is_none());

    let index = input
        .build_trace_index([1, 0], &AtomicBool::new(false))
        .unwrap();
    assert_eq!(index.trace(), [1, 0]);
    assert_eq!(index.overview()[0].first.value, point.value(1, 0).unwrap());
}

#[test]
fn version_2_triangular_matrices_restore_reciprocal_values_and_references() {
    let mut input = open(
        "[Version] 2.1\n# kHz S DB\n[Number of Ports] 3\n[Number of Frequencies] 1\n[Reference]\n50 60\n 75\n[Matrix Format] Lower\n[Network Data]\n10 -20 0\n -6.020599913279624\n 90 -20 0\n -40 180 -40 0 -inf 0\n[End]\n",
        "three-port.ts",
    );
    assert_eq!(input.metadata().reference_ohms, [50.0, 60.0, 75.0]);
    let point = input.next_point().unwrap().unwrap();
    assert_eq!(point.frequency_hz, 10_000.0);
    assert_value(&point, 0, 1, 0.0, 0.5);
    assert_value(&point, 1, 0, 0.0, 0.5);
    assert_value(&point, 0, 2, -0.01, 0.0);
    assert_value(&point, 2, 2, 0.0, 0.0);
    assert!(input.next_point().unwrap().is_none());
}

#[test]
fn mixed_mode_order_uses_actual_port_labels_and_alternate_two_port_order() {
    let mut input = open(
        "[Version] 2.0\n# GHz S MA\n[Number of Ports] 2\n[Two-Port Data Order] 12_21\n[Number of Frequencies] 1\n[Mixed-Mode Order]\nD1,\n2 C1, 2\n[Network Data]\n1 0.1 0 0.2 90 0.3 180 0.4 -90\n[End]\n",
        "mixed.s2p",
    );
    assert_eq!(
        input.metadata().parameter_label(1, 0).as_deref(),
        Some("S(C1,2, D1,2)")
    );
    let point = input.next_point().unwrap().unwrap();
    assert_value(&point, 0, 1, 0.0, 0.2);
    assert_value(&point, 1, 0, -0.3, 0.0);
}

#[test]
fn legacy_and_version_2_noise_sections_do_not_extend_the_network_sweep() {
    let mut legacy = open(
        "# MHz S RI\n20 1 0 2 0 3 0 4 0\n30 1 0 2 0 3 0 4 0\n10 .5 .25 20 40\n",
        "noise.s2p",
    );
    assert!(legacy.next_point().unwrap().is_some());
    assert!(legacy.next_point().unwrap().is_some());
    assert!(legacy.next_point().unwrap().is_none());
    let mut modern = open(
        "[Version] 2.1\n# Hz S RI\n[Number of Ports] 2\n[Two-Port Data Order] 21_12\n[Number of Frequencies] 1\n[Number of Noise Frequencies] 1\n[Network Data]\n200 1 0 2 0 3 0 4 0\n[Noise Data]\n100 .5 .25 20 40\n[End]\n",
        "noise.ts",
    );
    assert_eq!(modern.metadata().declared_noise_points, Some(1));
    assert!(modern.next_point().unwrap().is_some());
    assert!(modern.next_point().unwrap().is_none());
}

#[test]
fn ambiguous_ts_extension_requires_a_valid_option_line_and_first_matrix() {
    let filename = Path::new("source.ts");
    assert!(!recognizes(b"export const x = 5;\n", filename));
    assert!(!recognizes(
        b"# This is a typescript example\n1 0 0\n",
        filename
    ));
    assert!(!recognizes(b"# Hz S RI\n1 0 0 1 0\n", filename));
    assert!(recognizes(b"# Hz S RI\n1 0 0\n", filename));
    let data = "# Hz S RI\n1 1 0 2 0 3 0\n4 0 5 0 6 0\n7 0 8 0 9 0\n";
    let mut reader = open(data, "three-port.ts");
    assert_eq!(reader.metadata().ports, 3);
    assert_value(&reader.next_point().unwrap().unwrap(), 2, 1, 8.0, 0.0);
}

#[test]
fn malformed_counts_and_incomplete_network_rows_report_errors() {
    let mut truncated = open("# Hz S RI\n1 .2\n", "truncated.s1p");
    assert!(matches!(truncated.next_point(), Err(Error::Truncated)));
    let mut excess = open(
        "[Version] 2.0\n# Hz S RI\n[Number of Ports] 1\n[Number of Frequencies] 1\n[Network Data]\n1 .2 0\n2 .3 0\n[End]\n",
        "excess.ts",
    );
    assert!(excess.next_point().unwrap().is_some());
    assert!(matches!(excess.next_point(), Err(Error::Invalid(_))));
}

#[test]
fn carriage_return_records_keep_correct_offsets_and_skip_comment_bytes() {
    let bytes = b"! Comment from an instrument: \xff\r# Hz S RI\r10 1 0\r20 2 0\r";
    let path = Path::new("legacy.s1p");
    let mut reader = Reader::open(Cursor::new(bytes), path).unwrap();
    let cancelled = AtomicBool::new(false);
    let index = reader.build_index(&cancelled).unwrap();
    let selection = reader.read_window(&index, 1..2, &cancelled).unwrap();
    assert_eq!(index.band_hz(), [10.0, 20.0]);
    assert_value(&selection[0], 0, 0, 2.0, 0.0);
}

#[test]
fn band_index_preserves_narrow_notch_and_locates_exact_record_windows() {
    use std::fmt::Write;
    let mut text = String::from("# Hz S RI\n");
    for frequency in 1..=4097 {
        let magnitude = if frequency == 3000 { 0.0001 } else { 0.5 };
        writeln!(text, "{frequency} {magnitude} 0").unwrap();
    }
    let cancelled = AtomicBool::new(false);
    let mut reader = open(&text, "notch.s1p");
    let index = reader.build_index(&cancelled).unwrap();
    assert_eq!(index.points(), 4097);
    assert_eq!(index.band_hz(), [1.0, 4097.0]);
    assert!(index.overview().len() <= 1024);
    assert!(index.overview().iter().any(|bin| bin.minimum.index == 2999));
    let window = reader.read_window(&index, 2998..3001, &cancelled).unwrap();
    assert_eq!(
        window
            .iter()
            .map(|point| point.frequency_hz)
            .collect::<Vec<_>>(),
        [2999.0, 3000.0, 3001.0]
    );
    assert_value(&window[1], 0, 0, 0.0001, 0.0);
    let marker = reader.nearest(&index, 3000.2, &cancelled).unwrap();
    assert_eq!(marker.index, 2999);
    assert_value(&marker, 0, 0, 0.0001, 0.0);
}

#[test]
fn a_prefix_preview_stops_at_its_byte_limit_and_reports_partial_coverage() {
    let header = Cursor::new(b"# Hz S RI\n1 .5 0\n");
    let payload = header.chain(Cursor::new(b"2 .5 0\n".repeat(40_000)));
    let cancelled = AtomicBool::new(false);
    struct Counted<R> {
        input: R,
        read: usize,
    }
    impl<R: Read> Read for Counted<R> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let read = self.input.read(output)?;
            self.read += read;
            Ok(read)
        }
    }
    let mut payload = Counted {
        input: payload,
        read: 0,
    };
    let preview = summary(&mut payload, Path::new("large.s1p"), 32, &cancelled).unwrap();
    assert_eq!(payload.read as u64, MAX_HEADER_BYTES + 1);
    assert_eq!(preview.points.len(), 32);
    assert!(!preview.complete);
}

#[test]
fn a_cancelled_scan_does_not_publish_an_index() {
    let mut input = open("# Hz S RI\n1 .2 0\n", "cancelled.s1p");
    assert!(matches!(
        input.build_index(&AtomicBool::new(true)),
        Err(Error::Cancelled)
    ));
}
