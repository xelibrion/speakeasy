//! Microphone capture via cpal, resampled to 16 kHz mono for the transcriber.
//!
//! Cross-platform as written: cpal talks to CoreAudio on macOS and
//! ALSA/PulseAudio/PipeWire on Linux.

use std::sync::{Arc, Mutex};

use speakeasy_core::{AudioSource, Error, Result, TARGET_SAMPLE_RATE};

#[derive(Default)]
struct Capture {
    /// Samples at the device's native rate, mono-downmixed.
    samples: Vec<f32>,
    native_rate: u32,
    // Set when the stream is built; used by the data callback to downmix.
    #[allow(dead_code, reason = "read once cpal capture is implemented")]
    channels: u16,
    /// Rolling RMS for the level indicator.
    level: f32,
}

pub struct CpalAudioSource {
    capture: Arc<Mutex<Capture>>,
    // The active cpal stream, dropped on `stop`. cpal streams are !Send, so a
    // real implementation must own this on a dedicated audio thread and talk to
    // it over a channel rather than storing it here.
    _private: (),
}

impl CpalAudioSource {
    pub fn new() -> Result<Self> {
        Ok(Self {
            capture: Arc::new(Mutex::new(Capture::default())),
            _private: (),
        })
    }
}

impl AudioSource for CpalAudioSource {
    fn start(&self) -> Result<()> {
        // TODO: build the cpal input stream on the audio thread:
        //   1. cpal::default_host().default_input_device()
        //   2. pick a supported f32 input config; remember rate + channel count
        //   3. in the data callback, downmix to mono, append to `capture.samples`,
        //      and update `capture.level` with a rolling RMS
        //   4. map a missing device to Error::AudioUnavailable, and a macOS
        //      TCC denial to Error::MicrophonePermissionDenied
        let mut capture = self.capture.lock().expect("capture mutex poisoned");
        *capture = Capture::default();
        Err(Error::AudioUnavailable(
            "cpal capture not implemented yet".to_string(),
        ))
    }

    fn stop(&self) -> Result<Vec<f32>> {
        let mut capture = self.capture.lock().expect("capture mutex poisoned");
        let samples = std::mem::take(&mut capture.samples);
        capture.level = 0.0;

        if capture.native_rate == 0 || capture.native_rate == TARGET_SAMPLE_RATE {
            return Ok(samples);
        }
        resample(&samples, capture.native_rate, TARGET_SAMPLE_RATE)
    }

    fn level(&self) -> f32 {
        self.capture.lock().expect("capture mutex poisoned").level
    }
}

/// Resample mono f32 from `from_rate` to `to_rate`.
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    // TODO: rubato::SincFixedIn, processing in chunks. Left unimplemented so
    // the shape of the port is reviewable before the DSP lands.
    let _ = (samples, from_rate, to_rate);
    Err(Error::AudioUnavailable(
        "resampling not implemented yet".to_string(),
    ))
}
