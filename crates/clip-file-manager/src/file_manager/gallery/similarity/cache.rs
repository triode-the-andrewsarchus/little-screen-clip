//! Worker-owned persistent photo features, independent of the audio constellation DB.
//!
//! A hit requires byte-exact path, pinned source identity/revision, and encoder revision.
//! The encoder key includes preprocessing version and the cached model's identity, so
//! replacing a model invalidates embeddings even when its filename stays unchanged.
use std::{os::unix::ffi::OsStrExt, path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension, params};

use super::embedding::FeatureVector;
use crate::{
    file_source::FileSource,
    onnx_runtime::{SIGLIP2_GIANT_MODEL_DATA_NAME, SIGLIP2_GIANT_MODEL_NAME, models_cache_dir},
};

const VISION_DIMENSIONS: usize = 1536;
const DHASH_REVISION: &[u8] = b"dhash-v1";

pub(super) fn model_revision() -> Option<Vec<u8>> {
    let source = FileSource::capture(&models_cache_dir().join(SIGLIP2_GIANT_MODEL_NAME)).ok()?;
    let mut revision = b"siglip2-giant-fp16-rgb256-normalized-v1".to_vec();
    revision.extend(source.revision_key());
    if let Ok(weights) =
        FileSource::capture(&models_cache_dir().join(SIGLIP2_GIANT_MODEL_DATA_NAME))
    {
        revision.extend(weights.revision_key());
    }
    Some(revision)
}

/// One connection per indexing worker. No cache access belongs on the UI thread.
pub(super) struct FeatureCache(Connection);

impl FeatureCache {
    pub(super) fn open() -> Result<Self, String> {
        // models/ and photo-similarity.sqlite share the application's XDG cache root.
        Self::open_path(
            &models_cache_dir()
                .parent()
                .ok_or("Missing cache parent")?
                .join("photo-similarity.sqlite"),
        )
    }

    pub(super) fn open_path(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS features (
                path BLOB PRIMARY KEY NOT NULL,
                source BLOB NOT NULL,
                encoder BLOB NOT NULL,
                vector BLOB NOT NULL
             );",
            )
            .map_err(|e| e.to_string())?;
        Ok(Self(connection))
    }

    pub(super) fn load(
        &self,
        path: &Path,
        source: &FileSource,
        model: Option<&[u8]>,
    ) -> Result<Option<FeatureVector>, String> {
        let encoder = model.unwrap_or(DHASH_REVISION);
        let bytes: Option<Vec<u8>> = self
            .0
            .query_row(
                "SELECT vector FROM features WHERE path=?1 AND source=?2 AND encoder=?3",
                params![path.as_os_str().as_bytes(), source.revision_key(), encoder],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(bytes.and_then(|bytes| decode(&bytes, model.is_some())))
    }

    pub(super) fn store(
        &self,
        path: &Path,
        source: &FileSource,
        model: Option<&[u8]>,
        feature: &FeatureVector,
    ) -> Result<(), String> {
        // Revalidate the requested name too: a symlink may have changed its target.
        if FileSource::capture(path)? != *source {
            return Err("Image changed during feature extraction".into());
        }
        let (encoder, bytes) = match feature {
            FeatureVector::DHash(hash) => (DHASH_REVISION, hash.to_le_bytes().to_vec()),
            FeatureVector::Dense(values) => {
                let encoder = model.ok_or("Missing neural model revision")?;
                let bytes: Vec<u8> = values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect();
                if decode(&bytes, true).is_none() {
                    return Err("Invalid neural feature vector".into());
                }
                (encoder, bytes)
            }
        };
        self.0
            .execute(
                "INSERT OR REPLACE INTO features(path,source,encoder,vector) VALUES(?1,?2,?3,?4)",
                params![
                    path.as_os_str().as_bytes(),
                    source.revision_key(),
                    encoder,
                    bytes
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn decode(bytes: &[u8], neural: bool) -> Option<FeatureVector> {
    if !neural {
        return Some(FeatureVector::DHash(u64::from_le_bytes(
            bytes.try_into().ok()?,
        )));
    }
    if bytes.len() != VISION_DIMENSIONS * 4 {
        return None;
    }
    let values: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let norm: f32 = values.iter().map(|v| v * v).sum();
    (values.iter().all(|v| v.is_finite()) && (norm - 1.0).abs() < 0.01)
        .then_some(FeatureVector::Dense((values).into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    fn vector() -> FeatureVector {
        let mut values = vec![0.0; VISION_DIMENSIONS];
        values[0] = 1.0;
        FeatureVector::Dense((values).into())
    }

    #[test]
    fn reopening_preserves_features_and_byte_exact_paths() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("photos.sqlite");
        let path = directory
            .path()
            .join(OsString::from_vec(b"image-\xff.png".to_vec()));
        std::fs::write(&path, b"fixture").unwrap();
        let source = FileSource::capture(&path).unwrap();
        let cache = FeatureCache::open_path(&db).unwrap();
        cache
            .store(&path, &source, Some(b"model-v1"), &vector())
            .unwrap();
        drop(cache);
        let cache = FeatureCache::open_path(&db).unwrap();
        assert_eq!(
            cache.load(&path, &source, Some(b"model-v1")).unwrap(),
            Some(vector())
        );
        assert_eq!(cache.load(&path, &source, Some(b"model-v2")).unwrap(), None);
        assert_eq!(cache.load(&path, &source, None).unwrap(), None);
        let other = directory
            .path()
            .join(OsString::from_vec(b"image-\xfe.png".to_vec()));
        std::fs::write(&other, b"fixture").unwrap();
        assert_eq!(
            cache.load(&other, &source, Some(b"model-v1")).unwrap(),
            None
        );
    }

    #[test]
    fn changed_source_is_neither_reused_nor_published() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        std::fs::write(&path, b"before").unwrap();
        let source = FileSource::capture(&path).unwrap();
        let cache = FeatureCache::open_path(&directory.path().join("photos.sqlite")).unwrap();
        cache
            .store(&path, &source, None, &FeatureVector::DHash(42))
            .unwrap();
        std::fs::write(&path, b"after edit").unwrap();
        let changed = FileSource::capture(&path).unwrap();
        assert_eq!(cache.load(&path, &changed, None).unwrap(), None);
        assert!(
            cache
                .store(&path, &source, None, &FeatureVector::DHash(43))
                .is_err()
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"before").unwrap();
        assert_eq!(
            cache
                .load(&path, &FileSource::capture(&path).unwrap(), None)
                .unwrap(),
            None
        );
    }

    #[test]
    fn malformed_vectors_are_cache_misses() {
        assert!(decode(&[0; 7], false).is_none());
        assert!(decode(&[0; 4], true).is_none());
        assert!(decode(&vec![0; VISION_DIMENSIONS * 4], true).is_none());
        let mut bad = vec![0.0; VISION_DIMENSIONS];
        bad[0] = f32::NAN;
        let bytes: Vec<u8> = bad.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert!(decode(&bytes, true).is_none());
        assert_eq!(
            decode(&42u64.to_le_bytes(), false),
            Some(FeatureVector::DHash(42))
        );
    }
}
