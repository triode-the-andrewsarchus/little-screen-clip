//! Incremental Touchstone network-parameter decoding for guarded capture previews.

mod header;
mod index;
mod stream;

pub use index::{Envelope, Index, Summary, TracePoint, summary};
pub use realfft::num_complex::Complex64;
pub use stream::Reader;

use std::{io, path::Path};
use thiserror::Error;

const MAX_PORTS: usize = 128;
const MAX_HEADER_BYTES: u64 = 256 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Failure to recognize, read, or safely decode a Touchstone sweep.
#[derive(Debug, Error)]
pub enum Error {
    /// The underlying input could not be read or repositioned.
    #[error("Cannot read Touchstone capture: {0}")]
    Io(#[from] io::Error),
    /// A required format field or network record is invalid.
    #[error("Invalid Touchstone capture: {0}")]
    Invalid(&'static str),
    /// A documented option is outside the implemented format.
    #[error("Unsupported Touchstone capture: {0}")]
    Unsupported(&'static str),
    /// A complete network record was not available before EOF.
    #[error("Touchstone capture ended inside its header or a network record")]
    Truncated,
    /// A format field or requested window exceeds a bounded allocation.
    #[error("Touchstone limit exceeded: {0}")]
    Limit(&'static str),
    /// The owning job cancelled the read.
    #[error("Touchstone read cancelled")]
    Cancelled,
}

/// Version of the on-disk network data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// Legacy option line with a global reference resistance.
    V1,
    /// Legacy option line with per-port reference resistances.
    V1_1,
    /// Version 2.0 keyword header.
    V2,
    /// Version 2.1 keyword header.
    V2_1,
}

impl Version {
    /// Returns the version label used by the specification.
    pub fn label(self) -> &'static str {
        match self {
            Self::V1 => "1.0",
            Self::V1_1 => "1.1",
            Self::V2 => "2.0",
            Self::V2_1 => "2.1",
        }
    }

    fn legacy(self) -> bool {
        matches!(self, Self::V1 | Self::V1_1)
    }
}

/// Kind of network parameter stored in the sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parameter {
    /// Scattering parameters.
    S,
    /// Impedance parameters.
    Z,
    /// Admittance parameters.
    Y,
    /// Hybrid parameters.
    H,
    /// Inverse hybrid parameters.
    G,
}

impl Parameter {
    /// Returns the letter used in parameter labels.
    pub fn label(self) -> &'static str {
        match self {
            Self::S => "S",
            Self::Z => "Z",
            Self::Y => "Y",
            Self::H => "H",
            Self::G => "G",
        }
    }
}

/// Axis of a mixed-mode matrix, using the file's one-based port numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortMode {
    /// Single-ended port.
    Single(usize),
    /// Differential excitation or response on a pair.
    Differential(usize, usize),
    /// Common-mode excitation or response on a pair.
    Common(usize, usize),
}

impl std::fmt::Display for PortMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Single(port) => write!(f, "S{port}"),
            Self::Differential(a, b) => write!(f, "D{a},{b}"),
            Self::Common(a, b) => write!(f, "C{a},{b}"),
        }
    }
}

/// Measurement metadata available before scanning the network data.
#[derive(Clone, Debug, PartialEq)]
pub struct Metadata {
    /// On-disk version.
    pub version: Version,
    /// Parameter kind.
    pub parameter: Parameter,
    /// Matrix rank.
    pub ports: usize,
    /// Resistance for each single-ended port, in ohms.
    pub reference_ohms: Vec<f64>,
    /// Frequency point count from a version 2 header, if available.
    pub declared_points: Option<u64>,
    /// Ordinal ordering of mixed-mode matrix axes, empty for single-ended data.
    pub mixed_mode: Vec<PortMode>,
    /// Optional version 2 noise record count, separate from the network sweep.
    pub declared_noise_points: Option<u64>,
}

impl Metadata {
    /// Returns whether the file uses reference-normalized network values.
    pub fn normalized(&self) -> bool {
        self.parameter == Parameter::S || self.version.legacy()
    }

    /// Formats a matrix label, with zero-based response and stimulus indices.
    pub fn parameter_label(&self, response: usize, stimulus: usize) -> Option<String> {
        if response >= self.ports || stimulus >= self.ports {
            return None;
        }
        let name = self.parameter.label();
        if self.mixed_mode.is_empty() {
            let (response, stimulus) = (response + 1, stimulus + 1);
            return Some(if self.ports < 10 {
                format!("{name}{response}{stimulus}")
            } else {
                format!("{name}({response},{stimulus})")
            });
        }
        Some(format!(
            "{name}({}, {})",
            self.mixed_mode[response], self.mixed_mode[stimulus]
        ))
    }
}

/// One exact frequency record in response-by-stimulus matrix order.
///
/// Values use rectangular coordinates and retain the file's normalization.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// Source record ordinal.
    pub index: u64,
    /// Center frequency in hertz.
    pub frequency_hz: f64,
    ports: usize,
    values: Box<[Complex64]>,
}

impl Point {
    /// Returns one matrix value, with zero-based response and stimulus indices.
    pub fn value(&self, response: usize, stimulus: usize) -> Option<Complex64> {
        (response < self.ports && stimulus < self.ports)
            .then(|| self.values[response * self.ports + stimulus])
    }

    /// Returns all matrix values with the stimulus axis varying fastest.
    pub fn values(&self) -> &[Complex64] {
        &self.values
    }
}

/// Returns the matrix rank encoded in a conventional `.sNp` filename.
pub fn extension_ports(path: &Path) -> Option<usize> {
    let extension = path.extension()?.to_str()?.as_bytes();
    if !extension.first()?.eq_ignore_ascii_case(&b's')
        || !extension.last()?.eq_ignore_ascii_case(&b'p')
    {
        return None;
    }
    let count = std::str::from_utf8(extension.get(1..extension.len().saturating_sub(1))?).ok()?;
    if !count.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    count
        .parse()
        .ok()
        .filter(|ports| (1..=MAX_PORTS).contains(ports))
}

/// Returns whether the filename warrants a bounded Touchstone content probe.
pub fn is_candidate_path(path: &Path) -> bool {
    extension_ports(path).is_some()
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
}

/// Validates a bounded prefix before routing a file to the measurements viewer.
///
/// Callers can supply up to 256 KiB. A prefix that ends before enough format
/// evidence arrives leaves the file in its original category.
pub fn recognizes(prefix: &[u8], path: &Path) -> bool {
    if !is_candidate_path(path) {
        return false;
    }
    let prefix = &prefix[..prefix.len().min(MAX_HEADER_BYTES as usize)];
    Reader::open(std::io::Cursor::new(prefix), path)
        .and_then(|mut reader| reader.next_point())
        .is_ok_and(|point| point.is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Encoding {
    RealImaginary,
    MagnitudeAngle,
    DecibelAngle,
}

impl Encoding {
    fn pair(self, first: f64, second: f64) -> Result<Complex64, Error> {
        let result = match self {
            Self::RealImaginary => Complex64::new(first, second),
            Self::MagnitudeAngle if first >= 0.0 => {
                Complex64::from_polar(first, second.to_radians())
            }
            Self::MagnitudeAngle => return Err(Error::Invalid("negative magnitude")),
            Self::DecibelAngle => {
                Complex64::from_polar(10.0f64.powf(first / 20.0), second.to_radians())
            }
        };
        if result.re.is_finite() && result.im.is_finite() {
            Ok(result)
        } else {
            Err(Error::Invalid("complex parameter is out of range"))
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Matrix {
    #[default]
    Full,
    Lower,
    Upper,
}

#[cfg(test)]
mod tests;
