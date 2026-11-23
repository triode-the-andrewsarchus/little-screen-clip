use std::{fs::File, io::Write, sync::atomic::AtomicBool};

use flate2::Crc;

use super::super::{Capture, V1_MAGIC};

fn crc(bytes: &[u8]) -> [u8; 4] {
    let mut crc = Crc::new();
    crc.update(bytes);
    crc.sum().to_le_bytes()
}

fn tag(file: &mut Vec<u8>, tag: &[u8; 3], data: &[u8]) -> usize {
    let start = file.len();
    file.extend_from_slice(tag);
    file.push(0);
    file.extend_from_slice(&(data.len() as u32).to_le_bytes());
    file.extend_from_slice(data);
    file.resize((file.len() + 4).next_multiple_of(8) - 4, 0);
    file.extend(crc(&file[start..]));
    start
}

fn collection(file: &mut [u8], start: usize, end: usize) {
    file[start + 8..start + 16].copy_from_slice(&(end as u64).to_le_bytes());
    let checksum = crc(&file[start..start + 20]);
    file[start + 20..start + 24].copy_from_slice(&checksum);
}

fn test_recording(encoding: &str, data: &[u8], sample_count: u64) -> File {
    let mut bytes = vec![0; 32];
    let config = serde_json::json!({
        "type": "config",
        "data_format": encoding,
        "samples_per_block": 8,
        "samples_per_tlv": 8,
        "samples_per_reduction": 2,
        "sampling_frequency": 2000,
    });
    tag(&mut bytes, b"MJS", config.to_string().as_bytes());
    let outer = tag(&mut bytes, b"CLS", &[0; 12]);
    let inner = tag(&mut bytes, b"CLS", &[0; 12]);
    tag(&mut bytes, b"ABN", data);
    let inner_end = tag(&mut bytes, b"CLE", &[]);
    let outer_end = tag(&mut bytes, b"CLE", &[]);
    collection(&mut bytes, inner, inner_end);
    collection(&mut bytes, outer, outer_end);
    let footer = serde_json::json!({ "type": "footer", "size": sample_count });
    tag(&mut bytes, b"MJS", footer.to_string().as_bytes());
    tag(&mut bytes, b"END", &[]);

    let length = bytes.len() as u64;
    bytes[..16].copy_from_slice(V1_MAGIC);
    bytes[16..24].copy_from_slice(&length.to_le_bytes());
    bytes[27] = 1;
    let checksum = crc(&bytes[..28]);
    bytes[28..32].copy_from_slice(&checksum);
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(&bytes).unwrap();
    file
}

#[test]
fn v1_without_calibration_exposes_adc_values_and_missing_samples() {
    let raw = [
        100u16 << 2,
        200 << 2,
        u16::MAX,
        u16::MAX,
        300 << 2,
        400 << 2,
    ];
    let encoded = raw
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let capture = Capture::open(test_recording("raw", &encoded, 3)).unwrap();
    assert_eq!(capture.channels()[0].unit, "LSB");
    assert_eq!(capture.channels()[1].unit, "LSB");
    let samples = capture
        .read_samples(0, 0..3, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(samples[0], 100.0);
    assert!(samples[1].is_nan());
    assert_eq!(samples[2], 300.0);
    assert_eq!(
        capture.integral(0, 0..3, &AtomicBool::new(false)).unwrap(),
        None,
    );
}

#[test]
fn v1_float_arrays_respect_actual_final_chunk_length() {
    let values = [0.125f32, 0.25, 0.375, 0.5];
    let mut encoded = b"ARRY".to_vec();
    for value in [40, 24, (1 << 24) + 8, 0x0102, 16u32] {
        encoded.extend(value.to_le_bytes());
    }
    encoded.extend(values.into_iter().flat_map(f32::to_le_bytes));
    let capture = Capture::open(test_recording("float32_v2", &encoded, 4)).unwrap();
    assert_eq!(capture.channels().len(), 1);
    assert_eq!(capture.channels()[0].unit, "A");
    assert_eq!(
        capture
            .read_samples(0, 1..4, &AtomicBool::new(false))
            .unwrap(),
        [0.25, 0.375, 0.5],
    );
}
