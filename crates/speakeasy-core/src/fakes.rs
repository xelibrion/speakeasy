//! Test doubles for every port, behind the `fakes` feature.
//!
//! These let the engine, and any shell, run headless — no microphone, no model
//! weights, no Accessibility permission.

use std::sync::{Arc, Mutex};

use crate::config::Hotkey;
use crate::error::{Error, Result};
use crate::ports::{
    AudioSource, HotkeyCallback, HotkeyEvent, HotkeyListener, TextSink, Transcriber,
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
    ready: bool,
}

impl FakeTranscriber {
    pub fn returning(text: &str) -> Self {
        Self {
            result: Ok(text.to_string()),
            ready: true,
        }
    }

    pub fn failing(error: Error) -> Self {
        Self {
            result: Err(error),
            ready: true,
        }
    }

    pub fn not_ready() -> Self {
        Self {
            result: Ok(String::new()),
            ready: false,
        }
    }
}

impl Transcriber for FakeTranscriber {
    fn transcribe(&self, _samples: &[f32], _language: Option<&str>) -> Result<String> {
        self.result.clone()
    }

    fn is_ready(&self) -> bool {
        self.ready
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
}

impl HotkeyListener for Arc<FakeHotkeys> {
    fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback) -> Result<()> {
        self.as_ref().register(hotkey, on_event)
    }

    fn unregister(&self) -> Result<()> {
        self.as_ref().unregister()
    }
}
