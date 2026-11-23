use super::super::inputs::Input;
use little_clip_core::engineering::analysis::{Correlation, Value, correlate, strongest_peaks};
use std::{
    ops::Range,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_WINDOW: usize = 2048;
const MAX_SIDE: usize = 2048;

#[derive(Clone)]
pub(super) struct Request {
    pub reference: Input,
    pub target: Input,
    pub center: f64,
    pub duration: f64,
    pub search: f64,
}

pub(super) struct Report {
    pub points: Vec<Correlation>,
    pub peaks: Vec<Correlation>,
    pub step: f64,
    pub radius: usize,
    pub interval: Range<f64>,
    reference: Vec<f32>,
    target: Vec<f32>,
}

impl Report {
    pub fn change(&self, start: usize) -> f64 {
        (self.radius as f64 - start as f64) * self.step
    }

    pub fn traces(&self, start: usize) -> Option<[&[f32]; 2]> {
        let target = self
            .target
            .get(start..start.checked_add(self.reference.len())?)?;
        Some([&self.reference, target])
    }
}

pub(super) fn compare(request: &Request, cancel: &AtomicBool) -> Result<Report, String> {
    let duration = request.duration;
    let search = request.search;
    if !request.center.is_finite()
        || !duration.is_finite()
        || !search.is_finite()
        || duration <= 0.0
        || search <= 0.0
    {
        return Err("Choose a positive reference duration and search radius".into());
    }
    let analysis_rate = (MAX_WINDOW as f64 / duration).min(MAX_SIDE as f64 / search);
    let rate = [request.reference.rate(), request.target.rate()]
        .into_iter()
        .flatten()
        .fold(analysis_rate, f64::min);
    let count = (duration * rate).floor() as usize;
    let radius = (search * rate).floor() as usize;
    if count < 8 || radius == 0 || !rate.is_finite() || rate <= 0.0 {
        return Err("The reference and search radius need at least eight reference samples and one offset step".into());
    }
    let count = count.min(MAX_WINDOW);
    let radius = radius.min(MAX_SIDE);
    let step = rate.recip();
    let start = request.center - count as f64 * step * 0.5;
    let test_start = (radius as f64).mul_add(-step, start);
    let reference = sample(&request.reference, start, step, count, cancel)?;
    let target = sample(
        &request.target,
        test_start,
        step,
        count + 2 * radius,
        cancel,
    )?;
    let points = correlate(&reference, &target, cancel)?;
    let peaks = strongest_peaks(&points, 6);
    Ok(Report {
        points,
        peaks,
        step,
        radius,
        interval: start..(count as f64).mul_add(step, start),
        reference,
        target,
    })
}

fn sample(
    input: &Input,
    start: f64,
    step: f64,
    count: usize,
    cancel: &AtomicBool,
) -> Result<Vec<f32>, String> {
    let end = (count as f64).mul_add(step, start);
    if !start.is_finite() || !end.is_finite() || end <= start || start + step <= start {
        return Err("Alignment interval exceeds the time precision".into());
    }
    let data = input.read(&(start..end), cancel)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        if index % 256 == 0 && cancel.load(Ordering::Relaxed) {
            return Err("Alignment cancelled".into());
        }
        let time = (index as f64).mul_add(step, start);
        let value = match data.at(time) {
            Some(Value::Numeric(value)) => value as f32,
            Some(Value::Predicate(high)) => f32::from(u8::from(high)),
            None => f32::NAN,
        };
        values.push(value);
    }
    Ok(values)
}
