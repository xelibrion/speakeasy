//! [`ModelFetcher`] over HTTP.
//!
//! Blocking on purpose: the port's contract is blocking, so an async runtime
//! would be a dependency with nothing to do. This crate exists so that
//! `speakeasy-core` does not have to carry a TLS stack — the same split that
//! keeps `whisper-rs` behind the `Transcriber` port.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use speakeasy_core::ports::ModelFetcher;
use speakeasy_core::{Error, Result};

/// How much to read before checking the cancel flag.
///
/// Small enough that a cancel feels immediate on a slow link, large enough not
/// to syscall per kilobyte on a fast one.
const CHUNK: usize = 64 * 1024;

#[derive(Default)]
pub struct HttpFetcher;

impl HttpFetcher {
    pub fn new() -> Self {
        Self
    }
}

impl ModelFetcher for HttpFetcher {
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut response = ureq::get(url)
            .call()
            .map_err(|err| Error::ModelDownload(format!("{url}: {err}")))?;

        // Advisory only: the manager trusts the catalog's size for the UI and
        // the catalog's digest for correctness. This is here so a fetch of
        // something outside the catalog can still show a progress bar.
        let total = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        let mut file = std::fs::File::create(destination).map_err(|err| {
            Error::ModelDownload(format!("could not create {}: {err}", destination.display()))
        })?;

        let mut reader = response.body_mut().as_reader();
        let mut buffer = vec![0u8; CHUNK];
        let mut received = 0u64;

        loop {
            if cancel.load(Ordering::Relaxed) {
                // The port's contract: nothing survives a cancel, because the
                // caller reads a surviving file as a finished download. Dropped
                // first so the handle is closed before the file is unlinked.
                drop(file);
                let _ = std::fs::remove_file(destination);
                tracing::info!(url, received, "model download cancelled");
                return Ok(());
            }

            let read = reader
                .read(&mut buffer)
                .map_err(|err| Error::ModelDownload(format!("{url}: {err}")))?;
            if read == 0 {
                break;
            }

            file.write_all(&buffer[..read]).map_err(|err| {
                Error::ModelDownload(format!("could not write {}: {err}", destination.display()))
            })?;
            received += read as u64;
            progress(received, total);
        }

        file.flush().map_err(|err| {
            Error::ModelDownload(format!("could not flush {}: {err}", destination.display()))
        })?;
        tracing::info!(url, received, "model downloaded");
        Ok(())
    }
}
