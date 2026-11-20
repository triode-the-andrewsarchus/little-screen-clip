//! IAU 2006/2000A celestial-to-terrestrial rotation with offline IERS corrections.
use anise::time::{Epoch, TimeScale};
use chrono::{DateTime, Datelike, Utc};
use erfa::{
    earth::earth_rotation_angle_00,
    prenut::{bpn_to_xy, pn_matrix_06a},
    time::S06,
    vectors_and_matrices::{init_matrix, rotate_x, rotate_z},
};

const J2000: f64 = 2_451_545.0;
const SECONDS_PER_DAY: f64 = 86_400.0;
const ARCSECOND: f64 = std::f64::consts::PI / (180.0 * 3600.0);
type Matrix = [[f64; 3]; 3];

#[derive(Clone, Copy, Debug, Default)]
struct Record {
    mjd: f64,
    // Interpolate UT1-TAI so UTC leap seconds don't get smeared across the day.
    ut1_tai: f64,
    pole: [f64; 4],
    observed: bool,
}

#[derive(Default)]
pub(super) struct Orientation(Vec<Record>);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in super::super) enum RotationSource {
    Observed,
    Predicted,
    Modeled,
}

impl Orientation {
    pub fn parse(text: &str) -> Self {
        Self(text.lines().filter_map(Record::parse).collect())
    }

    pub fn at(&self, time: DateTime<Utc>) -> (Epoch, Matrix, RotationSource) {
        let utc_days = utc_days(time);
        let date = time.date_naive();
        let year = f64::from(date.year())
            + (f64::from(date.ordinal0()) + (utc_days + 0.5).rem_euclid(1.0))
                / if date.leap_year() { 366.0 } else { 365.0 };
        let modeled = delta_t(year) / SECONDS_PER_DAY;
        let (tt, ut1, pole, source) = if time.year() < 1972 {
            // Before leap-second UTC, interpret the civil label as proleptic UT1.
            (
                utc_days + modeled,
                utc_days,
                [0.0; 4],
                RotationSource::Modeled,
            )
        } else {
            let tt = utc_days + (tai_minus_utc(time) + 32.184) / SECONDS_PER_DAY;
            match self.correction(utc_days + 51_544.5) {
                Some(record) => (
                    tt,
                    tt + (record.ut1_tai - 32.184) / SECONDS_PER_DAY,
                    record.pole,
                    if record.observed {
                        RotationSource::Observed
                    } else {
                        RotationSource::Predicted
                    },
                ),
                None => (tt, tt - modeled, [0.0; 4], RotationSource::Modeled),
            }
        };
        (
            Epoch::from_jde_in_time_scale(J2000 + tt, TimeScale::TT),
            terrestrial_matrix(tt, ut1, pole),
            source,
        )
    }

    fn correction(&self, mjd: f64) -> Option<Record> {
        let i = self
            .0
            .partition_point(|record| record.mjd <= mjd)
            .checked_sub(1)?;
        let left = self.0[i];
        if left.mjd == mjd {
            return Some(left);
        }
        let right = *self.0.get(i + 1)?;
        let days = right.mjd - left.mjd;
        if days > 1.0 || days <= 0.0 {
            return None;
        }
        let f = (mjd - left.mjd) / days;
        Some(Record {
            mjd,
            ut1_tai: left.ut1_tai + f * (right.ut1_tai - left.ut1_tai),
            pole: std::array::from_fn(|i| left.pole[i] + f * (right.pole[i] - left.pole[i])),
            observed: left.observed && right.observed,
        })
    }
}

impl Record {
    fn parse(line: &str) -> Option<Self> {
        let get = |a, b| {
            line.get(a..b)?
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
        };
        let mjd = get(7, 15)?;
        let time =
            DateTime::from_timestamp(((mjd - 40_587.0) * SECONDS_PER_DAY).round() as i64, 0)?;
        let dut1 = get(154, 165).or_else(|| get(58, 68))?;
        Some(Self {
            mjd,
            ut1_tai: dut1 - tai_minus_utc(time),
            pole: [
                get(134, 144).or_else(|| get(18, 27))? * ARCSECOND,
                get(144, 154).or_else(|| get(37, 46))? * ARCSECOND,
                get(165, 175).or_else(|| get(97, 106)).unwrap_or(0.0) * ARCSECOND / 1000.0,
                get(175, 185).or_else(|| get(116, 125)).unwrap_or(0.0) * ARCSECOND / 1000.0,
            ],
            observed: matches!(line.as_bytes().get(57), Some(b'I')),
        })
    }
}

fn utc_days(time: DateTime<Utc>) -> f64 {
    (time.timestamp() - 946_728_000) as f64 / SECONDS_PER_DAY
        + f64::from(time.timestamp_subsec_nanos()) / (SECONDS_PER_DAY * 1e9)
}

fn tai_minus_utc(time: DateTime<Utc>) -> f64 {
    // Hifitime's embedded leap-second table is shared with the SPK time conversions.
    Epoch::from_unix_seconds(time.timestamp() as f64)
        .leap_seconds(true)
        .unwrap_or(37.0)
}

fn terrestrial_matrix(tt: f64, ut1: f64, [xp, yp, dx, dy]: [f64; 4]) -> Matrix {
    let (x, y) = bpn_to_xy(pn_matrix_06a(J2000, tt));
    let (x, y) = (x + dx, y + dy);
    let s = S06(J2000, tt, x, y);
    // SOFA C2IXYS: locate the corrected celestial intermediate pole and CIO.
    let r2 = x * x + y * y;
    let e = y.atan2(x);
    let d = (r2 / (1.0 - r2)).sqrt().atan();
    let mut matrix = [[0.0; 3]; 3];
    init_matrix(&mut matrix);
    rotate_z(e, &mut matrix);
    rotate_y(d, &mut matrix);
    rotate_z(earth_rotation_angle_00(J2000, ut1) - e - s, &mut matrix);
    // Polar motion per SOFA POM00, with the IERS TIO locator s'.
    rotate_z(-47e-6 * ARCSECOND * tt / 36_525.0, &mut matrix);
    rotate_y(-xp, &mut matrix);
    rotate_x(-yp, &mut matrix);
    matrix
}

fn rotate_y(angle: f64, matrix: &mut Matrix) {
    let (s, c) = angle.sin_cos();
    let [x, _, z] = *matrix;
    matrix[0] = std::array::from_fn(|i| c * x[i] - s * z[i]);
    matrix[2] = std::array::from_fn(|i| s * x[i] + c * z[i]);
}

// NASA/Espenak-Meeus historical and extrapolated ΔT, in seconds.
// https://eclipse.gsfc.nasa.gov/SEcat5/deltatpoly.html
fn delta_t(y: f64) -> f64 {
    let (t, coefficients): (_, &[f64]) = match y {
        ..1600.0 => (
            (y - 1000.0) / 100.0,
            &[
                1574.2,
                -556.01,
                71.23472,
                0.319781,
                -0.8503463,
                -0.005050998,
                0.0083572073,
            ],
        ),
        ..1700.0 => (y - 1600.0, &[120.0, -0.9808, -0.01532, 1.0 / 7129.0]),
        ..1800.0 => (
            y - 1700.0,
            &[8.83, 0.1603, -0.0059285, 0.00013336, -1.0 / 1_174_000.0],
        ),
        ..1860.0 => (
            y - 1800.0,
            &[
                13.72,
                -0.332447,
                0.0068612,
                0.0041116,
                -0.00037436,
                0.0000121272,
                -0.0000001699,
                0.000000000875,
            ],
        ),
        ..1900.0 => (
            y - 1860.0,
            &[
                7.62,
                0.5737,
                -0.251754,
                0.01680668,
                -0.0004473624,
                1.0 / 233_174.0,
            ],
        ),
        ..1920.0 => (
            y - 1900.0,
            &[-2.79, 1.494119, -0.0598939, 0.0061966, -0.000197],
        ),
        ..1941.0 => (y - 1920.0, &[21.20, 0.84493, -0.076100, 0.0020936]),
        ..1961.0 => (y - 1950.0, &[29.07, 0.407, -1.0 / 233.0, 1.0 / 2547.0]),
        ..1986.0 => (y - 1975.0, &[45.45, 1.067, -1.0 / 260.0, -1.0 / 718.0]),
        ..2005.0 => (
            y - 2000.0,
            &[
                63.86,
                0.3345,
                -0.060374,
                0.0017275,
                0.000651814,
                0.00002373599,
            ],
        ),
        ..2050.0 => (y - 2000.0, &[62.92, 0.32217, 0.005589]),
        ..2150.0 => return -20.0 + 32.0 * ((y - 1820.0) / 100.0).powi(2) - 0.5628 * (2150.0 - y),
        _ => return -20.0 + 32.0 * ((y - 1820.0) / 100.0).powi(2),
    };
    coefficients.iter().rev().fold(0.0, |sum, c| sum * t + c)
}
