use std::{fs::File, os::unix::fs::FileExt, sync::atomic::AtomicBool};

use super::super::{Capture, V2_MAGIC};
use super::{FSR_DATA, FSR_HEAD, FSR_INDEX, FSR_SUMMARY, SIGNAL_DEF, SOURCE_DEF, raw::checksum};

struct Fixture {
    file: File,
    offset: u64,
}

impl Fixture {
    fn new() -> Self {
        Self {
            file: tempfile::tempfile().unwrap(),
            offset: 32,
        }
    }

    fn chunk(&mut self, tag: u8, meta: u16, payload: &[u8]) -> u64 {
        let offset = self.offset;
        let mut header = [0; 32];
        header[16] = tag;
        header[18..20].copy_from_slice(&meta.to_le_bytes());
        header[20..24].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        self.chunk_header(offset, header);
        self.offset += 32;
        if !payload.is_empty() {
            self.file.write_all_at(payload, self.offset).unwrap();
            self.offset += (payload.len() as u64 + 4).next_multiple_of(8);
            self.file
                .write_all_at(&checksum(payload).to_le_bytes(), self.offset - 4)
                .unwrap();
        }
        offset
    }

    fn chunk_header(&self, offset: u64, mut header: [u8; 32]) {
        let crc = checksum(&header[..28]);
        header[28..].copy_from_slice(&crc.to_le_bytes());
        self.file.write_all_at(&header, offset).unwrap();
    }

    fn link(&self, current: u64, next: u64) {
        let mut header = [0; 32];
        self.file.read_exact_at(&mut header, current).unwrap();
        header[..8].copy_from_slice(&next.to_le_bytes());
        self.chunk_header(current, header);
    }

    fn definitions(&mut self, datatype: u32) -> u64 {
        let mut source = vec![0; 64];
        source.extend_from_slice(b"Test device\0\x1fTest vendor\0\x1fJS220\0\x1f");
        self.chunk(SOURCE_DEF, 1, &source);
        let mut signal = vec![0; 128];
        signal[..2].copy_from_slice(&1u16.to_le_bytes());
        signal[4..8].copy_from_slice(&datatype.to_le_bytes());
        signal[8..12].copy_from_slice(&1_000u32.to_le_bytes());
        signal[12..16].copy_from_slice(&16u32.to_le_bytes());
        signal[16..20].copy_from_slice(&4u32.to_le_bytes());
        signal[20..24].copy_from_slice(&8u32.to_le_bytes());
        signal[24..28].copy_from_slice(&2u32.to_le_bytes());
        signal.extend_from_slice(b"Current\0\x1fA\0\x1f");
        let def = self.chunk(SIGNAL_DEF, 1, &signal);
        let head = self.chunk(FSR_HEAD, 1, &[0; 128]);
        self.link(def, head);
        head
    }

    fn track_head(&self, head: u64, offsets: &[u64]) {
        let mut payload = [0; 128];
        for (field, offset) in payload.as_chunks_mut::<8>().0.iter_mut().zip(offsets) {
            field.copy_from_slice(&offset.to_le_bytes());
        }
        self.file.write_all_at(&payload, head + 32).unwrap();
        self.file
            .write_all_at(&checksum(&payload).to_le_bytes(), head + 32 + 132)
            .unwrap();
    }

    fn sparse_payload(&mut self, length: u32) {
        let mut header = [0; 32];
        header[16] = 0x40;
        header[20..24].copy_from_slice(&length.to_le_bytes());
        self.chunk_header(self.offset, header);
        self.offset += 32 + (u64::from(length) + 4).next_multiple_of(8);
    }

    fn table(
        &mut self,
        tag: u8,
        meta: u16,
        time: i64,
        count: u32,
        bits: u16,
        values: &[u8],
    ) -> u64 {
        let mut payload = time.to_le_bytes().to_vec();
        payload.extend_from_slice(&count.to_le_bytes());
        payload.extend_from_slice(&bits.to_le_bytes());
        payload.extend_from_slice(&[0; 2]);
        payload.extend_from_slice(values);
        self.chunk(tag, meta, &payload)
    }

    fn finish(mut self) -> File {
        self.chunk(0xff, 0, &[]);
        let mut header = [0; 32];
        header[..16].copy_from_slice(V2_MAGIC);
        header[16..24].copy_from_slice(&self.offset.to_le_bytes());
        header[27] = 1;
        let crc = checksum(&header[..28]);
        header[28..].copy_from_slice(&crc.to_le_bytes());
        self.file.write_all_at(&header, 0).unwrap();
        self.file
    }
}

fn floats(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect()
}

#[test]
fn v2_sparse_recording_reads_index_and_unindexed_short_tail() {
    let mut fixture = Fixture::new();
    let head = fixture.definitions(0x2004);
    fixture.sparse_payload(1 << 30);
    let first = fixture.table(FSR_DATA, 1, -50, 16, 32, &floats(&[1.0; 16]));
    let second = fixture.table(FSR_DATA, 1, -34, 16, 32, &floats(&[2.0; 16]));
    fixture.link(first, second);
    let offsets = [first, second]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect::<Vec<_>>();
    let index = fixture.table(FSR_INDEX, 0x1001, -50, 2, 64, &offsets);
    let stats = [1.0, 0.0, 1.0, 1.0].repeat(4);
    let mut stats = stats;
    stats.extend([2.0, 0.0, 2.0, 2.0].repeat(4));
    fixture.table(FSR_SUMMARY, 0x1001, -50, 8, 128, &floats(&stats));
    let tail = fixture.table(FSR_DATA, 1, -18, 3, 32, &floats(&[4.0, 5.0, 6.0]));
    fixture.link(second, tail);
    fixture.track_head(head, &[first, index]);

    let capture = Capture::open(fixture.finish()).unwrap();
    assert_eq!(capture.model(), "JS220");
    assert_eq!(capture.channels()[0].sample_count, 35);
    let cancel = AtomicBool::new(false);
    assert_eq!(
        capture.read_samples(0, 30..35, &cancel).unwrap(),
        vec![2.0, 2.0, 4.0, 5.0, 6.0],
    );
    let bins = capture.envelope(0, 0..35, 6, &cancel).unwrap();
    assert_eq!(bins.extrema().min, 1.0);
    assert_eq!(bins.extrema().max, 6.0);
    assert!(bins.exact);
    let integral = capture.integral(0, 2..34, &cancel).unwrap().unwrap();
    assert!((integral - 0.055).abs() < 1e-12);
    let estimate = capture
        .summary_integral(0, 2..34, &cancel)
        .unwrap()
        .unwrap();
    assert!((estimate - integral).abs() < 1e-12);

    let channel = &capture.channels()[0];
    let measured = channel.time_at(0)..channel.time_at(10);
    let request = (measured.start - 0.005)..measured.end;
    let totals = crate::engineering::waveforms::Capture::Power(capture)
        .power_totals(request, &cancel)
        .unwrap()
        .unwrap();
    let charge = totals.charge.unwrap();
    assert_eq!(charge.time, measured);
    assert!(charge.partial && !charge.approximate);
    assert!((charge.value - 0.01).abs() < 1e-12);
}

#[test]
fn v2_packed_samples_and_omitted_constant_blocks_keep_values() {
    let mut fixture = Fixture::new();
    let head = fixture.definitions(0x0103);
    let first = fixture.table(FSR_DATA, 1, 100, 16, 1, &[0b1010_1010; 2]);
    let offsets = [first, 0]
        .into_iter()
        .flat_map(u64::to_le_bytes)
        .collect::<Vec<_>>();
    let index = fixture.table(FSR_INDEX, 0x1001, 100, 2, 64, &offsets);
    let mut stats = [0.5, 0.5, 0.0, 1.0].repeat(4);
    stats.extend([1.0, 0.0, 1.0, 1.0].repeat(4));
    fixture.table(FSR_SUMMARY, 0x1001, 100, 8, 128, &floats(&stats));
    fixture.track_head(head, &[first, index]);
    let capture = Capture::open(fixture.finish()).unwrap();
    assert_eq!(capture.channels()[0].sample_count, 32);
    let cancel = AtomicBool::new(false);
    let values = capture.read_samples(0, 13..19, &cancel).unwrap();
    assert_eq!(values, [1.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    assert!(capture.read_samples(0, 0..33, &cancel).is_err());
    assert!(
        capture
            .read_samples(0, 0..1, &AtomicBool::new(true))
            .is_err()
    );
}
