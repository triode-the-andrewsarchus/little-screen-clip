//! Bounded normalized vectors; database admission remains query-local.
use super::{DIMENSION, Store, decode_vector_array};
use rusqlite::functions::FunctionFlags;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

const BUDGET: usize = 512 * 1024 * 1024;
const OVERHEAD: usize = 64 * 1024;
type Vector = [f32; DIMENSION];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::file_manager::search) struct Epoch(u64, i64);

#[derive(Default)]
pub(super) struct VectorCache {
    epoch: Option<Epoch>,
    positions: HashMap<i64, usize>,
    values: Vec<Vector>,
    limit: usize,
    last_used: Option<Instant>,
}
impl VectorCache {
    fn prepare(&mut self, epoch: Epoch, count: u64, budget: usize) {
        self.last_used = Some(Instant::now());
        if self.epoch == Some(epoch) {
            return;
        }
        self.positions.clear();
        self.values.clear();
        self.epoch = Some(epoch);
        // Include conservative room for hash buckets/controls and allocator
        // overhead, not just vector payload. No second matrix on generation changes.
        self.limit = usize::try_from(count)
            .unwrap_or(usize::MAX)
            .min(budget.saturating_sub(OVERHEAD) / (size_of::<Vector>() + 128));
        if self.limit == 0 || self.values.capacity() < self.limit {
            self.positions = HashMap::new();
            self.values = Vec::new();
            if self.values.try_reserve_exact(self.limit).is_err()
                || self.positions.try_reserve(self.limit).is_err()
            {
                self.release();
            }
        }
    }
    fn release(&mut self) {
        *self = Self::default();
    }
    fn score(&mut self, id: i64, query: &[f32], encoded: Option<&[u8]>) -> Result<f32, String> {
        if let Some(&position) = self.positions.get(&id) {
            return normalized_score(query, &self.values[position]);
        }
        let vector = decode_vector_array(encoded.ok_or("Missing uncached vector")?)?;
        let score = normalized_score(query, &vector)?;
        if self.values.len() < self.limit {
            let position = self.values.len();
            self.values.push(vector);
            self.positions.insert(id, position);
        }
        Ok(score)
    }
    #[cfg(test)]
    fn reserved_bound(&self) -> usize {
        if self.values.capacity() == 0 && self.positions.capacity() == 0 {
            return 0;
        }
        OVERHEAD + self.values.capacity() * size_of::<Vector>() + self.positions.capacity() * 32
    }
}

fn normalized_score(query: &[f32], vector: &Vector) -> Result<f32, String> {
    let query: &Vector = query.try_into().map_err(|_| "Invalid query dimension")?;
    // Independent lanes remove the 768-add dependency chain and let LLVM
    // vectorize on the target's baseline ISA, without unsafe CPU dispatch or
    // fast-math. Both cached and streamed vectors use this same reduction.
    let mut lanes = [0.0_f32; 8];
    for (a, b) in query
        .as_chunks::<8>()
        .0
        .iter()
        .zip(vector.as_chunks::<8>().0)
    {
        for lane in 0..8 {
            lanes[lane] += a[lane] * b[lane];
        }
    }
    let score = lanes.into_iter().sum::<f32>();
    if !score.is_finite() {
        return Err("Invalid semantic similarity score".into());
    }
    Ok(score)
}

#[cfg(test)]
pub(in crate::file_manager::search) fn cosine_score(
    query: &[f32],
    encoded: &[u8],
) -> Result<f32, String> {
    normalized_score(query, &decode_vector_array(encoded)?)
}

impl Store {
    fn vector_epoch(&self) -> Result<Epoch, String> {
        let version = self
            .connection
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        Ok(Epoch(self.connection.total_changes(), version))
    }
    pub(in crate::file_manager::search) fn prepare_vector_cache(&self) -> Result<Epoch, String> {
        let epoch = self.vector_epoch()?;
        self.vectors
            .lock()
            .map_err(|_| "Vector cache unavailable")?
            .prepare(epoch, self.vector_count, BUDGET);
        let cache = Arc::clone(&self.vectors);
        // Presence can change as this query admits new vectors, so this callback
        // is deliberately not marked deterministic. It never authorizes a row.
        self.connection
            .create_scalar_function(
                "clip_vector_cached",
                1,
                FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DIRECTONLY,
                move |context| {
                    let id: i64 = context.get(0)?;
                    let cache = cache.lock().map_err(|_| {
                        rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                            "Vector cache unavailable",
                        )))
                    })?;
                    Ok(cache.positions.contains_key(&id))
                },
            )
            .map_err(|error| error.to_string())?;
        Ok(epoch)
    }
    pub(in crate::file_manager::search) fn finish_vector_cache(
        &self,
        epoch: Epoch,
    ) -> Result<(), String> {
        if self.vector_epoch()? != epoch {
            self.invalidate_vectors()?;
            return Err("Search index changed during retrieval; retry the query".into());
        }
        Ok(())
    }
    pub(in crate::file_manager::search) fn invalidate_vectors(&self) -> Result<(), String> {
        self.vectors
            .lock()
            .map_err(|_| "Vector cache unavailable")?
            .release();
        Ok(())
    }
    pub(in crate::file_manager::search) fn score_vector(
        &self,
        id: i64,
        query: &[f32],
        encoded: Option<&[u8]>,
    ) -> Result<f32, String> {
        self.vectors
            .lock()
            .map_err(|_| "Vector cache unavailable")?
            .score(id, query, encoded)
    }
    pub(in crate::file_manager::search) fn release_idle_vectors(&self, idle: Duration) {
        if let Ok(mut cache) = self.vectors.lock()
            && cache.last_used.is_some_and(|used| used.elapsed() >= idle)
        {
            cache.release();
        }
    }
    // Precision oracle only: the application still stores/loads canonical f16.
    // Seed original model outputs into the same normalized f32 scoring path so
    // the quality comparison does not duplicate admission or rank fusion.
    #[cfg(test)]
    pub(in crate::file_manager::search) fn install_f32_reference(
        &self,
        vectors: &[(i64, Vec<f32>)],
    ) -> Result<(), String> {
        self.invalidate_vectors()?;
        self.prepare_vector_cache()?;
        let mut cache = self
            .vectors
            .lock()
            .map_err(|_| "Vector cache unavailable")?;
        if vectors.len() > cache.limit {
            return Err("Float32 fixture exceeds the vector cache budget".into());
        }
        for (id, values) in vectors {
            let mut vector: Vector = values
                .as_slice()
                .try_into()
                .map_err(|_| "Invalid reference dimension")?;
            if vector.iter().any(|value| !value.is_finite()) {
                return Err("Invalid reference value".into());
            }
            // Match f32 encode normalization followed by decode normalization,
            // omitting only the half conversion in the production round trip.
            for _ in 0..2 {
                let norm = vector
                    .iter()
                    .map(|&value| f64::from(value).powi(2))
                    .sum::<f64>()
                    .sqrt();
                if norm < 1e-12 {
                    return Err("Invalid zero reference".into());
                }
                for value in &mut vector {
                    *value = (f64::from(*value) / norm) as f32;
                }
            }
            let position = cache.values.len();
            if cache.positions.insert(*id, position).is_some() {
                return Err("Duplicate reference segment".into());
            }
            cache.values.push(vector);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::file_manager::search) fn vector_cache_usage(&self) -> (usize, usize) {
        let cache = self.vectors.lock().unwrap();
        (cache.values.len(), cache.reserved_bound())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::store::encode_vector;

    #[test]
    fn lane_reduction_agrees_with_double_precision_for_dense_and_cancelling_vectors() {
        let mut seed = 0x7e57_u64;
        let mut dense = || {
            let mut vector = [0.0; DIMENSION];
            for value in &mut vector {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *value = ((seed >> 32) as i32 as f64 / i32::MAX as f64) as f32;
            }
            let norm = vector
                .iter()
                .map(|&v| f64::from(v).powi(2))
                .sum::<f64>()
                .sqrt();
            for value in &mut vector {
                *value = (f64::from(*value) / norm) as f32;
            }
            vector
        };
        for _ in 0..512 {
            let query = dense();
            let independent = dense();
            let opposite = query.map(|v| -v);
            let alternating = query.map(|v| v.abs());
            let mut cancelled = alternating;
            for value in cancelled.iter_mut().step_by(2) {
                *value = -*value;
            }
            for vector in [independent, query, opposite, cancelled] {
                let reference = query
                    .iter()
                    .zip(vector)
                    .map(|(&a, b)| f64::from(a) * f64::from(b))
                    .sum::<f64>();
                let score = normalized_score(&query, &vector).unwrap();
                assert!((f64::from(score) - reference).abs() < 1e-6);
            }
        }
        assert!(normalized_score(&[1.0; 3], &[1.0; DIMENSION]).is_err());
        assert!(normalized_score(&[f32::NAN; DIMENSION], &[1.0; DIMENSION]).is_err());
        assert!(normalized_score(&[f32::INFINITY; DIMENSION], &[1.0; DIMENSION]).is_err());
    }

    #[test]
    fn float32_oracle_avoids_half_quantization_and_rejects_invalid_inputs() {
        let folder = tempfile::tempdir().unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        // This unit tests only the oracle's working-vector representation.
        store.vector_count = 1;
        let mut values = vec![0.0; DIMENSION];
        values[0] = 0.1234567;
        values[1] = (1.0_f32 - values[0] * values[0]).sqrt();
        let mut query = vec![0.0; DIMENSION];
        query[0] = 1.0;
        store
            .install_f32_reference(&[(42, values.clone())])
            .unwrap();
        let reference = store.score_vector(42, &query, None).unwrap();
        let quantized = cosine_score(&query, &encode_vector(&values).unwrap()).unwrap();
        assert!((reference - values[0]).abs() < 1e-7);
        assert!((reference - quantized).abs() > 1e-6);
        for invalid in [
            vec![0.0; DIMENSION],
            vec![f32::NAN; DIMENSION],
            vec![f32::INFINITY; DIMENSION],
            vec![1.0; 3],
        ] {
            assert!(store.install_f32_reference(&[(42, invalid)]).is_err());
        }
        assert!(
            store
                .install_f32_reference(&[(42, values.clone()), (43, values)])
                .is_err()
        );
    }

    #[test]
    fn cached_scores_match_streaming_and_capacity_never_reduces_scope() {
        let mut query = vec![0.0; DIMENSION];
        query[0] = 1.0;
        let encoded = encode_vector(&query).unwrap();
        let mut cache = VectorCache::default();
        let budget = OVERHEAD + size_of::<Vector>() + 128;
        cache.prepare(Epoch(1, 1), 100, budget);
        let expected = cosine_score(&query, &encoded).unwrap();
        for id in [1, 2, 3] {
            assert_eq!(
                cache.score(id, &query, Some(&encoded)).unwrap().to_bits(),
                expected.to_bits()
            );
        }
        assert_eq!(cache.values.len(), 1);
        assert_eq!(
            cache.score(1, &query, None).unwrap().to_bits(),
            expected.to_bits()
        );
        assert!(cache.reserved_bound() <= budget);
        cache.prepare(Epoch(2, 1), 100, budget);
        assert!(cache.positions.is_empty());
        assert!(cache.score(1, &query, None).is_err());
    }

    #[test]
    fn full_capacity_stays_bounded_and_reuses_one_matrix_across_generations() {
        let mut cache = VectorCache::default();
        cache.prepare(Epoch(1, 1), u64::MAX, BUDGET);
        assert!(cache.reserved_bound() <= BUDGET);
        let pointer = cache.values.as_ptr();
        let mut query = vec![0.0; DIMENSION];
        query[0] = 1.0;
        let encoded = encode_vector(&query).unwrap();
        cache.score(1, &query, Some(&encoded)).unwrap();
        cache.prepare(Epoch(2, 1), u64::MAX, BUDGET);
        assert!(cache.values.is_empty() && cache.positions.is_empty());
        assert_eq!(cache.values.as_ptr(), pointer);
        assert!(cache.reserved_bound() <= BUDGET);
    }

    #[test]
    fn cache_rejects_corruption_and_releases_all_buffers() {
        let mut cache = VectorCache::default();
        cache.prepare(Epoch(1, 1), 2, BUDGET);
        let query = vec![1.0; DIMENSION];
        for bytes in [
            vec![0; 3],
            vec![0; DIMENSION * 2],
            half::f16::NAN.to_le_bytes().repeat(DIMENSION),
        ] {
            assert!(cache.score(1, &query, Some(&bytes)).is_err());
            assert!(cache.positions.is_empty());
        }
        cache.release();
        assert_eq!(cache.reserved_bound(), 0);
    }
}
