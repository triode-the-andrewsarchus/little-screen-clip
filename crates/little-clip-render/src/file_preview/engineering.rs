//! Shared guarded preparation for the engineering cards.

use super::{Iq, Preview};
use little_clip_core::{
    engineering::{
        can, flowgraph, iq, logic_preview, packets, pulses, radio, tags, touchstone, waveforms,
    },
    file_format::Format,
    file_source::ReadCheck,
};
use std::{fs::File, io::BufReader, path::Path, sync::atomic::AtomicBool};

pub(super) fn load(path: &Path, format: Format, cancel: &AtomicBool) -> Option<Preview> {
    match format {
        Format::Oscilloscope | Format::PowerRecording => {
            let recording = waveforms::Recording::open(path, ReadCheck::default(), cancel).ok()?;
            recording.overview(cancel).ok().map(Preview::Scope)
        }
        Format::LogicCapture => {
            let recording = logic_preview::Recording::open(path, cancel).ok()?;
            recording.overview(cancel).ok().map(Preview::Logic)
        }
        Format::Touchstone => read(path, cancel, |file| {
            touchstone::summary(file, path, 800, cancel).ok()
        })
        .map(Preview::Touchstone),
        Format::PacketCapture => read(path, cancel, |file| {
            packets::summary(BufReader::new(file), cancel).ok()
        })
        .map(Preview::Packets),
        Format::CanCapture => read(path, cancel, |file| {
            can::Capture::open(file.try_clone().ok()?)
                .ok()?
                .preview(cancel)
                .ok()
        })
        .map(Preview::Can),
        Format::PulseSignals => pulses::Capture::preview(path, cancel)
            .ok()
            .map(Preview::Pulses),
        Format::TagCapture | Format::OneWire => read(path, cancel, |file| {
            tags::preview(path, BufReader::new(file), cancel).ok()
        })
        .map(Preview::Tags),
        Format::RadioProgramming => radio::preview(path, cancel).ok().map(Preview::Radio),
        Format::Flowgraph => read(path, cancel, |file| {
            flowgraph::preview(file.try_clone().ok()?, cancel).ok()
        })
        .map(Preview::Flowgraph),
        Format::IqCapture => load_iq(path, cancel).map(Preview::Iq),
        _ => None,
    }
}

fn read<T>(
    path: &Path,
    cancel: &AtomicBool,
    prepare: impl FnOnce(&File) -> Option<T>,
) -> Option<T> {
    let (file, source) = ReadCheck::default().open(path, None, cancel).ok()?;
    let prepared = prepare(&file)?;
    source.check_file(&file).ok()?;
    Some(prepared)
}

fn load_iq(path: &Path, cancel: &AtomicBool) -> Option<Iq> {
    let recording = iq::Recording::open_sigmf(path, None, ReadCheck::default(), cancel).ok()?;
    recording
        .checked(cancel, |capture| {
            Ok(Iq {
                power: capture.overview(0..capture.sample_count(), 192, cancel)?,
                rate: capture.metadata().sample_rate_hz,
                center: capture.tuning_at(0).and_then(|tuning| tuning.center_hz),
                duration: capture.duration_seconds(),
            })
        })
        .ok()
}
