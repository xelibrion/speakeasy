//! [`Transcriber`] backed by whisper.cpp through `whisper-rs`.

use std::path::Path;
use std::sync::Mutex;

use speakeasy_core::{Error, Result, Transcriber};

pub struct WhisperTranscriber {
    // whisper.cpp state is not safe to use concurrently; one utterance at a
    // time is exactly what push-to-talk needs anyway.
    state: Mutex<Option<()>>,
}

impl WhisperTranscriber {
    /// Load weights from disk. Expensive — do it once, off the UI thread.
    pub fn load(model_path: &Path) -> Result<Self> {
        // TODO: WhisperContext::new_with_params, then create_state().
        let _ = model_path;
        Err(Error::Transcription(
            "whisper model loading not implemented yet".to_string(),
        ))
    }

    /// An unloaded transcriber, so the app can start without weights and
    /// report [`Error::ModelNotLoaded`] until the user picks a model.
    pub fn unloaded() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<String> {
        let state = self.state.lock().expect("whisper state mutex poisoned");
        if state.is_none() {
            return Err(Error::ModelNotLoaded);
        }

        // TODO: FullParams with SamplingStrategy::Greedy, set_language(language),
        // set_translate(false), set_print_* all false; run full() and
        // concatenate the segments.
        let _ = (samples, language);
        Err(Error::Transcription(
            "whisper inference not implemented yet".to_string(),
        ))
    }

    fn is_ready(&self) -> bool {
        self.state
            .lock()
            .expect("whisper state mutex poisoned")
            .is_some()
    }
}
