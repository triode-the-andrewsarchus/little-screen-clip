use super::Document;
use crate::file_source::FileSource;
use little_clip_core::{ImageThumbnail, presentation::midi::MidiData};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const PITCHES: usize = 36;

pub(in crate::file_manager) fn load(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Arc<Document>, String> {
    let source = FileSource::capture(path)?;
    let bytes = crate::file_source::read_file_bytes(path, 8 * 1024 * 1024, true, true)
        .map_err(|e| e.to_string())?;
    check(cancelled)?;
    let data = MidiData::parse_bytes(
        &bytes,
        path.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    )?;
    if data.notes.len() > 250_000 || !data.duration_sec.is_finite() || data.duration_sec > 3600.0 {
        return Err("MIDI exceeds the compact preview limits (250,000 notes or one hour)".into());
    }
    let document = summarize(data, cancelled)?;
    source.check()?;
    check(cancelled)?;
    Ok(document)
}

fn summarize(data: MidiData, cancelled: &AtomicBool) -> Result<Arc<Document>, String> {
    let mut histogram = [0_usize; 128];
    let mut groups = BTreeSet::new();
    for (index, note) in data.notes.iter().enumerate() {
        if index % 256 == 0 {
            check(cancelled)?;
        }
        if !note.start_sec.is_finite() || !note.duration_sec.is_finite() || note.duration_sec < 0.0
        {
            return Err("MIDI contains invalid note timing".into());
        }
        histogram[usize::from(note.key)] += 1;
        groups.insert((note.track, note.channel));
    }
    if groups.len() > 1024 {
        return Err("MIDI has too many track/channel groups for a compact preview".into());
    }
    let (low, high) = pitch_window(&histogram, data.min_key, data.max_key);
    let visible: usize = histogram[usize::from(low)..=usize::from(high)].iter().sum();
    Ok(Arc::new(Document {
        cropped: data.notes.len() - visible,
        data: Arc::new(data),
        low,
        high,
        groups: groups.into_iter().collect(),
    }))
}

fn pitch_window(histogram: &[usize; 128], low: u8, high: u8) -> (u8, u8) {
    if usize::from(high - low) + 1 <= PITCHES {
        return (low.saturating_sub(1), high.saturating_add(1).min(127));
    }
    // Maximize retained notes; ties prefer the center of the score's range.
    let center = (usize::from(low) + usize::from(high)) / 2;
    let start = (0..=128 - PITCHES)
        .max_by_key(|start| {
            let count: usize = histogram[*start..*start + PITCHES].iter().sum();
            (
                count,
                std::cmp::Reverse((start + PITCHES / 2).abs_diff(center)),
            )
        })
        .unwrap_or(0);
    (start as u8, (start + PITCHES - 1) as u8)
}

pub(in crate::file_manager) fn render(
    document: &Document,
    width: u32,
    height: u32,
    colors: &[[u8; 3]],
    cancelled: &AtomicBool,
) -> Option<ImageThumbnail> {
    let width = width.clamp(1, 2048);
    let height = height.clamp(1, 128);
    let pitches = u32::from(document.high - document.low) + 1;
    // Aggregate each pitch lane before expanding into the display raster.
    // Long overlapping notes never redraw every vertical pixel repeatedly.
    let mut lanes = vec![0_u8; width as usize * pitches as usize * 4];
    let mut strength = vec![0_u8; width as usize * pitches as usize];
    let groups: BTreeMap<_, _> = document
        .groups
        .iter()
        .copied()
        .zip(colors.iter().copied())
        .collect();
    let duration = document.data.duration_sec.max(0.001);
    for (index, note) in document.data.notes.iter().enumerate() {
        if index % 256 == 0 && check(cancelled).is_err() {
            return None;
        }
        if note.key < document.low || note.key > document.high {
            continue;
        }
        let x0 = ((note.start_sec / duration * width as f32).floor() as u32).min(width - 1);
        let x1 = (((note.start_sec + note.duration_sec) / duration * width as f32).ceil() as u32)
            .clamp(x0 + 1, width);
        let pitch = u32::from(document.high - note.key);
        let color = groups
            .get(&(note.track, note.channel))
            .copied()
            .unwrap_or([160; 3]);
        for x in x0..x1 {
            let cell = (pitch * width + x) as usize;
            if note.velocity >= strength[cell] {
                strength[cell] = note.velocity;
                lanes[cell * 4..cell * 4 + 3].copy_from_slice(&color);
                lanes[cell * 4 + 3] = 255;
            }
        }
    }
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    for y in 0..height {
        let pitch = (y * pitches / height).min(pitches - 1);
        let source = (pitch * width * 4) as usize;
        let target = (y * width * 4) as usize;
        pixels[target..target + width as usize * 4]
            .copy_from_slice(&lanes[source..source + width as usize * 4]);
    }
    Some(ImageThumbnail {
        width,
        height,
        premultiplied_rgba: pixels,
    })
}
fn check(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("MIDI preview cancelled".into())
    } else {
        Ok(())
    }
}
