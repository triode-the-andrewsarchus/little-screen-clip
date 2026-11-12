// SPDX-License-Identifier: MPL-2.0
// Every test here starts from the regenerated imageasset capture (docs/FIXTURES.md).
#![cfg(feature = "oracle-fixtures")]
//! HTTP boundary for the asset-bearing isometric image/icon boards.
#![cfg(not(target_arch = "wasm32"))]
use base64::Engine as _;
use d2_rs_core::CancellationToken;
use d2_rs_scenebuild::{
    ImageAssetLimits, NativeImageHttpClient, RasterImageResolver, UnrestrictedImageFiles,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

fn fixture() -> (ImageAssetLimits, Vec<u8>) {
    let row: serde_json::Value = serde_json::from_str(
        d2_rs_testdata::oracle_fixture!("tests/fixtures/imageasset-local-data-master.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    (
        serde_json::from_value(row["request"]["limits"].clone()).unwrap(),
        base64::engine::general_purpose::STANDARD
            .decode(row["expected"]["resource"]["data"].as_str().unwrap())
            .unwrap(),
    )
}
fn resolver(private: bool) -> RasterImageResolver {
    RasterImageResolver::new(
        std::path::Path::new("."),
        fixture().0,
        Arc::new(UnrestrictedImageFiles::new()),
    )
    .unwrap()
    .with_http_client(Arc::new(NativeImageHttpClient {
        allow_private_networks: private,
    }))
}
fn request(socket: &mut std::net::TcpStream) -> String {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 8192);
    }
    String::from_utf8(bytes).unwrap()
}

fn accept(server: &TcpListener) -> std::net::TcpStream {
    server.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match server.accept() {
            Ok((socket, _)) => {
                // macOS can inherit the listener's nonblocking flag. The
                // fixture reads a complete request with read_exact.
                socket.set_nonblocking(false).unwrap();
                return socket;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("HTTP fixture accept failed: {e}"),
        }
    }
}

#[test]
fn native_redirect_and_sniffing_preserve_bytes_and_fragment_memoization() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.local_addr().unwrap());
    let png = fixture().1;
    let expected = png.clone();
    let work = thread::spawn(move || {
        for path in ["/redirect?keep=1", "/image"] {
            let mut socket = accept(&server);
            let headers = request(&mut socket);
            assert!(headers.starts_with(&format!("GET {path} HTTP/1.1\r\n")));
            let headers = headers.to_ascii_lowercase();
            assert!(headers.contains("user-agent: mozilla/5.0 (compatible; d2 imageasset)\r\n"));
            assert!(headers.contains("accept-encoding: gzip, deflate, br\r\n"));
            if path.starts_with("/redirect") {
                socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /image\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            } else {
                write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: image/jpg; charset=binary\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",png.len()).unwrap();
                socket.write_all(&png).unwrap();
            }
        }
    });
    let resolver = resolver(true);
    let first = resolver
        .resolve_with_context(&format!("{url}/redirect?keep=1#one"), None)
        .unwrap();
    assert_eq!(first.resource.data.as_ref(), expected);
    assert_eq!(first.resource.mime_type, "image/png");
    let again = resolver
        .resolve_with_context(&format!("{url}/redirect?keep=1#two"), None)
        .unwrap();
    assert_eq!(again.resource.data.as_ref(), expected);
    assert_eq!(resolver.usage().assets, 1);
    work.join().unwrap();
}

#[test]
fn native_public_only_policy_rejects_loopback_without_consuming_budget() {
    let resolver = resolver(false);
    let error = resolver
        .resolve_with_context("http://127.0.0.1:9/private?secret=yes#private", None)
        .unwrap_err();
    assert!(error.unavailable);
    assert_eq!(
        error.message,
        "imageasset: load \"http://127.0.0.1:9/private\": HTTP request failed"
    );
    assert_eq!(resolver.usage().assets, 0);
}

#[test]
fn native_cancellation_drops_a_stalled_body_and_preserves_its_cause() {
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/private?token=secret",
        server.local_addr().unwrap()
    );
    let (ready_tx, ready_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel();
    let work = thread::spawn(move || {
        let mut socket = accept(&server);
        request(&mut socket);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 999\r\n\r\n")
            .unwrap();
        ready_tx.send(()).unwrap();
        finish_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let resolver = Arc::new(resolver(true));
    let token = CancellationToken::new();
    let child_token = token.clone();
    let task_resolver = resolver.clone();
    let task = thread::spawn(move || task_resolver.resolve_with_context(&url, Some(&child_token)));
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let started = Instant::now();
    token.expire();
    let error = task.join().unwrap().unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(!error.unavailable);
    assert!(error.message.ends_with(": context deadline exceeded"));
    assert!(!error.message.contains("secret"));
    assert_eq!(resolver.usage().assets, 0);
    finish_tx.send(()).unwrap();
    work.join().unwrap();
}
