use std::path::PathBuf;
use std::sync::OnceLock;

pub use little_clip_core::config::{
    DEFAULT_BIG_LAMA_MODEL_URL, DEFAULT_DEPTH_ANYTHING_V2_MODEL_URL,
    DEFAULT_FEYNOBG_FP16_MODEL_URL, DEFAULT_MOBILE_SAM_DECODER_MODEL_URL,
    DEFAULT_MOBILE_SAM_ENCODER_MODEL_URL, DEFAULT_SIGLIP2_GIANT_MODEL_URL,
    DEFAULT_SIGLIP2_GIANT_TEXT_MODEL_URL, DEFAULT_SIGLIP2_GIANT_TOKENIZER_URL,
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

/// File name for the FP16 SigLIP 2 Giant vision encoder ONNX graph.
pub const SIGLIP2_GIANT_MODEL_NAME: &str = "vision_model_fp16.onnx";
/// Default download URL for the SigLIP 2 Giant vision encoder ONNX model.
pub const SIGLIP2_GIANT_MODEL_URL: &str = DEFAULT_SIGLIP2_GIANT_MODEL_URL;
/// File name for the FP16 SigLIP 2 Giant vision encoder's external weights.
pub const SIGLIP2_GIANT_MODEL_DATA_NAME: &str = "vision_model_fp16.onnx_data";
/// Default download URL for the FP16 SigLIP 2 Giant vision encoder's external weights.
pub const SIGLIP2_GIANT_MODEL_DATA_URL: &str =
    little_clip_core::config::DEFAULT_SIGLIP2_GIANT_MODEL_DATA_URL;

/// File name for the SigLIP 2 Giant text encoder ONNX model.
pub const SIGLIP2_GIANT_TEXT_MODEL_NAME: &str = "siglip2-giant-text.onnx";
/// Default download URL for the SigLIP 2 Giant text encoder ONNX model.
pub const SIGLIP2_GIANT_TEXT_MODEL_URL: &str = DEFAULT_SIGLIP2_GIANT_TEXT_MODEL_URL;

/// File name for the SigLIP 2 Giant tokenizer JSON configuration.
pub const SIGLIP2_GIANT_TOKENIZER_NAME: &str = "siglip2-giant-tokenizer.json";
/// Default download URL for the SigLIP 2 Giant tokenizer JSON configuration.
pub const SIGLIP2_GIANT_TOKENIZER_URL: &str = DEFAULT_SIGLIP2_GIANT_TOKENIZER_URL;

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

/// Loads an ONNX model with CUDA device 0, falling back to CPU with a warning.
///
/// Registration can succeed even if CUDA fails during session creation. Both stages
/// are checked before returning a session. Unsupported individual nodes may still
/// execute on CPU after CUDA initialization succeeds.
///
/// # Errors
/// Returns an error if the environment, session options, or model cannot be loaded
/// on either provider. A failed CPU retry retains the CUDA error for diagnostics.
pub(crate) fn load_session(path: &std::path::Path) -> Result<ort::session::Session, String> {
    load_session_with_options(path, Ok)
}

fn load_session_with_options(
    path: &std::path::Path,
    configure: impl Fn(
        ort::session::builder::SessionBuilder,
    ) -> Result<ort::session::builder::SessionBuilder, String>,
) -> Result<ort::session::Session, String> {
    use ort::ep::ExecutionProvider;

    let env = get_ort_environment()?;
    let build =
        || ort::session::Session::builder(&env).map_err(|e| format!("ORT builder failed: {e}"));
    let mut builder = build()?;
    if let Err(error) = ort::ep::CUDA::default().register(&mut builder) {
        tracing::warn!(%error, model = %path.display(), "ONNX Runtime: CUDA registration failed; using CPU");
        // Discard potentially modified options after failed provider registration.
        return configure(build()?)?
            .commit_from_file(path)
            .map_err(|cpu_error| {
                format!("ORT model load failed: CUDA registration: {error}; CPU: {cpu_error}")
            });
    }
    match configure(builder)?.commit_from_file(path) {
        Ok(session) => {
            tracing::info!(model = %path.display(), "ONNX Runtime: CUDA session initialized (device 0)");
            Ok(session)
        }
        Err(cuda_error) => {
            tracing::warn!(%cuda_error, model = %path.display(), "ONNX Runtime: CUDA session failed; retrying on CPU");
            configure(build()?)?
                .commit_from_file(path)
                .map_err(|cpu_error| {
                    format!("ORT model load failed: CUDA: {cuda_error}; CPU: {cpu_error}")
                })
        }
    }
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
        SIGLIP2_GIANT_MODEL_DATA_NAME => config.siglip2_giant_model_data_url().to_string(),
        SIGLIP2_GIANT_TEXT_MODEL_NAME => config.siglip2_giant_text_model_url().to_string(),
        SIGLIP2_GIANT_TOKENIZER_NAME => config.siglip2_giant_tokenizer_url().to_string(),
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
        SIGLIP2_GIANT_MODEL_NAME => Some(903_822),
        SIGLIP2_GIANT_MODEL_DATA_NAME => Some(2_326_336_512),
        SIGLIP2_GIANT_TEXT_MODEL_NAME => Some(711_572_480),
        SIGLIP2_GIANT_TOKENIZER_NAME => Some(34_363_039),
        _ => None,
    }
}

/// Minimum file size (1 MB) to consider a cached model file valid rather than a truncated or stub download.
pub const MIN_VALID_MODEL_BYTES: u64 = 1_000_000;

/// Returns the local cache directory for downloaded neural models, honoring `XDG_CACHE_HOME`.
pub fn models_cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("little-screen-clip/models")
}

/// Checks whether a named ONNX model is already cached locally.
#[allow(dead_code)]
pub fn is_model_cached(model_name: &str) -> bool {
    let model_path = models_cache_dir().join(model_name);
    if let Ok(meta) = std::fs::metadata(&model_path) {
        is_valid_cached_model(model_name, meta.len())
    } else {
        false
    }
}

/// Ensures that a named ONNX model is available locally in the cache directory,
/// downloading it from the configured or default URL if necessary.
///
/// # Errors
///
/// Returns an error if the model download or file write operation fails.
pub fn ensure_model(model_name: &str, url_override: Option<&str>) -> Result<PathBuf, String> {
    ensure_model_with_progress(model_name, url_override, |_, _| {})
}

/// Ensures that a named ONNX model is available locally in the cache directory,
/// downloading it with progress reporting if necessary.
///
/// # Errors
///
/// Returns an error if the model download or file write operation fails.
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
    let has_external_data = if model_name == SIGLIP2_GIANT_MODEL_NAME {
        let config = little_clip_core::AppConfig::load();
        let has_custom_data_url = config
            .ai_models
            .siglip2_giant_data_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
        if url == SIGLIP2_GIANT_MODEL_URL || has_custom_data_url {
            ensure_model_with_url_progress(
                SIGLIP2_GIANT_MODEL_DATA_NAME,
                config.siglip2_giant_model_data_url(),
                |fraction, message| on_progress(fraction * 0.9, message),
            )?;
            true
        } else {
            false
        }
    } else {
        false
    };
    ensure_model_with_url_progress(model_name, &url, |fraction, message| {
        on_progress(
            if has_external_data {
                0.9 + fraction * 0.1
            } else {
                fraction
            },
            message,
        )
    })
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
    let models_dir = models_cache_dir();
    let model_path = models_dir.join(model_name);
    if model_path.exists() {
        if let Ok(meta) = std::fs::metadata(&model_path) {
            // Verify file has non-trivial size (> 1MB) so corrupted/stub downloads are discarded
            if is_valid_cached_model(model_name, meta.len()) {
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
                if !is_valid_cached_model(model_name, meta.len()) {
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(format!(
                        "Downloaded model {model_name} has an invalid size ({} bytes)",
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

fn is_valid_cached_model(model_name: &str, size: u64) -> bool {
    if model_name == SIGLIP2_GIANT_MODEL_DATA_NAME {
        return expected_model_size(model_name) == Some(size);
    }
    size > MIN_VALID_MODEL_BYTES
        || (model_name == SIGLIP2_GIANT_MODEL_NAME && expected_model_size(model_name) == Some(size))
}

/// Runs an opt-in model probe and checks actual execution-provider trace events.
#[cfg(test)]
pub(crate) fn assert_cuda_execution(
    model_name: &str,
    run: impl FnOnce(&mut ort::session::Session),
) {
    let profile = tempfile::tempdir().unwrap();
    let model = models_cache_dir().join(model_name);
    assert!(
        model.is_file(),
        "Cache {model_name} before running this test"
    );
    let mut session = load_session_with_options(&model, |builder| {
        builder
            .with_profiling(profile.path().join("ort"))
            .map_err(|e| e.to_string())
    })
    .unwrap();
    run(&mut session);
    let trace = session.end_profiling().unwrap();
    let events: serde_json::Value = serde_json::from_slice(&std::fs::read(trace).unwrap()).unwrap();
    let cuda_nodes = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["args"]["provider"] == "CUDAExecutionProvider")
        .count();
    let cpu_nodes = events
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["args"]["provider"] == "CPUExecutionProvider")
        .count();
    assert!(
        cuda_nodes > 0,
        "{model_name}: no model kernels executed on CUDA"
    );
    eprintln!("{model_name}: executed {cuda_nodes} CUDA nodes, {cpu_nodes} CPU nodes");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires cached depth model; validates isolated CPU fallback"]
    fn cpu_fallback_without_visible_gpu() {
        const CHILD_FLAG: &str = "LITTLE_CLIP_ORT_CPU_FALLBACK_TEST";
        if std::env::var_os(CHILD_FLAG).is_some() {
            let mut session =
                load_session(&models_cache_dir().join(DEPTH_ANYTHING_V2_MODEL_NAME)).unwrap();
            let outputs = session.run([float_input(&[1, 3, 518, 518])]).unwrap();
            assert!(!outputs[0].try_extract_array::<f32>().unwrap().is_empty());
            return;
        }
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "onnx_runtime::tests::cpu_fallback_without_visible_gpu",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_FLAG, "1")
            .env("CUDA_VISIBLE_DEVICES", "-1")
            .status()
            .unwrap();
        assert!(
            status.success(),
            "CPU fallback failed with CUDA devices hidden"
        );
    }

    fn float_input(shape: &[i64]) -> ort::session::SessionInputValue<'static> {
        let count = shape.iter().product::<i64>() as usize;
        ort::value::Tensor::from_array((shape.to_vec(), vec![0.0_f32; count]))
            .unwrap()
            .into()
    }

    #[test]
    #[ignore = "requires cached Image Studio models and CUDA GPU"]
    fn image_tools_execute_on_cuda() {
        assert_cuda_execution(FEYNOBG_FP16_MODEL_NAME, |session| {
            session.run([float_input(&[1, 3, 1024, 1024])]).unwrap();
        });
        assert_cuda_execution(DEPTH_ANYTHING_V2_MODEL_NAME, |session| {
            session.run([float_input(&[1, 3, 518, 518])]).unwrap();
        });
        assert_cuda_execution(BIG_LAMA_MODEL_NAME, |session| {
            session
                .run([
                    float_input(&[1, 3, 512, 512]),
                    float_input(&[1, 1, 512, 512]),
                ])
                .unwrap();
        });
    }

    #[test]
    #[ignore = "requires cached MobileSAM models and CUDA GPU"]
    fn mobile_sam_executes_on_cuda() {
        assert_cuda_execution(MOBILE_SAM_ENCODER_MODEL_NAME, |session| {
            session.run([float_input(&[64, 64, 3])]).unwrap();
        });
        assert_cuda_execution(MOBILE_SAM_DECODER_MODEL_NAME, |session| {
            let size = ort::value::Tensor::from_array(([2], vec![64.0_f32, 64.0])).unwrap();
            session
                .run([
                    float_input(&[1, 256, 64, 64]),
                    float_input(&[1, 1, 2]),
                    float_input(&[1, 1]),
                    float_input(&[1, 1, 256, 256]),
                    float_input(&[1]),
                    size.into(),
                ])
                .unwrap();
        });
    }

    #[test]
    #[ignore = "requires cached SigLIP text model and CUDA GPU"]
    fn siglip_text_executes_on_cuda() {
        assert_cuda_execution(SIGLIP2_GIANT_TEXT_MODEL_NAME, |session| {
            let tokens = ort::value::Tensor::from_array(([1, 4], vec![1_i64; 4])).unwrap();
            let mut inputs: Vec<ort::session::SessionInputValue<'_>> = vec![tokens.into()];
            if session
                .inputs()
                .iter()
                .any(|input| input.name() == "attention_mask")
            {
                inputs.push(
                    ort::value::Tensor::from_array(([1, 4], vec![1_i64; 4]))
                        .unwrap()
                        .into(),
                );
            }
            session.run(inputs.as_slice()).unwrap();
        });
    }

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
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_MODEL_DATA_NAME, Some(custom)),
            custom
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_TEXT_MODEL_NAME, Some(custom)),
            custom
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_TOKENIZER_NAME, Some(custom)),
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
            SIGLIP2_GIANT_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_MODEL_DATA_NAME, None),
            SIGLIP2_GIANT_MODEL_DATA_URL
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_TEXT_MODEL_NAME, None),
            SIGLIP2_GIANT_TEXT_MODEL_URL
        );
        assert_eq!(
            resolve_model_url(SIGLIP2_GIANT_TOKENIZER_NAME, None),
            SIGLIP2_GIANT_TOKENIZER_URL
        );
    }

    #[test]
    fn fp16_siglip_external_assets_validate_their_expected_sizes() {
        assert_eq!(expected_model_size(SIGLIP2_GIANT_MODEL_NAME), Some(903_822));
        assert_eq!(
            expected_model_size(SIGLIP2_GIANT_MODEL_DATA_NAME),
            Some(2_326_336_512)
        );
        assert!(is_valid_cached_model(SIGLIP2_GIANT_MODEL_NAME, 903_822));
        assert!(is_valid_cached_model(
            SIGLIP2_GIANT_MODEL_DATA_NAME,
            2_326_336_512
        ));
        assert!(!is_valid_cached_model(
            SIGLIP2_GIANT_MODEL_DATA_NAME,
            2_326_336_511
        ));
    }

    #[test]
    fn test_models_cache_dir_path_structure() {
        let dir = models_cache_dir();
        assert!(dir.ends_with("little-screen-clip/models"));
    }
}
