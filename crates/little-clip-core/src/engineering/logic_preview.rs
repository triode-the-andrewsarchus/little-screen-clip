//! Retained sources and bounded summaries for saved logic captures.

use super::logic::{Capture, Envelope, Lane};
use crate::file_source::{FileSource, ReadCheck};
use std::{
    fs::File,
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const PREVIEW_LANES: usize = 8;
const PREVIEW_COLUMNS: usize = 192;
const PREVIEW_SAMPLES: u64 = 4096;

const LANE_COLORS: [[u8; 3]; 8] = [
    [182, 191, 202],
    [238, 128, 73],
    [235, 94, 110],
    [239, 173, 68],
    [229, 211, 72],
    [96, 208, 136],
    [86, 165, 245],
    [185, 125, 230],
];

/// Stable preview ink for a logic channel in source order.
pub fn lane_rgb(index: usize) -> [u8; 3] {
    let tint = (index / LANE_COLORS.len()).min(4) as u16 * 22;
    LANE_COLORS[index % LANE_COLORS.len()]
        .map(|value| (u16::from(value) + u16::from(255 - value) * tint / 100) as u8)
}

pub struct Overview {
    pub label: &'static str,
    pub lanes: Vec<Lane>,
    pub span: Range<f64>,
    pub lanes_previewed: Vec<Envelope>,
    pub note: Option<String>,
}

impl Overview {
    pub fn sampled(&self) -> bool {
        self.lanes_previewed.iter().any(Envelope::sampled)
    }
}

pub struct Recording {
    file: File,
    source: FileSource,
    capture: Capture,
    check: ReadCheck,
}

impl Recording {
    pub fn open(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let check = ReadCheck::default();
        let (file, source) = check.open(path, None, cancel)?;
        let capture = Capture::open(file.try_clone().map_err(|error| error.to_string())?, path)?;
        source.check_file(&file)?;
        Ok(Self {
            file,
            source,
            capture,
            check,
        })
    }

    pub fn with_check(&mut self, check: ReadCheck) {
        self.check = check;
    }

    pub fn source(&self) -> &FileSource {
        &self.source
    }

    pub fn domain(&self) -> Range<f64> {
        let mut start = f64::INFINITY;
        let mut end = f64::NEG_INFINITY;
        for bounds in
            (0..self.capture.lanes().len()).filter_map(|lane| self.capture.time_bounds(lane))
        {
            start = start.min(bounds.start);
            end = end.max(bounds.end);
        }
        start..end.max(start + f64::EPSILON)
    }

    pub fn overview(&self, cancel: &AtomicBool) -> Result<Overview, String> {
        self.checked(cancel, |capture| {
            let span = self.domain();
            let lanes_previewed = (0..capture.lanes().len().min(PREVIEW_LANES))
                .map(|index| {
                    let time = capture.time_bounds(index).unwrap_or_else(|| span.clone());
                    if time.is_empty() {
                        return Ok(Envelope {
                            time,
                            bins: Vec::new(),
                        });
                    }
                    capture.summarize(index, time, PREVIEW_COLUMNS, PREVIEW_SAMPLES, cancel)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Overview {
                label: label(self.source.path()),
                lanes: capture.lanes().to_vec(),
                span,
                lanes_previewed,
                note: capture.note().map(str::to_owned),
            })
        })
    }

    pub fn refine(
        &self,
        previous: Range<f64>,
        cancel: &AtomicBool,
    ) -> Result<(bool, Option<Overview>), String> {
        let complete = self.checked(cancel, |capture| capture.refine(cancel))?;
        let updated = (self.domain() != previous)
            .then(|| self.overview(cancel))
            .transpose()?;
        Ok((complete, updated))
    }

    pub fn checked<T>(
        &self,
        cancel: &AtomicBool,
        read: impl FnOnce(&Capture) -> Result<T, String>,
    ) -> Result<T, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Logic read cancelled".into());
        }
        self.check.validate(cancel)?;
        self.source.check_file(&self.file)?;
        let value = read(&self.capture)?;
        self.source.check_file(&self.file)?;
        self.check.validate(cancel)?;
        Ok(value)
    }
}

fn label(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if extension.eq_ignore_ascii_case("sr") {
        "Sigrok"
    } else if extension.eq_ignore_ascii_case("dsl") {
        "DSView"
    } else if extension.eq_ignore_ascii_case("vcd") {
        "VCD"
    } else {
        "Saleae Logic"
    }
}
