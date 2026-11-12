// SPDX-License-Identifier: MPL-2.0
use d2_rs_core::CancellationToken;
use d2_rs_scenebuild::{
    ImageAssetLimits, ImageHttpClient, ImageHttpResponse, ImageLocalFiles, ImageResolveError,
    RasterImageResolver,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Debug, Default)]
struct Transport(Arc<AtomicUsize>);
impl ImageHttpClient for Transport {
    fn close_idle_connections(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn get(
        &self,
        _: &str,
        _: i64,
        _: Option<&CancellationToken>,
    ) -> Result<ImageHttpResponse, ImageResolveError> {
        panic!("unexpected HTTP request")
    }
}
fn resolver() -> RasterImageResolver {
    RasterImageResolver::new(
        Path::new("."),
        ImageAssetLimits {
            max_fetched_bytes: 1024,
            max_encoded_bytes: 1024,
            max_decompressed_bytes: 1024,
            max_svg_bytes: 1024,
            max_decoded_width: 1024,
            max_decoded_height: 1024,
            max_decoded_pixels: 1024,
            max_assets: 16,
            max_cumulative_encoded_bytes: 16384,
            max_cumulative_decoded_bytes: 16384,
        },
        Arc::new(ImageLocalFiles::default()),
    )
    .unwrap()
}
#[test]
fn only_owned_transports_release_idle_connections() {
    let default = resolver();
    assert!(default.owns_http_transport());
    default.close_idle_connections();
    default.close_idle_connections();
    let borrowed = Arc::new(Transport::default());
    let r = resolver().with_http_client(borrowed.clone());
    assert!(!r.owns_http_transport());
    r.close_idle_connections();
    r.close_idle_connections();
    assert_eq!(borrowed.0.load(Ordering::SeqCst), 0);
    let owned = Transport::default();
    let calls = owned.0.clone();
    let r = r.with_owned_http_client(Box::new(owned));
    assert!(r.owns_http_transport());
    r.close_idle_connections();
    r.close_idle_connections();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(borrowed.0.load(Ordering::SeqCst), 0);
    let r = r.with_http_client(borrowed.clone());
    assert!(!r.owns_http_transport());
    r.close_idle_connections();
    assert_eq!(borrowed.0.load(Ordering::SeqCst), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Dropping a borrowing resolver must leave the client's explicit lifetime
    // with its caller as well.
    drop(r);
    borrowed.close_idle_connections();
    assert_eq!(borrowed.0.load(Ordering::SeqCst), 1);
}
