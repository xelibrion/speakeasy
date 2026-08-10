//! Checks the transcriber against real weights and real speech.
//!
//! Ignored by default: it needs a model on disk (~141 MB) and a 16 kHz mono
//! wav, so it cannot run in a plain `cargo test`. Run it with:
//!
//! ```sh
//! SPEAKEASY_MODEL=models/ggml-base.en.bin SPEAKEASY_WAV=/tmp/speech.wav \
//!   cargo test -p speakeasy-whisper -- --ignored --nocapture
//! ```
//!
//! Generate a sample without a microphone:
//!
//! ```sh
//! say -o /tmp/speech.aiff "Testing speakeasy dictation, one two three."
//! afconvert -f WAVE -d LEI16@16000 -c 1 /tmp/speech.aiff /tmp/speech.wav
//! ```

use std::path::Path;

use speakeasy_core::{Transcriber, TARGET_SAMPLE_RATE};
use speakeasy_whisper::WhisperTranscriber;

fn read_wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("wav should open");
    let spec = reader.spec();
    assert_eq!(
        spec.sample_rate, TARGET_SAMPLE_RATE,
        "the transcriber expects {TARGET_SAMPLE_RATE} Hz"
    );
    assert_eq!(spec.channels, 1, "the transcriber expects mono");

    reader
        .samples::<i16>()
        .map(|s| s.expect("sample should decode") as f32 / i16::MAX as f32)
        .collect()
}

#[test]
#[ignore = "needs SPEAKEASY_MODEL and SPEAKEASY_WAV"]
fn transcribes_real_speech() {
    let model = std::env::var("SPEAKEASY_MODEL").expect("set SPEAKEASY_MODEL");
    let wav = std::env::var("SPEAKEASY_WAV").expect("set SPEAKEASY_WAV");

    let transcriber = WhisperTranscriber::load(Path::new(&model)).expect("model should load");
    assert!(transcriber.is_ready());

    let samples = read_wav(Path::new(&wav));
    let text = transcriber
        .transcribe(&samples, Some("en"))
        .expect("transcription should succeed");

    println!("transcript: {text:?}");

    // Assert on content words rather than exact output. Punctuation, casing
    // and number formatting all vary between models and versions — base.en
    // renders "one two three" as "123" — so pinning them makes this brittle.
    let lower = text.to_lowercase();
    for word in ["testing", "dictation"] {
        assert!(lower.contains(word), "expected {word:?} in {text:?}");
    }
}

/// Switching models without a restart is the whole point of `Transcriber::load`.
#[test]
#[ignore = "needs SPEAKEASY_MODEL"]
fn load_swaps_the_model_in_place() {
    let model = std::env::var("SPEAKEASY_MODEL").expect("set SPEAKEASY_MODEL");

    let transcriber = WhisperTranscriber::unloaded();
    assert!(!transcriber.is_ready(), "starts with no weights");

    Transcriber::load(&transcriber, Path::new(&model)).expect("load should succeed");

    assert!(transcriber.is_ready(), "weights are live without a restart");
}

/// The reason the context is built before the lock is taken: a model that will
/// not load must not cost the user the one that already works.
#[test]
#[ignore = "needs SPEAKEASY_MODEL"]
fn a_failed_load_leaves_the_previous_model_working() {
    let model = std::env::var("SPEAKEASY_MODEL").expect("set SPEAKEASY_MODEL");

    let transcriber = WhisperTranscriber::load(Path::new(&model)).expect("model should load");
    let err = Transcriber::load(&transcriber, Path::new("/nonexistent/ggml-nope.bin"))
        .expect_err("a missing file should fail to load");

    assert!(transcriber.is_ready(), "the working model survived: {err}");
}
