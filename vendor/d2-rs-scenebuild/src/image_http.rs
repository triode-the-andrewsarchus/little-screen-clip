// SPDX-License-Identifier: MPL-2.0
//! imageasset.loadHTTP/selectContentType/parseContentEncodings at 6a7b48378.
use super::{ImageAssetLimits, context, error, limit, quote};
use crate::ImageResolveError;
use d2_rs_core::CancellationToken;

#[cfg(not(target_arch = "wasm32"))]
#[path = "image_http_native.rs"]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::NativeImageHttpClient;

pub const IMAGE_HTTP_HEADERS: [(&str, &str); 3] = [
    ("User-Agent", "Mozilla/5.0 (compatible; D2 imageasset)"),
    (
        "Accept",
        "image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
    ),
    ("Accept-Encoding", "gzip, deflate, br"),
];

/// Raw final HTTP response. Content encodings have not been decoded.
#[derive(Debug)]
pub struct ImageHttpResponse {
    pub status: u16,
    pub content_length: i64,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Explicit injected-client boundary, equivalent to http.Client.Do. The client
/// owns redirect/network policy. It must enforce the supplied fetched-byte
/// ceiling while streaming, check response_head before reading the body, send
/// IMAGE_HTTP_HEADERS and honor cancellation. Errors must be sanitized and
/// preserve availability (transport absence versus a redirect-policy error).
pub trait ImageHttpClient: std::fmt::Debug + Send + Sync {
    /// Release idle connections without interrupting active requests. A client
    /// without a retained pool needs no action. Resolver sessions invoke this
    /// only for clients whose ownership was explicitly transferred to them.
    fn close_idle_connections(&self) {}

    fn get(
        &self,
        url: &str,
        max_fetched_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<ImageHttpResponse, ImageResolveError>;
}

impl ImageHttpResponse {
    /// Validate metadata before consuming the body. Used by native and injected
    /// streaming clients as well as by the resolver before accepting a result.
    pub fn validate_head(
        &self,
        max_fetched_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<(), ImageResolveError> {
        response_head(
            self.status,
            self.content_length,
            &self.headers,
            max_fetched_bytes,
            cancel,
        )
        .map(|_| ())
    }
}

fn response_head(
    status_code: u16,
    content_length: i64,
    headers: &[(String, String)],
    max_fetched_bytes: i64,
    cancel: Option<&CancellationToken>,
) -> Result<(String, Vec<String>), ImageResolveError> {
    context(cancel)?;
    if status_code != 200 {
        return Err(ImageResolveError {
            unavailable: true,
            message: format!("expected status 200 but got status code {status_code}"),
        });
    }
    limit("fetched bytes", content_length, max_fetched_bytes)?;
    let mut mime = String::new();
    let mut total = 0;
    for (_, value) in headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Type"))
    {
        context(cancel)?;
        total += value.len();
        limit("Content-Type header bytes", total as i64, 1024)?;
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if !mime.is_empty() {
            return Err(error("multiple Content-Type header values"));
        }
        mime = value.into();
    }
    let mut encodings = Vec::new();
    total = 0;
    for (_, value) in headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Encoding"))
    {
        total += value.len();
        limit("Content-Encoding header bytes", total as i64, 256)?;
        for token in value.split(',') {
            context(cancel)?;
            let token = token.trim();
            if token.is_empty() {
                return Err(error("Content-Encoding contains an empty token"));
            }
            let encoding = token.to_ascii_lowercase();
            if !["identity", "gzip", "x-gzip", "deflate", "br"].contains(&encoding.as_str()) {
                return Err(error(format!(
                    "unsupported content encoding {}",
                    quote(token)
                )));
            }
            if encodings.len() == 4 {
                return Err(error("content encoding layer count 5 exceeds limit 4"));
            }
            encodings.push(encoding);
        }
    }
    Ok((mime, encodings))
}

pub(super) fn load(
    client: &dyn ImageHttpClient,
    url: &str,
    limits: ImageAssetLimits,
    cancel: Option<&CancellationToken>,
) -> Result<(Vec<u8>, i64, String), ImageResolveError> {
    let response = client.get(url, limits.max_fetched_bytes, cancel)?;
    let (mime, encodings) = response_head(
        response.status,
        response.content_length,
        &response.headers,
        limits.max_fetched_bytes,
        cancel,
    )?;
    let fetched = response.body.len() as i64;
    limit("fetched bytes", fetched, limits.max_fetched_bytes)?;
    let data = d2_rs_math::image_svg::decode_http_content(
        &response.body,
        &encodings,
        limits.max_decompressed_bytes as usize,
        cancel,
    )
    .map_err(error)?;
    Ok((data, fetched, mime))
}

/// net/url.Parse canonicalization relevant to HTTP: retain the escaped path,
/// credentials and query in the private request/key, drop the fragment only.
pub(super) fn canonical_url(source: &str) -> Result<String, ImageResolveError> {
    let scheme_end = source.find(':').unwrap();
    let rest = &source[scheme_end + 1..];
    let Some(authority) = rest.strip_prefix("//") else {
        return Err(error("HTTP URL has no host"));
    };
    if authority
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .is_empty()
    {
        return Err(error("HTTP URL has no host"));
    }
    let bytes = source.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte < 0x20
            || byte == 0x7f
            || (byte == b'%'
                && !bytes
                    .get(i + 1..i + 3)
                    .is_some_and(|v| v.iter().all(u8::is_ascii_hexdigit)))
        {
            return Err(error("invalid HTTP URL"));
        }
    }
    url::Url::parse(source).map_err(|_| error("invalid HTTP URL"))?;
    Ok(format!(
        "{}:{}",
        source[..scheme_end].to_ascii_lowercase(),
        rest.split('#').next().unwrap()
    ))
}

pub(super) fn display_url(source: &str) -> String {
    if canonical_url(source).is_err() {
        return "<malformed URI>".into();
    }
    let (scheme, rest) = source.split_once(':').unwrap();
    let rest = rest.split(['?', '#']).next().unwrap();
    let rest = rest.strip_prefix("//").unwrap();
    let (authority, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    let host = authority.rsplit('@').next().unwrap();
    format!("{}://{host}{path}", scheme.to_ascii_lowercase())
}
