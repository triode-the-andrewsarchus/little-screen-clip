//! Sweep boundaries in source sample order.

use super::{MAX_WAVEFORM_POINTS, RawAnalysisKind};
use crate::eda::check_cancelled;
use std::cmp::Ordering;
use std::ops::Range;
use std::sync::atomic::AtomicBool;

// Reserve space for endpoints and magnitude/phase extrema of each sweep.
pub(super) const MAX_SWEEPS: usize = MAX_WAVEFORM_POINTS / 6;

pub(super) fn source_sweeps(
    axis: impl ExactSizeIterator<Item = f64>,
    dimensions: Option<&[usize]>,
    kind: RawAnalysisKind,
    stepped: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<Range<usize>>, String> {
    let len = axis.len();
    if len == 0 {
        return Ok(Vec::new());
    }
    if let Some(width) = dimensions.and_then(|dims| inner_sweep_width(dims, len)) {
        check_sweep_limit(len / width)?;
        return Ok((0..len)
            .step_by(width)
            .map(|start| start..start + width)
            .collect());
    }
    if !stepped
        && matches!(
            kind,
            RawAnalysisKind::DcOperatingPoint | RawAnalysisKind::Other
        )
    {
        return Ok(std::iter::once(0..len).collect());
    }
    infer_sweeps(axis, stepped, cancelled)
}

fn inner_sweep_width(dims: &[usize], samples: usize) -> Option<usize> {
    let (&width, outer) = dims.split_last()?;
    if outer.is_empty() || width == 0 {
        return None;
    }
    let product = dims.iter().try_fold(1usize, |n, &dim| n.checked_mul(dim))?;
    (product == samples).then_some(width)
}

fn check_sweep_limit(count: usize) -> Result<(), String> {
    if count > MAX_SWEEPS {
        return Err(format!(
            "SPICE waveform exceeds the preview limit of {MAX_SWEEPS} sweeps"
        ));
    }
    Ok(())
}

fn infer_sweeps(
    mut axis: impl ExactSizeIterator<Item = f64>,
    stepped: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<Range<usize>>, String> {
    let len = axis.len();
    let mut first_index = 0_usize;
    let origin = loop {
        if first_index.is_multiple_of(1024) {
            check_cancelled(cancelled, "SPICE sweep analysis cancelled")?;
        }
        match axis.next() {
            Some(x) if x.is_finite() => break x,
            Some(_) => first_index += 1,
            None => return Ok(std::iter::once(0..len).collect()),
        }
    };
    let mut sweeps = Vec::new();
    let mut start = 0;
    let mut traversal = Traversal::new(origin);
    for (index, x) in (first_index + 1..).zip(axis) {
        if index.is_multiple_of(1024) {
            check_cancelled(cancelled, "SPICE sweep analysis cancelled")?;
        }
        if let Some(boundary) = traversal.observe(index, x, stepped) {
            check_sweep_limit(sweeps.len() + 2)?;
            sweeps.push(start..boundary);
            start = boundary;
        }
    }
    if let Some(boundary) = traversal.reset.filter(|_| stepped) {
        check_sweep_limit(sweeps.len() + 2)?;
        sweeps.push(start..boundary);
        start = boundary;
    }
    sweeps.push(start..len);
    Ok(sweeps)
}

struct Traversal {
    origin: f64,
    previous: f64,
    direction: Ordering,
    monotonic: bool,
    reset: Option<usize>,
}

impl Traversal {
    fn new(origin: f64) -> Self {
        Self {
            origin,
            previous: origin,
            direction: Ordering::Equal,
            monotonic: true,
            reset: None,
        }
    }

    fn observe(&mut self, index: usize, x: f64, stepped: bool) -> Option<usize> {
        if !x.is_finite() {
            self.reset = None;
            self.monotonic = false;
            return None;
        }
        let previous = std::mem::replace(&mut self.previous, x);
        if x == self.origin {
            if previous != self.origin && (stepped || self.monotonic) {
                self.reset = Some(index);
            }
            return None;
        }

        // Without a step flag, a reset needs both a monotone traversal before
        // the return and a subsequent traversal in the same direction.
        let side = if x < self.origin {
            Ordering::Less
        } else {
            Ordering::Greater
        };
        let boundary = self
            .reset
            .take()
            .filter(|_| stepped || side == self.direction);
        if boundary.is_some() || self.direction == Ordering::Equal {
            self.direction = side;
            self.monotonic = true;
        } else if x != previous {
            self.monotonic &= (x > previous) == (self.direction == Ordering::Greater);
        }
        boundary
    }
}
