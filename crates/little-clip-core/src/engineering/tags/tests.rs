use super::*;
use std::fmt::Write as _;
use std::io::Cursor;

static STOP: AtomicBool = AtomicBool::new(false);

fn load(path: &str, bytes: &[u8]) -> Document {
    preview(Path::new(path), Cursor::new(bytes), &STOP).unwrap()
}

#[test]
fn flipper_classic_preserves_missing_bytes_and_sector_positions() {
    let source = b"Filetype: Flipper NFC device\nVersion: 4\nDevice type: Mifare Classic\nUID: 00 12 34 56\nMifare Classic type: 4K\nATQA: 00 04\nBlock 143: 00 01 02 03 04 05 ?? XX 08 09 0A 0B 0C 0D 0E 0F\n";
    assert!(recognizes(source, Path::new("fixture.nfc")));
    let Document::Memory(memory) = load("fixture.nfc", source) else {
        panic!("memory expected")
    };
    assert_eq!(memory.rows.len(), 256);
    assert_eq!(memory.coverage(), [14, 4096]);
    let trailer = &memory.window(143..144)[0];
    assert_eq!(trailer.sector, Some(32));
    assert_eq!(trailer.role, RowRole::SectorTrailer);
    assert_eq!(trailer.bytes[0], Some(0));
    assert_eq!(trailer.bytes[6], None);
    assert!(!recognizes(source, Path::new("not-an-ibutton.ibtn")));
}

#[test]
fn flipper_1wire_keeps_rom_integrity_and_sram_regions() {
    let source = b"Filetype: Flipper iButton key\nVersion: 2\nProtocol: DS1992\nRom Data: 08 00 00 00 00 00 00 3D\nSram Data: 42 65 6E 63 68 00\n";
    let Document::Memory(memory) = load("fixture.ibtn", source) else {
        panic!("memory expected")
    };
    assert_eq!(memory.rows.len(), 2);
    assert_eq!(memory.rows[0].role, RowRole::Rom);
    assert_eq!(memory.rows[1].name, "SRAM 0");
    assert_eq!(memory.rows[1].bytes[0], Some(0x42));
    assert_eq!(memory.rom_check.unwrap().calculated, 0xf1);
    assert!(!memory.rom_check.unwrap().valid());
    let examples = b"Filetype: Flipper iButton key\nVersion: 1\nKey type: Dallas\nData: 08 DE AD BE EF FA CE 4E\n";
    let Document::Memory(dallas) = load("legacy.ibtn", examples) else {
        panic!("memory expected")
    };
    assert!(dallas.rom_check.unwrap().valid());
}

#[test]
fn raw_and_hex_dumps_agree_without_stealing_rfc5322_email() {
    let raw = vec![0; 1024];
    let hex = "00000000000000000000000000000000\n".repeat(64);
    assert!(recognizes(hex.as_bytes(), Path::new("fixture.eml")));
    assert!(!recognizes_with_length(
        hex.as_bytes(),
        Path::new("partial.eml"),
        hex.len() as u64 + 32,
    ));
    let email = format!("From: sender@example.test\nSubject: tag testing\n\n{hex}");
    assert!(!recognizes(email.as_bytes(), Path::new("email.eml")));
    let Document::Memory(raw) = load("fixture.mfd", &raw) else {
        panic!("memory expected")
    };
    let Document::Memory(hex) = load("fixture.eml", hex.as_bytes()) else {
        panic!("memory expected")
    };
    assert_eq!(raw.rows, hex.rows);
    assert_eq!(raw.rows[3].role, RowRole::SectorTrailer);
}

#[test]
fn trace_probe_distinguishes_can_and_exact_frames_seek_through_checkpoints() {
    let mut trace = Vec::new();
    for row in 0..1024_u32 {
        trace.extend((u32::MAX - 500).wrapping_add(row).to_le_bytes());
        trace.extend(15_u16.to_le_bytes());
        let length = if row % 2 == 0 { 2_u16 } else { 0x8002 };
        trace.extend(length.to_le_bytes());
        trace.extend([(row % 253) as u8, 0x00, 0x80]);
    }
    assert!(recognizes(&trace, Path::new("framed.trace")));
    assert!(!recognizes(
        b"date Mon Jan 1\nbase hex timestamps absolute\n0.00 1 123 Rx d 0\n",
        Path::new("can.trace")
    ));
    let mut truncated = trace.clone();
    truncated.pop();
    assert!(!recognizes(&truncated, Path::new("truncated.trace")));
    let index = build_trace_index(Cursor::new(&trace), &STOP).unwrap();
    assert_eq!(index.rows(), 1024);
    assert_eq!(index.bin_frames, 2);
    assert_eq!(index.bins.iter().map(|bin| bin.tags).sum::<u64>(), 512);
    let meta = read_frame_info(Cursor::new(&trace), &index, 500..503, &STOP).unwrap();
    let frames = read_frames(Cursor::new(&trace), &index, 500..503, &STOP).unwrap();
    assert_eq!(frames[1].info, meta[1]);
    assert_eq!(frames[1].data, [248, 0]);
    assert_eq!(frames[1].parity, [0x80]);
    assert_eq!(frames[1].info.timestamp_ticks, 0);
}

#[test]
fn graph_buffer_overview_retains_single_sample_peaks_and_exact_rows() {
    let mut signal = String::new();
    for row in 0..20_000 {
        let sample = if row == 12_345 { 127 } else { -3 };
        writeln!(signal, "{sample}").unwrap();
    }
    assert!(recognizes_with_length(
        &signal.as_bytes()[..400],
        Path::new("graph.pm3"),
        signal.len() as u64
    ));
    let Document::Signal(preview) = load("graph.pm3", signal.as_bytes()) else {
        panic!("signal expected")
    };
    assert_eq!(preview.samples.len(), 4096);
    assert!(!preview.complete);
    let index = build_signal_index(Cursor::new(&signal), &STOP).unwrap();
    assert_eq!(index.samples(), 20_000);
    assert!(index.bins.iter().any(|bin| bin.max == 127));
    let samples = read_samples(Cursor::new(&signal), &index, 12_344..12_347, &STOP).unwrap();
    assert_eq!(samples, [-3, 127, -3]);
}
