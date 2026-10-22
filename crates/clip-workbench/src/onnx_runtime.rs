use std::path::PathBuf;
use std::sync::OnceLock;

pub use little_clip_core::config::{
    DEFAULT_BIG_LAMA_MODEL_URL, DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL,
    DEFAULT_FEYNOBG_FP16_MODEL_URL, DEFAULT_MOBILE_SAM_DECODER_MODEL_URL,
    DEFAULT_MOBILE_SAM_ENCODER_MODEL_URL, DEFAULT_SIGLIP2_GIANT_MODEL_URL,
};

pub const BIG_LAMA_MODEL_NAME: &str = "big-lama.onnx";
pub const BIG_LAMA_MODEL_URL: &str = DEFAULT_BIG_LAMA_MODEL_URL;

pub const FEYNOBG_FP16_MODEL_NAME: &str = "feynobg-fp16.onnx";
pub const FEYNOBG_FP16_MODEL_URL: &str = DEFAULT_FEYNOBG_FP16_MODEL_URL;

pub const RMBG_14_MODEL_NAME: &str = FEYNOBG_FP16_MODEL_NAME;
pub const RMBG_14_MODEL_URL: &str = FEYNOBG_FP16_MODEL_URL;

pub const DEPTH_ANYTHING_V2_MODEL_NAME: &str = "depth-anything-v2-small.onnx";
pub const DEPTH_ANYTHING_V2_MODEL_URL: &str = DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL;

pub const MOBILE_SAM_ENCODER_MODEL_NAME: &str = "mobile_sam_encoder.onnx";
pub const MOBILE_SAM_ENCODER_MODEL_URL: &str = DEFAULT_MOBILE_SAM_ENCODER_MODEL_URL;

pub const MOBILE_SAM_DECODER_MODEL_NAME: &str = "mobile_sam_decoder.onnx";
pub const MOBILE_SAM_DECODER_MODEL_URL: &str = DEFAULT_MOBILE_SAM_DECODER_MODEL_URL;

pub const SIGLIP2_GIANT_MODEL_NAME: &str = "siglip2-giant-vision.onnx";
pub const SIGLIP2_GIANT_MODEL_URL: &str = DEFAULT_SIGLIP2_GIANT_MODEL_URL;

/// Get or initialize the global ONNX Runtime environment.
///
/// ONNX Runtime requires that only a single `Environment` exists per process.
pub fn get_ort_environment() -> Result<ort::environment::Environment, String> {
    static ENV: OnceLock<ort::environment::Environment> = OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env.clone());
    }

    // Protect initialization across threads
    static INIT_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = INIT_MUTEX.lock().map_err(|e| e.to_string())?;
    if let Some(env) = ENV.get() {
        return Ok(env.clone());
    }

    let env = ort::init()
        .build()
        .map_err(|e| format!("ORT init failed: {e}"))?;
    let _ = ENV.set(env.clone());
    Ok(env)
}

/// Resolves the download URL for a named model, checking any explicit override first,
/// then checking the runtime configuration file (`config.toml`), and finally falling back
/// to the default hard-coded URL.
pub fn resolve_model_url(model_name: &str, url_override: Option<&str>) -> String {
    if let Some(url) = url_override {
        if !url.trim().is_empty() {
            return url.trim().to_string();
        }
    }
    let config = little_clip_core::AppConfig::load();
    match model_name {
        BIG_LAMA_MODEL_NAME => config.lama_model_url().to_string(),
        FEYNOBG_FP16_MODEL_NAME | "rmbg-1.4.onnx" => config.rmbg_model_url().to_string(),
        DEPTH_ANYTHING_V2_MODEL_NAME => config.depth_model_url().to_string(),
        MOBILE_SAM_ENCODER_MODEL_NAME => config.sam_encoder_model_url().to_string(),
        MOBILE_SAM_DECODER_MODEL_NAME => config.sam_decoder_model_url().to_string(),
        SIGLIP2_GIANT_MODEL_NAME => config.siglip2_giant_model_url().to_string(),
        _ => String::new(),
    }
}

/// Helper to determine the expected file size in bytes for progress reporting.
pub fn expected_model_size(model_name: &str) -> Option<u64> {
    match model_name {
        FEYNOBG_FP16_MODEL_NAME => Some(563_545_826),
        DEPTH_ANYTHING_V2_MODEL_NAME => Some(99_110_037),
        BIG_LAMA_MODEL_NAME => Some(208_851_664),
        "rmbg-1.4.onnx" => Some(176_153_355),
        MOBILE_SAM_ENCODER_MODEL_NAME => Some(28_157_093),
        MOBILE_SAM_DECODER_MODEL_NAME => Some(16_501_323),
        SIGLIP2_GIANT_MODEL_NAME => Some(1_170_827_444),
        _ => None,
    }
}

/// Ensure that a named ONNX model is available locally in the cache directory,
/// downloading it from the configured or default URL if necessary.
pub fn ensure_model(model_name: &str, url_override: Option<&str>) -> Result<PathBuf, String> {
    ensure_model_with_progress(model_name, url_override, |_, _| {})
}

/// Ensure that a named ONNX model is available locally in the cache directory,
/// downloading it with progress reporting if necessary.
pub fn ensure_model_with_progress<F>(
    model_name: &str,
    url_override: Option<&str>,
    on_progress: F,
) -> Result<PathBuf, String>
where
    F: Fn(f32, &str),
{
    let url = resolve_model_url(model_name, url_override);
    if url.is_empty() {
        return Err(format!("No download URL configured for model {model_name}"));
    }
    ensure_model_with_url_progress(model_name, &url, on_progress)
}

/// Ensure that a named ONNX model is available locally in the cache directory,
/// downloading it from a specific URL with progress reporting if necessary.
pub fn ensure_model_with_url_progress<F>(
    model_name: &str,
    url: &str,
    on_progress: F,
) -> Result<PathBuf, String>
where
    F: Fn(f32, &str),
{
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let models_dir = PathBuf::from(home).join(".cache/little-screen-clip/models");
    let model_path = models_dir.join(model_name);
    if model_path.exists() {
        if let Ok(meta) = std::fs::metadata(&model_path) {
            // Verify file has non-trivial size (> 1MB) so corrupted/stub downloads are discarded
            if meta.len() > 1_000_000 {
                return Ok(model_path);
            }
        }
        let _ = std::fs::remove_file(&model_path);
    }
    let _ = std::fs::create_dir_all(&models_dir);
    let tmp_path = models_dir.join(format!("{}.tmp.{}", model_name, std::process::id()));

    let mut child = std::process::Command::new("curl")
        .arg("-L")
        .arg("-f")
        .arg("--connect-timeout")
        .arg("15")
        .arg("-o")
        .arg(&tmp_path)
        .arg(url)
        .spawn()
        .map_err(|err| format!("Failed to run curl to download model: {err}"))?;

    let expected = expected_model_size(model_name);
    let total_mb = expected.map(|e| e as f32 / 1_000_000.0).unwrap_or(100.0);

    on_progress(0.01, &format!("Connecting to download {model_name}..."));

    while let Ok(None) = child.try_wait() {
        std::thread::sleep(std::time::Duration::from_millis(150));
        if let Ok(meta) = std::fs::metadata(&tmp_path) {
            let dl_bytes = meta.len();
            let dl_mb = dl_bytes as f32 / 1_000_000.0;
            let frac = if let Some(tot) = expected {
                (dl_bytes as f32 / tot as f32).clamp(0.01, 0.99)
            } else {
                (dl_mb / (dl_mb + 50.0)).clamp(0.01, 0.99)
            };
            on_progress(
                frac,
                &format!(
                    "Downloading {model_name}: {:.0}% ({:.1}/{:.1} MB)",
                    frac * 100.0,
                    dl_mb,
                    total_mb
                ),
            );
        }
    }

    let status = child.wait().map_err(|e| format!("curl wait failed: {e}"))?;

    match status {
        st if st.success() && tmp_path.exists() => {
            if let Ok(meta) = std::fs::metadata(&tmp_path) {
                if meta.len() <= 1_000_000 {
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(format!(
                        "Downloaded model {model_name} is too small ({} bytes)",
                        meta.len()
                    ));
                }
            }
            std::fs::rename(&tmp_path, &model_path)
                .map_err(|e| format!("Failed to save downloaded model: {e}"))?;
            on_progress(1.0, &format!("Model {model_name} download complete"));
            Ok(model_path)
        }
        st => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(format!("curl download failed with exit code {st}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_model_url_with_explicit_override() {
        let custom = "https://custom.domain/my-model.onnx";
        assert_eq!(resolve_model_url(BIG_LAMA_MODEL_NAME, Some(custom)), custom);
        assert_eq!(resolve_model_url(RMBG_14_MODEL_NAME, Some(custom)), custom);
        assert_eq!(
            resolve_model_url(DEPTH_ANYTHING_V2_MODEL_NAME, Some(custom)),
            custom
        );
        assert_eq!(
            resolve_model_url(MOBILE_SAM_ENCODER_MODEL_NAME, Some(custom)),
            custom
        );
        assert_eq!(
            resolve_model_url(MOBILE_SAM_DECODER_MODEL_NAME, Some(custom)),
            custom
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_MODEL_NAME, Some(custom)),
            custom
        );
    }

    #[test]
    fn test_resolve_model_url_default_fallback() {
        assert_eq!(
            resolve_model_url(BIG_LAMA_MODEL_NAME, None),
            BIG_LAMA_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(RMBG_14_MODEL_NAME, None),
            RMBG_14_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(DEPTH_ANYTHING_V2_MODEL_NAME, None),
            DEPTH_ANYTHING_V2_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(MOBILE_SAM_ENCODER_MODEL_NAME, None),
            MOBILE_SAM_ENCODER_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(MOBILE_SAM_DECODER_MODEL_NAME, None),
            MOBILE_SAM_DECODER_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_MODEL_NAME, None),
            DEFAULT_SIGLIP2_GIANT_MODEL_URL
        );
    }
}
