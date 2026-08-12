//! [`Transcriber`] backed by whisper.cpp through `whisper-rs`.

mod segmenter;

pub use segmenter::WhisperSegmenter;

use std::path::Path;
use std::sync::Mutex;

use speakeasy_core::{Error, Result, Transcriber, TARGET_SAMPLE_RATE};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct WhisperTranscriber {
    // whisper.cpp state is not safe to use concurrently; one utterance at a
    // time is exactly what push-to-talk needs anyway.
    context: Mutex<Option<WhisperContext>>,
}

impl WhisperTranscriber {
    /// Load weights from disk. Expensive — do it once, off the UI thread.
    pub fn load(model_path: &Path) -> Result<Self> {
        Ok(Self {
            context: Mutex::new(Some(Self::open(model_path)?)),
        })
    }

    /// Build a context without touching any that already exists.
    ///
    /// Split out so [`Transcriber::load`] can construct the replacement before
    /// it takes the lock. See that method for why the order matters.
    fn open(model_path: &Path) -> Result<WhisperContext> {
        // whisper.cpp and ggml print progress straight to stdout otherwise,
        // which is no use in a menu-bar app. This routes them through `log`,
        // which tracing-subscriber picks up.
        static HOOKS: std::sync::Once = std::sync::Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);

        if !model_path.exists() {
            return Err(Error::Transcription(format!(
                "no model at {}",
                model_path.display()
            )));
        }

        let path = model_path.to_str().ok_or_else(|| {
            Error::Transcription(format!("model path is not UTF-8: {}", model_path.display()))
        })?;

        // `default()` sets `use_gpu` from whisper-rs's own `_gpu` feature, which
        // our `metal` feature turns on. Read back rather than assumed: this is
        // the one line that says whether transcription is on the GPU, and a
        // feature that quietly stopped being enabled is exactly the kind of
        // regression that only shows up as "dictation got slow".
        let params = WhisperContextParameters::default();
        let gpu = params.use_gpu;

        let context = WhisperContext::new_with_params(path, params)
            .map_err(|err| Error::Transcription(format!("could not load {path}: {err}")))?;

        // Named "speech model", and the detector's line names itself, because
        // whisper.cpp logs both contexts through the same hooks with nothing to
        // tell them apart — a `no GPU found` from the detector reads as if
        // transcription had fallen off the GPU.
        tracing::info!(
            model = path,
            backend = if gpu { "Metal (GPU)" } else { "CPU" },
            "speech model loaded"
        );
        Ok(context)
    }

    /// An unloaded transcriber, so the app can start without weights and
    /// report [`Error::ModelNotLoaded`] until the user picks a model.
    pub fn unloaded() -> Self {
        Self {
            context: Mutex::new(None),
        }
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<String> {
        let guard = self.context.lock().expect("whisper context mutex poisoned");
        let context = guard.as_ref().ok_or(Error::ModelNotLoaded)?;

        // A state owns the KV cache. Creating one per utterance keeps this
        // struct non-self-referential, and the cost is trivial next to
        // inference itself.
        let mut state = context.create_state().map_err(|err| {
            Error::Transcription(format!("could not create decoder state: {err}"))
        })?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(language);
        params.set_translate(false);
        // whisper.cpp writes to stdout unless every one of these is off.
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let seconds = samples.len() as f32 / TARGET_SAMPLE_RATE as f32;
        let started = std::time::Instant::now();

        state
            .full(params, samples)
            .map_err(|err| Error::Transcription(format!("inference failed: {err}")))?;

        let segments = state.full_n_segments();

        let mut text = String::new();
        for i in 0..segments {
            let segment = state
                .get_segment(i)
                .ok_or_else(|| Error::Transcription(format!("segment {i} vanished")))?;
            // `_lossy` so a stray byte in one segment cannot fail the whole
            // utterance — dropping a character beats dropping the dictation.
            let piece = segment.to_str_lossy().map_err(|err| {
                Error::Transcription(format!("could not read segment {i}: {err}"))
            })?;
            text.push_str(&piece);
        }

        tracing::debug!(
            audio_s = seconds,
            elapsed_s = started.elapsed().as_secs_f32(),
            segments,
            "transcribed"
        );
        Ok(text)
    }

    fn is_ready(&self) -> bool {
        self.context
            .lock()
            .expect("whisper context mutex poisoned")
            .is_some()
    }

    fn load(&self, path: &Path) -> Result<()> {
        // Built *before* the lock is taken, and deliberately so. On failure we
        // return without ever touching `self`, so a swap that goes wrong costs
        // the user nothing — which is what lets the download flow verify and
        // load a new model while the old one keeps serving dictation. It also
        // means a slow load does not hold the mutex against an in-flight
        // transcription.
        let context = Self::open(path)?;
        *self.context.lock().expect("whisper context mutex poisoned") = Some(context);
        Ok(())
    }
}
