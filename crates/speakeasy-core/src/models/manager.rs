//! Owns the models directory: what is installed, what is downloading, and the
//! swap into the live transcriber.
//!
//! Separate from [`crate::engine::Engine`] because that file is already large
//! and this is a self-contained unit with its own state machine and its own
//! tests. The engine holds one and delegates to it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::ModelChoice;
use crate::error::{Error, Result};
use crate::event::ModelStatus;
use crate::models::catalog::{self, ModelId, ModelInfo, CATALOG};
use crate::ports::{ModelFetcher, Transcriber};

/// The smallest gap between two progress events.
///
/// The Tauri bridge hops to the main thread for every event the tray consumes,
/// so an unthrottled 1.6 GB download would schedule thousands of redundant
/// main-thread updates. Injectable in tests so they do not have to sleep.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Invoked on every model status change. Mirrors [`crate::ports::HotkeyCallback`]:
/// called from whatever thread the install runs on, so it must not block.
pub type StatusCallback = Box<dyn Fn(ModelStatus) + Send + Sync>;

pub struct ModelManager {
    fetcher: Box<dyn ModelFetcher>,
    transcriber: Arc<dyn Transcriber>,
    directory: PathBuf,
    status: Mutex<ModelStatus>,
    cancel: AtomicBool,
    /// Set for the whole of an install, so a second one can be refused.
    installing: Mutex<bool>,
    progress_interval: Duration,
    /// Where status changes go. The engine points this at its event stream.
    on_status: Mutex<Option<StatusCallback>>,
}

impl ModelManager {
    pub fn new(
        fetcher: Box<dyn ModelFetcher>,
        transcriber: Arc<dyn Transcriber>,
        directory: PathBuf,
    ) -> Self {
        Self {
            fetcher,
            transcriber,
            directory,
            status: Mutex::new(ModelStatus::Absent),
            cancel: AtomicBool::new(false),
            installing: Mutex::new(false),
            progress_interval: PROGRESS_INTERVAL,
            on_status: Mutex::new(None),
        }
    }

    /// Test seam for the throttle. See [`PROGRESS_INTERVAL`].
    #[cfg(test)]
    fn with_progress_interval(mut self, interval: Duration) -> Self {
        self.progress_interval = interval;
        self
    }

    /// Observe status changes. Replaces any previous sink.
    pub fn on_status(&self, sink: StatusCallback) {
        *self.on_status.lock().expect("status sink mutex poisoned") = Some(sink);
    }

    pub fn status(&self) -> ModelStatus {
        self.status.lock().expect("status mutex poisoned").clone()
    }

    /// Where this choice's weights live. Catalog models sit in our directory; a
    /// local choice is wherever the user put it.
    ///
    /// Returns an `Option` even though both arms are `Some` today because the
    /// callers already branch on "is there a file to load", and a future choice
    /// — a model bundled with the app, say — may not have a path of its own.
    pub fn path_for(&self, choice: &ModelChoice) -> Option<PathBuf> {
        match choice {
            ModelChoice::Catalog { id } => Some(self.directory.join(id.file_name())),
            ModelChoice::Local { path } => Some(PathBuf::from(path)),
        }
    }

    /// Catalog models whose file is on disk, in catalog order.
    pub fn installed(&self) -> Vec<ModelId> {
        CATALOG
            .iter()
            .filter(|info| self.directory.join(info.id.file_name()).is_file())
            .map(|info| info.id.clone())
            .collect()
    }

    /// Delete a downloaded model.
    ///
    /// A model that is not there is success: the caller wants it gone, and it
    /// is. That keeps the UI from having to disable the button on a stale list.
    pub fn remove(&self, id: &ModelId) -> Result<()> {
        let path = self.directory.join(id.file_name());
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::Other(format!(
                "could not remove {}: {err}",
                path.display()
            ))),
        }
    }

    /// Ask an in-flight install to stop. Returns immediately; the install
    /// notices on its next chunk.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Download, verify and activate a catalog model.
    ///
    /// Blocking. `Ok(None)` means the user cancelled, which is not a failure.
    pub fn install(&self, id: &ModelId) -> Result<Option<PathBuf>> {
        let info = catalog::find(id)
            .ok_or_else(|| Error::ModelDownload(format!("{id} is not a model I know about")))?;
        self.install_info(info)
    }

    /// One install at a time.
    ///
    /// A second request is refused rather than queued or silently replacing the
    /// first: abandoning a 1.5 GB transfer because of a stray second click
    /// would be worse than saying no, and the UI has an explicit Cancel.
    fn install_info(&self, info: &ModelInfo) -> Result<Option<PathBuf>> {
        {
            let mut installing = self.installing.lock().expect("installing mutex poisoned");
            if *installing {
                return Err(Error::ModelDownload(
                    "another model is already downloading".to_string(),
                ));
            }
            *installing = true;
        }

        let outcome = self.run_install(info);

        *self.installing.lock().expect("installing mutex poisoned") = false;
        // Cleared here rather than at the start, so a cancel that arrives while
        // this install is winding down cannot leak into the next one.
        self.cancel.store(false, Ordering::Relaxed);
        outcome
    }

    /// Fetch, verify, rename, swap — in that order, which is the whole design.
    ///
    /// The live model is only replaced once the new one is known-good and
    /// loaded, so a download can run for ten minutes while the user keeps
    /// dictating, and every failure short of the last step is a no-op.
    fn run_install(&self, info: &ModelInfo) -> Result<Option<PathBuf>> {
        std::fs::create_dir_all(&self.directory).map_err(|err| {
            Error::ModelDownload(format!(
                "could not create {}: {err}",
                self.directory.display()
            ))
        })?;

        let final_path = self.directory.join(info.id.file_name());
        let partial = final_path.with_extension("bin.part");

        self.set_status(ModelStatus::Downloading {
            id: info.id.clone(),
            received: 0,
            total: info.bytes,
        });

        let last = Mutex::new(std::time::Instant::now());
        let progress = |received: u64, reported_total: u64| {
            let mut last = last.lock().expect("progress clock mutex poisoned");
            if last.elapsed() < self.progress_interval {
                return;
            }
            *last = std::time::Instant::now();
            self.set_status(ModelStatus::Downloading {
                id: info.id.clone(),
                received,
                // Trust the catalog over the server: it is what the size in the
                // UI was drawn from, and a header can be absent or wrong.
                total: if info.bytes > 0 {
                    info.bytes
                } else {
                    reported_total
                },
            });
        };

        if let Err(err) = self
            .fetcher
            .fetch(info.url, &partial, &progress, &self.cancel)
        {
            let _ = std::fs::remove_file(&partial);
            self.set_status(ModelStatus::Failed { error: err.clone() });
            return Err(err);
        }

        if self.cancel.load(Ordering::Relaxed) || !partial.exists() {
            let _ = std::fs::remove_file(&partial);
            self.set_status(self.status_for_loaded());
            return Ok(None);
        }

        if !self.digest_matches(&partial, info.sha256)? {
            let _ = std::fs::remove_file(&partial);
            self.set_status(ModelStatus::Failed {
                error: Error::ModelVerification,
            });
            return Err(Error::ModelVerification);
        }

        std::fs::rename(&partial, &final_path).map_err(|err| {
            let error =
                Error::ModelDownload(format!("could not install {}: {err}", final_path.display()));
            self.set_status(ModelStatus::Failed {
                error: error.clone(),
            });
            error
        })?;

        self.activate(&final_path, info.label)?;
        Ok(Some(final_path))
    }

    /// Load `path` and report it as live.
    ///
    /// Failure leaves the previous model in place — that is
    /// [`Transcriber::load`]'s contract, and it is why the swap is last.
    pub fn activate(&self, path: &Path, label: &str) -> Result<()> {
        match self.transcriber.load(path) {
            Ok(()) => {
                self.set_status(ModelStatus::Ready {
                    label: label.to_string(),
                });
                Ok(())
            }
            Err(err) => {
                self.set_status(ModelStatus::Failed { error: err.clone() });
                Err(err)
            }
        }
    }

    /// Streamed rather than read whole: the largest catalog entry is over a
    /// gigabyte, and this runs on every install.
    fn digest_matches(&self, path: &Path, expected: &str) -> Result<bool> {
        use sha2::{Digest, Sha256};

        let mut file = std::fs::File::open(path).map_err(|err| {
            Error::ModelDownload(format!("could not read {}: {err}", path.display()))
        })?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher).map_err(|err| {
            Error::ModelDownload(format!("could not hash {}: {err}", path.display()))
        })?;

        Ok(format!("{:x}", hasher.finalize()) == expected)
    }

    /// What to report once an install stops without replacing anything.
    ///
    /// A cancel must not look like a failure, and it must not claim a model is
    /// absent when one is loaded and working.
    fn status_for_loaded(&self) -> ModelStatus {
        match self.status() {
            ready @ ModelStatus::Ready { .. } => ready,
            _ if self.transcriber.is_ready() => ModelStatus::Ready {
                label: "Model".to_string(),
            },
            _ => ModelStatus::Absent,
        }
    }

    fn set_status(&self, next: ModelStatus) {
        *self.status.lock().expect("status mutex poisoned") = next.clone();
        if let Some(sink) = self
            .on_status
            .lock()
            .expect("status sink mutex poisoned")
            .as_ref()
        {
            sink(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeFetcher, FakeTranscriber};
    use crate::testing::TempDir;

    fn manager(dir: &TempDir) -> ModelManager {
        ModelManager::new(
            Box::new(FakeFetcher::serving(Vec::new())),
            Arc::new(FakeTranscriber::not_ready()),
            dir.0.clone(),
        )
    }

    #[test]
    fn a_catalog_choice_resolves_into_the_models_directory() {
        let dir = TempDir::new("layout");

        assert_eq!(
            manager(&dir).path_for(&ModelChoice::Catalog {
                id: ModelId::new("base.en")
            }),
            Some(dir.join("ggml-base.en.bin"))
        );
    }

    #[test]
    fn a_local_choice_resolves_to_its_own_path() {
        let dir = TempDir::new("layout-local");

        assert_eq!(
            manager(&dir).path_for(&ModelChoice::Local {
                path: "/elsewhere/m.bin".into()
            }),
            Some(PathBuf::from("/elsewhere/m.bin"))
        );
    }

    #[test]
    fn only_files_that_exist_count_as_installed() {
        let dir = TempDir::new("installed");
        let manager = manager(&dir);
        assert!(manager.installed().is_empty());

        std::fs::write(dir.join("ggml-base.en.bin"), b"weights").expect("write should succeed");

        assert_eq!(manager.installed(), vec![ModelId::new("base.en")]);
    }

    /// A `.part` file is an interrupted download, not a model.
    #[test]
    fn a_partial_download_is_not_installed() {
        let dir = TempDir::new("partial");
        std::fs::write(dir.join("ggml-base.en.bin.part"), b"half").expect("write should succeed");

        assert!(manager(&dir).installed().is_empty());
    }

    #[test]
    fn removing_a_model_deletes_its_file() {
        let dir = TempDir::new("remove");
        let manager = manager(&dir);
        std::fs::write(dir.join("ggml-base.en.bin"), b"weights").expect("write should succeed");

        manager
            .remove(&ModelId::new("base.en"))
            .expect("remove should succeed");

        assert!(manager.installed().is_empty());
    }

    #[test]
    fn removing_a_model_that_is_not_there_is_not_an_error() {
        let dir = TempDir::new("remove-missing");

        assert!(manager(&dir).remove(&ModelId::new("base.en")).is_ok());
    }

    /// A catalog entry describing bytes the fake will actually serve.
    ///
    /// The real catalog cannot be used here: its digests belong to files far
    /// too large to put in a unit test.
    fn entry(body: &[u8]) -> ModelInfo {
        use sha2::{Digest, Sha256};

        ModelInfo {
            id: ModelId::new("base.en"),
            label: "Base (English)",
            note: "test entry",
            bytes: body.len() as u64,
            // `sha256` is `&'static str` for the real catalog's sake; leaking a
            // few bytes per test is the cheapest way to satisfy that here.
            sha256: Box::leak(format!("{:x}", Sha256::digest(body)).into_boxed_str()),
            url: "https://example.invalid/ggml-base.en.bin",
            english_only: true,
        }
    }

    fn manager_serving(dir: &TempDir, body: &[u8]) -> (ModelManager, Arc<FakeTranscriber>) {
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let manager = ModelManager::new(
            Box::new(FakeFetcher::serving(body.to_vec())),
            transcriber.clone(),
            dir.0.clone(),
        );
        (manager, transcriber)
    }

    #[test]
    fn a_successful_install_writes_the_file_and_swaps_the_transcriber() {
        let dir = TempDir::new("install-ok");
        let body = b"pretend weights";
        let (manager, transcriber) = manager_serving(&dir, body);

        let path = manager
            .install_info(&entry(body))
            .expect("install should succeed")
            .expect("install should not report a cancel");

        assert_eq!(path, dir.join("ggml-base.en.bin"));
        assert_eq!(std::fs::read(&path).expect("file should exist"), body);
        assert_eq!(transcriber.loaded(), Some(path), "the new model is live");
        assert!(matches!(manager.status(), ModelStatus::Ready { .. }));
    }

    #[test]
    fn a_digest_mismatch_leaves_no_file_and_reports_verification() {
        let dir = TempDir::new("install-corrupt");
        let (manager, transcriber) = manager_serving(&dir, b"corrupted bytes");

        let err = manager
            .install_info(&entry(b"the expected bytes"))
            .expect_err("a mismatch should fail");

        assert_eq!(err, Error::ModelVerification);
        assert!(
            manager.installed().is_empty(),
            "nothing was renamed into place"
        );
        assert!(
            !dir.join("ggml-base.en.bin.part").exists(),
            "the partial file is cleaned up"
        );
        assert_eq!(transcriber.loaded(), None, "nothing was swapped in");
    }

    #[test]
    fn a_transport_failure_surfaces_as_a_download_error() {
        let dir = TempDir::new("install-transport");
        let manager = ModelManager::new(
            Box::new(FakeFetcher::failing(Error::ModelDownload("offline".into()))),
            Arc::new(FakeTranscriber::not_ready()),
            dir.0.clone(),
        );

        let err = manager.install_info(&entry(b"x")).expect_err("should fail");

        assert!(matches!(err, Error::ModelDownload(_)));
        assert!(matches!(manager.status(), ModelStatus::Failed { .. }));
    }

    /// The reason the swap is last: a download that arrives intact but will not
    /// load must not cost the user the model they are already dictating with.
    #[test]
    fn a_failed_swap_leaves_the_previous_model_loaded() {
        let dir = TempDir::new("install-badswap");
        let body = b"pretend weights";
        let (manager, transcriber) = manager_serving(&dir, body);
        transcriber
            .load(Path::new("/previous/ggml-old.bin"))
            .expect("the previous model loads");
        transcriber.fail_next_load(Error::Transcription("unsupported format".into()));

        let err = manager
            .install_info(&entry(body))
            .expect_err("the swap should fail");

        assert!(matches!(err, Error::Transcription(_)));
        assert_eq!(
            transcriber.loaded(),
            Some(PathBuf::from("/previous/ggml-old.bin")),
            "the working model is still the live one"
        );
    }

    #[test]
    fn a_cancelled_install_reports_no_path_and_leaves_nothing_behind() {
        let dir = TempDir::new("install-cancel");
        let body = b"pretend weights";
        let fetcher = Arc::new(FakeFetcher::serving(body.to_vec()));
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let manager = ModelManager::new(
            Box::new(fetcher.clone()),
            transcriber.clone(),
            dir.0.clone(),
        );

        manager.cancel();
        let outcome = manager
            .install_info(&entry(body))
            .expect("a cancel is not an error");

        assert_eq!(outcome, None);
        assert!(
            fetcher.observed_cancel(),
            "the cancel reached the transport"
        );
        assert!(manager.installed().is_empty());
        assert_eq!(transcriber.loaded(), None);
    }

    #[test]
    fn a_second_install_is_refused_while_one_is_in_flight() {
        let dir = TempDir::new("install-busy");
        let (manager, _transcriber) = manager_serving(&dir, b"weights");
        *manager
            .installing
            .lock()
            .expect("installing mutex poisoned") = true;

        let err = manager
            .install_info(&entry(b"weights"))
            .expect_err("a second install should be refused");

        assert!(matches!(err, Error::ModelDownload(_)));
    }

    /// An unthrottled multi-gigabyte download would schedule thousands of
    /// redundant main-thread updates in the Tauri shell.
    #[test]
    fn progress_events_are_throttled() {
        let dir = TempDir::new("install-throttle");
        let body = vec![7u8; 4096];
        let seen = Arc::new(Mutex::new(Vec::new()));

        let manager = ModelManager::new(
            Box::new(FakeFetcher::serving(body.clone()).in_chunks(500)),
            Arc::new(FakeTranscriber::not_ready()),
            dir.0.clone(),
        )
        .with_progress_interval(Duration::from_secs(3600));

        let recorder = seen.clone();
        manager.on_status(Box::new(move |status| {
            recorder.lock().expect("seen mutex poisoned").push(status);
        }));

        manager
            .install_info(&entry(&body))
            .expect("install should succeed");

        let downloads = seen
            .lock()
            .expect("seen mutex poisoned")
            .iter()
            .filter(|status| matches!(status, ModelStatus::Downloading { .. }))
            .count();
        assert_eq!(
            downloads, 1,
            "500 chunks inside one interval must collapse to the opening tick"
        );
    }

    #[test]
    fn installing_a_model_outside_the_catalog_is_an_error() {
        let dir = TempDir::new("install-unknown");
        let (manager, _transcriber) = manager_serving(&dir, b"weights");

        let err = manager
            .install(&ModelId::new("enormous-v9"))
            .expect_err("an unknown id should fail");

        assert!(matches!(err, Error::ModelDownload(_)));
    }
}
