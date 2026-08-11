//! Test doubles for every port, behind the `fakes` feature.
//!
//! These let the engine, and any shell, run headless — no microphone, no model
//! weights, no Accessibility permission.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::config::Hotkey;
use crate::error::{Error, Result};
use crate::ports::{
    AudioSource, CaptureCallback, HotkeyCallback, HotkeyEvent, HotkeyListener, ModelFetcher,
    TextSink, Transcriber,
};

/// Returns a canned buffer from `stop`, and records whether it is capturing.
pub struct FakeAudio {
    samples: Vec<f32>,
    recording: Mutex<bool>,
    fail_with: Option<Error>,
}

impl FakeAudio {
    pub fn with_samples(samples: Vec<f32>) -> Self {
        Self {
            samples,
            recording: Mutex::new(false),
            fail_with: None,
        }
    }

    /// An audio source whose `start` always fails — e.g. permission denied.
    pub fn failing(error: Error) -> Self {
        Self {
            samples: Vec::new(),
            recording: Mutex::new(false),
            fail_with: Some(error),
        }
    }

    pub fn is_recording(&self) -> bool {
        *self.recording.lock().expect("recording mutex poisoned")
    }
}

impl AudioSource for FakeAudio {
    fn start(&self) -> Result<()> {
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        *self.recording.lock().expect("recording mutex poisoned") = true;
        Ok(())
    }

    fn stop(&self) -> Result<Vec<f32>> {
        *self.recording.lock().expect("recording mutex poisoned") = false;
        Ok(self.samples.clone())
    }

    fn level(&self) -> f32 {
        if self.is_recording() {
            0.5
        } else {
            0.0
        }
    }

    /// A fixed tilt rather than a flat 0.5, so a test can tell band order from
    /// the values alone.
    fn spectrum(&self) -> Vec<f32> {
        if !self.is_recording() {
            return vec![0.0; crate::SPECTRUM_BANDS];
        }
        (0..crate::SPECTRUM_BANDS)
            .map(|band| 1.0 - band as f32 / crate::SPECTRUM_BANDS as f32)
            .collect()
    }
}

/// Returns a fixed transcript, ignoring the audio.
pub struct FakeTranscriber {
    result: Result<String>,
    ready: Mutex<bool>,
    loaded: Mutex<Option<PathBuf>>,
    load_fails_with: Mutex<Option<Error>>,
}

impl FakeTranscriber {
    pub fn returning(text: &str) -> Self {
        Self {
            result: Ok(text.to_string()),
            ready: Mutex::new(true),
            loaded: Mutex::new(None),
            load_fails_with: Mutex::new(None),
        }
    }

    pub fn failing(error: Error) -> Self {
        Self {
            result: Err(error),
            ..Self::returning("")
        }
    }

    pub fn not_ready() -> Self {
        Self {
            ready: Mutex::new(false),
            ..Self::returning("")
        }
    }

    /// Make the next `load` fail, so a test can assert that the previously
    /// loaded model survives a bad swap.
    pub fn fail_next_load(&self, error: Error) {
        *self
            .load_fails_with
            .lock()
            .expect("load failure mutex poisoned") = Some(error);
    }

    /// The path most recently loaded, if any.
    pub fn loaded(&self) -> Option<PathBuf> {
        self.loaded.lock().expect("loaded mutex poisoned").clone()
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(&self, _samples: &[f32], _language: Option<&str>) -> Result<String> {
        self.result.clone()
    }

    fn is_ready(&self) -> bool {
        *self.ready.lock().expect("ready mutex poisoned")
    }

    fn load(&self, path: &Path) -> Result<()> {
        // Taken rather than cloned, so a test arms one failure rather than
        // permanently breaking the fake.
        if let Some(error) = self
            .load_fails_with
            .lock()
            .expect("load failure mutex poisoned")
            .take()
        {
            return Err(error);
        }
        // Real implementations refuse a path that is not there, and callers
        // depend on that — the engine's fallback only runs because loading a
        // missing file fails. A fake that accepts anything would hide it.
        if !path.exists() {
            return Err(Error::Transcription(format!(
                "no model at {}",
                path.display()
            )));
        }
        *self.loaded.lock().expect("loaded mutex poisoned") = Some(path.to_path_buf());
        *self.ready.lock().expect("ready mutex poisoned") = true;
        Ok(())
    }
}

/// So a test can hold an `Arc<FakeTranscriber>` to assert on while the engine
/// owns the same instance behind a trait object.
impl Transcriber for Arc<FakeTranscriber> {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<String> {
        self.as_ref().transcribe(samples, language)
    }

    fn is_ready(&self) -> bool {
        self.as_ref().is_ready()
    }

    fn load(&self, path: &Path) -> Result<()> {
        self.as_ref().load(path)
    }
}

/// Writes canned bytes to the destination, one progress tick per chunk.
pub struct FakeFetcher {
    /// What to write. The manager hashes this, so a test controls whether
    /// verification passes by controlling these bytes.
    body: Vec<u8>,
    chunks: usize,
    fail_with: Option<Error>,
    /// Set once `fetch` has observed the cancel flag, so a test can assert the
    /// cancel actually reached the transport rather than racing past it.
    observed_cancel: Mutex<bool>,
    calls: Mutex<usize>,
}

impl FakeFetcher {
    pub fn serving(body: impl Into<Vec<u8>>) -> Self {
        Self {
            body: body.into(),
            chunks: 4,
            fail_with: None,
            observed_cancel: Mutex::new(false),
            calls: Mutex::new(0),
        }
    }

    pub fn failing(error: Error) -> Self {
        Self {
            fail_with: Some(error),
            ..Self::serving(Vec::new())
        }
    }

    /// How many progress ticks a fetch reports. The throttle test uses this to
    /// generate far more ticks than should be emitted.
    pub fn in_chunks(mut self, chunks: usize) -> Self {
        self.chunks = chunks.max(1);
        self
    }

    pub fn observed_cancel(&self) -> bool {
        *self.observed_cancel.lock().expect("cancel mutex poisoned")
    }

    pub fn calls(&self) -> usize {
        *self.calls.lock().expect("calls mutex poisoned")
    }
}

impl ModelFetcher for FakeFetcher {
    fn fetch(
        &self,
        _url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        *self.calls.lock().expect("calls mutex poisoned") += 1;

        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }

        let total = self.body.len() as u64;
        let chunk = (self.body.len() / self.chunks).max(1);
        let mut written: Vec<u8> = Vec::new();

        for piece in self.body.chunks(chunk) {
            if cancel.load(Ordering::Relaxed) {
                *self.observed_cancel.lock().expect("cancel mutex poisoned") = true;
                // The port's contract: no file survives a cancel.
                let _ = std::fs::remove_file(destination);
                return Ok(());
            }
            written.extend_from_slice(piece);
            progress(written.len() as u64, total);
        }

        std::fs::write(destination, &written)
            .map_err(|err| Error::ModelDownload(format!("fake write failed: {err}")))?;
        Ok(())
    }
}

impl ModelFetcher for Arc<FakeFetcher> {
    fn fetch(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<()> {
        self.as_ref().fetch(url, destination, progress, cancel)
    }
}

/// Records everything injected so tests can assert on it.
#[derive(Default)]
pub struct FakeSink {
    injected: Mutex<Vec<String>>,
    fail_with: Option<Error>,
}

impl FakeSink {
    pub fn failing(error: Error) -> Self {
        Self {
            injected: Mutex::new(Vec::new()),
            fail_with: Some(error),
        }
    }

    pub fn injected(&self) -> Vec<String> {
        self.injected
            .lock()
            .expect("injected mutex poisoned")
            .clone()
    }
}

impl TextSink for FakeSink {
    fn inject(&self, text: &str) -> Result<usize> {
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        self.injected
            .lock()
            .expect("injected mutex poisoned")
            .push(text.to_string());
        Ok(text.chars().count())
    }
}

/// So a test can hold an `Arc<FakeSink>` to assert on while the engine owns a
/// `Box<dyn TextSink>` built from the same instance.
impl TextSink for Arc<FakeSink> {
    fn inject(&self, text: &str) -> Result<usize> {
        self.as_ref().inject(text)
    }
}

/// Accepts any registration and lets the test fire press/release by hand.
#[derive(Default)]
pub struct FakeHotkeys {
    callback: Mutex<Option<HotkeyCallback>>,
    bound: Mutex<Option<Hotkey>>,
    capturing: Mutex<Option<CaptureCallback>>,
}

impl FakeHotkeys {
    /// Simulate the user pressing or releasing the push-to-talk key.
    pub fn fire(&self, event: HotkeyEvent) {
        if let Some(callback) = self
            .callback
            .lock()
            .expect("callback mutex poisoned")
            .as_ref()
        {
            callback(event);
        }
    }

    /// What is currently bound, if anything.
    pub fn bound(&self) -> Option<Hotkey> {
        self.bound.lock().expect("bound mutex poisoned").clone()
    }

    /// Simulate the user pressing the key they want to bind.
    pub fn finish_capture(&self, hotkey: Hotkey) {
        let captured = self
            .capturing
            .lock()
            .expect("capturing mutex poisoned")
            .take();
        if let Some(callback) = captured {
            callback(hotkey);
        }
    }

    /// Whether a capture is armed. The real listener swallows keystrokes while
    /// one is, so a test that leaves it armed is a test that would wedge the
    /// keyboard in production.
    pub fn is_capturing(&self) -> bool {
        self.capturing
            .lock()
            .expect("capturing mutex poisoned")
            .is_some()
    }
}

impl HotkeyListener for FakeHotkeys {
    fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback) -> Result<()> {
        *self.bound.lock().expect("bound mutex poisoned") = Some(hotkey.clone());
        *self.callback.lock().expect("callback mutex poisoned") = Some(on_event);
        Ok(())
    }

    fn unregister(&self) -> Result<()> {
        *self.bound.lock().expect("bound mutex poisoned") = None;
        *self.callback.lock().expect("callback mutex poisoned") = None;
        Ok(())
    }

    fn capture(&self, on_captured: CaptureCallback) -> Result<()> {
        *self.capturing.lock().expect("capturing mutex poisoned") = Some(on_captured);
        Ok(())
    }

    fn cancel_capture(&self) {
        *self.capturing.lock().expect("capturing mutex poisoned") = None;
    }
}

impl HotkeyListener for Arc<FakeHotkeys> {
    fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback) -> Result<()> {
        self.as_ref().register(hotkey, on_event)
    }

    fn unregister(&self) -> Result<()> {
        self.as_ref().unregister()
    }

    fn capture(&self, on_captured: CaptureCallback) -> Result<()> {
        self.as_ref().capture(on_captured)
    }

    fn cancel_capture(&self) {
        self.as_ref().cancel_capture();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModifierKey;

    #[test]
    fn a_capture_reports_the_key_that_was_pressed() {
        let hotkeys = Arc::new(FakeHotkeys::default());
        let captured = Arc::new(Mutex::new(None));

        let sink = Arc::clone(&captured);
        hotkeys
            .capture(Box::new(move |hotkey| {
                *sink.lock().expect("captured mutex poisoned") = Some(hotkey);
            }))
            .expect("capture arms");

        let pressed = Hotkey::Modifier {
            key: ModifierKey::LeftCommand,
        };
        hotkeys.finish_capture(pressed.clone());

        assert_eq!(
            *captured.lock().expect("captured mutex poisoned"),
            Some(pressed)
        );
    }

    /// A capture that stayed armed would go on swallowing keystrokes with
    /// nothing on screen to explain it.
    #[test]
    fn a_capture_disarms_once_it_has_reported() {
        let hotkeys = FakeHotkeys::default();
        hotkeys.capture(Box::new(|_| {})).expect("capture arms");
        assert!(hotkeys.is_capturing());

        hotkeys.finish_capture(Hotkey::default());
        assert!(!hotkeys.is_capturing());
    }

    #[test]
    fn a_cancelled_capture_reports_nothing() {
        let hotkeys = Arc::new(FakeHotkeys::default());
        let captured = Arc::new(Mutex::new(None));

        let sink = Arc::clone(&captured);
        hotkeys
            .capture(Box::new(move |hotkey| {
                *sink.lock().expect("captured mutex poisoned") = Some(hotkey);
            }))
            .expect("capture arms");
        hotkeys.cancel_capture();
        hotkeys.finish_capture(Hotkey::default());

        assert_eq!(*captured.lock().expect("captured mutex poisoned"), None);
    }
}
