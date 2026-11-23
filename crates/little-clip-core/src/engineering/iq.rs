//! Read-only I/Q capture files, SigMF sidecars, and bounded RF views.
mod metadata;
mod samples;
mod spectra;

pub use samples::Encoding;
pub use spectra::{PowerBin, PowerOverview, Spectrum, Waterfall};

use crate::file_source::{FileSource, ReadCheck};
use rustfft::num_complex::Complex32;
use std::{
    fs::File,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Largest contiguous read in complex samples.
pub const MAX_SAMPLES_PER_READ: u64 = 1 << 20;

/// Explicit interpretation of a headerless I/Q file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawInterpretation {
    pub encoding: Encoding,
    pub sample_rate_hz: Option<f64>,
    pub center_hz: Option<f64>,
}

/// Capture metadata scoped to the segment beginning at `start_sample`.
#[derive(Clone, Debug, PartialEq)]
pub struct Tuning {
    /// Index relative to this file's first sample.
    pub start_sample: u64,
    pub center_hz: Option<f64>,
    pub datetime: Option<String>,
    /// Original stream index, when the recorder reported one.
    pub original_sample: Option<u64>,
}

/// A user or recorder annotation associated with samples and optionally frequency.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub samples: Range<u64>,
    pub frequency_hz: Option<Range<f64>>,
    pub label: Option<String>,
    pub comment: Option<String>,
}

/// Metadata for a single saved complex stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    pub encoding: Encoding,
    pub sample_rate_hz: Option<f64>,
    /// Sample index of the start of the dataset in the original recording.
    pub sample_offset: u64,
    pub hardware: Option<String>,
    pub description: Option<String>,
    pub tunings: Vec<Tuning>,
    pub annotations: Vec<Annotation>,
}

/// A sample stream backed by one retained, already guarded file.
pub struct Capture {
    file: File,
    metadata: Metadata,
    offsets: Vec<(u64, u64)>,
    count: u64,
}

impl Capture {
    /// Opens an explicitly interpreted, guarded raw I/Q handle.
    ///
    /// # Errors
    /// Returns an error for an invalid interpretation or partial sample frame.
    pub fn open_raw(file: File, raw: RawInterpretation) -> Result<Self, String> {
        validate_rate(raw.sample_rate_hz)?;
        validate_frequency(raw.center_hz)?;
        let metadata = Metadata {
            encoding: raw.encoding,
            sample_rate_hz: raw.sample_rate_hz,
            sample_offset: 0,
            hardware: None,
            description: None,
            tunings: vec![Tuning {
                start_sample: 0,
                center_hz: raw.center_hz,
                datetime: None,
                original_sample: None,
            }],
            annotations: Vec::new(),
        };
        Self::from_parts(file, metadata, Vec::new(), 0, Vec::new())
    }

    fn from_parts(
        file: File,
        mut metadata: Metadata,
        header_offsets: Vec<(u64, u64)>,
        trailing_bytes: u64,
        annotations: Vec<metadata::AnnotationInput>,
    ) -> Result<Self, String> {
        let file_metadata = file.metadata().map_err(io_error)?;
        if !file_metadata.is_file() {
            return Err("An I/Q capture must be a regular file.".into());
        }
        let headers = header_offsets.last().map_or(0, |&(_, total)| total);
        let bytes = file_metadata
            .len()
            .checked_sub(headers)
            .and_then(|size| size.checked_sub(trailing_bytes))
            .ok_or("I/Q capture headers extend beyond the file.")?;
        let stride = metadata.encoding.frame_bytes() as u64;
        if bytes < stride || !bytes.is_multiple_of(stride) {
            return Err("I/Q capture has no samples or ends in a partial sample.".into());
        }
        let count = bytes / stride;
        if metadata
            .sample_rate_hz
            .is_some_and(|rate| !(count as f64 / rate).is_finite())
        {
            return Err("I/Q capture duration exceeds the supported time range.".into());
        }
        if metadata
            .tunings
            .last()
            .is_some_and(|tuning| tuning.start_sample >= count)
            || metadata.sample_offset.checked_add(count).is_none()
        {
            return Err("I/Q metadata points outside the captured samples.".into());
        }
        metadata.annotations = metadata::annotations(annotations, &metadata.tunings, count)?;
        Ok(Self {
            file,
            metadata,
            offsets: header_offsets,
            count,
        })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn sample_count(&self) -> u64 {
        self.count
    }

    pub fn duration_seconds(&self) -> Option<f64> {
        self.metadata
            .sample_rate_hz
            .map(|rate| self.count as f64 / rate)
    }

    pub fn tuning_at(&self, sample: u64) -> Option<&Tuning> {
        if sample >= self.count {
            return None;
        }
        let prior = self
            .metadata
            .tunings
            .partition_point(|tuning| tuning.start_sample <= sample);
        prior
            .checked_sub(1)
            .map(|index| &self.metadata.tunings[index])
    }

    /// Reads normalized interleaved I/Q pairs in a half-open sample interval.
    ///
    /// The request uses dataset-relative indices. Non-finite float values
    /// remain invalid samples and are excluded from signal summaries.
    /// # Errors
    /// Returns an error for invalid bounds, cancellation, or read failure.
    pub fn read_samples(
        &self,
        range: Range<u64>,
        cancel: &AtomicBool,
    ) -> Result<Vec<Complex32>, String> {
        self.validate_range(&range)?;
        let count = range.end - range.start;
        if count > MAX_SAMPLES_PER_READ {
            return Err("I/Q sample request exceeds the range-read limit.".into());
        }
        check_cancel(cancel)?;
        let mut output = Vec::with_capacity(count as usize);
        let mut sample = range.start;
        let stride = self.metadata.encoding.frame_bytes() as u64;
        let mut next = self.offsets.partition_point(|&(start, _)| start <= sample);
        while sample < range.end {
            let skipped = next.checked_sub(1).map_or(0, |index| self.offsets[index].1);
            let end = self
                .offsets
                .get(next)
                .map_or(range.end, |&(start, _)| range.end.min(start));
            samples::read(
                &self.file,
                self.metadata.encoding,
                sample * stride + skipped,
                end - sample,
                &mut output,
                cancel,
            )?;
            sample = end;
            next += 1;
        }
        Ok(output)
    }

    fn validate_range(&self, range: &Range<u64>) -> Result<(), String> {
        if range.start > range.end || range.end > self.count {
            Err("I/Q sample window falls outside the recording.".into())
        } else {
            Ok(())
        }
    }
}

/// Guarded I/Q recording with both sidecar and signal-data observations.
pub struct Recording {
    capture: Capture,
    data: FileSource,
    metadata: Option<(File, FileSource)>,
    selected_metadata: bool,
    check: ReadCheck,
}

impl Recording {
    /// Opens a SigMF metadata file or its appropriately named dataset sibling.
    ///
    /// # Errors
    /// Returns an error for missing, malformed, stale, or unsupported sources.
    pub fn open_sigmf(
        path: &Path,
        expected: Option<&FileSource>,
        check: ReadCheck,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let selected_metadata = has_extension(path, "sigmf-meta");
        if !selected_metadata && !has_extension(path, "sigmf-data") {
            return Err("Select a SigMF metadata or data file.".into());
        }
        let meta_path = if selected_metadata {
            path.to_path_buf()
        } else {
            path.with_extension("sigmf-meta")
        };
        let (meta_file, meta_source) =
            check.open(&meta_path, expected.filter(|_| selected_metadata), cancel)?;
        let descriptor = metadata::Descriptor::read(&meta_file, cancel)?;
        let data_path = descriptor.dataset.as_ref().map_or_else(
            || meta_path.with_extension("sigmf-data"),
            |name| meta_path.with_file_name(name),
        );
        let data_path = paired_dataset(
            path,
            selected_metadata,
            data_path,
            descriptor.dataset.is_some(),
        )?;
        let (data_file, data) =
            check.open(&data_path, expected.filter(|_| !selected_metadata), cancel)?;
        let capture = Self {
            capture: descriptor.attach(data_file)?,
            data,
            metadata: Some((meta_file, meta_source)),
            selected_metadata,
            check,
        };
        capture.validate(cancel)?;
        Ok(capture)
    }

    /// Opens a guarded raw I/Q stream the user has already interpreted.
    ///
    /// # Errors
    /// Returns an error for a stale file, invalid interpretation, or I/O failure.
    pub fn open_raw(
        path: &Path,
        raw: RawInterpretation,
        expected: Option<&FileSource>,
        check: ReadCheck,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let (file, data) = check.open(path, expected, cancel)?;
        let capture = Self {
            capture: Capture::open_raw(file, raw)?,
            data,
            metadata: None,
            selected_metadata: false,
            check,
        };
        capture.validate(cancel)?;
        Ok(capture)
    }

    pub fn source(&self) -> &FileSource {
        if self.selected_metadata
            && let Some((_, source)) = &self.metadata
        {
            return source;
        }
        &self.data
    }

    pub fn set_read_check(&mut self, check: ReadCheck) {
        self.check = check;
    }

    pub fn capture(&self) -> &Capture {
        &self.capture
    }

    /// Cache revision covering the retained dataset and metadata observations.
    pub fn revision_key(&self) -> Vec<u8> {
        let mut revision = self.data.revision_key();
        if let Some((_, source)) = &self.metadata {
            revision.extend(source.revision_key());
        } else {
            let metadata = self.capture.metadata();
            revision.extend(metadata.encoding.name().as_bytes());
            revision.extend(metadata.sample_rate_hz.unwrap_or(f64::NAN).to_le_bytes());
            revision.extend(
                self.capture
                    .tuning_at(0)
                    .and_then(|tuning| tuning.center_hz)
                    .unwrap_or(f64::NAN)
                    .to_le_bytes(),
            );
        }
        revision
    }

    /// Performs a deferred read with both source observations retained.
    pub fn checked<T>(
        &self,
        cancel: &AtomicBool,
        read: impl FnOnce(&Capture) -> Result<T, String>,
    ) -> Result<T, String> {
        self.validate(cancel)?;
        let result = read(&self.capture)?;
        self.validate(cancel)?;
        Ok(result)
    }

    fn validate(&self, cancel: &AtomicBool) -> Result<(), String> {
        check_cancel(cancel)?;
        self.check.validate(cancel)?;
        if let Some((file, source)) = &self.metadata {
            source.check_file(file)?;
        }
        self.data.check_file(&self.capture.file)?;
        self.check.validate(cancel)
    }
}

fn paired_dataset(
    path: &Path,
    selected_metadata: bool,
    sibling: PathBuf,
    explicit_name: bool,
) -> Result<PathBuf, String> {
    if selected_metadata {
        return Ok(sibling);
    }
    if explicit_name && path.file_name() != sibling.file_name() {
        return Err("SigMF metadata names a different data file.".into());
    }
    Ok(path.to_path_buf())
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

/// Filename hint for the two SigMF recording members.
pub fn is_sigmf_path(path: &Path) -> bool {
    has_extension(path, "sigmf-meta") || has_extension(path, "sigmf-data")
}

fn validate_rate(rate: Option<f64>) -> Result<(), String> {
    if rate.is_some_and(|rate| {
        !rate.is_finite() || rate < f64::MIN_POSITIVE || !rate.recip().is_finite()
    }) {
        return Err("I/Q sample rate must be a finite positive value.".into());
    }
    Ok(())
}

fn validate_frequency(frequency: Option<f64>) -> Result<(), String> {
    if frequency.is_some_and(|value| !value.is_finite()) {
        return Err("I/Q center frequency must be finite.".into());
    }
    Ok(())
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("I/Q capture read cancelled.".into())
    } else {
        Ok(())
    }
}

fn io_error(error: std::io::Error) -> String {
    format!("Reading I/Q capture. {error}")
}

#[cfg(test)]
mod tests;
