use super::*;
use std::io::{Seek, SeekFrom, Write};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn archived(parts: &[(&str, &[u8], CompressionMethod)]) -> File {
    let mut zip = ZipWriter::new(tempfile::tempfile().unwrap());
    for &(name, data, method) in parts {
        zip.start_file(
            name,
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap()
}

fn fixture(bytes: &[u8]) -> File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file
}

#[test]
fn sigrok_preserves_a_narrow_pulse_across_members() {
    use CompressionMethod::{Deflated, Stored};
    let metadata = b"[device 1]\nunitsize=1\ntotal probes=8\nsamplerate=1 kHz\ncapturefile=logic-1\nprobe3=Clock\n";
    let file = archived(&[
        ("metadata", metadata, Deflated),
        ("version", b"2", Stored),
        ("logic-1-2", &[4, 0, 0, 0], Deflated),
        ("logic-1-1", &[0; 4], Stored),
    ]);
    let mut retained = file.try_clone().unwrap();
    retained.seek(SeekFrom::Start(11)).unwrap();
    let capture = Capture::open(file, Path::new("logic.sr")).unwrap();
    let cancel = AtomicBool::new(false);
    let trace = capture.read_edges(0, 0.0..0.008, 16, &cancel).unwrap();
    assert_eq!(capture.lanes()[0].name, "Clock");
    assert_eq!(trace.initial, Some(Level::Low));
    assert_eq!(trace.inspected_until, 0.008);
    assert_eq!(
        trace.edges,
        [
            Edge {
                time_seconds: 0.004,
                level: Level::High
            },
            Edge {
                time_seconds: 0.005,
                level: Level::Low
            },
        ]
    );
    let summary = capture.summarize(0, 0.0..0.008, 1, 20, &cancel).unwrap();
    assert!(!summary.sampled());
    assert!(summary.bins[0].high);
    assert!(summary.bins[0].low);
    assert_eq!(summary.bins[0].transitions, 2);
    assert_eq!(retained.stream_position().unwrap(), 11);
}

#[test]
fn dsview_verifies_archive_identity_and_decodes_trigger_relative_bits() {
    let path = Path::new("test.dsl");
    assert!(!probe(&fixture(b"diagram { src -> dst; }"), path));
    use CompressionMethod::Deflated;
    let header = b"[version]\nversion=3\n[header]\ndevice mode=0\ncapturefile=data\nsamplerate=1 MHz\ntotal samples=9\ntotal probes=2\ntotal blocks=1\ntrigger pos=4\nprobe1=Signal\n";
    let file = archived(&[
        ("header", header, Deflated),
        ("L-1/0", &[0b0001_0000, 0b1111_1110], Deflated),
    ]);
    let capture = Capture::open(file, path).unwrap();
    let time = capture.time_bounds(0).unwrap();
    assert_eq!(time, -4e-6..5e-6);
    let trace = capture
        .read_edges(0, time.clone(), 10, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(trace.inspected_until, time.end);
    assert_eq!(trace.edges.len(), 2);
    assert_eq!(
        trace.edges[0],
        Edge {
            time_seconds: 0.0,
            level: Level::High
        }
    );
    assert!((trace.edges[1].time_seconds - 1e-6).abs() < 1e-20);
}

#[test]
fn saleae_digital_export_preserves_gaps_and_exact_sparse_activity() {
    let mut file = b"<SALEAE>\x01\0\0\0\0\0\0\0".to_vec();
    file.extend(2_u64.to_le_bytes());
    for (initial, start, end, transitions) in [
        (0_u32, 0.0_f64, 1.0_f64, &[0.1_f64, 0.100000001][..]),
        (1, 2.0, 3.0, &[][..]),
    ] {
        file.extend(initial.to_le_bytes());
        file.extend(80e6_f64.to_le_bytes());
        file.extend(start.to_le_bytes());
        file.extend(end.to_le_bytes());
        file.extend((transitions.len() as u64).to_le_bytes());
        file.extend(transitions.iter().flat_map(|event| event.to_le_bytes()));
    }
    assert!(matches_binary_header(&file[..16]));
    let path = Path::new("digital.bin");
    let capture = Capture::open(fixture(&file), path).unwrap();
    let cancel = AtomicBool::new(false);
    let trace = capture.read_edges(0, 0.0..3.0, 15, &cancel).unwrap();
    assert_eq!(trace.initial, Some(Level::Low));
    assert_eq!(
        trace.edges[2],
        Edge {
            time_seconds: 1.0,
            level: Level::Unknown
        }
    );
    let envelope = capture.summarize(0, 0.0..3.0, 3, 1, &cancel).unwrap();
    assert!(!envelope.sampled());
    assert!(envelope.bins[0].high && envelope.bins[0].low);
    assert_eq!(envelope.bins[0].transitions, 2);
    assert!(envelope.bins[1].unknown);
    assert!(envelope.bins[2].high);
    assert!(!probe(
        &fixture(b"some digits 1234567890 not a signal"),
        path
    ));
}

#[test]
fn native_saleae_rle_grows_untrimmed_extent_and_reports_exact_edges() {
    let mut digital = b"<SALEAE>\x02\0\0\0\x64\0\0\0\x01".to_vec();
    digital.extend(1e6_f64.to_le_bytes());
    digital.extend([0; 18]);
    digital.extend(2_u64.to_le_bytes());
    for (start, end, flags, bytes) in [(0_u64, 5_u64, 0_u64, [1, 2]), (5, 8, 1, [0, 1])] {
        for value in [start, end, flags | (2 << 16)] {
            digital.extend(value.to_le_bytes());
        }
        digital.extend([0, 0]);
        digital.extend(bytes);
    }
    let meta = br#"{"version":20,"data":{"rowsSettings":[{"name":"Data","type":"channel","channel":{"category":"legacy","type":"Digital","deviceChannel":2}}]},"binData":[{"category":"legacy","type":"Digital","deviceChannel":2,"file":"./digital-2.bin"}]}"#;
    let file = archived(&[
        ("meta.json", meta, CompressionMethod::Deflated),
        ("digital-2.bin", &digital, CompressionMethod::Deflated),
    ]);
    let capture = Capture::open(file, Path::new("native.sal")).unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(capture.lanes()[0].name, "Data");
    assert_eq!(capture.time_bounds(0).unwrap(), 0.0..5e-6);
    assert!(capture.refine(&cancel).unwrap());
    assert_eq!(capture.time_bounds(0).unwrap(), 0.0..8e-6);
    let trace = capture.read_edges(0, 0.0..8e-6, 8, &cancel).unwrap();
    assert_eq!(
        trace.edges,
        [
            Edge {
                time_seconds: 2e-6,
                level: Level::High
            },
            Edge {
                time_seconds: 6e-6,
                level: Level::Low
            },
        ]
    );
}

#[test]
fn vcd_decodes_bit_lanes_aliases_and_unknown_values_without_decimation() {
    let bytes = b"$timescale 10 ns $end\n$scope module board $end\n$var wire 1 ! clock $end\n$var wire 2 # bus [1:0] $end\n$var wire 1 ! alias $end\n$upscope $end\n$enddefinitions $end\n#0\n$dumpvars\n0!\nbx #\n$end\n#5\n1!\nbz1 #\n#6\n0!\n#10\n";
    let capture = Capture::open(fixture(bytes), Path::new("logic.vcd")).unwrap();
    let cancel = AtomicBool::new(false);
    assert_eq!(capture.lanes()[1].name, "board.bus[1]");
    assert_eq!(capture.lanes()[3].name, "board.alias");
    let bounds = capture.time_bounds(0).unwrap();
    let trace = capture.read_edges(1, bounds.clone(), 5, &cancel).unwrap();
    assert_eq!(trace.initial, Some(Level::Unknown));
    assert_eq!(trace.edges.len(), 1);
    assert_eq!(trace.edges[0].level, Level::HighImpedance);
    assert!((trace.edges[0].time_seconds - 50e-9).abs() < 1e-22);
    let envelope = capture.summarize(3, bounds, 1, 50, &cancel).unwrap();
    assert!(!envelope.sampled());
    assert!(envelope.bins[0].high);
    assert!(envelope.bins[0].low);
    assert_eq!(envelope.bins[0].transitions, 2);
}

#[test]
fn a_long_deflate_seek_can_progress_across_bounded_jobs() {
    let data = b"0123456789abcdef".repeat(4096);
    let file = archived(&[("samples", &data, CompressionMethod::Deflated)]);
    let len = file.metadata().unwrap().len();
    let mut archive = archive::Archive::open(Arc::new(file), len).unwrap();
    let member = archive.member("samples").unwrap();
    let cancel = AtomicBool::new(false);
    let mut result = [0; 4];
    let mut budget = 4096;
    let start = data.len() as u64 - result.len() as u64;
    assert!(
        !member
            .read_exact_at(&mut result, start, &mut budget, &cancel)
            .unwrap()
    );
    assert_eq!(budget, 0);
    let mut budget = start;
    assert!(
        member
            .read_exact_at(&mut result, start, &mut budget, &cancel)
            .unwrap()
    );
    assert_eq!(&result, b"cdef");
}

#[test]
fn validates_zip64_directory_with_compatible_classic_fields() {
    use std::io::Read;
    let mut input = archived(&[("samples", b"digital", CompressionMethod::Stored)]);
    let base = input.seek(SeekFrom::End(-22)).unwrap();
    let mut classic = [0; 22];
    input.read_exact(&mut classic).unwrap();
    let count = u16::from_le_bytes(classic[10..12].try_into().unwrap()) as u64;
    let size = u32::from_le_bytes(classic[12..16].try_into().unwrap()) as u64;
    let offset = u32::from_le_bytes(classic[16..20].try_into().unwrap()) as u64;
    input.seek(SeekFrom::Start(base)).unwrap();
    input.write_all(b"PK\x06\x06").unwrap();
    input.write_all(&44_u64.to_le_bytes()).unwrap();
    input
        .write_all(&[45, 0, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    for value in [count, count, size, offset] {
        input.write_all(&value.to_le_bytes()).unwrap();
    }
    input.write_all(b"PK\x06\x07\0\0\0\0").unwrap();
    input.write_all(&base.to_le_bytes()).unwrap();
    input.write_all(&1_u32.to_le_bytes()).unwrap();
    input.write_all(&classic).unwrap();
    let len = input.metadata().unwrap().len();
    let mut archive = archive::Archive::open(Arc::new(input), len).unwrap();
    let reader = archive.member("samples").unwrap();
    let mut result = [0; 7];
    assert!(
        reader
            .read_exact_at(&mut result, 0, &mut 0, &AtomicBool::new(false))
            .unwrap()
    );
    assert_eq!(&result, b"digital");
}
