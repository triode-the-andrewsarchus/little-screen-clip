//! Decorrelation Stretch (DCS) algorithm for RGB and multispectral color enhancement.
//!
//! Ported from the Alley (JPL 1996) Algorithm Theoretical Basis Document (ASTER AST06)
//! and the implementation in `shannietron/decorrelation_stretch`.
//!
//! # Method Summary
//! 1. Subsample pixel values with strided sampling to estimate per-band mean, variance, covariance, and correlation.
//! 2. Perform cyclic Jacobi eigen-decomposition on the 3x3 covariance or correlation matrix.
//! 3. Stretch principal components inversely proportional to `sqrt(lambda)`.
//! 4. Rotate back into RGB color space, scale to target standard deviations, and shift to target means.
//! 5. Optionally apply percentile contrast clipping (`tol`) to map extremes to [0, 255].
//! 6. Apply affine transform `out = T * in + offset` to each pixel, clamping to [0, 255] and preserving alpha.

use image::{Rgba, RgbaImage};

/// Matrix to decompose: Correlation (equal band weight) or Covariance (weighted by variance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecorrelationMatrixMode {
    #[default]
    Correlation,
    Covariance,
}

impl DecorrelationMatrixMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Correlation => "Correlation (Equal band weight)",
            Self::Covariance => "Covariance (Weight by variance)",
        }
    }
}

/// Target mean or standard deviation value: Preserve original band statistics or set custom value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetValue {
    Preserve,
    Custom(f64),
}

impl Default for TargetValue {
    fn default() -> Self {
        Self::Custom(127.5)
    }
}

/// Parameters for computing the decorrelation stretch transform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecorrelationStretchParams {
    /// Matrix mode: Correlation (default) or Covariance.
    pub mode: DecorrelationMatrixMode,
    /// Target mean for each band (or Preserve). Default is 127.5.
    pub target_mean: TargetValue,
    /// Target standard deviation for each band (or Preserve). Default is 50.0.
    pub target_sigma: TargetValue,
    /// Optional percentile contrast clip (0.0 for off, or e.g. 0.005, 0.01, 0.02).
    pub tol: f64,
    /// Sampling stride in pixels (0 for auto, or explicit >= 1).
    pub stride: usize,
}

impl Default for DecorrelationStretchParams {
    fn default() -> Self {
        Self::nasa()
    }
}

impl DecorrelationStretchParams {
    /// NASA / ASTER preset: Correlation matrix, target mean 127.5, target sigma 50.0, tol 0.0.
    pub fn nasa() -> Self {
        Self {
            mode: DecorrelationMatrixMode::Correlation,
            target_mean: TargetValue::Custom(127.5),
            target_sigma: TargetValue::Custom(50.0),
            tol: 0.0,
            stride: 0,
        }
    }

    /// MATLAB preset: Correlation matrix, preserve each band's own mean and sigma, tol 0.0.
    pub fn matlab_preserve() -> Self {
        Self {
            mode: DecorrelationMatrixMode::Correlation,
            target_mean: TargetValue::Preserve,
            target_sigma: TargetValue::Preserve,
            tol: 0.0,
            stride: 0,
        }
    }
}

/// Compute recommended sampling stride aiming for ~100,000 samples.
pub fn auto_stride(width: u32, height: u32) -> usize {
    let total_pixels = (width as usize) * (height as usize);
    let s = ((total_pixels as f64 / 100_000.0).sqrt().floor() as usize).clamp(1, 8);
    let sample_count = ((width as usize + s - 1) / s) * ((height as usize + s - 1) / s);
    if sample_count < 1000 { 1 } else { s }
}

/// Sample statistics from strided inspection of an image.
#[derive(Debug, Clone, PartialEq)]
pub struct DecorrelationStats {
    pub sample_count: usize,
    pub mean: [f64; 3],
    pub sd: [f64; 3],
    pub cov: [[f64; 3]; 3],
    pub corr: [[f64; 3]; 3],
    pub samples: Vec<[u8; 3]>,
}

/// Compute band statistics from an RGBA image.
pub fn compute_stats(
    img: &RgbaImage,
    stride: usize,
    max_samples: Option<usize>,
) -> DecorrelationStats {
    compute_stats_cancellable(img, stride, max_samples, &|| false)
        .expect("non-cancellable statistics computation completes")
}

fn compute_stats_cancellable(
    img: &RgbaImage,
    stride: usize,
    max_samples: Option<usize>,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<DecorrelationStats> {
    let w = img.width() as usize;
    let h = img.height() as usize;
    let stride = stride.max(1);
    let max_samples = max_samples.unwrap_or(120_000);

    let cap = ((w + stride - 1) / stride) * ((h + stride - 1) / stride);
    let keep_every = ((cap + max_samples - 1) / max_samples).max(1);
    let mut samples = Vec::with_capacity(cap.min(max_samples));

    let mut n: usize = 0;
    let mut k: usize = 0;
    let mut s0: f64 = 0.0;
    let mut s1: f64 = 0.0;
    let mut s2: f64 = 0.0;
    let mut s00: f64 = 0.0;
    let mut s01: f64 = 0.0;
    let mut s02: f64 = 0.0;
    let mut s11: f64 = 0.0;
    let mut s12: f64 = 0.0;
    let mut s22: f64 = 0.0;

    for y in (0..h).step_by(stride) {
        if is_cancelled() {
            return None;
        }
        for x in (0..w).step_by(stride) {
            let pixel = img.get_pixel(x as u32, y as u32);
            let r = pixel[0] as f64;
            let g = pixel[1] as f64;
            let b = pixel[2] as f64;

            s0 += r;
            s1 += g;
            s2 += b;
            s00 += r * r;
            s01 += r * g;
            s02 += r * b;
            s11 += g * g;
            s12 += g * b;
            s22 += b * b;
            n += 1;

            if (k % keep_every) == 0 && samples.len() < max_samples {
                samples.push([pixel[0], pixel[1], pixel[2]]);
            }
            k += 1;
        }
    }

    let mut mean = [0.0; 3];
    let mut cov = [[0.0; 3]; 3];
    if n >= 2 {
        let n_f = n as f64;
        let s = [s0, s1, s2];
        let x_prods = [s00, s01, s02, s11, s12, s22];
        for l in 0..3 {
            mean[l] = s[l] / n_f;
        }
        let idx = [[0, 1, 2], [1, 3, 4], [2, 4, 5]];
        for l in 0..3 {
            for m in 0..3 {
                cov[l][m] = (x_prods[idx[l][m]] - s[l] * s[m] / n_f) / (n_f - 1.0);
            }
        }
    }

    let eps = 1e-9;
    let mut sd = [0.0; 3];
    for l in 0..3 {
        sd[l] = cov[l][l].max(0.0).sqrt();
    }

    let mut corr = [[0.0; 3]; 3];
    for l in 0..3 {
        for m in 0..3 {
            if sd[l] > eps && sd[m] > eps {
                corr[l][m] = cov[l][m] / (sd[l] * sd[m]);
            } else if l == m {
                corr[l][m] = 1.0;
            } else {
                corr[l][m] = 0.0;
            }
        }
    }

    Some(DecorrelationStats {
        sample_count: n,
        mean,
        sd,
        cov,
        corr,
        samples,
    })
}

/// Result of cyclic Jacobi eigen-decomposition for a symmetric 3x3 matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct EigenSym3 {
    /// Eigenvalues on the diagonal.
    pub values: [f64; 3],
    /// Orthogonal eigenvector matrix where column `i` is the eigenvector for `values[i]`.
    /// `vectors[k][i]` is component `k` of eigenvector `i`.
    pub vectors: [[f64; 3]; 3],
}

/// Cyclic Jacobi eigen-decomposition of a symmetric 3x3 matrix.
pub fn eigen_sym3(m: [[f64; 3]; 3]) -> EigenSym3 {
    let mut a = m;
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    for _sweep in 0..60 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-15 {
            break;
        }

        for p in 0..2 {
            for q in (p + 1)..3 {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = (if theta >= 0.0 { 1.0 } else { -1.0 })
                    / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;

                for k in 0..3 {
                    let kp = a[k][p];
                    let kq = a[k][q];
                    a[k][p] = c * kp - s * kq;
                    a[k][q] = s * kp + c * kq;
                }
                for k in 0..3 {
                    let pk = a[p][k];
                    let qk = a[q][k];
                    a[p][k] = c * pk - s * qk;
                    a[q][k] = s * pk + c * qk;
                }
                for k in 0..3 {
                    let kp = v[k][p];
                    let kq = v[k][q];
                    v[k][p] = c * kp - s * kq;
                    v[k][q] = s * kp + c * kq;
                }
            }
        }
    }

    EigenSym3 {
        values: [a[0][0], a[1][1], a[2][2]],
        vectors: v,
    }
}

/// Computed 3x3 affine transformation and diagnostic statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct DecorrelationTransform {
    /// 3x3 transformation matrix `T` in row-major order: `T[row][col]`.
    pub transform: [[f32; 3]; 3],
    /// Offset vector for `[R, G, B]`.
    pub offset: [f32; 3],
    /// Matrix mode used.
    pub mode: DecorrelationMatrixMode,
    /// Target mean values for `[R, G, B]`.
    pub target_mean: [f64; 3],
    /// Target standard deviation values for `[R, G, B]`.
    pub target_sigma: [f64; 3],
    /// Eigenvalues from decomposition.
    pub eigenvalues: [f64; 3],
    /// Eigenvectors (columns).
    pub eigenvectors: [[f64; 3]; 3],
    /// Diagnostic warnings regarding zero-variance bands, collinear channels, or noise magnification.
    pub warnings: Vec<String>,
    /// Tol percentile clip limits per band: `[[min, max]; 3]`, if enabled.
    pub tol_limits: Option<[[f32; 2]; 3]>,
}

/// Build the 3x3 transform matrix `T` and offset vector from sampled stats and parameters.
pub fn build_transform(
    stats: &DecorrelationStats,
    params: &DecorrelationStretchParams,
) -> DecorrelationTransform {
    let mode = params.mode;
    let mut warnings = Vec::new();
    let eps = 1e-9;
    let mut pre = [1.0; 3];

    let m = match mode {
        DecorrelationMatrixMode::Correlation => {
            for b in 0..3 {
                if stats.sd[b] > eps {
                    pre[b] = 1.0 / stats.sd[b];
                } else {
                    pre[b] = 0.0;
                    let band_name = match b {
                        0 => 'R',
                        1 => 'G',
                        _ => 'B',
                    };
                    warnings.push(format!(
                        "Band {} has zero variance; it cannot be stretched.",
                        band_name
                    ));
                }
            }
            stats.corr
        }
        DecorrelationMatrixMode::Covariance => {
            for b in 0..3 {
                if stats.sd[b] <= eps {
                    let band_name = match b {
                        0 => 'R',
                        1 => 'G',
                        _ => 'B',
                    };
                    warnings.push(format!(
                        "Band {} has zero variance; it cannot be stretched.",
                        band_name
                    ));
                }
            }
            stats.cov
        }
    };

    let eig = eigen_sym3(m);
    let trace = eig.values[0] + eig.values[1] + eig.values[2];
    let mut s = [0.0; 3];
    let mut lmax = 0.0f64;

    for i in 0..3 {
        let l = eig.values[i];
        if l > lmax {
            lmax = l;
        }
        if l > 1e-7 * trace.max(eps) {
            s[i] = 1.0 / l.sqrt();
        } else {
            s[i] = 0.0;
            warnings.push(format!(
                "Eigenvalue {} is ~0 (one band is a linear combination of the others); that axis collapses to the mean.",
                i + 1
            ));
        }
    }

    if trace > eps && lmax / trace > 0.98 {
        warnings.push(
            "One principal axis holds >98% of the variance. Minor axes are being amplified heavily; noise may be exaggerated.".to_string(),
        );
    }

    let r = eig.vectors;
    let mut core = [[0.0; 3]; 3];
    for a in 0..3 {
        for b in 0..3 {
            let mut acc = 0.0;
            for i in 0..3 {
                acc += r[a][i] * s[i] * r[b][i];
            }
            core[a][b] = acc;
        }
    }

    let tsig = match params.target_sigma {
        TargetValue::Preserve => stats.sd,
        TargetValue::Custom(v) => [v, v, v],
    };

    let tmean = match params.target_mean {
        TargetValue::Preserve => stats.mean,
        TargetValue::Custom(v) => [v, v, v],
    };

    let mut t = [[0.0f64; 3]; 3];
    let mut offset = [0.0f64; 3];

    for a in 0..3 {
        let mut o = tmean[a];
        for b in 0..3 {
            let val = tsig[a] * core[a][b] * pre[b];
            t[a][b] = val;
            o -= val * stats.mean[b];
        }
        offset[a] = o;
    }

    // Optional linear contrast stretch (Tol): clip per-band percentiles of the output to [0, 255]
    let mut tol_limits = None;
    if params.tol > 0.0 && stats.samples.len() >= 90 {
        let ns = stats.samples.len();
        let mut y = [vec![0.0f32; ns], vec![0.0f32; ns], vec![0.0f32; ns]];

        for (k, smp) in stats.samples.iter().enumerate() {
            let r_v = smp[0] as f64;
            let g_v = smp[1] as f64;
            let b_v = smp[2] as f64;
            for a in 0..3 {
                y[a][k] = (t[a][0] * r_v + t[a][1] * g_v + t[a][2] * b_v + offset[a]) as f32;
            }
        }

        let mut limits = [[0.0f32; 2]; 3];
        for a in 0..3 {
            y[a].sort_by(|x1, x2| x1.partial_cmp(x2).unwrap_or(std::cmp::Ordering::Equal));
            let lo_idx = ((params.tol * (ns - 1) as f64).floor() as usize).min(ns - 1);
            let hi_idx = (((1.0 - params.tol) * (ns - 1) as f64).ceil() as usize).min(ns - 1);
            let lo = y[a][lo_idx];
            let hi = y[a][hi_idx];
            limits[a] = [lo, hi];

            if (hi - lo) > (eps as f32) {
                let sc = 255.0 / (hi - lo);
                for b in 0..3 {
                    t[a][b] *= sc as f64;
                }
                offset[a] = (offset[a] - lo as f64) * sc as f64;
            }
        }
        tol_limits = Some(limits);
    }

    let mut t_f32 = [[0.0f32; 3]; 3];
    let mut off_f32 = [0.0f32; 3];
    for a in 0..3 {
        for b in 0..3 {
            t_f32[a][b] = t[a][b] as f32;
        }
        off_f32[a] = offset[a] as f32;
    }

    DecorrelationTransform {
        transform: t_f32,
        offset: off_f32,
        mode,
        target_mean: tmean,
        target_sigma: tsig,
        eigenvalues: eig.values,
        eigenvectors: eig.vectors,
        warnings,
        tol_limits,
    }
}

/// Apply the affine transform to an RGBA image, returning a new stretched image.
///
/// Pixel assignment clamps outputs to [0, 255] and preserves the source pixel's alpha channel.
pub fn apply_transform(src: &RgbaImage, transform: &DecorrelationTransform) -> RgbaImage {
    apply_transform_cancellable(src, transform, &|| false)
        .expect("non-cancellable transform completes")
}

fn apply_transform_cancellable(
    src: &RgbaImage,
    transform: &DecorrelationTransform,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<RgbaImage> {
    let w = src.width();
    let h = src.height();
    let mut dst = RgbaImage::new(w, h);

    let t = transform.transform;
    let off = transform.offset;

    let t00 = t[0][0];
    let t01 = t[0][1];
    let t02 = t[0][2];
    let t10 = t[1][0];
    let t11 = t[1][1];
    let t12 = t[1][2];
    let t20 = t[2][0];
    let t21 = t[2][1];
    let t22 = t[2][2];

    let o0 = off[0];
    let o1 = off[1];
    let o2 = off[2];

    for (x, y, pixel) in src.enumerate_pixels() {
        if x == 0 && is_cancelled() {
            return None;
        }
        let r = pixel[0] as f32;
        let g = pixel[1] as f32;
        let b = pixel[2] as f32;
        let a = pixel[3];

        let rout = (t00 * r + t01 * g + t02 * b + o0).clamp(0.0, 255.0).round() as u8;
        let gout = (t10 * r + t11 * g + t12 * b + o1).clamp(0.0, 255.0).round() as u8;
        let bout = (t20 * r + t21 * g + t22 * b + o2).clamp(0.0, 255.0).round() as u8;

        dst.put_pixel(x, y, Rgba([rout, gout, bout, a]));
    }

    Some(dst)
}

/// Convenience function to compute stats, build the transform, and produce the stretched image.
pub fn run_decorrelation_stretch(
    img: &RgbaImage,
    params: &DecorrelationStretchParams,
) -> (DecorrelationStats, DecorrelationTransform, RgbaImage) {
    run_decorrelation_stretch_cancellable(img, params, &|| false)
        .expect("non-cancellable decorrelation stretch completes")
}

/// Run the decorrelation stretch with cancellation checks between image rows.
pub fn run_decorrelation_stretch_cancellable(
    img: &RgbaImage,
    params: &DecorrelationStretchParams,
    is_cancelled: &dyn Fn() -> bool,
) -> Option<(DecorrelationStats, DecorrelationTransform, RgbaImage)> {
    let stride = if params.stride == 0 {
        auto_stride(img.width(), img.height())
    } else {
        params.stride
    };

    let stats = compute_stats_cancellable(img, stride, None, is_cancelled)?;
    let transform = build_transform(&stats, params);
    let result_img = apply_transform_cancellable(img, &transform, is_cancelled)?;

    Some((stats, transform, result_img))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eigen_sym3_diagonal() {
        let diag = [[4.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 1.0]];
        let eig = eigen_sym3(diag);
        assert!((eig.values[0] - 4.0).abs() < 1e-7);
        assert!((eig.values[1] - 2.0).abs() < 1e-7);
        assert!((eig.values[2] - 1.0).abs() < 1e-7);
    }

    #[test]
    fn test_eigen_sym3_symmetric_matrix() {
        let m = [[2.0, 1.0, 0.0], [1.0, 2.0, 1.0], [0.0, 1.0, 2.0]];
        let eig = eigen_sym3(m);
        // Trace of symmetric matrix must equal sum of eigenvalues
        let trace_m = m[0][0] + m[1][1] + m[2][2];
        let sum_eig = eig.values[0] + eig.values[1] + eig.values[2];
        assert!((trace_m - sum_eig).abs() < 1e-6);

        // Verify M * v_i = lambda_i * v_i for each eigenvector
        for i in 0..3 {
            let lambda = eig.values[i];
            let v_col = [eig.vectors[0][i], eig.vectors[1][i], eig.vectors[2][i]];
            for row in 0..3 {
                let m_v = m[row][0] * v_col[0] + m[row][1] * v_col[1] + m[row][2] * v_col[2];
                let l_v = lambda * v_col[row];
                assert!((m_v - l_v).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn test_auto_stride() {
        assert_eq!(auto_stride(100, 100), 1);
        assert!(auto_stride(4000, 3000) >= 8);
    }

    #[test]
    fn test_decorrelation_nasa_preset() {
        let mut img = RgbaImage::new(100, 100);
        for (x, y, p) in img.enumerate_pixels_mut() {
            // Gradient with correlated channels
            let val = ((x + y) * 255 / 200) as u8;
            *p = Rgba([val, val / 2, val / 3, 255]);
        }

        let params = DecorrelationStretchParams::nasa();
        let (stats, transform, stretched) = run_decorrelation_stretch(&img, &params);

        assert_eq!(stats.sample_count, 10000);
        assert_eq!(stretched.width(), 100);
        assert_eq!(stretched.height(), 100);
        assert!(transform.warnings.is_empty() || !transform.eigenvalues.iter().any(|v| v.is_nan()));

        // Alpha channel must be preserved
        assert_eq!(stretched.get_pixel(50, 50)[3], 255);
    }

    #[test]
    fn cancellable_stretch_stops_before_stats_when_cancelled() {
        let img = RgbaImage::from_pixel(64, 64, Rgba([10, 20, 30, 255]));
        assert!(
            run_decorrelation_stretch_cancellable(
                &img,
                &DecorrelationStretchParams::nasa(),
                &|| true
            )
            .is_none()
        );
    }

    #[test]
    fn cancellable_stretch_checks_between_stats_rows() {
        let img = RgbaImage::from_fn(128, 96, |x, y| {
            Rgba([(x * 3) as u8, (y * 5) as u8, (x ^ y) as u8, 255])
        });
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let output = run_decorrelation_stretch_cancellable(
            &img,
            &DecorrelationStretchParams::nasa(),
            &|| checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 4,
        );
        assert!(output.is_none());
        assert_eq!(checks.load(std::sync::atomic::Ordering::Relaxed), 5);
    }

    #[test]
    fn test_decorrelation_matlab_preset() {
        let mut img = RgbaImage::new(60, 60);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let r = (x * 4) as u8;
            let g = (y * 4) as u8;
            let b = ((x + y) * 2) as u8;
            *p = Rgba([r, g, b, 200]);
        }

        let params = DecorrelationStretchParams::matlab_preserve();
        let (_stats, _transform, stretched) = run_decorrelation_stretch(&img, &params);

        assert_eq!(stretched.width(), 60);
        assert_eq!(stretched.height(), 60);
        assert_eq!(stretched.get_pixel(10, 10)[3], 200);
    }

    #[test]
    fn test_decorrelation_with_tol_clip() {
        let mut img = RgbaImage::new(50, 50);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let v = (x * 5) as u8;
            *p = Rgba([v, v, v, 255]);
        }

        let mut params = DecorrelationStretchParams::nasa();
        params.tol = 0.01;
        let (_stats, transform, stretched) = run_decorrelation_stretch(&img, &params);

        assert!(transform.tol_limits.is_some());
        assert_eq!(stretched.width(), 50);
    }
}
