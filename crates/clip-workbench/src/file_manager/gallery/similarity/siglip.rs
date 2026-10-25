//! SigLIP 2 Giant ONNX image & text feature embedder with batched inference.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use ndarray::{Array2, Array4};
use ort::value::Tensor;

use super::embedding::FeatureVector;
use crate::{
    file_manager::gallery::thumbnail,
    onnx_runtime::{
        SIGLIP2_GIANT_MODEL_NAME, SIGLIP2_GIANT_TEXT_MODEL_NAME, SIGLIP2_GIANT_TOKENIZER_NAME,
        ensure_model, load_session,
    },
};

/// Batch size for SigLIP 2 vision inference.
pub const SIGLIP_BATCH_SIZE: usize = 6;
/// Pixel dimension (width and height) for input images fed to the SigLIP 2 vision encoder.
pub const SIGLIP_INPUT_SIZE: u32 = 256;
/// Maximum sequence length for tokenized text inputs.
pub const SIGLIP_MAX_SEQ_LEN: usize = 64;
const PIXEL_SCALE: f32 = 127.5;
const MIN_NORM_EPSILON: f32 = 1e-7;

/// Vision feature extractor backed by a SigLIP 2 Giant ONNX model.
pub struct SiglipEmbedder {
    session: ort::session::Session,
    /// Revision captured around session creation, rather than after inference.
    pub(super) encoder_revision: Vec<u8>,
}

impl SiglipEmbedder {
    /// Attempts to load the SigLIP 2 Giant ONNX vision model from local cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the model file cannot be located, downloaded, or initialized into an
    /// ONNX Runtime session.
    pub fn try_new() -> Result<Self, String> {
        let model_path = ensure_model(SIGLIP2_GIANT_MODEL_NAME, None)
            .map_err(|e| format!("Failed to ensure SigLIP 2 model: {e}"))?;

        let encoder_revision =
            super::cache::model_revision().ok_or("Cannot identify SigLIP vision model")?;
        let session = load_session(&model_path)
            .map_err(|e| format!("ORT SigLIP 2 model load failed: {e}"))?;
        if super::cache::model_revision().as_ref() != Some(&encoder_revision) {
            return Err("SigLIP vision model changed while loading".into());
        }
        Ok(Self {
            session,
            encoder_revision,
        })
    }

    /// Retries images individually after a batch failure, preserving healthy neural features.
    pub fn embed_resilient(
        &mut self,
        paths: &[PathBuf],
        cancelled: &AtomicBool,
    ) -> Vec<Option<FeatureVector>> {
        embed_with_isolation(paths, cancelled, |paths| self.embed_batch(paths, cancelled))
    }

    /// Embeds a batch of images into L2-normalized feature vectors.
    ///
    /// # Errors
    ///
    /// Returns an error if ONNX tensor execution fails.
    pub fn embed_batch(
        &mut self,
        paths: &[PathBuf],
        cancelled: &AtomicBool,
    ) -> Result<Vec<FeatureVector>, String> {
        let b = paths.len();
        if b == 0 {
            return Ok(Vec::new());
        }

        // 1. Preprocess images into [B, 3, 256, 256] array
        let mut batch_arr =
            Array4::<f32>::zeros((b, 3, SIGLIP_INPUT_SIZE as usize, SIGLIP_INPUT_SIZE as usize));

        for (idx, path) in paths.iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }

            let thumb = thumbnail::load(path, SIGLIP_INPUT_SIZE, 1, cancelled)
                .ok_or_else(|| format!("Failed to load thumbnail for {}", path.display()))?;

            let src_w = thumb.width as usize;
            let src_h = thumb.height as usize;
            let target_size = SIGLIP_INPUT_SIZE as usize;

            for y in 0..target_size {
                let sy = (y * src_h) / target_size;
                let row_offset = sy * src_w * 4;
                for x in 0..target_size {
                    let sx = (x * src_w) / target_size;
                    let p_idx = row_offset + sx * 4;
                    if p_idx + 2 < thumb.premultiplied_rgba.len() {
                        let a = thumb
                            .premultiplied_rgba
                            .get(p_idx + 3)
                            .copied()
                            .unwrap_or(255) as u32;
                        let (r, g, b_val) = if let Some(a_val) = std::num::NonZeroU32::new(a) {
                            (
                                ((thumb.premultiplied_rgba[p_idx] as u32 * 255) / a_val.get())
                                    .min(255) as f32,
                                ((thumb.premultiplied_rgba[p_idx + 1] as u32 * 255) / a_val.get())
                                    .min(255) as f32,
                                ((thumb.premultiplied_rgba[p_idx + 2] as u32 * 255) / a_val.get())
                                    .min(255) as f32,
                            )
                        } else {
                            (0.0, 0.0, 0.0)
                        };

                        // SigLIP normalization: (pixel / PIXEL_SCALE) - 1.0 -> range [-1.0, 1.0]
                        batch_arr[[idx, 0, y, x]] = (r / PIXEL_SCALE) - 1.0;
                        batch_arr[[idx, 1, y, x]] = (g / PIXEL_SCALE) - 1.0;
                        batch_arr[[idx, 2, y, x]] = (b_val / PIXEL_SCALE) - 1.0;
                    }
                }
            }
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }

        // 2. Run ONNX Runtime inference
        let input_tensor = Tensor::from_array(batch_arr)
            .map_err(|e| format!("ORT tensor conversion failed: {e}"))?;

        let outputs = self
            .session
            .run(ort::inputs![input_tensor])
            .map_err(|e| format!("SigLIP 2 inference failed: {e}"))?;

        let out_tensor = if let Some(v) = outputs.get("pooler_output") {
            v.try_extract_array::<f32>()
        } else {
            outputs[0].try_extract_array::<f32>()
        }
        .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

        // 3. Extract and L2-normalize embeddings for each image in batch
        let embeddings = extract_normalized_embeddings(&out_tensor, b)?;
        Ok(embeddings
            .into_iter()
            .map(|embedding| FeatureVector::Dense(embedding.into()))
            .collect())
    }
}

fn embed_with_isolation(
    paths: &[PathBuf],
    cancelled: &AtomicBool,
    mut embed: impl FnMut(&[PathBuf]) -> Result<Vec<FeatureVector>, String>,
) -> Vec<Option<FeatureVector>> {
    match embed(paths) {
        Ok(features) => features.into_iter().map(Some).collect(),
        Err(error) => {
            tracing::warn!(%error, "SigLIP batch failed; isolating individual images");
            paths
                .iter()
                .map(|path| {
                    if cancelled.load(Ordering::Relaxed) {
                        return None;
                    }
                    embed(std::slice::from_ref(path)).ok()?.into_iter().next()
                })
                .collect()
        }
    }
}

/// Extracts embeddings from a 2D `[B, D]` or 3D `[B, Tokens, D]` output tensor and normalizes them.
fn extract_normalized_embeddings(
    out_tensor: &ndarray::ArrayViewD<'_, f32>,
    batch_size: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let mut results = Vec::with_capacity(batch_size);

    if out_tensor.ndim() == 2 {
        // Shape: [B, D]
        let dim = out_tensor.shape()[1];
        for i in 0..batch_size {
            let mut vec = Vec::with_capacity(dim);
            for d in 0..dim {
                vec.push(out_tensor[[i, d]]);
            }
            normalize_l2(&mut vec);
            results.push(vec);
        }
    } else if out_tensor.ndim() == 3 {
        // Shape: [B, Tokens, D] -> Mean pool over tokens
        let tokens = out_tensor.shape()[1];
        let dim = out_tensor.shape()[2];
        let inv_tokens = 1.0 / (tokens as f32);

        for i in 0..batch_size {
            let mut vec = vec![0.0f32; dim];
            for t in 0..tokens {
                for d in 0..dim {
                    vec[d] += out_tensor[[i, t, d]];
                }
            }
            for val in &mut vec {
                *val *= inv_tokens;
            }
            normalize_l2(&mut vec);
            results.push(vec);
        }
    } else {
        return Err(format!(
            "Unexpected output tensor ndim: {}",
            out_tensor.ndim()
        ));
    }

    Ok(results)
}

/// Normalizes a floating-point slice to unit L2 length in-place.
///
/// If the vector norm is below an epsilon threshold, the slice is left unchanged.
pub fn normalize_l2(vec: &mut [f32]) {
    let norm_sq: f32 = vec.iter().map(|&x| x * x).sum();
    let norm = norm_sq.sqrt();
    if norm > MIN_NORM_EPSILON {
        let inv_norm = 1.0 / norm;
        for x in vec.iter_mut() {
            *x *= inv_norm;
        }
    }
}

/// Text feature extractor backed by a SigLIP 2 Giant ONNX model and tokenizer.
pub struct SiglipTextEmbedder {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
}

impl SiglipTextEmbedder {
    /// Attempts to load the SigLIP 2 Giant text encoder and tokenizer from local cache or downloads.
    ///
    /// # Errors
    ///
    /// Returns an error if the model or tokenizer cannot be loaded.
    pub fn try_new() -> Result<Self, String> {
        let model_path = ensure_model(SIGLIP2_GIANT_TEXT_MODEL_NAME, None)
            .map_err(|e| format!("Failed to ensure SigLIP 2 text model: {e}"))?;
        let tokenizer_path = ensure_model(SIGLIP2_GIANT_TOKENIZER_NAME, None)
            .map_err(|e| format!("Failed to ensure SigLIP 2 tokenizer: {e}"))?;

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| format!("Failed to load SigLIP 2 tokenizer: {e}"))?;

        let session = load_session(&model_path)
            .map_err(|e| format!("ORT SigLIP 2 text model load failed: {e}"))?;

        Ok(Self { session, tokenizer })
    }

    /// Encodes a text query (e.g. "cat", "nature") into a normalized 1536-d feature vector.
    ///
    /// # Errors
    ///
    /// Returns an error if the query text is empty, tokenization fails, or tensor execution fails.
    pub fn embed_text(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Empty search query".into());
        }

        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("Tokenization failed: {e}"))?;

        let raw_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let seq_len = raw_ids.len().clamp(1, SIGLIP_MAX_SEQ_LEN);
        let mut input_arr = Array2::<i64>::zeros((1, seq_len));
        for (i, &id) in raw_ids.iter().take(seq_len).enumerate() {
            input_arr[[0, i]] = id;
        }

        let input_tensor = Tensor::from_array(input_arr)
            .map_err(|e| format!("ORT tensor conversion failed: {e}"))?;

        let has_mask = self
            .session
            .inputs()
            .iter()
            .any(|inp| inp.name() == "attention_mask");

        let outputs = if has_mask {
            let mask_arr = Array2::<i64>::ones((1, seq_len));
            let mask_tensor = Tensor::from_array(mask_arr)
                .map_err(|e| format!("ORT mask tensor conversion failed: {e}"))?;
            self.session
                .run(ort::inputs![input_tensor, mask_tensor])
                .map_err(|e| format!("SigLIP 2 text inference failed: {e}"))?
        } else {
            self.session
                .run(ort::inputs![input_tensor])
                .map_err(|e| format!("SigLIP 2 text inference failed: {e}"))?
        };

        let out_tensor = if let Some(v) = outputs.get("pooler_output") {
            v.try_extract_array::<f32>()
        } else {
            outputs[0].try_extract_array::<f32>()
        }
        .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

        let mut embeddings = extract_normalized_embeddings(&out_tensor, 1)?;
        embeddings
            .pop()
            .ok_or_else(|| "Failed to extract text embedding".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_pixel_batch(batch_size: usize) -> Array4<f32> {
        Array4::zeros((
            batch_size,
            3,
            SIGLIP_INPUT_SIZE as usize,
            SIGLIP_INPUT_SIZE as usize,
        ))
    }

    #[test]
    fn failed_image_does_not_downgrade_other_batch_members() {
        let paths = [
            PathBuf::from("good"),
            PathBuf::from("bad"),
            PathBuf::from("good2"),
        ];
        let features = embed_with_isolation(&paths, &AtomicBool::new(false), |paths| {
            if paths.len() != 1 || paths[0] == PathBuf::from("bad") {
                return Err("decode failure".into());
            }
            Ok(vec![FeatureVector::Dense((vec![1.0]).into())])
        });
        assert!(features[0].is_some());
        assert!(features[1].is_none());
        assert!(features[2].is_some());
    }

    /// Requires the cached vision model and a working CUDA runtime. Unlike provider
    /// availability checks, the ORT trace proves that model kernels execute on CUDA.
    #[test]
    #[ignore = "requires cached SigLIP model and CUDA GPU"]
    fn vision_kernels_execute_on_cuda() {
        crate::onnx_runtime::assert_cuda_execution(SIGLIP2_GIANT_MODEL_NAME, |session| {
            let input = Tensor::from_array(zero_pixel_batch(1)).unwrap();
            let outputs = session.run(ort::inputs![input]).unwrap();
            assert_eq!(outputs[0].try_extract_array::<f32>().unwrap().shape()[0], 1);
        });
    }

    /// Reports repeated CUDA inference timing for one externally selected safe batch size.
    /// Run as separate processes so each size starts from the same model allocation baseline.
    #[test]
    #[ignore = "opt-in RTX benchmark; run one SIGLIP_BENCH_BATCH_SIZE (1..=8) per process"]
    fn benchmark_siglip_cuda_batch_size() {
        let batch_size = std::env::var("SIGLIP_BENCH_BATCH_SIZE")
            .expect("set SIGLIP_BENCH_BATCH_SIZE to a value from 1 through 8")
            .parse::<usize>()
            .expect("batch size must be an integer");
        assert!(
            (1..=8).contains(&batch_size),
            "batch size exceeds shared-GPU benchmark ceiling"
        );
        crate::onnx_runtime::assert_cuda_execution(SIGLIP2_GIANT_MODEL_NAME, |session| {
            let mut run = || {
                let input = Tensor::from_array(zero_pixel_batch(batch_size)).unwrap();
                let outputs = session.run(ort::inputs![input]).unwrap();
                assert_eq!(
                    outputs[0].try_extract_array::<f32>().unwrap().shape()[0],
                    batch_size
                );
            };
            for _ in 0..2 {
                run();
            }
            let mut timings = (0..5)
                .map(|_| {
                    let start = std::time::Instant::now();
                    run();
                    start.elapsed().as_secs_f64()
                })
                .collect::<Vec<_>>();
            timings.sort_by(f64::total_cmp);
            let median = timings[timings.len() / 2];
            eprintln!(
                "SIGLIP_BENCH batch={batch_size} median_ms={:.2} images_per_second={:.2}",
                median * 1000.0,
                batch_size as f64 / median,
            );
        });
    }

    #[test]
    #[ignore = "requires cached SigLIP encoders and cat/burger fixtures in SIMILARITY_SAMPLES_DIR"]
    fn test_siglip_multimodal_alignment() {
        let samples_dir = std::env::var_os("SIMILARITY_SAMPLES_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_default()
                    .join("Pictures/similarity-samples")
            });
        let cat_path = samples_dir.join("n02123394_Persian_cat.JPEG");
        let burger_path = samples_dir.join("n07697313_cheeseburger.JPEG");
        assert!(
            cat_path.is_file() && burger_path.is_file(),
            "Set SIMILARITY_SAMPLES_DIR to the cat/burger fixture directory"
        );
        let mut vision = SiglipEmbedder::try_new().expect("Initialize vision encoder");
        let mut text = SiglipTextEmbedder::try_new().expect("Initialize text encoder");
        let vectors = vision
            .embed_batch(&[cat_path, burger_path], &AtomicBool::new(false))
            .unwrap();
        let cat = vectors[0].to_vec();
        let burger = vectors[1].to_vec();
        let cat_text = text.embed_text("cat").unwrap();
        let food_text = text.embed_text("food").unwrap();
        let score = |a: &[f32], b: &[f32]| {
            assert_eq!(a.len(), b.len(), "Encoders must share a feature space");
            a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>()
        };
        assert!(
            score(&cat, &cat_text) > score(&cat, &food_text) + 0.05,
            "Cat must match 'cat' better than 'food'"
        );
        assert!(
            score(&burger, &food_text) > score(&burger, &cat_text) + 0.05,
            "Burger must match 'food' better than 'cat'"
        );
    }
}
