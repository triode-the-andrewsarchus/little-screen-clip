//! Incremental worker-side indexing: disk hits never initialize the neural model.
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use super::{
    cache::{FeatureCache, model_revision},
    dhash::compute_dhash_from_path,
    embedding::FeatureVector,
    siglip::{SIGLIP_BATCH_SIZE, SiglipEmbedder},
};
use crate::{file_manager::filesystem::check_revision, file_source::FileSource};

type Batch = Vec<(PathBuf, u64, FeatureVector)>;

pub(super) fn index_images(
    entries: Vec<(PathBuf, u64)>,
    cancelled: &AtomicBool,
    output: Sender<Batch>,
) {
    let cache = FeatureCache::open().map_err(|error| {
        tracing::warn!(%error, "Photo embedding cache unavailable; indexing without persistence");
    }).ok();
    index_with_cache(entries, cancelled, output, cache, || {
        SiglipEmbedder::try_new()
            .map_err(|error| {
                tracing::warn!(%error, "SigLIP initialization failed; using perceptual hashes");
            })
            .ok()
    });
}

fn index_with_cache(
    entries: Vec<(PathBuf, u64)>,
    cancelled: &AtomicBool,
    output: Sender<Batch>,
    cache: Option<FeatureCache>,
    mut initialize: impl FnMut() -> Option<SiglipEmbedder>,
) {
    let mut encoder = model_revision();
    // The outer Option records whether initialization was attempted, including failure.
    let mut embedder: Option<Option<SiglipEmbedder>> = None;
    for chunk in entries.chunks(SIGLIP_BATCH_SIZE) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let (mut results, missing) =
            cached_features(chunk, cache.as_ref(), encoder.as_deref(), cancelled);
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        // Publish persistent hits before any potentially expensive initialization.
        if !results.is_empty() && output.send(std::mem::take(&mut results)).is_err() {
            return;
        }
        if missing.is_empty() {
            continue;
        }
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let neural = embedder.get_or_insert_with(|| {
            let result = initialize();
            // Model download may have completed during initialization.
            if let Some(model) = &result {
                encoder = Some(model.encoder_revision.clone());
            }
            result
        });
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        let paths: Vec<PathBuf> = missing.iter().map(|(path, _, _)| (*path).clone()).collect();
        let features = neural
            .as_mut()
            .map(|model| model.embed_resilient(&paths, cancelled));
        results = complete_batch(
            missing,
            features,
            cache.as_ref(),
            encoder.as_deref(),
            cancelled,
        );
        if cancelled.load(Ordering::Relaxed) {
            return;
        }
        if !results.is_empty() && output.send(results).is_err() {
            return;
        }
    }
}

type PendingFeature = (PathBuf, u64, FileSource);

fn cached_features(
    entries: &[(PathBuf, u64)],
    cache: Option<&FeatureCache>,
    encoder: Option<&[u8]>,
    cancelled: &AtomicBool,
) -> (Batch, Vec<PendingFeature>) {
    let mut results = Vec::new();
    let mut missing = Vec::new();
    for (path, revision) in entries {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let Ok(source) = FileSource::capture(path) else {
            continue;
        };
        if check_revision(path, *revision).is_err() {
            continue;
        }
        let cached = cache.and_then(|cache| {
            cache.load(path, &source, encoder).unwrap_or_else(|error| {
                tracing::warn!(%error, "Cannot read photo embedding cache");
                None
            })
        });
        if let Some(feature) = cached {
            if FileSource::capture(path).ok().as_ref() == Some(&source) {
                results.push((path.clone(), *revision, feature));
            }
        } else {
            missing.push((path.clone(), *revision, source));
        }
    }
    (results, missing)
}

fn complete_batch(
    missing: Vec<PendingFeature>,
    features: Option<Vec<Option<FeatureVector>>>,
    cache: Option<&FeatureCache>,
    encoder: Option<&[u8]>,
    cancelled: &AtomicBool,
) -> Batch {
    let mut results = Vec::new();
    let stable_model = model_revision().as_deref() == encoder;
    for (index, (path, revision, source)) in missing.into_iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let feature = match &features {
            Some(features) if stable_model => features
                .get(index)
                .cloned()
                .flatten()
                .or_else(|| compute_dhash_from_path(&path, cancelled).map(FeatureVector::DHash)),
            Some(_) => None, // Model was replaced while inference was in flight.
            None => compute_dhash_from_path(&path, cancelled).map(FeatureVector::DHash),
        };
        let Some(feature) = feature else { continue };
        if FileSource::capture(&path).ok().as_ref() != Some(&source) {
            continue;
        }
        if let Some(cache) = cache {
            if let Err(error) = cache.store(&path, &source, encoder, &feature) {
                tracing::warn!(%error, "Cannot persist photo embedding");
            }
        }
        results.push((path, revision, feature));
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn restart_loads_cached_features_without_initializing_inference() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        std::fs::write(&path, b"fixture").unwrap();
        let entry = crate::file_manager::filesystem::read_entry(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .unwrap()
                .unwrap(),
        );
        let database = directory.path().join("photos.sqlite");
        let source = FileSource::capture(&path).unwrap();
        let encoder = model_revision();
        let feature = if encoder.is_some() {
            let mut values = vec![0.0; 1536];
            values[0] = 1.0;
            FeatureVector::Dense((values).into())
        } else {
            FeatureVector::DHash(123)
        };
        let cache = FeatureCache::open_path(&database).unwrap();
        cache
            .store(&path, &source, encoder.as_deref(), &feature)
            .unwrap();
        drop(cache);
        let (tx, rx) = mpsc::channel();
        index_with_cache(
            vec![(path.clone(), entry.revision)],
            &AtomicBool::new(false),
            tx,
            Some(FeatureCache::open_path(&database).unwrap()),
            || panic!("A fully cached restart must not initialize inference"),
        );
        assert_eq!(
            rx.into_iter().flatten().collect::<Vec<_>>(),
            vec![(path, entry.revision, feature)]
        );
    }

    #[test]
    fn cancelled_index_does_not_initialize_inference_or_publish() {
        let (tx, rx) = mpsc::channel();
        index_with_cache(
            vec![(PathBuf::from("unused"), 0)],
            &AtomicBool::new(true),
            tx,
            None,
            || panic!("Cancelled indexing must not initialize inference"),
        );
        assert!(rx.into_iter().next().is_none());
    }
}
