//! Dimension-agnostic 2D dimensionality reduction via Principal Component Analysis (PCA).
//!
//! Supports arbitrary feature dimensions (e.g. 64-bit d-hash bit-vectors, or 728-dimensional
//! neural embeddings like EmbeddingGemma2) without materializing large covariance matrices.

/// Dimension of a difference hash expanded into floating-point coordinates.
pub const DHASH_DIMENSION: usize = 64;
const POWER_ITERATION_MAX_STEPS: usize = 40;
const MIN_NORM_EPSILON: f32 = 1e-7;
const MIN_SPAN_EPSILON: f32 = 1e-5;
const PAIRWISE_DIFF_EPSILON: f32 = 1e-6;
const JITTER_SCALE: f32 = 0.001;
const JITTER_FREQUENCY: f32 = 0.0001;

/// A generalized feature vector representation for similarity clustering.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureVector {
    /// 64-bit difference hash expanded into 64 zero-centered float coordinates.
    DHash(u64),
    /// Dense float embedding vector (e.g. 728-d Gemma/Gemini embedding).
    Dense(Vec<f32>),
}

impl FeatureVector {
    /// Converts the feature into a continuous floating-point vector.
    pub fn to_vec(&self) -> Vec<f32> {
        match self {
            Self::DHash(hash) => {
                let mut v = Vec::with_capacity(DHASH_DIMENSION);
                for b in 0..DHASH_DIMENSION {
                    v.push(if (hash & (1u64 << b)) != 0 { 0.5 } else { -0.5 });
                }
                v
            }
            Self::Dense(dense) => dense.clone(),
        }
    }

    /// Returns the dimensionality of this feature representation.
    #[allow(dead_code)]
    pub fn dimension(&self) -> usize {
        match self {
            Self::DHash(_) => DHASH_DIMENSION,
            Self::Dense(dense) => dense.len(),
        }
    }
}

/// Projects arbitrary feature vectors into continuous 2D coordinates `(x, y)`.
///
/// Computes the principal components via matrix-free power iteration:
/// $C v = \frac{1}{N} X^T (X v)$
/// which scales as $O(N \cdot D)$ rather than $O(D^2)$, gracefully handling 64-d hashes,
/// 728-d Gemma embeddings, or 1024-d embeddings.
pub fn project_features_to_2d(features: &[Vec<f32>]) -> Vec<(f32, f32)> {
    let n = features.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(0.0, 0.0)];
    }

    let d = features.iter().map(|f| f.len()).max().unwrap_or(0);
    if d == 0 {
        return vec![(0.0, 0.0); n];
    }

    if n == 2 {
        let diff: f32 = (0..d)
            .map(|j| {
                let a = features[0].get(j).copied().unwrap_or(0.0);
                let b = features[1].get(j).copied().unwrap_or(0.0);
                (a - b).powi(2)
            })
            .sum();
        if diff < PAIRWISE_DIFF_EPSILON {
            return vec![(-0.5, 0.0), (0.5, 0.0)];
        }
        return vec![(-1.0, 0.0), (1.0, 0.0)];
    }

    // 1. Compute per-feature mean
    let mut means = vec![0.0f32; d];
    for feat in features {
        for (j, &val) in feat.iter().enumerate() {
            means[j] += val;
        }
    }
    let inv_n = 1.0 / (n as f32);
    for m in &mut means {
        *m *= inv_n;
    }

    // 2. Build centered data matrix X (N x D)
    let mut x_centered = Vec::with_capacity(n);
    for feat in features {
        let mut row = Vec::with_capacity(d);
        for (j, &mean) in means.iter().enumerate().take(d) {
            let val = feat.get(j).copied().unwrap_or(0.0) - mean;
            row.push(val);
        }
        x_centered.push(row);
    }

    // 3. Power iteration for 1st principal component e1
    // Matrix-free: C v = 1/N * X^T * (X * v)
    let e1 = power_iteration(&x_centered, d, None, 0.0);

    // Lambda1 = 1/N * ||X * e1||^2
    let x_e1 = mat_vec_mul(&x_centered, &e1);
    let lambda1: f32 = x_e1.iter().map(|&val| val * val).sum::<f32>() * inv_n;

    // 4. Power iteration for 2nd principal component e2 with deflation
    let e2 = power_iteration(&x_centered, d, Some(&e1), lambda1);

    // 5. Project each sample onto e1 and e2
    let mut coords = Vec::with_capacity(n);
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;

    for (i, row) in x_centered.iter().enumerate() {
        let mut x: f32 = row.iter().zip(e1.iter()).map(|(a, b)| a * b).sum();
        let mut y: f32 = row.iter().zip(e2.iter()).map(|(a, b)| a * b).sum();

        // Deterministic tiny jitter so duplicate points don't occupy identical float coords
        let jitter = (i as f32 * JITTER_FREQUENCY).sin() * JITTER_SCALE;
        x += jitter;
        y += (i as f32 * JITTER_FREQUENCY).cos() * JITTER_SCALE;

        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
        coords.push((x, y));
    }

    // 6. Normalize to [-1.0, 1.0]
    let span_x = (max_x - min_x).max(MIN_SPAN_EPSILON);
    let span_y = (max_y - min_y).max(MIN_SPAN_EPSILON);
    for (x, y) in &mut coords {
        *x = ((*x - min_x) / span_x) * 2.0 - 1.0;
        *y = ((*y - min_y) / span_y) * 2.0 - 1.0;
    }

    coords
}

/// Projects 64-bit difference hashes into continuous 2D coordinates.
#[allow(dead_code)]
pub fn project_hashes_to_2d(hashes: &[u64]) -> Vec<(f32, f32)> {
    let features: Vec<Vec<f32>> = hashes
        .iter()
        .map(|&h| FeatureVector::DHash(h).to_vec())
        .collect();
    project_features_to_2d(&features)
}

fn power_iteration(
    x_centered: &[Vec<f32>],
    d: usize,
    deflate_against: Option<&[f32]>,
    deflate_lambda: f32,
) -> Vec<f32> {
    let mut v = vec![0.0f32; d];
    for (i, val) in v.iter_mut().enumerate() {
        *val = if i % 2 == 0 { 1.0 } else { -0.5 };
    }
    if let Some(other) = deflate_against {
        orthogonalize(&mut v, other);
    }
    normalize(&mut v);

    let n = x_centered.len();
    let inv_n = 1.0 / (n as f32);

    for _ in 0..POWER_ITERATION_MAX_STEPS {
        // y = X * v (N x 1)
        let y = mat_vec_mul(x_centered, &v);

        // w = 1/N * X^T * y (D x 1)
        let mut w = vec![0.0f32; d];
        for (i, row) in x_centered.iter().enumerate() {
            let y_i = y[i];
            for (j, &val) in row.iter().enumerate() {
                w[j] += val * y_i;
            }
        }
        for val in &mut w {
            *val *= inv_n;
        }

        // Deflate if needed: w' = w - lambda * e1 * (e1 . v)
        if let Some(other) = deflate_against {
            let dot: f32 = other.iter().zip(v.iter()).map(|(a, b)| a * b).sum();
            let factor = deflate_lambda * dot;
            for (wj, &oj) in w.iter_mut().zip(other.iter()) {
                *wj -= factor * oj;
            }
            orthogonalize(&mut w, other);
        }

        if !normalize(&mut w) {
            break;
        }
        v = w;
    }
    v
}

fn mat_vec_mul(x: &[Vec<f32>], v: &[f32]) -> Vec<f32> {
    x.iter()
        .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
        .collect()
}

fn orthogonalize(v: &mut [f32], other: &[f32]) {
    let dot: f32 = v.iter().zip(other.iter()).map(|(a, b)| a * b).sum();
    for (vi, &oi) in v.iter_mut().zip(other.iter()) {
        *vi -= dot * oi;
    }
}

fn normalize(v: &mut [f32]) -> bool {
    let norm_sq: f32 = v.iter().map(|&x| x * x).sum();
    let norm = norm_sq.sqrt();
    if norm < MIN_NORM_EPSILON {
        return false;
    }
    let inv_norm = 1.0 / norm;
    for x in v.iter_mut() {
        *x *= inv_norm;
    }
    true
}
