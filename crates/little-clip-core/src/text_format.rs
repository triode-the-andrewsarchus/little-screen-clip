//! Shared filename hints and bounded TypeScript/transport-stream disambiguation.
use std::path::Path;

/// Maximum bytes used by the existing TypeScript disambiguation probe.
pub const TYPESCRIPT_PROBE_BYTES: usize = 8192;

/// Identifies suffixes shared by TypeScript and transport-stream video.
pub fn typescript_suffix(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ts" | "mts" | "cts"
            )
        })
}

/// Resolve extensions shared by TypeScript and transport-stream video on workers.
pub fn is_typescript_source(path: &Path) -> bool {
    if !typescript_suffix(path) {
        return false;
    }
    crate::file_source::read_file_bytes(path, TYPESCRIPT_PROBE_BYTES, false, true)
        .is_ok_and(|bytes| typescript_bytes(&bytes))
}

/// Applies the bounded plain-text heuristic to already guarded source bytes.
///
/// This is not a compiler or import resolver. Callers own source access and
/// revision verification; a valid truncated UTF-8 prefix is accepted.
pub fn typescript_bytes(bytes: &[u8]) -> bool {
    if bytes
        .iter()
        .any(|byte| *byte < 32 && !matches!(*byte, b'\n' | b'\r' | b'\t' | 12))
    {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(error) if error.error_len().is_none() => true,
        Err(_) => {
            String::from_utf8_lossy(bytes)
                .chars()
                .filter(|character| *character == '\u{fffd}')
                .count()
                <= bytes.len().div_ceil(100)
        }
    }
}

/// Returns an existing source/text filename hint without payload access.
pub fn known_text(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    crate::syntax::is_code_file(path)
        || matches!(
            name.as_str(),
            "makefile"
                | "dockerfile"
                | "license"
                | "readme"
                | ".gitignore"
                | ".gitattributes"
                | ".editorconfig"
                | ".env"
        )
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "log" | "csv" | "tsv" | "nix" | "cmake" | "srt" | "vtt"
                )
            })
}
