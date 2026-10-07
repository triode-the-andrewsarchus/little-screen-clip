//! Decode only small endpoint windows; Files never retains full-track PCM.
use super::{Music, MusicRange, decode_file};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use rodio::Source;
use std::{sync::mpsc, time::Duration};

pub(super) struct Snapping {
    ticket: JobTicket,
    original: MusicRange,
    receiver: mpsc::Receiver<Result<Result<MusicRange, String>, JobError>>,
}
impl Drop for Snapping {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}
impl Music {
    pub fn snap_selection(&mut self, range: MusicRange) {
        let original = range.clone();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(30), move || snap(range));
        self.snapping = Some(Snapping {
            ticket,
            original,
            receiver,
        });
    }
    pub(super) fn poll_selection(&mut self, selection: &mut Option<MusicRange>) {
        let Some(job) = &self.snapping else {
            return;
        };
        let Some(result) = crate::file_manager::poll_job(&job.receiver) else {
            return;
        };
        let applies = selection
            .as_ref()
            .is_some_and(|range| range == &job.original);
        self.snapping = None;
        if applies {
            match result {
                Ok(range) => *selection = Some(range),
                Err(error) => self.error = Some(format!("Cannot snap selection: {error}")),
            }
        }
    }
}

pub(super) fn snap(mut range: MusicRange) -> Result<MusicRange, String> {
    range.source.check()?;
    if crate::file_source::FileSource::capture(&range.path)? != range.source {
        return Err("Audio path changed while selecting".into());
    }
    if range.snapped {
        return Ok(range);
    }
    let mut decoder = decode_file(&range.path)?;
    range.start = snap_endpoint(&mut decoder, range.start)?;
    range.end = snap_endpoint(&mut decoder, range.end)?;
    range.source.check()?;
    range.snapped = true;
    Ok(range)
}

fn snap_endpoint(decoder: &mut super::AudioDecoder, time: f32) -> Result<f32, String> {
    let rate = decoder.sample_rate().get();
    let channels = decoder.channels().get() as usize;
    let frame = (time.max(0.0) * rate as f32).round() as usize;
    let first = frame.saturating_sub(500);
    let offset = first as f32 / rate as f32;
    decoder
        .try_seek(Duration::from_secs_f64(first as f64 / rate as f64))
        .map_err(|e| e.to_string())?;
    let mut left = Vec::with_capacity(1002);
    let mut right = Vec::with_capacity(1002);
    for (i, sample) in decoder.take(1002 * channels).enumerate() {
        match i % channels {
            0 => left.push(sample),
            1 => right.push(sample),
            _ => {}
        }
    }
    Ok(offset
        + little_clip_core::audio::snap_samples_to_zero_crossing(
            &left,
            (channels > 1).then_some(right.as_slice()),
            rate,
            time - offset,
            500,
        ))
}
