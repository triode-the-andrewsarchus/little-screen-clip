//! Registration and residuals for independent sweeps of SPICE RAW results.

use super::{SpicePreviewSample, SpiceRawDocument, SpiceRawPlot, SpiceRawTrace};
use crate::{eda::read_snapshot, file_source::FileSource};
use std::sync::atomic::AtomicBool;

pub struct Selection {
    pub name: String,
    pub indices: [Option<usize>; 2],
}

pub struct Analysis {
    pub selection: Selection,
    pub signals: Vec<Selection>,
}

pub struct RawComparison {
    pub documents: [SpiceRawDocument; 2],
    pub analyses: Vec<Analysis>,
}

impl RawComparison {
    pub fn load(files: [&FileSource; 2], cancel: &AtomicBool) -> Result<Self, String> {
        let read = |file: &FileSource| -> Result<SpiceRawDocument, String> {
            let (raw, observed) = read_snapshot(file.path(), cancel, |bytes, _| {
                SpiceRawDocument::parse(bytes, cancel)
            })?;
            if &observed != file {
                return Err("Comparison source changed".into());
            }
            Ok(raw)
        };
        Ok(Self::new([read(files[0])?, read(files[1])?]))
    }

    fn new(documents: [SpiceRawDocument; 2]) -> Self {
        let plots = documents.each_ref().map(|doc| doc.plots.as_slice());
        let analyses = match_named(plots, |plot| &plot.plot_name, |a, b| a.kind == b.kind)
            .into_iter()
            .map(|selection| {
                let traces = [0, 1].map(|side| {
                    selection.indices[side]
                        .map(|index| plots[side][index].traces.as_slice())
                        .unwrap_or_default()
                });
                let signals = match_named(traces, |trace| &trace.name, |_, _| true);
                Analysis { selection, signals }
            })
            .collect();
        Self {
            documents,
            analyses,
        }
    }
}

fn match_named<T>(
    [a, b]: [&[T]; 2],
    name: impl Fn(&T) -> &str,
    compatible: impl Fn(&T, &T) -> bool,
) -> Vec<Selection> {
    let mut used = vec![false; b.len()];
    let mut choices: Vec<_> = a
        .iter()
        .enumerate()
        .map(|(left, item)| {
            let right = b.iter().enumerate().position(|(index, candidate)| {
                !used[index]
                    && name(item)
                        .trim()
                        .eq_ignore_ascii_case(name(candidate).trim())
                    && compatible(item, candidate)
            });
            if let Some(index) = right {
                used[index] = true;
            }
            Selection {
                name: name(item).to_owned(),
                indices: [Some(left), right],
            }
        })
        .collect();
    choices.extend(
        b.iter()
            .zip(used)
            .enumerate()
            .filter_map(|(index, (item, used))| {
                (!used).then(|| Selection {
                    name: name(item).to_owned(),
                    indices: [None, Some(index)],
                })
            }),
    );
    choices.sort_by_key(|choice| choice.indices.contains(&None));
    choices
}

#[derive(Clone, Copy)]
pub struct Sweep<'a> {
    pub plot: &'a SpiceRawPlot,
    pub trace: &'a SpiceRawTrace,
    pub samples: &'a [SpicePreviewSample],
}

impl Sweep<'_> {
    pub fn partial(self) -> bool {
        self.trace.samples().len() < self.plot.total_points
    }
}

pub fn compatible_axes(a: Sweep<'_>, b: Sweep<'_>) -> bool {
    let same = |a: &str, b: &str| !a.trim().is_empty() && a.trim().eq_ignore_ascii_case(b.trim());
    a.plot.kind == b.plot.kind
        && a.plot.is_complex == b.plot.is_complex
        && same(&a.plot.x_name, &b.plot.x_name)
        && same(&a.plot.x_unit, &b.plot.x_unit)
        && same(&a.trace.unit_type, &b.trace.unit_type)
}

/// Computes B minus A on the union of measured coordinates within the common sweep extent.
pub fn residual(a: Sweep<'_>, b: Sweep<'_>) -> Result<Vec<[f64; 2]>, &'static str> {
    if !compatible_axes(a, b) {
        return Err("the axes or signal units differ");
    }
    if a.plot.is_complex {
        return Err("complex waveforms have magnitude and phase overlays");
    }
    if a.partial() || b.partial() {
        return Err("the source traces contain reduced or missing samples");
    }
    let [a, b] = [ordered(a.samples)?, ordered(b.samples)?];
    let lo = a[0].x.max(b[0].x);
    let hi = a.last().unwrap().x.min(b.last().unwrap().x);
    if lo > hi {
        return Err("the sweeps have disjoint axis ranges");
    }

    let to_grid = |s: &[SpicePreviewSample]| {
        s.iter()
            .map(|point| point.x)
            .filter(|x| (lo..=hi).contains(x))
            .collect::<Vec<_>>()
    };
    let mut grid = to_grid(&a);
    grid.extend(to_grid(&b));
    grid.sort_unstable_by(f64::total_cmp);
    grid.dedup_by(|a, b| *a == *b);
    grid.into_iter()
        .map(|x| {
            let value = interpolate(&b, x) - interpolate(&a, x);
            value
                .is_finite()
                .then_some([x, value])
                .ok_or("B − A exceeds the numeric range")
        })
        .collect()
}

fn ordered(samples: &[SpicePreviewSample]) -> Result<Vec<SpicePreviewSample>, &'static str> {
    let Some(first) = samples.first() else {
        return Err("a sweep has no valid samples");
    };
    let mut ordered = samples.to_vec();
    if ordered.last().is_some_and(|last| last.x < first.x) {
        ordered.reverse();
    }
    let axis_length = ordered[ordered.len() - 1].x - ordered[0].x;
    if !axis_length.is_finite() {
        return Err("a sweep has a nonfinite axis span");
    }
    if !ordered
        .windows(2)
        .all(|pair| pair[0].x < pair[1].x && pair[0].shares_run_with(&pair[1]))
    {
        return Err("a sweep has gaps, repeated coordinates or a nonmonotonic axis");
    }
    Ok(ordered)
}

fn interpolate(sweep: &[SpicePreviewSample], x: f64) -> f64 {
    // The caller selected x from the common extent of the two ordered grids.
    let index = sweep.partition_point(|point| point.x < x);
    let b = sweep[index];
    if b.x == x {
        return b.value;
    }
    let a = sweep[index - 1];
    let weight = (x - a.x) / (b.x - a.x);
    a.value * (1.0 - weight) + b.value * weight
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(points: &[[f64; 2]]) -> SpiceRawDocument {
        raw_with_dimensions(points, "")
    }

    fn raw_with_dimensions(points: &[[f64; 2]], dimensions: &str) -> SpiceRawDocument {
        let mut text = format!(
            "Title: Compare\nPlotname: Transient Analysis\nFlags: real\nNo. Variables: 2\nNo. Points: {}\n{dimensions}Variables:\n\t0\ttime\ttime\n\t1\tV(out)\tvoltage\nValues:\n",
            points.len()
        );
        for (index, [x, y]) in points.iter().enumerate() {
            use std::fmt::Write as _;
            writeln!(text, "{index}\t{x}\n\t{y}").unwrap();
        }
        SpiceRawDocument::parse(text.as_bytes(), &AtomicBool::new(false)).unwrap()
    }

    fn sweep(document: &SpiceRawDocument, ordinal: usize) -> Sweep<'_> {
        let plot = document.primary().unwrap();
        let trace = &plot.traces[0];
        Sweep {
            plot,
            trace,
            samples: trace.sweeps().nth(ordinal).unwrap(),
        }
    }

    #[test]
    fn raw_compare_detects_different_shape_with_equal_extrema() {
        let a = raw(&[[0., 0.], [1., 1.], [2., 0.], [3., -1.], [4., 0.]]);
        let b = raw(&[[0., 0.], [1., 0.], [2., 1.], [3., -1.], [4., 0.]]);
        assert_eq!(
            residual(sweep(&a, 0), sweep(&b, 0)).unwrap(),
            [[0., 0.], [1., -1.], [2., 1.], [3., 0.], [4., 0.]]
        );
    }

    #[test]
    fn raw_compare_adaptive_grids_align_within_the_shared_domain() {
        let a = raw(&[[0., 0.], [2., 4.], [4., 8.]]);
        let b = raw(&[[5., 11.], [3., 7.], [1., 3.]]);
        assert_eq!(
            residual(sweep(&a, 0), sweep(&b, 0)).unwrap(),
            [[1., 1.], [2., 1.], [3., 1.], [4., 1.]]
        );
        let disjoint = raw(&[[6., 0.], [7., 1.]]);
        assert_eq!(
            residual(sweep(&a, 0), sweep(&disjoint, 0)),
            Err("the sweeps have disjoint axis ranges")
        );
    }

    #[test]
    fn raw_compare_matches_duplicate_names_in_source_order() {
        let [a, b] = [["V(OUT)", "V(out)", "I(R1)"], ["v(out)", "I(R2)", "v(OUT)"]];
        let names = match_named([&a, &b], |s| s, |_, _| true);
        assert_eq!(
            names.iter().map(|entry| entry.indices).collect::<Vec<_>>(),
            [
                [Some(0), Some(0)],
                [Some(1), Some(2)],
                [Some(2), None],
                [None, Some(1)]
            ]
        );
    }

    #[test]
    fn raw_compare_residual_stays_inside_one_step_and_rejects_reduced_traces() {
        let a = raw_with_dimensions(
            &[[0., 1.], [1., 2.], [0., 10.], [1., 20.]],
            "Dimensions: 2,2\n",
        );
        let b = raw_with_dimensions(
            &[[0., 1.], [1., 2.], [0., 30.], [1., 40.]],
            "Dimensions: 2,2\n",
        );
        assert_eq!(
            residual(sweep(&a, 0), sweep(&b, 0)).unwrap(),
            [[0., 0.], [1., 0.]]
        );
        assert_eq!(
            residual(sweep(&a, 1), sweep(&b, 1)).unwrap(),
            [[0., 20.], [1., 20.]]
        );
        let dense = raw(&(0..8192).map(|i| [f64::from(i), 1.0]).collect::<Vec<_>>());
        assert_eq!(
            residual(sweep(&dense, 0), sweep(&dense, 0)),
            Err("the source traces contain reduced or missing samples")
        );
    }
}
