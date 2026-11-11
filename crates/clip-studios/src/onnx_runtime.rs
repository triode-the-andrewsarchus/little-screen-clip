use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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
pub fn load_session(path: &std::path::Path) -> Result<ort::session::Session, String> {
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
    ensure_model_cancellable(
        model_name,
        url_override,
        &AtomicBool::new(false),
        on_progress,
    )
}

/// Downloads a model while honoring the caller's cancellation token.
///
/// # Errors
/// Returns an error on cancellation, a failed or stalled transfer, an invalid
/// download, or a cache I/O failure.
pub fn ensure_model_cancellable(
    model_name: &str,
    url_override: Option<&str>,
    cancelled: &AtomicBool,
    on_progress: impl Fn(f32, &str),
) -> Result<PathBuf, String> {
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
            download_model(
                &models_cache_dir(),
                SIGLIP2_GIANT_MODEL_DATA_NAME,
                config.siglip2_giant_model_data_url(),
                cancelled,
                MODEL_DOWNLOAD_TIMEOUT,
                |fraction, message| on_progress(fraction * 0.9, message),
            )?;
            true
        } else {
            false
        }
    } else {
        false
    };
    download_model(
        &models_cache_dir(),
        model_name,
        &url,
        cancelled,
        MODEL_DOWNLOAD_TIMEOUT,
        |fraction, message| {
            on_progress(
                if has_external_data {
                    0.9 + fraction * 0.1
                } else {
                    fraction
                },
                message,
            )
        },
    )
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
    download_model(
        &models_cache_dir(),
        model_name,
        url,
        &AtomicBool::new(false),
        MODEL_DOWNLOAD_TIMEOUT,
        on_progress,
    )
}

const MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

fn download_model(
    models_dir: &Path,
    model_name: &str,
    url: &str,
    cancelled: &AtomicBool,
    timeout: Duration,
    on_progress: impl Fn(f32, &str),
) -> Result<PathBuf, String> {
    use little_clip_core::kernel_interface::{create_content_directory, open_content};

    if Path::new(model_name)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(model_name)
    {
        return Err("Model name must be a single filename".into());
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Model download cancelled".into());
    }

    let directory = create_content_directory(models_dir).map_err(|error| error.to_string())?;
    let anchor = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let target = anchor.join(model_name);
    let model_path = models_dir.join(model_name);
    let cached = || match open_content(&target, true) {
        Ok(file) => file
            .metadata()
            .map(|metadata| is_valid_cached_model(model_name, metadata.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    };
    if cached().map_err(|error| error.to_string())? {
        return Ok(model_path);
    }

    let staged = tempfile::NamedTempFile::new_in(&anchor).map_err(|error| error.to_string())?;
    let (request, fd) = little_clip_core::CommandRequest::new("curl")
        .args([
            "--disable",
            "--location",
            "--fail",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "15",
            "--speed-limit",
            "1",
            "--speed-time",
            "60",
        ])
        .timeout(timeout)
        .own_process_group()
        .die_with_parent()
        .inherit_handle(staged.as_file())
        .map_err(|error| error.to_string())?;

    let expected = expected_model_size(model_name);
    let total_mb = expected.map_or(100.0, |bytes| bytes as f32 / 1_000_000.0);
    on_progress(0.01, &format!("Connecting to download {model_name}..."));
    let mut last_progress = Instant::now();
    let output = request
        .arg("--output")
        .arg(format!("/proc/self/fd/{fd}"))
        .args(["--", url])
        .run_cancellable_with_progress(cancelled, || {
            if last_progress.elapsed() < Duration::from_millis(150) {
                return;
            }
            last_progress = Instant::now();
            if let Ok(metadata) = staged.as_file().metadata() {
                let bytes = metadata.len() as f32;
                let mb = bytes / 1_000_000.0;
                let fraction = expected
                    .map_or(mb / (mb + 50.0), |size| bytes / size as f32)
                    .clamp(0.01, 0.99);
                on_progress(
                    fraction,
                    &format!(
                        "Downloading {model_name}: {:.0}% ({mb:.1}/{total_mb:.1} MB)",
                        fraction * 100.0
                    ),
                );
            }
        })
        .map_err(|error| format!("Could not download {model_name}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not download {model_name}: {}",
            output.stderr_lossy()
        ));
    }
    let bytes = staged
        .as_file()
        .metadata()
        .map_err(|error| error.to_string())?
        .len();
    if !is_valid_cached_model(model_name, bytes) {
        return Err(format!(
            "Downloaded model {model_name} has an invalid size ({bytes} bytes)"
        ));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err("Model download cancelled".into());
    }
    // Keep the old entry until a complete replacement is ready. Another
    // downloader may already have published a valid model in the meantime.
    if !cached().map_err(|error| error.to_string())? {
        staged
            .persist(&target)
            .map_err(|error| format!("Failed to save downloaded model: {error}"))?;
    }
    on_progress(1.0, &format!("Model {model_name} download complete"));
    Ok(model_path)
}

fn is_valid_cached_model(model_name: &str, size: u64) -> bool {
    if model_name == SIGLIP2_GIANT_MODEL_DATA_NAME {
        return expected_model_size(model_name) == Some(size);
    }
    size > MIN_VALID_MODEL_BYTES
        || (model_name == SIGLIP2_GIANT_MODEL_NAME && expected_model_size(model_name) == Some(size))
}

/// Runs an opt-in model probe and checks actual execution-provider trace events.
#[cfg(any(test, feature = "test-support"))]
pub fn assert_cuda_execution(model_name: &str, run: impl FnOnce(&mut ort::session::Session)) {
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
    fn concurrent_downloads_only_publish_complete_models() {
        let fixture = tempfile::tempdir().unwrap();
        let cache = fixture.path().join("models");
        std::fs::create_dir(&cache).unwrap();
        let name = "test.onnx";
        std::fs::write(cache.join(name), b"previous incomplete model").unwrap();
        let size = MIN_VALID_MODEL_BYTES as usize + 1;
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for byte in [b'a', b'b'] {
                let source = fixture.path().join(format!("{byte}.onnx"));
                std::fs::write(&source, vec![byte; size]).unwrap();
                let url = format!("file://{}", source.display());
                let cache = &cache;
                let barrier = &barrier;
                scope.spawn(move || {
                    download_model(
                        &cache,
                        name,
                        &url,
                        &AtomicBool::new(false),
                        Duration::from_secs(5),
                        |_, message| {
                            if message.starts_with("Connecting ") {
                                barrier.wait();
                            }
                        },
                    )
                    .unwrap();
                });
            }
        });
        let bytes = std::fs::read(cache.join(name)).unwrap();
        assert_eq!(bytes.len(), size);
        assert!(matches!(bytes[0], b'a' | b'b'));
        assert!(bytes.iter().all(|&byte| byte == bytes[0]));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
    }

    #[test]
    fn stalled_model_downloads_cancel_or_time_out_without_publishing() {
        // A private local listener accepts the TCP handshake but never sends a
        // response. No real model host or shared cache is used.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let cache = tempfile::tempdir().unwrap();
        let name = "test.onnx";
        let target = cache.path().join(name);
        let previous = b"previous incomplete model";
        std::fs::write(&target, previous).unwrap();

        for cancel in [false, true] {
            let cancelled = AtomicBool::new(false);
            let error = download_model(
                cache.path(),
                name,
                &url,
                &cancelled,
                if cancel {
                    Duration::from_secs(5)
                } else {
                    Duration::from_millis(300)
                },
                |_, message| {
                    if cancel && message.starts_with("Downloading ") {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                },
            )
            .unwrap_err();
            assert!(
                error.contains(if cancel { "cancelled" } else { "timeout" }),
                "{error}"
            );
            assert_eq!(std::fs::read(&target).unwrap(), previous);
            assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);
        }
    }

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
