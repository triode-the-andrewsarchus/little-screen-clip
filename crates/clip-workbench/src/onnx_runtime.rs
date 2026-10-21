use std::path::PathBuf;
use std::sync::OnceLock;

pub use little_clip_core::config::{
    DEFAULT_BIG_LAMA_MODEL_URL, DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL, DEFAULT_FEYNOBG_FP16_MODEL_URL,
};

pub const BIG_LAMA_MODEL_NAME: &str = "big-lama.onnx";
pub const BIG_LAMA_MODEL_URL: &str = DEFAULT_BIG_LAMA_MODEL_URL;

pub const FEYNOBG_FP16_MODEL_NAME: &str = "feynobg-fp16.onnx";
pub const FEYNOBG_FP16_MODEL_URL: &str = DEFAULT_FEYNOBG_FP16_MODEL_URL;

pub const RMBG_14_MODEL_NAME: &str = FEYNOBG_FP16_MODEL_NAME;
pub const RMBG_14_MODEL_URL: &str = FEYNOBG_FP16_MODEL_URL;

pub const DEPTH_ANYTHING_V2_MODEL_NAME: &str = "depth-anything-v2-small.onnx";
pub const DEPTH_ANYTHING_V2_MODEL_URL: &str = DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL;

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
        _ => String::new(),
    }
}

/// Ensure that a named ONNX model is available locally in the cache directory,
/// downloading it from the configured or default URL if necessary.
pub fn ensure_model(model_name: &str, url_override: Option<&str>) -> Result<PathBuf, String> {
    let url = resolve_model_url(model_name, url_override);
    if url.is_empty() {
        return Err(format!("No download URL configured for model {model_name}"));
    }
    ensure_model_with_url(model_name, &url)
}

/// Ensure that a named ONNX model is available locally in the cache directory,
/// downloading it from a specific URL if necessary.
pub fn ensure_model_with_url(model_name: &str, url: &str) -> Result<PathBuf, String> {
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
    let status = std::process::Command::new("curl")
        .arg("-L")
        .arg("-f")
        .arg("--connect-timeout")
        .arg("15")
        .arg("-o")
        .arg(&tmp_path)
        .arg(url)
        .status();

    match status {
        Ok(st) if st.success() && tmp_path.exists() => {
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
            Ok(model_path)
        }
        Ok(st) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(format!("curl download failed with exit code {st}"))
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(format!("Failed to run curl to download model: {err}"))
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
    }
}
