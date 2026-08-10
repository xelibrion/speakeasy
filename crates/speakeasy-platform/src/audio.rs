//! Microphone capture via cpal, resampled to 16 kHz mono for the transcriber.
//!
//! Cross-platform as written: cpal talks to CoreAudio on macOS and
//! ALSA/PulseAudio/PipeWire on Linux.
//!
//! A `cpal::Stream` is `!Send`, so it cannot live in this struct — the port is
//! `Send + Sync`. Instead a dedicated thread owns the stream and takes
//! commands over a channel; `start` and `stop` block on a reply so callers
//! still see ordinary synchronous errors.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::SampleFormat;
use rubato::{FftFixedIn, Resampler};
use speakeasy_core::{AudioSource, Error, Result, TARGET_SAMPLE_RATE};

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

        if rate == 0 || rate == TARGET_SAMPLE_RATE || samples.is_empty() {
            return Ok(samples);
        }
        resample(&samples, rate, TARGET_SAMPLE_RATE)
    }

    fn level(&self) -> f32 {
        self.capture.lock().expect("capture mutex poisoned").level
    }
}

/// Resample mono f32 from `from_rate` to `to_rate`.
///
/// Band-limited rather than naive decimation: dropping samples from 48 kHz
/// aliases speech energy back down into the range whisper cares about.
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Result<Vec<f32>> {
    const CHUNK: usize = 1024;

    let mut resampler = FftFixedIn::<f32>::new(from_rate as usize, to_rate as usize, CHUNK, 2, 1)
        .map_err(|err| {
        Error::AudioUnavailable(format!("could not build the resampler: {err}"))
    })?;

    let mut out = Vec::with_capacity(samples.len() * to_rate as usize / from_rate as usize + CHUNK);
    let mut cursor = 0;

    while cursor + CHUNK <= samples.len() {
        let frames = resampler
            .process(&[&samples[cursor..cursor + CHUNK]], None)
            .map_err(|err| Error::AudioUnavailable(format!("resampling failed: {err}")))?;
        out.extend_from_slice(&frames[0]);
        cursor += CHUNK;
    }

    // Flush the tail, which is shorter than a chunk.
    if cursor < samples.len() {
        let frames = resampler
            .process_partial(Some(&[&samples[cursor..]]), None)
            .map_err(|err| Error::AudioUnavailable(format!("resampling the tail failed: {err}")))?;
        out.extend_from_slice(&frames[0]);
    }

    Ok(out)
}
