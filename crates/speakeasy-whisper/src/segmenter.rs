//! [`SpeechSegmenter`] backed by whisper.cpp's Silero VAD.
//!
//! The weights are compiled in rather than downloaded. Previews are not
//! optional enough to justify a first-run network failure, and at 865 KB the
//! model is small enough that bundling it costs nothing worth measuring.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use speakeasy_core::{Error, Result, SpeechSegment, SpeechSegmenter};
use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

/// Silero v5.1.2, MIT licensed, from `ggml-org/whisper-vad`.
const WEIGHTS: &[u8] = include_bytes!("../vendor/ggml-silero-v5.1.2.bin");

const FILENAME: &str = "ggml-silero-v5.1.2.bin";

pub struct WhisperSegmenter {
    // Its own context, so voice detection never queues behind a transcription.
    context: Mutex<WhisperVadContext>,
}

impl WhisperSegmenter {
    /// Materialise the weights into `models_dir` if absent, and open them.
    ///
    /// [`WhisperVadContext::new`] takes a path rather than bytes, which is the
    /// only reason this touches the filesystem at all.
    pub fn new(models_dir: &Path) -> Result<Self> {
        let path = Self::materialize(models_dir)?;
        let mut params = WhisperVadContextParams::new();
        // On the CPU deliberately, and do not "optimise" this to the GPU: the
        // Silero graph contains an op the Metal backend cannot run, and
        // whisper.cpp aborts the process rather than falling back —
        // "pre-allocated tensor (leaf_0) in a buffer (Metal) that cannot run
        // the operation". The model is 865 KB, so the CPU is the right home for
        // it regardless, and it leaves the GPU free for transcription.
        params.set_use_gpu(false);

        let opened = path.to_str().ok_or_else(|| {
            Error::Other(format!("vad model path is not UTF-8: {}", path.display()))
        })?;
        let context = WhisperVadContext::new(opened, params)
            .map_err(|err| Error::Other(format!("could not load the vad model: {err}")))?;

        // Says "by design" because whisper.cpp has just printed
        // `whisper_backend_init_gpu: no GPU found` on our behalf, which reads
        // like a failure and is not one. It is also the wrong thing to worry
        // about: the line to check for transcription is "speech model loaded".
        tracing::info!(
            model = opened,
            backend = "CPU (by design: Silero aborts on Metal)",
            "voice detector loaded"
        );

        Ok(Self {
            context: Mutex::new(context),
        })
    }

    /// Write the bundled weights out if they are not already there.
    ///
    /// Rewritten whenever the size differs, so a truncated write from an
    /// interrupted first run repairs itself rather than failing forever.
    fn materialize(models_dir: &Path) -> Result<PathBuf> {
        let path = models_dir.join(FILENAME);
        let present = std::fs::metadata(&path)
            .map(|meta| meta.len() == WEIGHTS.len() as u64)
            .unwrap_or(false);
        if present {
            return Ok(path);
        }

        std::fs::create_dir_all(models_dir)
            .map_err(|err| Error::Other(format!("could not create the models dir: {err}")))?;
        std::fs::write(&path, WEIGHTS)
            .map_err(|err| Error::Other(format!("could not write the vad model: {err}")))?;
        tracing::info!(path = %path.display(), "wrote the bundled vad model");
        Ok(path)
    }
}

impl SpeechSegmenter for WhisperSegmenter {
    fn segments(&self, samples: &[f32]) -> Result<Vec<SpeechSegment>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }

        let mut context = self.context.lock().expect("vad context mutex poisoned");
        let segments = context
            .segments_from_samples(WhisperVadParams::new(), samples)
            .map_err(|err| Error::Other(format!("voice detection failed: {err}")))?;

        let mut spans = Vec::new();
        for index in 0..segments.num_segments() {
            let Some(segment) = segments.get_segment(index) else {
                continue;
            };
            // whisper.cpp reports these in **centiseconds**; the port is in
            // seconds. Skipping this conversion does not fail loudly — spans
            // come back 100x too long, no utterance ever looks finished, and
            // the preview simply never commits.
            spans.push(SpeechSegment {
                start: segment.start / 100.0,
                end: segment.end / 100.0,
            });
        }
        Ok(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence must report no speech. This is the gate that stops whisper
    /// hallucinating words into a pause.
    #[test]
    fn silence_contains_no_speech() {
        let dir = std::env::temp_dir().join("speakeasy-vad-test");
        let segmenter = WhisperSegmenter::new(&dir).expect("vad model loads");

        let spans = segmenter
            .segments(&vec![0.0; 16_000 * 3])
            .expect("detection runs");

        assert!(spans.is_empty(), "silence reported as speech: {spans:?}");
    }

    #[test]
    fn the_bundled_model_is_written_once() {
        let dir = std::env::temp_dir().join("speakeasy-vad-materialize");
        let _ = std::fs::remove_dir_all(&dir);

        let first = WhisperSegmenter::materialize(&dir).expect("writes");
        let second = WhisperSegmenter::materialize(&dir).expect("reuses");

        assert_eq!(first, second);
        assert_eq!(
            std::fs::metadata(&first).expect("exists").len(),
            WEIGHTS.len() as u64
        );
    }

    /// Spans must lie inside the audio they came from.
    ///
    /// This is the unit check: whisper.cpp reports centiseconds, so a missing
    /// conversion puts every span 100x past the end of the buffer. Opt-in like
    /// `tests/transcribe.rs`, because it needs real speech:
    ///
    /// ```text
    /// SPEAKEASY_WAV=/tmp/speech.wav cargo test -p speakeasy-whisper -- --ignored
    /// ```
    #[test]
    #[ignore = "needs a wav of real speech; set SPEAKEASY_WAV"]
    fn spans_lie_within_the_audio() {
        let wav = std::env::var("SPEAKEASY_WAV").expect("set SPEAKEASY_WAV");
        let mut reader = hound::WavReader::open(&wav).expect("wav should open");
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|sample| sample.expect("sample") as f32 / i16::MAX as f32)
            .collect();
        let seconds = samples.len() as f32 / 16_000.0;

        let dir = std::env::temp_dir().join("speakeasy-vad-bounds");
        let segmenter = WhisperSegmenter::new(&dir).expect("vad model loads");
        let spans = segmenter.segments(&samples).expect("detection runs");

        assert!(!spans.is_empty(), "no speech found in a wav of speech");
        for span in &spans {
            assert!(
                span.end <= seconds + 0.1,
                "span {span:?} runs past {seconds}s of audio — check the \
                 centisecond conversion"
            );
        }
    }
}
