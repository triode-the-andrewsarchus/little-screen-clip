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
        ensure_model, get_ort_environment,
    },
};

/// Batch size for SigLIP 2 vision inference.
pub const SIGLIP_BATCH_SIZE: usize = 8;
pub const SIGLIP_INPUT_SIZE: u32 = 256;

pub struct SiglipEmbedder {
    session: ort::session::Session,
}

impl SiglipEmbedder {
    /// Attempts to load the SigLIP 2 Giant ONNX vision model from local cache.
    pub fn try_new() -> Result<Self, String> {
        let model_path = ensure_model(SIGLIP2_GIANT_MODEL_NAME, None)
            .map_err(|e| format!("Failed to ensure SigLIP 2 model: {e}"))?;

        let env = get_ort_environment()?;
        let session = ort::session::Session::builder(&env)
            .map_err(|e| format!("ORT builder failed: {e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| format!("ORT SigLIP 2 model load failed: {e}"))?;

        Ok(Self { session })
    }

    /// Embeds a batch of images into L2-normalized feature vectors.
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
                        let (r, g, b_val) = if a > 0 {
                            (
                                ((thumb.premultiplied_rgba[p_idx] as u32) * 255 / a).min(255)
                                    as f32,
                                ((thumb.premultiplied_rgba[p_idx + 1] as u32) * 255 / a).min(255)
                                    as f32,
                                ((thumb.premultiplied_rgba[p_idx + 2] as u32) * 255 / a).min(255)
                                    as f32,
                            )
                        } else {
                            (0.0, 0.0, 0.0)
                        };

                        // SigLIP normalization: (pixel / 127.5) - 1.0 -> range [-1.0, 1.0]
                        batch_arr[[idx, 0, y, x]] = (r / 127.5) - 1.0;
                        batch_arr[[idx, 1, y, x]] = (g / 127.5) - 1.0;
                        batch_arr[[idx, 2, y, x]] = (b_val / 127.5) - 1.0;
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

        let out_tensor = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

        // 3. Extract and L2-normalize embeddings for each image in batch
        let mut results = Vec::with_capacity(b);

        if out_tensor.ndim() == 2 {
            // Shape: [B, D]
            let dim = out_tensor.shape()[1];
            for i in 0..b {
                let mut vec = Vec::with_capacity(dim);
                for d in 0..dim {
                    vec.push(out_tensor[[i, d]]);
                }
                normalize_l2(&mut vec);
                results.push(FeatureVector::Dense(vec));
            }
        } else if out_tensor.ndim() == 3 {
            // Shape: [B, Tokens, D] -> Mean pool over tokens
            let tokens = out_tensor.shape()[1];
            let dim = out_tensor.shape()[2];
            let inv_tokens = 1.0 / (tokens as f32);

            for i in 0..b {
                let mut vec = vec![0.0f32; dim];
                for t in 0..tokens {
                    for d in 0..dim {
                        vec[d] += out_tensor[[i, t, d]];
                    }
                }
                for d in 0..dim {
                    vec[d] *= inv_tokens;
                }
                normalize_l2(&mut vec);
                results.push(FeatureVector::Dense(vec));
            }
        } else {
            return Err(format!(
                "Unexpected output tensor ndim: {}",
                out_tensor.ndim()
            ));
        }

        Ok(results)
    }
}

/// L2-normalizes an embedding vector in-place.
pub fn normalize_l2(vec: &mut [f32]) {
    let norm_sq: f32 = vec.iter().map(|&x| x * x).sum();
    let norm = norm_sq.sqrt();
    if norm > 1e-7 {
        let inv_norm = 1.0 / norm;
        for x in vec.iter_mut() {
            *x *= inv_norm;
        }
    }
}

pub struct SiglipTextEmbedder {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
}

impl SiglipTextEmbedder {
    /// Attempts to load the SigLIP 2 Giant text encoder and tokenizer from local cache or downloads.
    pub fn try_new() -> Result<Self, String> {
        let model_path = ensure_model(SIGLIP2_GIANT_TEXT_MODEL_NAME, None)
            .map_err(|e| format!("Failed to ensure SigLIP 2 text model: {e}"))?;
        let tokenizer_path = ensure_model(SIGLIP2_GIANT_TOKENIZER_NAME, None)
            .map_err(|e| format!("Failed to ensure SigLIP 2 tokenizer: {e}"))?;

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| format!("Failed to load SigLIP 2 tokenizer: {e}"))?;

        let env = get_ort_environment()?;
        let session = ort::session::Session::builder(&env)
            .map_err(|e| format!("ORT builder failed: {e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| format!("ORT SigLIP 2 text model load failed: {e}"))?;

        Ok(Self { session, tokenizer })
    }

    /// Encodes a text query (e.g. "cat", "nature") into a normalized 1536-d feature vector.
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
        let seq_len = raw_ids.len().clamp(1, 64);
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

        let out_tensor = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| format!("Failed to extract output tensor: {e}"))?;

        let mut embedding = if out_tensor.ndim() == 2 {
            let dim = out_tensor.shape()[1];
            let mut vec = Vec::with_capacity(dim);
            for d in 0..dim {
                vec.push(out_tensor[[0, d]]);
            }
            vec
        } else if out_tensor.ndim() == 3 {
            let tokens = out_tensor.shape()[1];
            let dim = out_tensor.shape()[2];
            let inv_tokens = 1.0 / (tokens as f32);
            let mut vec = vec![0.0f32; dim];
            for t in 0..tokens {
                for d in 0..dim {
                    vec[d] += out_tensor[[0, t, d]];
                }
            }
            for d in 0..dim {
                vec[d] *= inv_tokens;
            }
            vec
        } else {
            return Err(format!(
                "Unexpected text output tensor ndim: {}",
                out_tensor.ndim()
            ));
        };

        normalize_l2(&mut embedding);
        Ok(embedding)
    }
}
