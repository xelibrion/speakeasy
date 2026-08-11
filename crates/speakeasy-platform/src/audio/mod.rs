//! Microphone capture via cpal, resampled to 16 kHz mono for the transcriber.
//!
//! Cross-platform as written: cpal talks to CoreAudio on macOS and
//! ALSA/PulseAudio/PipeWire on Linux.
//!
//! A `cpal::Stream` is `!Send`, so it cannot live in this struct — the port is
//! `Send + Sync`. Instead a dedicated thread owns the stream and takes
//! commands over a channel; `start` and `stop` block on a reply so callers
//! still see ordinary synchronous errors.
//!
//! This module is the device: opening streams, downmixing, and handing samples
//! on. What those samples *mean* to the UI lives in [`spectrum`], and rate
//! conversion in [`resample`].

mod resample;
mod spectrum;

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use speakeasy_core::{AudioSource, Error, Result, TARGET_SAMPLE_RATE};

use resample::resample;
use spectrum::{Analyzer, WINDOW};

#[derive(Default)]
struct Capture {
    /// Samples at the device's native rate, already downmixed to mono.
    samples: Vec<f32>,
    native_rate: u32,
    /// Rolling RMS for the level indicator, 0.0..=1.0.
    level: f32,
}

enum Command {
    Start(SyncSender<Result<()>>),
    Stop(SyncSender<()>),
}

pub struct CpalAudioSource {
    capture: Arc<Mutex<Capture>>,
    commands: SyncSender<Command>,
    /// Its own lock, so analysing a window never blocks the audio callback.
    analyzer: Mutex<Analyzer>,
}

impl CpalAudioSource {
    pub fn new() -> Result<Self> {
        let capture = Arc::new(Mutex::new(Capture::default()));
        let (tx, rx) = sync_channel(4);

        let thread_capture = Arc::clone(&capture);
        std::thread::Builder::new()
            .name("speakeasy-audio".into())
            .spawn(move || audio_thread(rx, thread_capture))
            .map_err(|err| {
                Error::AudioUnavailable(format!("could not start the audio thread: {err}"))
            })?;

        Ok(Self {
            capture,
            commands: tx,
            analyzer: Mutex::new(Analyzer::new()),
        })
    }
}

/// Owns the cpal stream for its whole life. Exits when the sender is dropped.
fn audio_thread(commands: Receiver<Command>, capture: Arc<Mutex<Capture>>) {
    let mut stream: Option<cpal::Stream> = None;

    while let Ok(command) = commands.recv() {
        match command {
            Command::Start(reply) => {
                let result = build_stream(&capture).and_then(|s| {
                    s.play().map_err(|err| {
                        Error::AudioUnavailable(format!("could not start: {err}"))
                    })?;
                    Ok(s)
                });
                match result {
                    Ok(s) => {
                        // Holding the stream is what keeps capture running;
                        // replacing it drops any previous one.
                        if stream.replace(s).is_some() {
                            tracing::warn!("restarted capture while a stream was already open");
                        }
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        drop(stream.take());
                        let _ = reply.send(Err(err));
                    }
                }
            }
            Command::Stop(reply) => {
                // Dropping the stream stops capture and guarantees the data
                // callback is no longer running before we read the buffer.
                drop(stream.take());
                let _ = reply.send(());
            }
        }
    }
}

fn build_stream(capture: &Arc<Mutex<Capture>>) -> Result<cpal::Stream> {
    let device = cpal::default_host()
        .default_input_device()
        .ok_or_else(|| Error::AudioUnavailable("no input device".to_string()))?;

    let config = device.default_input_config().map_err(|err| {
        // macOS reports a denied microphone as a config failure rather than
        // anything more specific.
        Error::AudioUnavailable(format!("no usable input config: {err}"))
    })?;

    let sample_format = config.sample_format();
    let channels = config.config().channels as usize;
    let rate = config.config().sample_rate.0;

    {
        let mut guard = capture.lock().expect("capture mutex poisoned");
        *guard = Capture {
            samples: Vec::new(),
            native_rate: rate,
            level: 0.0,
        };
    }

    let sink = Arc::clone(capture);
    let on_error = |err| tracing::warn!(%err, "audio stream error");

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &config.config(),
            move |data: &[f32], _| accumulate(&sink, data, channels),
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &config.config(),
            move |data: &[i16], _| {
                let converted: Vec<f32> =
                    data.iter().map(|s| *s as f32 / i16::MAX as f32).collect();
                accumulate(&sink, &converted, channels)
            },
            on_error,
            None,
        ),
        other => {
            return Err(Error::AudioUnavailable(format!(
                "unsupported sample format {other:?}"
            )))
        }
    };

    stream.map_err(|err| Error::AudioUnavailable(format!("could not open the microphone: {err}")))
}

/// Downmix to mono, append, and update the level meter.
fn accumulate(capture: &Arc<Mutex<Capture>>, data: &[f32], channels: usize) {
    let mut guard = capture.lock().expect("capture mutex poisoned");

    let mut sum_squares = 0.0;
    for frame in data.chunks(channels.max(1)) {
        let mono = frame.iter().sum::<f32>() / frame.len() as f32;
        sum_squares += mono * mono;
        guard.samples.push(mono);
    }

    let frames = data.len() / channels.max(1);
    if frames > 0 {
        let rms = (sum_squares / frames as f32).sqrt();
        // Smooth so the indicator does not strobe at callback rate.
        guard.level = guard.level * 0.7 + rms.clamp(0.0, 1.0) * 0.3;
    }
}

impl AudioSource for CpalAudioSource {
    fn start(&self) -> Result<()> {
        let (tx, rx) = sync_channel(1);
        self.commands
            .send(Command::Start(tx))
            .map_err(|_| Error::AudioUnavailable("the audio thread has stopped".to_string()))?;
        rx.recv()
            .map_err(|_| Error::AudioUnavailable("the audio thread has stopped".to_string()))?
    }

    fn stop(&self) -> Result<Vec<f32>> {
        let (tx, rx) = sync_channel(1);
        self.commands
            .send(Command::Stop(tx))
            .map_err(|_| Error::AudioUnavailable("the audio thread has stopped".to_string()))?;
        rx.recv()
            .map_err(|_| Error::AudioUnavailable("the audio thread has stopped".to_string()))?;

        let mut guard = self.capture.lock().expect("capture mutex poisoned");
        let samples = std::mem::take(&mut guard.samples);
        let rate = guard.native_rate;
        guard.level = 0.0;
        drop(guard);

        self.analyzer
            .lock()
            .expect("analyzer mutex poisoned")
            .reset();

        if rate == 0 || rate == TARGET_SAMPLE_RATE || samples.is_empty() {
            return Ok(samples);
        }
        resample(&samples, rate, TARGET_SAMPLE_RATE)
    }

    fn snapshot(&self) -> Result<Vec<f32>> {
        // Copy out under the lock and resample after releasing it: the audio
        // callback takes this same lock on every buffer and must never wait on
        // a resample. Same shape as `spectrum` below, for the same reason.
        let (samples, rate) = {
            let guard = self.capture.lock().expect("capture mutex poisoned");
            (guard.samples.clone(), guard.native_rate)
        };

        if rate == 0 || rate == TARGET_SAMPLE_RATE || samples.is_empty() {
            return Ok(samples);
        }
        resample(&samples, rate, TARGET_SAMPLE_RATE)
    }

    fn level(&self) -> f32 {
        self.capture.lock().expect("capture mutex poisoned").level
    }

    fn spectrum(&self) -> Vec<f32> {
        // Copy the tail out and release the capture lock before transforming
        // it: the audio callback takes this same lock on every buffer, and it
        // must never wait on an FFT.
        let (tail, rate) = {
            let guard = self.capture.lock().expect("capture mutex poisoned");
            let start = guard.samples.len().saturating_sub(WINDOW);
            (guard.samples[start..].to_vec(), guard.native_rate)
        };

        self.analyzer
            .lock()
            .expect("analyzer mutex poisoned")
            .analyze(&tail, rate)
    }
}
