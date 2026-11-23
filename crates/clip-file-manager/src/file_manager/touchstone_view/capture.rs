use crate::file_source::{FileSource, ReadCheck};
use little_clip_core::engineering::touchstone::{
    self, BandFinder, BandSearch, Index, Point, Reader, Summary, ThresholdDirection, TracePoint,
    magnitude_db,
};
use std::{
    fmt::Write,
    fs::File,
    io::BufReader,
    ops::Range,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) type Trace = [usize; 2];
pub(super) type Band = [f64; 2];

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Cutoff {
    Above(f64),
    Below(f64),
    PeakDrop(f64),
}

const MAX_MATRIX_VALUES: usize = 512 * 1024;
const MAX_DETAIL: usize = 4096;
pub(super) const MAX_COPY: u64 = 16_384;

pub(super) struct Capture {
    path: PathBuf,
    revision: FileSource,
    pub check: ReadCheck,
}

impl Capture {
    pub fn open(path: &Path, rows: usize, cancel: &AtomicBool) -> Result<(Self, Summary), String> {
        let check = ReadCheck::default();
        let (input, revision) = check.open(path, None, cancel)?;
        let summary = touchstone::summary(&input, path, rows, cancel).map_err(as_string)?;
        let capture = Self {
            path: path.to_owned(),
            revision,
            check,
        };
        capture.validate(&input, cancel)?;
        Ok((capture, summary))
    }

    fn validate(&self, input: &File, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Touchstone read cancelled".into());
        }
        self.revision.check_file(input)?;
        if FileSource::capture(&self.path)? != self.revision {
            return Err("Touchstone source changed; reopen the sweep".into());
        }
        self.check.validate(cancel)
    }

    fn read<T>(
        &self,
        cancel: &AtomicBool,
        work: impl FnOnce(&mut Reader<BufReader<&File>>) -> Result<T, touchstone::Error>,
    ) -> Result<T, String> {
        let (input, _) = self.check.open(&self.path, Some(&self.revision), cancel)?;
        let mut reader = Reader::open(BufReader::new(&input), &self.path).map_err(as_string)?;
        let result = work(&mut reader).map_err(as_string)?;
        self.validate(&input, cancel)?;
        Ok(result)
    }

    pub fn index(&self, trace: Trace, cancel: &AtomicBool) -> Result<Index, String> {
        self.read(cancel, |reader| reader.build_trace_index(trace, cancel))
    }

    pub fn marker(&self, index: &Index, hz: f64, cancel: &AtomicBool) -> Result<Point, String> {
        self.read(cancel, |reader| reader.nearest(index, hz, cancel))
    }

    pub fn row(&self, index: &Index, row: u64, cancel: &AtomicBool) -> Result<Point, String> {
        self.read(cancel, |reader| {
            reader
                .read_window(index, row..row + 1, cancel)?
                .pop()
                .ok_or(touchstone::Error::Invalid("row outside sweep"))
        })
    }

    pub fn detail(
        &self,
        index: &Index,
        band: Band,
        trace: Trace,
        cancel: &AtomicBool,
    ) -> Result<Vec<TracePoint>, String> {
        self.read(cancel, |reader| {
            let rows = rows_for_band(reader, index, Some(band), 1, cancel)?;
            let ports = reader.metadata().ports;
            let limit = MAX_DETAIL.min(MAX_MATRIX_VALUES / (ports * ports)) as u64;
            if rows.end - rows.start > limit {
                return Ok(Vec::new());
            }
            let points = reader.read_window(index, rows, cancel)?;
            Ok(trace_points(&points, trace))
        })
    }

    pub fn exact_band(
        &self,
        index: &Index,
        band: Band,
        trace: Trace,
        neighbors: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<TracePoint>, String> {
        self.read(cancel, |reader| {
            let range = rows_for_band(reader, index, Some(band), neighbors as u64, cancel)?;
            if range.end - range.start > MAX_COPY {
                return Err(touchstone::Error::Limit(
                    "zoom to a band with at most 16,384 points",
                ));
            }
            let mut output = Vec::with_capacity((range.end - range.start) as usize);
            trace_window(reader, index, range, trace, cancel, |point| {
                output.push(point)
            })?;
            Ok(output)
        })
    }

    pub fn search_bands(
        &self,
        index: &Index,
        band: Option<Band>,
        trace: Trace,
        cutoff: Cutoff,
        cancel: &AtomicBool,
    ) -> Result<BandSearch, String> {
        self.read(cancel, |reader| {
            let rows = rows_for_band(reader, index, band, 0, cancel)?;
            let includes = |point: TracePoint| {
                band.is_none_or(|[first, last]| (first..=last).contains(&point.frequency_hz))
            };
            let (threshold, direction) = match cutoff {
                Cutoff::Above(db) => (db, ThresholdDirection::AtLeast),
                Cutoff::Below(db) => (db, ThresholdDirection::AtMost),
                Cutoff::PeakDrop(amount) => {
                    let mut peak = f64::NEG_INFINITY;
                    if band.is_none() && index.trace() == trace {
                        for bucket in index.overview() {
                            peak = peak.max(magnitude_db(bucket.maximum.value));
                        }
                    } else {
                        trace_window(reader, index, rows.clone(), trace, cancel, |point| {
                            if includes(point) {
                                peak = peak.max(magnitude_db(point.value));
                            }
                        })?;
                    }
                    (peak - amount, ThresholdDirection::AtLeast)
                }
            };
            let mut finder = BandFinder::new(threshold, direction).ok_or(
                touchstone::Error::Invalid("no finite cutoff for this search range"),
            )?;
            trace_window(reader, index, rows, trace, cancel, |point| {
                if includes(point) {
                    finder.push(point);
                }
            })?;
            Ok(finder.finish())
        })
    }

    pub fn copy(
        &self,
        index: &Index,
        rows: [u64; 2],
        trace: Trace,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let start = rows[0].min(rows[1]);
        let end = rows[0].max(rows[1]).saturating_add(1);
        if end - start > MAX_COPY {
            return Err("Select at most 16,384 frequency records to copy".into());
        }
        let (csv, count) = self.read(cancel, |reader| {
            let [response, stimulus] = trace;
            let label = reader
                .metadata()
                .parameter_label(response, stimulus)
                .ok_or(touchstone::Error::Invalid("matrix element outside sweep"))?;
            let mut csv = format!("frequency_hz,\"{label} real\",\"{label} imaginary\"\n");
            trace_window(reader, index, start..end, trace, cancel, |point| {
                let _ = writeln!(
                    csv,
                    "{},{},{}",
                    point.frequency_hz, point.value.re, point.value.im
                );
            })?;
            Ok((csv, end - start))
        })?;
        little_clip_core::copy_text_to_system_clipboard(&csv).map_err(as_string)?;
        Ok(format!("Copied {count} frequency records"))
    }
}

fn rows_for_band(
    reader: &mut Reader<BufReader<&File>>,
    index: &Index,
    band: Option<Band>,
    neighbors: u64,
    cancel: &AtomicBool,
) -> Result<Range<u64>, touchstone::Error> {
    let Some([lo, hi]) = band else {
        return Ok(0..index.points());
    };
    let a = reader.nearest(index, lo, cancel)?.index;
    let b = reader.nearest(index, hi, cancel)?.index;
    let first = a.min(b).saturating_sub(neighbors);
    let last = a.max(b).saturating_add(neighbors).saturating_add(1);
    Ok(first..last.min(index.points()))
}

fn trace_window(
    reader: &mut Reader<BufReader<&File>>,
    index: &Index,
    mut rows: Range<u64>,
    [response, stimulus]: Trace,
    cancel: &AtomicBool,
    mut emit: impl FnMut(TracePoint),
) -> Result<(), touchstone::Error> {
    let ports = reader.metadata().ports;
    let page = (MAX_MATRIX_VALUES / (ports * ports)).min(2048) as u64;
    while rows.start < rows.end {
        let next = (rows.start + page).min(rows.end);
        for point in reader.read_window(index, rows.start..next, cancel)? {
            let value = point
                .value(response, stimulus)
                .ok_or(touchstone::Error::Invalid("matrix element outside sweep"))?;
            emit(TracePoint {
                index: point.index,
                frequency_hz: point.frequency_hz,
                value,
            });
        }
        rows.start = next;
    }
    Ok(())
}

pub(super) fn trace_points(points: &[Point], [response, stimulus]: Trace) -> Vec<TracePoint> {
    points
        .iter()
        .filter_map(|point| {
            Some(TracePoint {
                index: point.index,
                frequency_hz: point.frequency_hz,
                value: point.value(response, stimulus)?,
            })
        })
        .collect()
}

pub(super) fn envelope_points(index: &Index) -> Vec<TracePoint> {
    let mut points = Vec::with_capacity(4 * index.overview().len());
    for bucket in index.overview() {
        let mut candidates = [bucket.first, bucket.minimum, bucket.maximum, bucket.last];
        candidates.sort_unstable_by_key(|point| point.index);
        for point in candidates {
            if points
                .last()
                .is_none_or(|last: &TracePoint| last.index != point.index)
            {
                points.push(point);
            }
        }
    }
    points
}

fn as_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}
