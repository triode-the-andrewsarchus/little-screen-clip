//! Bounded ICC source LUT execution (ICC.1:2022, types lut8/lut16/lutAToB).
//! The program retains one fallibly copied profile; parsing and execution use
//! fixed-size channel/curve/corner arrays. No expanded CLUT, baked grid, curve
//! tables, dynamic stage list or per-pixel allocation is constructed.
use super::{try_copy, ResolveError};
use crate::portable_math::powf;

type Result<T> = std::result::Result<T, ResolveError>;
#[derive(Clone, Copy)]
enum Curve {
    Identity,
    Table {
        offset: usize,
        count: usize,
        bytes: usize,
    },
    Param {
        kind: u16,
        values: [f64; 7],
    },
}
#[derive(Clone, Copy)]
struct Grid {
    offset: usize,
    dims: [usize; 4],
    inputs: usize,
    bytes: usize,
}
pub(super) struct Lut {
    data: Vec<u8>,
    inputs: usize,
    lab: bool,
    lab_v2: bool,
    a: [Curve; 4],
    b: [Curve; 4],
    m: [Curve; 4],
    grid: Option<Grid>,
    matrix: Option<[[f64; 4]; 3]>,
}
fn unsupported<T>() -> Result<T> {
    Err(ResolveError::Unsupported)
}
fn span(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    data.get(start..start.checked_add(len).ok_or(ResolveError::Unsupported)?)
        .ok_or(ResolveError::Unsupported)
}
fn word(data: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_be_bytes(
        span(data, at, 4)?
            .try_into()
            .map_err(|_| ResolveError::Unsupported)?,
    ) as usize)
}
pub(super) fn has_tag(profile: &[u8], name: &[u8; 4]) -> Result<bool> {
    let count = word(profile, 128)?;
    Ok(span(
        profile,
        132,
        count.checked_mul(12).ok_or(ResolveError::Unsupported)?,
    )?
    .as_chunks::<12>()
    .0
    .iter()
    .any(|t| &t[..4] == name))
}
fn short(data: &[u8], at: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(
        span(data, at, 2)?
            .try_into()
            .map_err(|_| ResolveError::Unsupported)?,
    ))
}
fn fixed(data: &[u8], at: usize) -> Result<f64> {
    Ok(i32::from_be_bytes(
        span(data, at, 4)?
            .try_into()
            .map_err(|_| ResolveError::Unsupported)?,
    ) as f64
        / 65536.0)
}
fn sample(data: &[u8], at: usize, bytes: usize) -> Result<f64> {
    match bytes {
        1 => Ok(f64::from(*data.get(at).ok_or(ResolveError::Unsupported)?) / 255.0),
        2 => Ok(f64::from(short(data, at)?) / 65535.0),
        _ => unsupported(),
    }
}
fn curve(data: &[u8], at: usize) -> Result<(Curve, usize)> {
    match span(data, at, 4)? {
        b"curv" => {
            let count = word(data, at + 8)?;
            let size = count
                .checked_mul(2)
                .and_then(|v| v.checked_add(12))
                .ok_or(ResolveError::Unsupported)?;
            span(data, at, size)?;
            let c = match count {
                0 => Curve::Identity,
                1 => {
                    let mut values = [0.; 7];
                    values[0] = f64::from(short(data, at + 12)?) / 256.;
                    Curve::Param { kind: 0, values }
                }
                _ => Curve::Table {
                    offset: at + 12,
                    count,
                    bytes: 2,
                },
            };
            Ok((c, size.div_ceil(4) * 4))
        }
        b"para" => {
            let kind = short(data, at + 8)?;
            let count = *([1, 3, 4, 5, 7]
                .get(kind as usize)
                .ok_or(ResolveError::Unsupported)?);
            let mut values = [0.; 7];
            for (i, v) in values[..count].iter_mut().enumerate() {
                *v = fixed(data, at + 12 + i * 4)?;
            }
            if values[0] < 0. || (kind > 0 && values[1] <= 0.) {
                return unsupported();
            }
            Ok((Curve::Param { kind, values }, 12 + count * 4))
        }
        _ => unsupported(),
    }
}
fn curves(data: &[u8], offset: usize, count: usize) -> Result<[Curve; 4]> {
    let mut result = [Curve::Identity; 4];
    let mut at = offset;
    for c in result.get_mut(..count).ok_or(ResolveError::Unsupported)? {
        let (value, size) = curve(data, at)?;
        *c = value;
        at = at.checked_add(size).ok_or(ResolveError::Unsupported)?;
    }
    Ok(result)
}
impl Curve {
    fn eval(self, data: &[u8], x: f64) -> Result<f64> {
        let y = match self {
            Self::Identity => x,
            Self::Table {
                offset,
                count,
                bytes,
            } => {
                if count < 2 {
                    return unsupported();
                }
                let at = x.clamp(0., 1.) * (count - 1) as f64;
                let low = at as usize;
                let high = (low + 1).min(count - 1);
                let a = sample(data, offset + low * bytes, bytes)?;
                let b = sample(data, offset + high * bytes, bytes)?;
                a + (b - a) * (at - low as f64)
            }
            Self::Param { kind, values: p } => {
                let sign = x.signum();
                let x = x.abs();
                let value = match kind {
                    0 => powf(x, p[0]),
                    1 => {
                        if x >= -p[2] / p[1] {
                            powf(p[1] * x + p[2], p[0])
                        } else {
                            0.
                        }
                    }
                    2 => {
                        if x >= -p[2] / p[1] {
                            powf(p[1] * x + p[2], p[0]) + p[3]
                        } else {
                            p[3]
                        }
                    }
                    3 => {
                        if x >= p[4] {
                            powf(p[1] * x + p[2], p[0])
                        } else {
                            p[3] * x
                        }
                    }
                    4 => {
                        if x >= p[4] {
                            powf(p[1] * x + p[2], p[0]) + p[5]
                        } else {
                            p[3] * x + p[6]
                        }
                    }
                    _ => return unsupported(),
                };
                sign * value
            }
        };
        if !y.is_finite() {
            return unsupported();
        }
        Ok(y)
    }
}
impl Grid {
    fn validate(self, data: &[u8]) -> Result<()> {
        let count = self.dims[..self.inputs].iter().try_fold(3_usize, |n, d| {
            if *d < 2 {
                return unsupported();
            }
            n.checked_mul(*d).ok_or(ResolveError::Unsupported)
        })?;
        span(
            data,
            self.offset,
            count
                .checked_mul(self.bytes)
                .ok_or(ResolveError::Unsupported)?,
        )?;
        Ok(())
    }
    fn eval(self, data: &[u8], input: [f64; 4]) -> Result<[f64; 4]> {
        let mut lo = [0; 4];
        let mut fraction = [0.; 4];
        for i in 0..self.inputs {
            let x = input[i].clamp(0., 1.) * (self.dims[i] - 1) as f64;
            lo[i] = (x as usize).min(self.dims[i] - 2);
            fraction[i] = x - lo[i] as f64;
        }
        let mut result = [0.; 4];
        for corner in 0..(1 << self.inputs) {
            let mut index = 0_usize;
            let mut weight = 1.;
            for i in 0..self.inputs {
                let upper = (corner >> i) & 1;
                index = index * self.dims[i] + lo[i] + upper;
                weight *= if upper == 0 {
                    1. - fraction[i]
                } else {
                    fraction[i]
                };
            }
            for (channel, v) in result[..3].iter_mut().enumerate() {
                *v += weight
                    * sample(
                        data,
                        self.offset + (index * 3 + channel) * self.bytes,
                        self.bytes,
                    )?;
            }
        }
        Ok(result)
    }
}

impl Lut {
    pub(super) fn stored(self) -> Result<Vec<Self>> {
        super::color::cms_program_allocation(|| {
            let mut stored = Vec::new();
            stored
                .try_reserve_exact(1)
                .map_err(|_| ResolveError::Memory)?;
            stored.push(self);
            Ok(stored)
        })
    }
    /// `profile` is already extent-validated. Return None when no selected A2B
    /// exists, leaving matrix/TRC and legacy fallthrough to the caller.
    pub(super) fn parse(profile: &[u8]) -> Result<Option<Self>> {
        let count = word(profile, 128)?;
        let table = span(
            profile,
            132,
            count.checked_mul(12).ok_or(ResolveError::Unsupported)?,
        )?;
        let mut selected = None;
        for wanted in [b"A2B0", b"A2B1"] {
            if let Some(tag) = table.as_chunks::<12>().0.iter().find(|t| &t[..4] == wanted) {
                selected = Some(span(profile, word(tag, 4)?, word(tag, 8)?)?);
                break;
            }
        }
        let Some(data) = selected else {
            return Ok(None);
        };
        let model = span(profile, 16, 4)?;
        let inputs = match model {
            b"RGB " => 3,
            b"GRAY" => 1,
            _ => return unsupported(),
        };
        let lab = match span(profile, 20, 4)? {
            b"XYZ " => false,
            b"Lab " => true,
            _ => return unsupported(),
        };
        let mut lut = Self {
            data: Vec::new(),
            inputs,
            lab,
            lab_v2: false,
            a: [Curve::Identity; 4],
            b: [Curve::Identity; 4],
            m: [Curve::Identity; 4],
            grid: None,
            matrix: None,
        };
        match span(data, 0, 4)? {
            b"mft1" | b"mft2" => {
                if data.get(8) != Some(&(inputs as u8)) || data.get(9) != Some(&3) {
                    return unsupported();
                }
                let bytes = if span(data, 0, 4)? == b"mft1" { 1 } else { 2 };
                let n = if bytes == 1 {
                    256
                } else {
                    short(data, 48)? as usize
                };
                let out = if bytes == 1 {
                    256
                } else {
                    short(data, 50)? as usize
                };
                if !(2..=4096).contains(&n) || !(2..=4096).contains(&out) {
                    return unsupported();
                }
                let mut at = if bytes == 1 { 48 } else { 52 };
                for c in &mut lut.a[..inputs] {
                    *c = Curve::Table {
                        offset: at,
                        count: n,
                        bytes,
                    };
                    span(data, at, n * bytes)?;
                    at += n * bytes;
                }
                let grid = Grid {
                    offset: at,
                    dims: [usize::from(*data.get(10).ok_or(ResolveError::Unsupported)?); 4],
                    inputs,
                    bytes,
                };
                grid.validate(data)?;
                at += grid.dims[..inputs].iter().product::<usize>() * 3 * bytes;
                lut.grid = Some(grid);
                for c in &mut lut.b[..3] {
                    *c = Curve::Table {
                        offset: at,
                        count: out,
                        bytes,
                    };
                    span(data, at, out * bytes)?;
                    at += out * bytes;
                }
                lut.lab_v2 = lab && profile[8] < 4 && bytes == 2;
            }
            b"mAB " => {
                if data.get(8) != Some(&(inputs as u8)) || data.get(9) != Some(&3) {
                    return unsupported();
                }
                let (b, matrix, m, grid, a) = (
                    word(data, 12)?,
                    word(data, 16)?,
                    word(data, 20)?,
                    word(data, 24)?,
                    word(data, 28)?,
                );
                if b == 0
                    || (matrix == 0) != (m == 0)
                    || (grid == 0) != (a == 0)
                    || [b, matrix, m, grid, a].iter().any(|v| v % 4 != 0)
                {
                    return unsupported();
                }
                lut.b = curves(data, b, 3)?;
                if a != 0 {
                    lut.a = curves(data, a, inputs)?;
                    let mut dims = [1; 4];
                    for (i, v) in dims[..inputs].iter_mut().enumerate() {
                        *v = usize::from(*data.get(grid + i).ok_or(ResolveError::Unsupported)?);
                    }
                    let bytes = usize::from(*data.get(grid + 16).ok_or(ResolveError::Unsupported)?);
                    if !matches!(bytes, 1 | 2) {
                        return unsupported();
                    }
                    let g = Grid {
                        offset: grid + 20,
                        dims,
                        inputs,
                        bytes,
                    };
                    g.validate(data)?;
                    lut.grid = Some(g);
                } else if inputs != 3 {
                    return unsupported();
                }
                if m != 0 {
                    lut.m = curves(data, m, 3)?;
                    let scale = if lab { 1. } else { 65535. / 32768. };
                    let mut values = [[0.; 4]; 3];
                    for (r, row) in values.iter_mut().enumerate() {
                        for (c, v) in row[..3].iter_mut().enumerate() {
                            *v = fixed(data, matrix + (r * 3 + c) * 4)? * scale;
                        }
                        row[3] = fixed(data, matrix + 36 + r * 4)? * scale;
                    }
                    lut.matrix = Some(values);
                }
            }
            _ => return unsupported(),
        }
        // This is the only untrusted-dependent allocation in this executor.
        lut.data = super::color::cms_allocation(|| try_copy(data))?;
        Ok(Some(lut))
    }
    pub(super) fn gray(&self) -> bool {
        self.inputs == 1
    }
    pub(super) fn pixel(&self, rgb: [u8; 3]) -> Result<[u8; 3]> {
        let mut values = [0.; 4];
        for i in 0..self.inputs {
            values[i] = self.a[i].eval(&self.data, f64::from(rgb[i]) / 255.)?;
        }
        if let Some(grid) = self.grid {
            values = grid.eval(&self.data, values)?;
        }
        if let Some(matrix) = self.matrix {
            for (v, c) in values[..3].iter_mut().zip(self.m) {
                *v = c.eval(&self.data, *v)?;
            }
            let input = values;
            for (v, row) in values[..3].iter_mut().zip(matrix) {
                *v = row[0] * input[0] + row[1] * input[1] + row[2] * input[2] + row[3];
            }
        }
        for (v, c) in values[..3].iter_mut().zip(self.b) {
            *v = c.eval(&self.data, *v)?;
        }
        let mut xyz = [values[0], values[1], values[2]];
        if self.lab {
            if self.lab_v2 {
                for v in &mut xyz {
                    *v *= 65535. / 65280.;
                }
            }
            let fy = (xyz[0] * 100. + 16.) / 116.;
            let fx = fy + (xyz[1] * 255. - 128.) / 500.;
            let fz = fy - (xyz[2] * 255. - 128.) / 200.;
            let inv = |v: f64| {
                if v > 6. / 29. {
                    v * v * v
                } else {
                    (v - 4. / 29.) * 108. / 841.
                }
            };
            xyz = [inv(fx) * 0.9642, inv(fy), inv(fz) * 0.8249];
        }
        // D50-adapted linear sRGB inverse colorant matrix (ICC sRGB PCS).
        let linear = [
            3.1338561 * xyz[0] - 1.6168667 * xyz[1] - 0.4906146 * xyz[2],
            -0.9787684 * xyz[0] + 1.9161415 * xyz[1] + 0.0334540 * xyz[2],
            0.0719453 * xyz[0] - 0.2289914 * xyz[1] + 1.4052427 * xyz[2],
        ];
        if !linear.iter().all(|v| v.is_finite()) {
            return unsupported();
        }
        Ok(linear.map(|v| {
            let v = if v <= 0.0031308 {
                12.92 * v
            } else {
                1.055 * powf(v, 1. / 2.4) - 0.055
            };
            (v.clamp(0., 1.) * 255.).round() as u8
        }))
    }
}
