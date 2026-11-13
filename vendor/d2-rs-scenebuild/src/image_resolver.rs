// SPDX-License-Identifier: MPL-2.0
//! Bounded image resources from local files, HTTP and data URIs. Source:
//! lib/imageasset/{imageasset,resolve,probe}.go at master 6a7b48378.
//! Shared caches are caller-owned and explicitly scoped.
//! PNG/WebP use the existing static container admission; APNG and animated
//! WebP acquisition remain open. GIF configuration admits animation.

use base64::Engine as _;
use d2_rs_core::CancellationToken;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::{ImageKind, ImageResolveError, ImageResolver, ImageResource};
#[path = "image_http.rs"]
mod http;
#[cfg(not(target_arch = "wasm32"))]
pub use http::NativeImageHttpClient;
pub use http::{IMAGE_HTTP_HEADERS, ImageHttpClient, ImageHttpResponse};
#[path = "image_cache.rs"]
mod cache;
pub use cache::{ImageCacheUsage, ImageMemoryCache};

#[path = "image_local.rs"]
mod local;
pub use local::{ImageLocalFiles, LocalFileError};

// Go's syscall.Getcwd on js/wasm calls process.cwd (fs_js.go). Rust's
// std::env::current_dir is unsupported on wasm32-unknown-unknown.
#[cfg(target_arch = "wasm32")]
fn current_dir() -> Result<PathBuf, ImageResolveError> {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = process, js_name = cwd, catch)]
        fn process_cwd() -> Result<String, JsValue>;
    }
    process_cwd()
        .map(PathBuf::from)
        .map_err(|_| error("getwd: not implemented on js"))
}

#[cfg(not(target_arch = "wasm32"))]
fn current_dir() -> Result<PathBuf, ImageResolveError> {
    std::env::current_dir().map_err(|e| error(e.to_string()))
}

/// Inclusive imageasset.Limits. Callers must supply every positive ceiling.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageAssetLimits {
    pub max_fetched_bytes: i64,
    pub max_encoded_bytes: i64,
    pub max_decompressed_bytes: i64,
    #[serde(rename = "MaxSVGBytes")]
    pub max_svg_bytes: i64,
    pub max_decoded_width: i64,
    pub max_decoded_height: i64,
    pub max_decoded_pixels: i64,
    pub max_assets: i64,
    pub max_cumulative_encoded_bytes: i64,
    pub max_cumulative_decoded_bytes: i64,
}

impl ImageAssetLimits {
    pub fn validate(&self) -> Result<(), ImageResolveError> {
        if [
            self.max_fetched_bytes,
            self.max_encoded_bytes,
            self.max_decompressed_bytes,
            self.max_svg_bytes,
            self.max_decoded_width,
            self.max_decoded_height,
            self.max_decoded_pixels,
            self.max_assets,
            self.max_cumulative_encoded_bytes,
            self.max_cumulative_decoded_bytes,
        ]
        .iter()
        .any(|&n| n <= 0)
        {
            return Err(error("imageasset: every limit must be positive"));
        }
        Ok(())
    }
}

fn error(message: impl Into<String>) -> ImageResolveError {
    ImageResolveError {
        unavailable: false,
        message: message.into(),
    }
}

fn context(cancel: Option<&CancellationToken>) -> Result<(), ImageResolveError> {
    match cancel.and_then(CancellationToken::error) {
        Some(cause) => Err(error(cause.to_string())),
        None => Ok(()),
    }
}

fn quote(value: &str) -> String {
    d2_rs_core::quote_go_bytes(value.as_bytes())
}

fn source_error(source: &str, stage: &str, cause: ImageResolveError) -> ImageResolveError {
    ImageResolveError {
        unavailable: cause.unavailable,
        message: format!("imageasset: {stage} {}: {}", quote(source), cause.message),
    }
}

fn limit(name: &str, actual: i64, ceiling: i64) -> Result<(), ImageResolveError> {
    if actual > ceiling {
        Err(error(format!("{name} {actual} exceeds limit {ceiling}")))
    } else {
        Ok(())
    }
}

/// Explicit I/O boundary. Implementations must enforce the inclusive read
/// ceiling and observe cancellation before returning bytes. Failed reads are
/// never memoized. No reader is installed globally or implicitly.
pub trait ImageFileReader: std::fmt::Debug + Send + Sync {
    /// A rooted policy supplies the default for an omitted resolver base
    /// directory (imageasset.New). Explicit base directories take precedence.
    fn root_path(&self) -> Option<&Path> {
        None
    }

    /// Canonical, policy-scoped key. Validate lexical access here before a
    /// shared-cache lookup. The default skips cross-session file caching;
    /// readers that permit sharing return a stable policy identity.
    fn cache_key(&self, _path: &Path) -> Result<Option<String>, ImageResolveError> {
        Ok(None)
    }

    fn read(
        &self,
        path: &Path,
        max_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError>;
}

/// Trusted localfile.Unrestricted policy. Construct explicitly for local apps.
#[derive(Debug)]
pub struct UnrestrictedImageFiles {
    _private: (),
}

impl UnrestrictedImageFiles {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for UnrestrictedImageFiles {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageFileReader for UnrestrictedImageFiles {
    fn cache_key(&self, path: &Path) -> Result<Option<String>, ImageResolveError> {
        Ok(Some(format!("unrestricted:{}", path.display())))
    }

    fn read(
        &self,
        path: &Path,
        max_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError> {
        context(cancel)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = options.open(path).map_err(|e| {
            let detail = match e.kind() {
                std::io::ErrorKind::NotFound => "no such file or directory".into(),
                std::io::ErrorKind::PermissionDenied => "permission denied".into(),
                _ => e.to_string(),
            };
            ImageResolveError {
                unavailable: e.kind() == std::io::ErrorKind::NotFound,
                message: format!(
                    "open local file {}: open {}: {detail}",
                    quote(&path.to_string_lossy()),
                    path.display()
                ),
            }
        })?;
        let metadata = file.metadata().map_err(|e| error(e.to_string()))?;
        if !metadata.is_file() {
            return Err(error(format!(
                "open local file {}: localfile: {} is not a regular file",
                quote(&path.to_string_lossy()),
                quote(&path.to_string_lossy())
            )));
        }
        limit(
            "fetched bytes",
            metadata.len().min(i64::MAX as u64) as i64,
            max_bytes,
        )?;
        let mut result = Vec::new();
        let mut input = file.take(max_bytes.saturating_add(1) as u64);
        let mut chunk = [0; 32 << 10];
        loop {
            context(cancel)?;
            let n = input.read(&mut chunk).map_err(|e| error(e.to_string()))?;
            if n == 0 {
                break;
            }
            result.extend_from_slice(&chunk[..n]);
            limit("fetched bytes", result.len() as i64, max_bytes)?;
        }
        context(cancel)?;
        Ok(result)
    }
}

/// A resource and its acquisition accounting. Encoded bytes are retained
/// unchanged; raster consumers decode the first frame when painting.
#[derive(Clone, Debug)]
pub struct ResolvedImageAsset {
    pub resource: ImageResource,
    pub fetched_bytes: i64,
    pub decompressed_bytes: i64,
}

impl ResolvedImageAsset {
    /// An owned canonical data URI; mutation cannot affect a cached resource.
    pub fn data_uri(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError> {
        context(cancel)?;
        let mut result = format!("data:{};base64,", self.resource.mime_type).into_bytes();
        for chunk in self.resource.data.chunks(48 << 10) {
            context(cancel)?;
            // Multiples of three allow independently encoded chunks.
            let mut encoded = String::new();
            base64::engine::general_purpose::STANDARD.encode_string(chunk, &mut encoded);
            result.extend_from_slice(encoded.as_bytes());
        }
        context(cancel)?;
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ImageAssetUsage {
    pub assets: i64,
    pub encoded: i64,
    pub decoded: i64,
}

#[derive(Debug, Default)]
struct Session {
    resources: HashMap<String, ResolvedImageAsset>,
    usage: ImageAssetUsage,
}

/// One bounded local/HTTP/data-URI image session. The same canonical source is
/// charged once; a failed operation never changes usage or poisons retries.
/// The serial session lock also coalesces simultaneous canonical requests.
#[derive(Debug)]
pub struct RasterImageResolver {
    base_dir: PathBuf,
    limits: ImageAssetLimits,
    files: Arc<dyn ImageFileReader>,
    http: Option<Arc<dyn ImageHttpClient>>,
    owns_http_transport: bool,
    cache: Option<(Arc<ImageMemoryCache>, String)>,
    session: Mutex<Session>,
}

impl RasterImageResolver {
    pub fn new(
        base_dir: &Path,
        limits: ImageAssetLimits,
        files: Arc<dyn ImageFileReader>,
    ) -> Result<Self, ImageResolveError> {
        limits.validate()?;
        let base_dir = if base_dir.as_os_str().is_empty() {
            files.root_path().unwrap_or_else(|| Path::new("."))
        } else {
            base_dir
        };
        let base_dir = if base_dir.is_absolute() {
            base_dir.to_owned()
        } else {
            current_dir()?.join(base_dir)
        };
        Ok(Self {
            base_dir: clean(&base_dir),
            limits,
            files,
            #[cfg(not(target_arch = "wasm32"))]
            http: Some(Arc::new(NativeImageHttpClient::default())),
            #[cfg(target_arch = "wasm32")]
            http: None,
            owns_http_transport: true,
            session: Mutex::default(),
            cache: None,
        })
    }

    /// Install a caller-owned client, preserving its redirect/network policy.
    pub fn with_http_client(mut self, client: Arc<dyn ImageHttpClient>) -> Self {
        self.http = Some(client);
        self.owns_http_transport = false;
        self
    }

    /// Transfer a session-owned client, such as an isolated policy clone.
    /// Unlike `with_http_client`, this permits explicit idle-pool cleanup.
    /// Policy adapters must clone/configure a borrowed transport before
    /// transferring it; this method never claims ownership of a borrowed pool.
    pub fn with_owned_http_client(mut self, client: Box<dyn ImageHttpClient>) -> Self {
        self.http = Some(Arc::from(client));
        self.owns_http_transport = true;
        self
    }

    pub fn owns_http_transport(&self) -> bool {
        self.owns_http_transport
    }

    /// imageasset.Resolver.CloseIdleConnections. Borrowed clients stay under
    /// caller control; repeated calls remain safe under the client contract.
    pub fn close_idle_connections(&self) {
        if self.owns_http_transport {
            if let Some(client) = &self.http {
                client.close_idle_connections();
            }
        }
    }

    /// Install an explicit bounded cache. Namespace identifies the caller's
    /// transport/tenant credentials. The network-policy tag must match the
    /// installed client's policy; it prevents private/public cache crossover.
    /// Custom file readers additionally control their own policy-scoped keys.
    pub fn with_shared_cache(
        mut self,
        cache: Arc<ImageMemoryCache>,
        namespace: &str,
        allow_private_networks: bool,
    ) -> Result<Self, ImageResolveError> {
        if namespace.is_empty() {
            return Err(error(
                "imageasset: CacheNamespace is required when Cache is set",
            ));
        }
        let network = if allow_private_networks {
            "private-network"
        } else {
            "public-only"
        };
        let hash = Sha256::digest(format!("{namespace}\0network-policy:{network}").as_bytes());
        self.cache = Some((cache, format!("namespace:{hash:x}:")));
        Ok(self)
    }

    pub fn usage(&self) -> ImageAssetUsage {
        self.session.lock().unwrap().usage
    }

    pub fn resolve_with_context(
        &self,
        source: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<ResolvedImageAsset, ImageResolveError> {
        self.resolve_inner(source, cancel, None)
    }

    /// Admit caller-acquired bytes under their original locator, using this
    /// resolver's operation-wide memo and limits without further I/O. Data
    /// URIs retain their source decoding and encoded-payload limits.
    pub fn resolve_acquired(
        &self,
        source: &str,
        data: &[u8],
    ) -> Result<ResolvedImageAsset, ImageResolveError> {
        self.resolve_inner(source, None, Some(data))
    }

    fn resolve_inner(
        &self,
        source: &str,
        cancel: Option<&CancellationToken>,
        acquired: Option<&[u8]>,
    ) -> Result<ResolvedImageAsset, ImageResolveError> {
        let label = display_source(source);
        context(cancel).map_err(|e| source_error(&label, "resolve", e))?;
        let data_uri = source
            .get(..5)
            .is_some_and(|s| s.eq_ignore_ascii_case("data:"));
        let http_url = source
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
            || source
                .get(..8)
                .is_some_and(|s| s.eq_ignore_ascii_case("https://"));
        let classified = || -> Result<String, ImageResolveError> {
            if source.is_empty() {
                return Err(error("empty source"));
            }
            if data_uri {
                let rest = &source[5..];
                let comma = rest
                    .as_bytes()
                    .iter()
                    .take(4097)
                    .position(|&b| b == b',')
                    .ok_or_else(|| {
                        if rest.len() > 4096 {
                            error("data URI metadata bytes 4097 exceeds limit 4096")
                        } else {
                            error("data URI has no comma separator")
                        }
                    })?;
                limit(
                    "fetched bytes",
                    (rest.len() - comma - 1) as i64,
                    self.limits.max_fetched_bytes,
                )?;
                let mut digest = Sha256::new();
                for chunk in source.as_bytes().chunks(32768) {
                    context(cancel)?;
                    digest.update(chunk);
                }
                return Ok(format!("data:{:x}", digest.finalize()));
            }
            limit("source locator bytes", source.len() as i64, 65536)?;
            if source.starts_with("//") {
                return Err(error(
                    "network-path references beginning with // are unsupported; use an explicit http:// or https:// URL",
                ));
            }
            if http_url {
                return http::canonical_url(source);
            }
            if let Some(scheme) = uri_scheme(source).filter(|_| !Path::new(source).is_absolute()) {
                return Err(error(format!("unsupported URI scheme {}", quote(scheme))));
            }
            Ok(clean(&self.base_dir.join(source))
                .to_string_lossy()
                .into_owned())
        };
        let key = classified().map_err(|e| source_error(&label, "classify source", e))?;
        let shared_key = if data_uri {
            None
        } else if http_url {
            Some(format!("http:{:x}", Sha256::digest(key.as_bytes())))
        } else {
            self.files
                .cache_key(Path::new(&key))
                .map_err(|e| source_error(&label, "classify source", e))?
        };
        let cache_key = self
            .cache
            .as_ref()
            .and_then(|(_, prefix)| shared_key.as_ref().map(|key| format!("{prefix}{key}")));
        context(cancel).map_err(|e| source_error(&label, "resolve", e))?;
        let mut session = self.session.lock().unwrap();
        context(cancel).map_err(|e| source_error(&label, "resolve", e))?;
        if let Some(resource) = session.resources.get(&key) {
            return Ok(resource.clone());
        }
        let cached = self
            .cache
            .as_ref()
            .zip(cache_key.as_ref())
            .and_then(|((cache, _), key)| cache.get(key));
        let result = if let Some(cached) = &cached {
            cache::validate(cached, self.limits)
                .map_err(|e| source_error(&label, "validate cached resource", e))?;
            context(cancel).map_err(|e| source_error(&label, "resolve", e))?;
            cached.as_ref().clone()
        } else {
            self.load_resource(source, &key, data_uri, http_url, &label, cancel, acquired)?
        };
        let resource = &result.resource;
        let reserve = || -> Result<(), ImageResolveError> {
            limit(
                "assets",
                session.usage.assets.saturating_add(1),
                self.limits.max_assets,
            )?;
            limit(
                "cumulative encoded bytes",
                session
                    .usage
                    .encoded
                    .saturating_add(resource.data.len() as i64),
                self.limits.max_cumulative_encoded_bytes,
            )?;
            limit(
                "cumulative decoded bytes",
                session.usage.decoded.saturating_add(resource.decoded_bytes),
                self.limits.max_cumulative_decoded_bytes,
            )
        };
        reserve().map_err(|e| {
            source_error(
                &label,
                if cached.is_some() {
                    "reserve resource"
                } else {
                    "probe"
                },
                e,
            )
        })?;
        context(cancel).map_err(|e| {
            source_error(
                &label,
                if cached.is_some() { "resolve" } else { "probe" },
                e,
            )
        })?;
        session.usage.assets += 1;
        session.usage.encoded += resource.data.len() as i64;
        session.usage.decoded += resource.decoded_bytes;
        if let Some(((cache, _), cache_key)) = self.cache.as_ref().zip(cache_key) {
            cache.put(cache_key, &result);
        }
        session.resources.insert(key, result.clone());
        Ok(result)
    }

    fn load_resource(
        &self,
        source: &str,
        key: &str,
        data_uri: bool,
        http_url: bool,
        label: &str,
        cancel: Option<&CancellationToken>,
        acquired: Option<&[u8]>,
    ) -> Result<ResolvedImageAsset, ImageResolveError> {
        let loaded = if data_uri {
            self.load_data(source, cancel)
                .map(|(bytes, len)| (bytes, len, String::new()))
        } else if let Some(bytes) = acquired {
            let admit = || {
                let len = bytes.len() as i64;
                limit("fetched bytes", len, self.limits.max_fetched_bytes)?;
                limit(
                    "decompressed bytes",
                    len,
                    self.limits.max_decompressed_bytes,
                )?;
                Ok((bytes.to_vec(), len, String::new()))
            };
            admit()
        } else if http_url {
            self.http
                .as_ref()
                .ok_or_else(|| error("HTTP client is not installed"))
                .and_then(|client| http::load(client.as_ref(), key, self.limits, cancel))
        } else {
            self.files
                .read(Path::new(&key), self.limits.max_fetched_bytes, cancel)
                .and_then(|bytes| {
                    let len = bytes.len() as i64;
                    limit("fetched bytes", len, self.limits.max_fetched_bytes)?;
                    limit(
                        "decompressed bytes",
                        len,
                        self.limits.max_decompressed_bytes,
                    )?;
                    Ok((bytes, len, String::new()))
                })
        };
        let (data, fetched_bytes, http_mime) =
            loaded.map_err(|e| source_error(label, "load", e))?;
        context(cancel).map_err(|e| source_error(label, "resolve", e))?;
        let mime_hint = if data_uri {
            source[5..].split([',', ';']).next().unwrap_or("").trim()
        } else if http_url {
            http_mime.split(';').next().unwrap_or("").trim()
        } else {
            match Path::new(&key).extension().and_then(|s| s.to_str()) {
                Some(extension)
                    if extension.eq_ignore_ascii_case("svg")
                        || extension.eq_ignore_ascii_case("svgz") =>
                {
                    "image/svg+xml"
                }
                _ => "",
            }
        };
        let resource = self
            .probe(data, mime_hint, cancel)
            .map_err(|e| source_error(label, "probe", e))?;
        let result = ResolvedImageAsset {
            decompressed_bytes: resource.data.len() as i64,
            fetched_bytes,
            resource,
        };
        Ok(result)
    }

    fn load_data(
        &self,
        source: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<(Vec<u8>, i64), ImageResolveError> {
        let (metadata, payload) = source[5..]
            .split_once(',')
            .ok_or_else(|| error("data URI has no comma separator"))?;
        let parts: Vec<_> = metadata.split(';').collect();
        let mut base64 = false;
        for (index, part) in parts.iter().enumerate().skip(1) {
            if part.trim().eq_ignore_ascii_case("base64") {
                if base64 || index != parts.len() - 1 {
                    return Err(error("data URI base64 marker must occur once and last"));
                }
                base64 = true;
            }
        }
        let decoded_limit = if base64 {
            self.limits.max_fetched_bytes
        } else {
            self.limits
                .max_fetched_bytes
                .min(self.limits.max_decompressed_bytes)
        };
        let mut decoded = Vec::new();
        let bytes = payload.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            if at % 32768 == 0 {
                context(cancel)?;
            }
            let byte = if bytes[at] == b'%' {
                let hex = bytes
                    .get(at + 1..at + 3)
                    .and_then(|v| std::str::from_utf8(v).ok())
                    .and_then(|v| u8::from_str_radix(v, 16).ok())
                    .ok_or_else(|| error("decode data URI escapes: invalid URL escape"))?;
                at += 3;
                hex
            } else {
                let byte = bytes[at];
                at += 1;
                byte
            };
            decoded.push(byte);
            limit(
                "data URI decoded payload bytes",
                decoded.len() as i64,
                decoded_limit,
            )
            .map_err(|e| error(format!("decode data URI escapes: {e}")))?;
        }
        let data = if base64 {
            let raw: Vec<_> = decoded
                .into_iter()
                .filter(|&b| b != b'\r' && b != b'\n')
                .collect();
            let encoding = if raw.len() % 4 == 0 {
                &base64::engine::general_purpose::STANDARD
            } else {
                &base64::engine::general_purpose::STANDARD_NO_PAD
            };
            let mut decoder = base64::read::DecoderReader::new(raw.as_slice(), encoding);
            let ceiling = self
                .limits
                .max_fetched_bytes
                .min(self.limits.max_decompressed_bytes);
            let mut result = Vec::new();
            let mut chunk = [0; 32768];
            loop {
                context(cancel)?;
                let remaining = (ceiling - result.len() as i64).saturating_add(1);
                let size = chunk.len().min(remaining as usize);
                let n = decoder
                    .read(&mut chunk[..size])
                    .map_err(|e| error(format!("decode data URI base64: {e}")))?;
                if n == 0 {
                    break;
                }
                result.extend_from_slice(&chunk[..n]);
                limit("decompressed bytes", result.len() as i64, ceiling)
                    .map_err(|e| error(format!("decode data URI base64: {e}")))?;
            }
            result
        } else {
            decoded
        };
        limit(
            "decompressed bytes",
            data.len() as i64,
            self.limits.max_decompressed_bytes,
        )?;
        context(cancel)?;
        Ok((data, payload.len() as i64))
    }

    fn probe(
        &self,
        data: Vec<u8>,
        mime_hint: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<ImageResource, ImageResolveError> {
        if data.is_empty() {
            return Err(error("empty image resource"));
        }
        context(cancel)?;
        // imageasset/probe.go normalizeMIME: declarations survive sniffing
        // failure, with the same canonical aliases as the original probe.
        let normalized_hint = mime_hint
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let mime_hint = match normalized_hint.as_str() {
            "image/jpg" | "image/pjpeg" => "image/jpeg",
            "image/x-png" => "image/png",
            "text/xml" | "application/xml" | "application/svg+xml" => "image/svg+xml",
            other => other,
        };
        let format = d2_rs_math::svg_image::signature(&data);
        if format.is_empty() {
            let declared = [
                "image/svg+xml",
                "text/xml",
                "application/xml",
                "application/svg+xml",
            ]
            .iter()
            .any(|value| mime_hint.eq_ignore_ascii_case(value));
            let xml = data
                .strip_prefix(&[0xef, 0xbb, 0xbf])
                .unwrap_or(&data)
                .iter()
                .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
                == Some(&b'<');
            if declared || xml {
                let decoded = d2_rs_math::image_svg::prepare_resource(
                    &data,
                    declared,
                    d2_rs_math::image_svg::Limits {
                        svg_bytes: self.limits.max_svg_bytes as usize,
                        encoded_bytes: self.limits.max_encoded_bytes as usize,
                        decompressed_bytes: self.limits.max_decompressed_bytes as usize,
                    },
                    cancel,
                )
                .map_err(error)?
                .into_owned();
                let len = decoded.len() as i64;
                limit("encoded bytes", len, self.limits.max_encoded_bytes)?;
                limit(
                    "decompressed bytes",
                    len,
                    self.limits.max_decompressed_bytes,
                )?;
                limit(
                    "decoded bytes",
                    len,
                    self.limits.max_cumulative_decoded_bytes,
                )?;
                context(cancel)?;
                return Ok(ImageResource {
                    kind: ImageKind::Svg,
                    mime_type: "image/svg+xml".into(),
                    data: decoded.into(),
                    pixel_width: 0,
                    pixel_height: 0,
                    decoded_bytes: len,
                });
            }
        }
        limit(
            "encoded bytes",
            data.len() as i64,
            self.limits.max_encoded_bytes,
        )?;
        limit(
            "decompressed bytes",
            data.len() as i64,
            self.limits.max_decompressed_bytes,
        )?;
        use d2_rs_math::svg_image_config as config;
        let config = match format {
            // rasterimage.Config: inspectPNG admits APNG by its first
            // frame, then image.DecodeConfig reads the IHDR.
            "png" => d2_rs_math::png_first_frame::inspect_png_container(&data)
                .and_then(|_| config::png_decode_config(&data, cancel)),
            "jpeg" => config::jpeg_config(&data, cancel),
            "gif" => config::gif_config(&data, cancel),
            // imageasset/probe.go probeRaster: rasterimage.Config admits
            // animated WebP by its first frame on the logical canvas.
            "webp" => config::webp_config(&data, cancel),
            _ => {
                return Err(error(if mime_hint.is_empty() {
                    let sniffed = d2_rs_render::detect_image_mime(&data);
                    format!(
                        "unsupported or malformed image (sniffed {:?})",
                        sniffed.split(';').next().unwrap_or("").trim()
                    )
                } else {
                    format!("unsupported or malformed image with MIME type {mime_hint:?}")
                }));
            }
        }
        .map_err(|e| error(format!("malformed {} image: {e}", format.to_uppercase())))?;
        limit("decoded width", config.width, self.limits.max_decoded_width)?;
        limit(
            "decoded height",
            config.height,
            self.limits.max_decoded_height,
        )?;
        limit(
            "decoded pixels",
            config
                .width
                .checked_mul(config.height)
                .ok_or_else(|| error("decoded pixel count overflows int64"))?,
            self.limits.max_decoded_pixels,
        )?;
        limit(
            "decoded bytes",
            config.decoded_bytes,
            self.limits.max_cumulative_decoded_bytes,
        )?;
        context(cancel)?;
        Ok(ImageResource {
            kind: ImageKind::Raster,
            mime_type: format!("image/{format}"),
            data: data.into(),
            pixel_width: config.width,
            pixel_height: config.height,
            decoded_bytes: config.decoded_bytes,
        })
    }
}

impl ImageResolver for RasterImageResolver {
    fn resolve(&self, source: &str) -> Result<Option<ImageResource>, ImageResolveError> {
        self.resolve_with_context(source, None)
            .map(|result| Some(result.resource))
    }
}

fn clean(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(part.as_os_str()),
        }
    }
    result
}

fn uri_scheme(value: &str) -> Option<&str> {
    if !value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
    {
        return None;
    }
    for (i, b) in value.bytes().enumerate().skip(1) {
        if b == b':' {
            return Some(&value[..i]);
        }
        if !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
            return None;
        }
    }
    None
}

fn display_source(source: &str) -> String {
    if source
        .get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
    {
        let metadata: String = source[5..].chars().take(129).collect();
        let media = metadata.split([',', ';']).next().unwrap_or("").trim();
        if !media.is_empty()
            && media.len() <= 128
            && media.bytes().filter(|&b| b == b'/').count() == 1
            && !media.starts_with('/')
            && !media.ends_with('/')
            && media
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'+' | b'-' | b'_'))
        {
            return format!("data:{media}");
        }
        return "data URI".into();
    }
    if source.starts_with("//") {
        return "<network-path reference>".into();
    }
    if source.len() > 65536 {
        return "<oversized source locator>".into();
    }
    if uri_scheme(source)
        .is_some_and(|s| s.eq_ignore_ascii_case("http") || s.eq_ignore_ascii_case("https"))
    {
        return http::display_url(source);
    }
    // Raster-only sessions do not fetch URI schemes. Preserve safe labels
    // without retaining credentials, queries, fragments or opaque payloads.
    if let Some(scheme) = uri_scheme(source).filter(|_| !Path::new(source).is_absolute()) {
        return format!("{}:<redacted>", scheme.to_ascii_lowercase());
    }
    source.into()
}
