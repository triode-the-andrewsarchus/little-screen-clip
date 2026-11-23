use std::{fs::File, io::Write, sync::atomic::AtomicBool};

use flate2::{Compression, write::ZlibEncoder};

use super::{Capture, Format, FrameKind, identify};

fn recording(bytes: &[u8]) -> File {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file
}

#[test]
fn asc_checkpoint_retains_relative_clock_and_pretrigger_time() {
    let mut source = b"date Mon Nov 2 03:22:02 2026\nbase hex timestamps relative\n".to_vec();
    source.extend_from_slice(b"-0.002 1 201 Rx d 1 FF\n");
    for _ in 1..600 {
        source.extend_from_slice(b"0.00005 CAN 1 Status:chip status error active\n");
        source.extend_from_slice(b"0.001 1 201 Rx d 1 AA\n");
    }
    assert_eq!(identify(&source), Some(Format::VectorAsc));
    let capture = Capture::open(recording(&source)).unwrap();
    let cancel = AtomicBool::new(false);
    let index = capture.index(&cancel).unwrap();
    assert_eq!(index.summary.frames, 600);
    assert_eq!(index.summary.time_bounds_ns.unwrap()[0], -2_000_000);
    let (start, end) = (400, 410);
    let page = capture.rows(&index, start..end, &cancel).unwrap();
    let first = i128::from(start) * 1_050_000 - 2_000_000;
    assert_eq!(page[0].timestamp_ns, first);
    assert_eq!(page[0].data, [0xaa]);
    let frame = capture
        .frames(&index, first..first + 1, 2, &cancel)
        .unwrap();
    assert_eq!(frame[0].row, start);
    let other = Capture::open(recording(&source)).unwrap();
    assert!(other.rows(&index, 0..1, &cancel).is_err());
}

#[test]
fn trace_recognition_requires_a_can_signature() {
    let unrelated = b"{\"traceEvents\":[{\"name\":\"blocked\",\"ph\":\"B\"}]}";
    assert_eq!(identify(unrelated), None);
    let binary = b"\x30\x29\x00\x00\x02\xc0\x05\x00\x32\x01\x00\xff\xfe\x00";
    assert_eq!(identify(binary), None);
    assert_eq!(identify(b"LOGG\x00\x00\x00\x00"), None);
    assert_eq!(identify(b"base hex timestamps relative\n"), None);
}

#[test]
fn pcan_trc_v3_keeps_xl_length_and_bus_errors() {
    let source = b";$FILEVERSION=3.0\n;$STARTTIME=46000.25\n;$COLUMNS=N,O,T,B,I,d,R,V,S,A,r,s,L,D\n1 0.120 XL 2 701 Rx - 0F 01 00112233 0 0 0 A5\n2 0.125 ER 2 - - - - - - - - - 00 11 22 33 44\n";
    let capture = Capture::open(recording(source)).unwrap();
    let cancel = AtomicBool::new(false);
    let index = capture.index(&cancel).unwrap();
    assert_eq!(index.summary.errors, 1);
    let frames = capture.rows(&index, 0..2, &cancel).unwrap();
    assert_eq!(frames[0].kind, FrameKind::CanXl);
    assert_eq!(frames[0].timestamp_ns, 120_000);
    assert_eq!(frames[0].data, [0xa5]);
    assert_eq!(frames[1].kind, FrameKind::Error);
    assert_eq!(frames[1].data, [0, 0x11, 0x22, 0x33, 0x44]);
}

fn object_header(size: u32, header: u16, kind: u32) -> Vec<u8> {
    let mut result = b"LOBJ".to_vec();
    result.extend(header.to_le_bytes());
    result.extend(1u16.to_le_bytes());
    result.extend(size.to_le_bytes());
    result.extend(kind.to_le_bytes());
    result
}

fn classic_blf_row(time: u64) -> Vec<u8> {
    let mut result = object_header(48, 32, 1);
    result.extend(2u32.to_le_bytes());
    result.extend([0; 4]);
    result.extend(time.to_le_bytes());
    result.extend([1, 0, 0, 8]);
    result.extend(0x321u32.to_le_bytes());
    result.extend([0x12, 0x34, 2, 3, 4, 5, 6, 7]);
    result
}

fn compressed_container(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data).unwrap();
    let encoded = encoder.finish().unwrap();
    let length = encoded.len() + 32;
    let mut output = object_header(length as u32, 16, 10);
    output.extend(2u16.to_le_bytes());
    output.extend([0; 6]);
    output.extend((data.len() as u32).to_le_bytes());
    output.extend([0; 4]);
    output.extend_from_slice(&encoded);
    output.resize(output.len() + length % 4, 0);
    output
}

#[test]
fn blf_checkpoint_resumes_an_object_split_across_compressed_containers() {
    let objects: Vec<_> = (0..600).flat_map(classic_blf_row).collect();
    let cross = 256 * 48 + 15;
    let mut bytes = vec![0u8; 144];
    bytes[..4].copy_from_slice(b"LOGG");
    bytes[4..8].copy_from_slice(&144u32.to_le_bytes());
    bytes.extend(compressed_container(&objects[..cross]));
    bytes.extend(compressed_container(&objects[cross..]));
    let capture = Capture::open(recording(&bytes)).unwrap();
    let cancel = AtomicBool::new(false);
    let preview = capture.preview(&cancel).unwrap();
    assert_eq!(preview.frames, 600);
    assert!(preview.complete);
    let index = capture.index(&cancel).unwrap();
    let page = capture.rows(&index, 256..257, &cancel).unwrap();
    assert_eq!(page[0].timestamp_ns, 256);
    assert_eq!(page[0].data, [0x12, 0x34, 2, 3, 4, 5, 6, 7]);
}
