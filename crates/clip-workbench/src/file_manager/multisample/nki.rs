//! Separately licensed native reader, isolated from application content and networking.
use little_clip_core::{
    multisample::{Document, Instrument},
    process::CommandRequest,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};
pub(super) fn read(bytes: Vec<u8>, token: &AtomicBool) -> Result<Document, String> {
    let helper = std::env::var_os("LITTLE_CLIP_NKI_HELPER")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("nki-helper")))
        })
        .ok_or("Cannot locate native sample-map helper")?;
    read_with(&helper, bytes, token)
}
pub(super) fn read_with(
    helper: &Path,
    bytes: Vec<u8>,
    token: &AtomicBool,
) -> Result<Document, String> {
    if !helper.join("little-clip-nki").is_file() {
        return Err("Native sample-map reader is not installed. Build it with: python3 scripts/build_multisample_helper.py.".into());
    }
    let result = CommandRequest::new("prlimit")
        .args([
            "--as=536870912",
            "--cpu=15",
            "--fsize=67108864",
            "--",
            "bwrap",
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/bin:/bin",
            "--setenv",
            "HOME",
            "/tmp",
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--dir",
            "/etc",
            "--ro-bind-try",
            "/etc/ld.so.cache",
            "/etc/ld.so.cache",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--ro-bind",
        ])
        .arg(helper)
        .args(["/helper", "--chdir", "/tmp", "/helper/little-clip-nki"])
        .stdin_bytes(bytes)
        .timeout(Duration::from_secs(20))
        .max_output_bytes(4 * 1024 * 1024)
        .run_cancellable(token)
        .map_err(|e| format!("Native sample-map reader: {e}"))?;
    if !result.status.success() || result.stdout_truncated {
        return Err("Native sample-map reader rejected this instrument or exceeded its resource budget. Protected containers, unsupported monoliths and missing native helper/Bubblewrap dependencies cannot be opened.".into());
    }
    let instruments: Vec<Instrument> = serde_json::from_slice(&result.stdout)
        .map_err(|e| format!("Invalid native sample-map response: {e}"))?;
    let doc=Document {instruments,notes:vec!["Sampler mapping metadata from the separate native reader. Instrument scripts, effects, modulators, group conditions and pitch processing are not executed. Audition plays only a chosen external raw sample; embedded/library samples and NCW extraction are unavailable.".into()],pcm:None};
    doc.validate()?;
    Ok(doc)
}
