// SPDX-License-Identifier: MPL-2.0
//! Native imageasset HTTP client; netpolicy validated dial/redirect boundary.
//! Port shares source mapping with the existing CLI http_asset adapter.
use super::super::{context, error, limit};
use super::{IMAGE_HTTP_HEADERS, ImageHttpClient, ImageHttpResponse};
use crate::ImageResolveError;
use crate::netpolicy::{BlockedAddress, host_is_blocked, public_client_builder};
use d2_rs_core::CancellationToken;
use std::time::Duration;
/// Default public-only policy. Private networks require an explicit opt-in.
/// No process-global policy, asset cache or runtime is installed.
#[derive(Debug, Default)]
pub struct NativeImageHttpClient {
    pub allow_private_networks: bool,
}

async fn cancelled(cancel: Option<&CancellationToken>) {
    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

impl ImageHttpClient for NativeImageHttpClient {
    fn get(
        &self,
        url: &str,
        max_fetched_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<ImageHttpResponse, ImageResolveError> {
        context(cancel)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| error("HTTP request failed"))?;
        let result = runtime.block_on(async {
            tokio::select! {
                result = self.get_async(url, max_fetched_bytes, cancel) => result,
                _ = cancelled(cancel) => Err(context(cancel).unwrap_err()),
            }
        });
        // DNS may still be inside the OS resolver. Cancellation never waits
        // for a detached blocking lookup to finish.
        runtime.shutdown_background();
        context(cancel)?;
        result
    }
}

impl NativeImageHttpClient {
    async fn get_async(
        &self,
        url: &str,
        max_fetched_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<ImageHttpResponse, ImageResolveError> {
        let allow_private = self.allow_private_networks;
        let client = if allow_private {
            reqwest::Client::builder()
        } else {
            public_client_builder()
        }
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if !allow_private && host_is_blocked(attempt.url()) {
                attempt.error(BlockedAddress)
            } else if attempt.previous().len() >= 10 {
                attempt.error("stopped after 10 redirects")
            } else {
                attempt.follow()
            }
        }));
        let unavailable = || ImageResolveError {
            unavailable: true,
            message: "HTTP request failed".into(),
        };
        let client = client.build().map_err(|_| error("HTTP request failed"))?;
        if !allow_private && reqwest::Url::parse(url).is_ok_and(|u| host_is_blocked(&u)) {
            return Err(unavailable());
        }
        let mut request = client.get(url);
        for (name, value) in IMAGE_HTTP_HEADERS {
            request = request.header(name, value);
        }
        let mut response = request.send().await.map_err(|e| {
            // A default redirect-count rejection carries a response in Go;
            // a dial-policy rejection does not. Keep those cases distinct.
            if e.is_redirect() && !blocked_error(&e) {
                error("HTTP request failed")
            } else {
                unavailable()
            }
        })?;
        context(cancel)?;
        let mut result = ImageHttpResponse {
            status: response.status().as_u16(),
            content_length: response
                .content_length()
                .and_then(|n| i64::try_from(n).ok())
                .unwrap_or(-1),
            headers: response
                .headers()
                .iter()
                .map(|(k, v)| {
                    Ok((
                        k.as_str().into(),
                        String::from_utf8(v.as_bytes().to_vec())
                            .map_err(|_| error("non-UTF-8 HTTP response header is unsupported"))?,
                    ))
                })
                .collect::<Result<_, ImageResolveError>>()?,
            body: Vec::new(),
        };
        result.validate_head(max_fetched_bytes, cancel)?;
        while let Some(chunk) = response.chunk().await.map_err(|_| ImageResolveError {
            unavailable: true,
            message: "read HTTP response body failed".into(),
        })? {
            context(cancel)?;
            let total = result.body.len().saturating_add(chunk.len()) as i64;
            limit("fetched bytes", total, max_fetched_bytes)?;
            result.body.extend_from_slice(&chunk);
        }
        context(cancel)?;
        Ok(result)
    }
}

fn blocked_error(error: &reqwest::Error) -> bool {
    use std::error::Error as _;
    let mut cause = error.source();
    while let Some(inner) = cause {
        if inner.downcast_ref::<BlockedAddress>().is_some() {
            return true;
        }
        cause = inner.source();
    }
    false
}
