//! Bounded power summaries and two-sided I/Q spectra.

use super::{Capture, check_cancel};
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::{
    f64::consts::TAU,
    ops::Range,
    sync::{Arc, Mutex, OnceLock, atomic::AtomicBool},
};

const MAX_FFT_SIZE: usize = 65_536;
const MAX_WATERFALL_POINTS: usize = 1 << 21;
const MAX_ROWS: usize = 2048;
const MAX_OVERVIEW_COLUMNS: usize = 1024;
const OVERVIEW_WINDOW: u64 = 256;
const OVERVIEW_WINDOWS_PER_BIN: u64 = 4;

/// Lower reporting limit for I/Q power measurements in dBFS.
pub const DBFS_FLOOR: f32 = -180.0;

/// Bounded power observations for a horizontal portion of an I/Q recording.
#[derive(Clone, Debug)]
pub struct PowerBin {
    pub samples: Range<u64>,
    pub observed_samples: u64,
    pub finite_samples: u64,
    pub minimum_dbfs: Option<f32>,
    pub mean_dbfs: Option<f32>,
    pub peak_dbfs: Option<f32>,
}

impl PowerBin {
    pub fn exact(&self) -> bool {
        self.observed_samples == self.samples.end - self.samples.start
    }
}

/// Power coverage at a bounded display resolution.
#[derive(Clone, Debug)]
pub struct PowerOverview {
    pub samples: Range<u64>,
    pub bins: Vec<PowerBin>,
}

impl PowerOverview {
    pub fn sampled(&self) -> bool {
        self.bins.iter().any(|bin| !bin.exact())
    }

    /// Collects adjacent bins with a peak above a chosen threshold.
    ///
    /// The returned ranges follow bin edges. A sampled overview can miss
    /// shorter events between its observed regions.
    pub fn bursts_above(&self, threshold_dbfs: f32) -> Vec<Range<u64>> {
        let mut bursts: Vec<Range<u64>> = Vec::new();
        if !threshold_dbfs.is_finite() {
            return bursts;
        }
        for bin in &self.bins {
            if bin.peak_dbfs.is_none_or(|peak| peak < threshold_dbfs) {
                continue;
            }
            if let Some(active) = bursts.last_mut()
                && active.end == bin.samples.start
            {
                active.end = bin.samples.end;
            } else {
                bursts.push(bin.samples.clone());
            }
        }
        bursts
    }
}

/// A frequency-shifted complex spectrum with negative frequencies first.
#[derive(Clone, Debug)]
pub struct Spectrum {
    pub samples: Range<u64>,
    pub sample_rate_hz: Option<f64>,
    /// Reported only when the window has one known center frequency.
    pub center_hz: Option<f64>,
    pub crosses_tuning: bool,
    pub finite_samples: u64,
    /// Equivalent noise bandwidth of the actual window, including invalid samples.
    pub noise_bandwidth_hz: Option<f64>,
    /// Coherent peak amplitude in dBFS relative to an I/Q magnitude of one.
    pub bins_dbfs: Vec<f32>,
}

impl Spectrum {
    /// Hertz from the tuning center, when the capture reports a sample rate.
    pub fn relative_hz_at(&self, bin: usize) -> Option<f64> {
        if bin >= self.bins_dbfs.len() {
            return None;
        }
        let rate = self.sample_rate_hz?;
        Some((bin as f64 / self.bins_dbfs.len() as f64 - 0.5) * rate)
    }

    /// Two-sided power density in dBFS per Hz when sample rate is known.
    pub fn density_dbfs_at(&self, bin: usize) -> Option<f32> {
        let power = *self.bins_dbfs.get(bin)?;
        let bandwidth = self.noise_bandwidth_hz?;
        let density = f64::from(power) - 10.0 * bandwidth.log10();
        density.is_finite().then_some(density as f32)
    }

    /// Absolute frequency where both the rate and center are known.
    pub fn frequency_hz_at(&self, bin: usize) -> Option<f64> {
        let frequency = self.center_hz? + self.relative_hz_at(bin)?;
        frequency.is_finite().then_some(frequency)
    }
}

/// Bounded spectrum windows across a selected sample interval.
#[derive(Clone, Debug)]
pub struct Waterfall {
    pub samples: Range<u64>,
    pub rows: Vec<Spectrum>,
    /// Some samples lie between spectrum windows.
    pub sampled: bool,
}

impl Capture {
    /// Summarizes up to four distributed contiguous windows in each column.
    ///
    /// A column is marked exact when all its samples were observed.
    /// # Errors
    /// Returns an error for an empty or out-of-range window, cancellation, or I/O failure.
    pub fn overview(
        &self,
        samples: Range<u64>,
        columns: usize,
        cancel: &AtomicBool,
    ) -> Result<PowerOverview, String> {
        self.validate_range(&samples)?;
        let count = samples.end - samples.start;
        if count == 0 {
            return Err("I/Q power overview requires a nonempty sample window.".into());
        }
        let columns = (columns.clamp(1, MAX_OVERVIEW_COLUMNS) as u64).min(count);
        let mut bins = Vec::with_capacity(columns as usize);
        for index in 0..columns {
            check_cancel(cancel)?;
            let range = boundary(&samples, index, columns)..boundary(&samples, index + 1, columns);
            bins.push(self.power_bin(range, cancel)?);
        }
        Ok(PowerOverview { samples, bins })
    }

    fn power_bin(&self, samples: Range<u64>, cancel: &AtomicBool) -> Result<PowerBin, String> {
        let count = samples.end - samples.start;
        let mut power = Powers::default();
        if count <= OVERVIEW_WINDOW * OVERVIEW_WINDOWS_PER_BIN {
            power.add(&self.read_samples(samples.clone(), cancel)?);
        } else {
            for index in 0..OVERVIEW_WINDOWS_PER_BIN {
                let range = boundary(&samples, index, OVERVIEW_WINDOWS_PER_BIN)
                    ..boundary(&samples, index + 1, OVERVIEW_WINDOWS_PER_BIN);
                let start = range.start + (range.end - range.start - OVERVIEW_WINDOW) / 2;
                power.add(&self.read_samples(start..start + OVERVIEW_WINDOW, cancel)?);
            }
        }
        Ok(power.bin(samples))
    }

    /// Computes a Hann-weighted spectrum from one contiguous sample window.
    ///
    /// Missing frequencies remain relative to the tuning center when known.
    /// # Errors
    /// Returns an error for a window outside 2–65536 samples or an unavailable read.
    pub fn spectrum(&self, samples: Range<u64>, cancel: &AtomicBool) -> Result<Spectrum, String> {
        self.validate_range(&samples)?;
        let count = samples.end - samples.start;
        if !(2..=MAX_FFT_SIZE as u64).contains(&count) {
            return Err("I/Q spectrum needs 2–65536 contiguous samples.".into());
        }
        let mut work = Transform::new((count as usize).next_power_of_two(), count as usize)?;
        self.spectral_window(samples, &mut work, cancel)
    }

    /// Computes display rows from bounded evenly-spaced windows.
    ///
    /// # Errors
    /// Returns an error for an invalid sample interval, FFT size, or read failure.
    pub fn waterfall(
        &self,
        samples: Range<u64>,
        fft_size: usize,
        rows: usize,
        cancel: &AtomicBool,
    ) -> Result<Waterfall, String> {
        self.validate_range(&samples)?;
        let count = samples.end - samples.start;
        if !fft_size.is_power_of_two() || !(2..=MAX_FFT_SIZE).contains(&fft_size) || count < 2 {
            return Err(
                "I/Q waterfall needs a nonempty window and a power-of-two FFT size.".into(),
            );
        }
        let width = (fft_size as u64).min(count);
        let max_rows = (MAX_WATERFALL_POINTS / fft_size).min(MAX_ROWS);
        let rows = (rows.clamp(1, max_rows) as u64).min(count - width + 1);
        let mut frames = Vec::with_capacity(rows as usize);
        let mut work = Transform::new(fft_size, width as usize)?;
        for row in 0..rows {
            check_cancel(cancel)?;
            let available = count - width;
            let stride = if rows == 1 {
                available / 2
            } else {
                fraction(available, row, rows - 1)
            };
            let start = samples.start + stride;
            frames.push(self.spectral_window(start..start + width, &mut work, cancel)?);
        }
        let sampled =
            rows * width < count || (rows > 1 && (count - width).div_ceil(rows - 1) > width);
        Ok(Waterfall {
            samples,
            rows: frames,
            sampled,
        })
    }

    fn spectral_window(
        &self,
        range: Range<u64>,
        transform: &mut Transform,
        cancel: &AtomicBool,
    ) -> Result<Spectrum, String> {
        let samples = self.read_samples(range.clone(), cancel)?;
        let (bins_dbfs, finite_samples, relative_bandwidth) = transform.spectrum(&samples);
        let noise_bandwidth_hz = self
            .metadata
            .sample_rate_hz
            .zip(relative_bandwidth)
            .map(|(rate, ratio)| rate * ratio)
            .filter(|bandwidth| bandwidth.is_finite() && *bandwidth > 0.0);
        check_cancel(cancel)?;
        let center = self
            .tuning_at(range.start)
            .and_then(|tuning| tuning.center_hz);
        let next = self
            .metadata
            .tunings
            .partition_point(|tuning| tuning.start_sample <= range.start);
        let retunes = self.metadata.tunings[next..]
            .iter()
            .take_while(|tuning| tuning.start_sample < range.end);
        let crosses_tuning = retunes.clone().next().is_some();
        let center_hz = retunes
            .into_iter()
            .all(|tuning| tuning.center_hz == center)
            .then_some(center)
            .flatten();
        Ok(Spectrum {
            samples: range,
            sample_rate_hz: self.metadata.sample_rate_hz,
            center_hz,
            crosses_tuning,
            finite_samples,
            noise_bandwidth_hz,
            bins_dbfs,
        })
    }
}

#[derive(Default)]
struct Powers {
    observed: u64,
    finite: u64,
    minimum: Option<f64>,
    peak: f64,
    sum: f64,
}

impl Powers {
    fn add(&mut self, samples: &[Complex32]) {
        self.observed += samples.len() as u64;
        for sample in samples
            .iter()
            .filter(|sample| sample.re.is_finite() && sample.im.is_finite())
        {
            let re = f64::from(sample.re);
            let im = f64::from(sample.im);
            let power = re * re + im * im;
            self.finite += 1;
            self.minimum = Some(self.minimum.map_or(power, |previous| previous.min(power)));
            self.peak = self.peak.max(power);
            self.sum += power;
        }
    }

    fn bin(&self, samples: Range<u64>) -> PowerBin {
        PowerBin {
            samples,
            observed_samples: self.observed,
            finite_samples: self.finite,
            minimum_dbfs: self.minimum.map(dbfs),
            mean_dbfs: (self.finite != 0).then(|| dbfs(self.sum / self.finite as f64)),
            peak_dbfs: (self.finite != 0).then(|| dbfs(self.peak)),
        }
    }
}

struct Transform {
    fft: Arc<dyn Fft<f32>>,
    values: Vec<Complex32>,
    weights: Vec<f32>,
    scratch: Vec<Complex32>,
}

impl Transform {
    fn new(fft_size: usize, input_len: usize) -> Result<Self, String> {
        static PLANNER: OnceLock<Mutex<FftPlanner<f32>>> = OnceLock::new();
        let fft = PLANNER
            .get_or_init(|| Mutex::new(FftPlanner::new()))
            .lock()
            .map_err(|_| "I/Q FFT planner is unavailable.")?
            .plan_fft_forward(fft_size);
        let weights = (0..input_len)
            .map(|index| (0.5 - 0.5 * (TAU * index as f64 / input_len as f64).cos()) as f32)
            .collect();
        let values = vec![Complex32::ZERO; fft_size];
        let scratch = vec![Complex32::ZERO; fft.get_inplace_scratch_len()];
        Ok(Self {
            fft,
            values,
            weights,
            scratch,
        })
    }

    fn spectrum(&mut self, samples: &[Complex32]) -> (Vec<f32>, u64, Option<f64>) {
        self.values.fill(Complex32::ZERO);
        let mut gain = 0.0_f64;
        let mut energy_gain = 0.0_f64;
        let mut finite = 0;
        let norm = f64::from(
            samples
                .iter()
                .filter(|sample| sample.re.is_finite() && sample.im.is_finite())
                .fold(f32::MIN_POSITIVE, |peak, sample| {
                    peak.max(sample.re.abs()).max(sample.im.abs())
                }),
        );
        for ((value, sample), weight) in self.values.iter_mut().zip(samples).zip(&self.weights) {
            if sample.re.is_finite() && sample.im.is_finite() {
                let scale = f64::from(*weight) / norm;
                *value = Complex32::new(
                    (f64::from(sample.re) * scale) as f32,
                    (f64::from(sample.im) * scale) as f32,
                );
                gain += f64::from(*weight);
                energy_gain += f64::from(*weight).powi(2);
                finite += 1;
            }
        }
        if gain == 0.0 {
            return (vec![f32::NAN; self.values.len()], finite, None);
        }
        self.fft
            .process_with_scratch(&mut self.values, &mut self.scratch);
        let scale = (norm / gain).powi(2);
        let split = self.values.len() / 2;
        let bins = self.values[split..]
            .iter()
            .chain(&self.values[..split])
            .map(|sample| dbfs(f64::from(sample.norm_sqr()) * scale))
            .collect();
        (bins, finite, Some(energy_gain / gain.powi(2)))
    }
}

fn dbfs(power: f64) -> f32 {
    ((10.0 * power.log10()) as f32).max(DBFS_FLOOR)
}

fn boundary(range: &Range<u64>, index: u64, count: u64) -> u64 {
    range.start + fraction(range.end - range.start, index, count)
}

fn fraction(length: u64, index: u64, count: u64) -> u64 {
    (u128::from(length) * u128::from(index) / u128::from(count)) as u64
}
