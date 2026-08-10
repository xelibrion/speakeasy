//! Exercises the real fetcher against a socket on localhost.
//!
//! No network and no dependency on Hugging Face being reachable, which matters
//! because this runs in a plain `cargo test`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use speakeasy_core::ports::ModelFetcher;
use speakeasy_core::Error;
use speakeasy_models::HttpFetcher;

/// Serves one response and returns the URL to reach it at.
fn serve(status: &'static str, body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("should bind");
    let port = listener
        .local_addr()
        .expect("should have an address")
        .port();

    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("should accept");
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request);

        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&body);
        let _ = stream.flush();
    });

    (format!("http://127.0.0.1:{port}/model.bin"), handle)
}

/// A path of our own, removed on drop.
struct TempFile(std::path::PathBuf);

impl TempFile {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "speakeasy-fetch-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn fetch_writes_the_body_and_reports_progress() {
    let body = vec![9u8; 200 * 1024];
    let (url, server) = serve("200 OK", body.clone());
    let destination = TempFile::new("ok");

    let ticks = Arc::new(Mutex::new(Vec::new()));
    let recorder = ticks.clone();

    HttpFetcher::new()
        .fetch(
            &url,
            &destination.0,
            &move |received, total| {
                recorder
                    .lock()
                    .expect("ticks mutex poisoned")
                    .push((received, total))
            },
            &AtomicBool::new(false),
        )
        .expect("fetch should succeed");
    server.join().expect("server thread should finish");

    assert_eq!(
        std::fs::read(&destination.0).expect("file should exist"),
        body
    );

    let ticks = ticks.lock().expect("ticks mutex poisoned");
    assert!(ticks.len() > 1, "progress was reported more than once");
    let (received, total) = *ticks.last().expect("at least one tick");
    assert_eq!(
        received,
        body.len() as u64,
        "the last tick is the whole body"
    );
    assert_eq!(total, body.len() as u64, "Content-Length becomes the total");
}

#[test]
fn a_cancelled_fetch_stops_early_and_leaves_no_file() {
    let (url, server) = serve("200 OK", vec![9u8; 4 * 1024 * 1024]);
    let destination = TempFile::new("cancel");

    HttpFetcher::new()
        .fetch(&url, &destination.0, &|_, _| {}, &AtomicBool::new(true))
        .expect("a cancel is not an error");
    let _ = server.join();

    assert!(
        !destination.0.exists(),
        "no partial file survives a cancel — the caller reads one as a finished download"
    );
}

#[test]
fn a_non_200_response_is_a_download_error() {
    let (url, server) = serve("404 Not Found", b"nope".to_vec());
    let destination = TempFile::new("missing");

    let err = HttpFetcher::new()
        .fetch(&url, &destination.0, &|_, _| {}, &AtomicBool::new(false))
        .expect_err("a 404 should not look like a download");
    let _ = server.join();

    assert!(
        matches!(err, Error::ModelDownload(_)),
        "unexpected error: {err:?}"
    );
}
