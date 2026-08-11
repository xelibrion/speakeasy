//! The push-to-talk state machine.
//!
//! Idle → Recording → Transcribing → Injecting → Idle, with any step able to
//! fall into `Failed` and recover to `Idle` on the next press.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::path::PathBuf;

use crate::config::{ActivationMode, Config, Hotkey, ModelChoice, WidgetPosition};
use crate::error::{Error, Result};
use crate::event::{Event, ModelStatus, State};
use crate::models::catalog::{self, ModelId, ModelInfo, CATALOG};
use crate::models::ModelManager;
use crate::ports::{
    AudioSource, HotkeyEvent, ModelFetcher, SpeechSegmenter, TextSink, Transcriber,
};
use crate::preview::{decide, Tick};

/// How often a preview decode runs while recording.
///
/// A decode costs ~0.31s on the measured hardware, so this is roughly a 20%
/// duty cycle. A constant rather than a setting until it has been lived with.
const PREVIEW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

/// What a hotkey event means, given the activation mode and the current state.
///
/// Resolved separately from being carried out because [`Engine::finish_recording`]
/// blocks and hotkey callbacks must not: the shell asks what to do on the
/// listener's thread, then moves the slow half elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Begin,
    Finish,
    Ignore,
}

/// Owns application state and drives the ports. Cheap to clone — clones share
/// one engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    audio: Box<dyn AudioSource>,
    /// `Arc` rather than `Box` so [`ModelManager`] can hold the same instance
    /// and swap its weights without going through the engine.
    transcriber: Arc<dyn Transcriber>,
    sink: Box<dyn TextSink>,
    models: ModelManager,
    config: Mutex<Config>,
    /// Where to persist config changes. `None` keeps the engine in memory only,
    /// which is what the tests want.
    config_path: Mutex<Option<PathBuf>>,
    state: Mutex<State>,
    started_at: Mutex<Option<Instant>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
    /// `None` disables previews entirely, which is what a shell with no
    /// detector — or a headless test — gets by doing nothing.
    segmenter: Mutex<Option<Arc<dyn SpeechSegmenter>>>,
    preview: Mutex<Preview>,
}

/// Preview progress within one recording.
#[derive(Default)]
struct Preview {
    /// Text from utterances that have finished. Never revised.
    committed: String,
    /// How much of the 16 kHz stream `committed` accounts for.
    committed_samples: usize,
}

impl Engine {
    pub fn new(
        audio: Box<dyn AudioSource>,
        transcriber: Arc<dyn Transcriber>,
        sink: Box<dyn TextSink>,
        fetcher: Box<dyn ModelFetcher>,
        models_dir: PathBuf,
        config: Config,
    ) -> Self {
        let inner = Arc::new(Inner {
            audio,
            transcriber: transcriber.clone(),
            sink,
            models: ModelManager::new(fetcher, transcriber, models_dir),
            config: Mutex::new(config),
            config_path: Mutex::new(None),
            state: Mutex::new(State::Idle),
            started_at: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            segmenter: Mutex::new(None),
            preview: Mutex::new(Preview::default()),
        });

        // The manager's status changes become ordinary events, so a shell
        // learns about a download the same way it learns about a state change.
        //
        // `Weak` because this closure lives *inside* the `Inner` it reports to;
        // an `Arc` here would be a cycle, and the engine would never drop.
        let weak = Arc::downgrade(&inner);
        inner.models.on_status(Box::new(move |status| {
            if let Some(inner) = weak.upgrade() {
                inner.emit(Event::ModelStatusChanged { status });
            }
        }));

        Self { inner }
    }

    /// Observe engine progress. Every subscriber receives every event; a
    /// subscriber whose receiver has been dropped is pruned on the next emit.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = channel();
        self.inner
            .subscribers
            .lock()
            .expect("subscribers mutex poisoned")
            .push(tx);
        rx
    }

    pub fn state(&self) -> State {
        self.inner
            .state
            .lock()
            .expect("state mutex poisoned")
            .clone()
    }

    pub fn config(&self) -> Config {
        self.inner
            .config
            .lock()
            .expect("config mutex poisoned")
            .clone()
    }

    /// Persist every subsequent config change to `path`. Chained onto `new` so
    /// the constructor stays the same shape for tests and headless shells.
    pub fn persisting_to(self, path: PathBuf) -> Self {
        *self
            .inner
            .config_path
            .lock()
            .expect("config path mutex poisoned") = Some(path);
        self
    }

    /// Enable previews, using `segmenter` to find where utterances end.
    ///
    /// Chained like [`Engine::persisting_to`] rather than added to `new`, which
    /// already takes six arguments — and so that a shell without a detector
    /// keeps working by changing nothing.
    pub fn with_segmenter(self, segmenter: Arc<dyn SpeechSegmenter>) -> Self {
        *self
            .inner
            .segmenter
            .lock()
            .expect("segmenter mutex poisoned") = Some(segmenter);
        self
    }

    pub fn set_config(&self, config: Config) {
        *self.inner.config.lock().expect("config mutex poisoned") = config;
        self.persist();
    }

    /// Remember where the user dragged the status widget.
    ///
    /// Its own method rather than a `set_config` round-trip: the shell learns
    /// about a move from a window event, and reading, mutating and writing back
    /// the whole config from there would race with the settings window.
    pub fn set_widget_position(&self, position: WidgetPosition) {
        {
            let mut config = self.inner.config.lock().expect("config mutex poisoned");
            if config.widget_position == Some(position) {
                return; // our own set_position echoing back; nothing to write
            }
            config.widget_position = Some(position);
        }
        self.persist();
    }

    /// Rebind the dictation key.
    ///
    /// Its own method rather than a `set_config` round-trip, for the same
    /// reason as [`Engine::set_widget_position`]: the settings window would
    /// otherwise read, mutate and write back a whole config that a capture in
    /// flight is also changing.
    ///
    /// Persisting is all this does. Actually binding the key belongs to
    /// whoever owns the listener, which is the shell — it re-registers when it
    /// sees the event.
    pub fn set_hotkey(&self, hotkey: Hotkey) {
        {
            let mut config = self.inner.config.lock().expect("config mutex poisoned");
            if config.hotkey == hotkey {
                return;
            }
            config.hotkey = hotkey.clone();
        }
        self.persist();
        // After the lock is released: a subscriber that calls back in while we
        // still held it would deadlock.
        self.emit(Event::HotkeyChanged { hotkey });
    }

    /// Best-effort: a config that cannot be written is worth a warning, not a
    /// failed dictation.
    fn persist(&self) {
        let path = self
            .inner
            .config_path
            .lock()
            .expect("config path mutex poisoned")
            .clone();
        let Some(path) = path else { return };

        if let Err(err) = self.config().save(&path) {
            tracing::warn!(%err, path = %path.display(), "could not save config");
        }
    }

    /// Interpret a hotkey edge. Cheap and non-blocking; the caller performs the
    /// returned action.
    ///
    /// In [`ActivationMode::Toggle`] the release edge is dropped entirely and
    /// the press edge alternates, which is why this needs the state rather than
    /// just the event.
    pub fn hotkey_action(&self, event: HotkeyEvent) -> HotkeyAction {
        match self.config().activation {
            ActivationMode::PushToTalk => match event {
                HotkeyEvent::Pressed => HotkeyAction::Begin,
                HotkeyEvent::Released => HotkeyAction::Finish,
            },
            ActivationMode::Toggle => match event {
                HotkeyEvent::Released => HotkeyAction::Ignore,
                HotkeyEvent::Pressed => match self.state() {
                    State::Recording => HotkeyAction::Finish,
                    // Mid-flight work owns the key; a stray tap must not queue
                    // a second recording behind it.
                    State::Transcribing | State::Injecting => HotkeyAction::Ignore,
                    State::Idle | State::Failed { .. } => HotkeyAction::Begin,
                },
            },
        }
    }

    /// Hotkey pressed. No-op unless idle, so key repeat cannot restart a
    /// recording mid-flight.
    pub fn begin_recording(&self) -> Result<()> {
        {
            let state = self.inner.state.lock().expect("state mutex poisoned");
            if matches!(
                *state,
                State::Recording | State::Transcribing | State::Injecting
            ) {
                return Ok(());
            }
        }

        if !self.inner.transcriber.is_ready() {
            let err = Error::ModelNotLoaded;
            self.fail(err.clone());
            return Err(err);
        }

        if let Err(err) = self.inner.audio.start() {
            self.fail(err.clone());
            return Err(err);
        }

        *self
            .inner
            .started_at
            .lock()
            .expect("started_at mutex poisoned") = Some(Instant::now());
        *self.inner.preview.lock().expect("preview mutex poisoned") = Preview::default();
        self.transition(State::Recording);
        self.spawn_preview_loop();
        Ok(())
    }

    /// Tick previews until the recording ends.
    ///
    /// Exits on any state other than `Recording`, so a release, a cancel and a
    /// failure all stop it without needing to be told.
    fn spawn_preview_loop(&self) {
        let enabled = self
            .inner
            .segmenter
            .lock()
            .expect("segmenter mutex poisoned")
            .is_some();
        if !enabled || !self.config().transcript_preview {
            return;
        }

        let engine = self.clone();
        std::thread::spawn(move || {
            while matches!(engine.state(), State::Recording) {
                std::thread::sleep(PREVIEW_INTERVAL);
                engine.preview_tick();
            }
        });
    }

    /// One preview pass: decode what has been said since the last commit and
    /// announce it.
    ///
    /// Best-effort throughout. Every failure here is logged and skipped rather
    /// than transitioning to `Failed` — a preview must never cost the user a
    /// dictation.
    fn preview_tick(&self) {
        if !matches!(self.state(), State::Recording) {
            return;
        }
        if !self.config().transcript_preview {
            return;
        }
        let Some(segmenter) = self
            .inner
            .segmenter
            .lock()
            .expect("segmenter mutex poisoned")
            .clone()
        else {
            return;
        };

        let all = match self.inner.audio.snapshot() {
            Ok(samples) => samples,
            Err(err) => {
                tracing::warn!(%err, "could not read the recording for a preview");
                return;
            }
        };

        let start = self
            .inner
            .preview
            .lock()
            .expect("preview mutex poisoned")
            .committed_samples;
        if all.len() <= start {
            return;
        }
        let slice = &all[start..];
        let slice_seconds = slice.len() as f32 / crate::TARGET_SAMPLE_RATE as f32;

        let spans = match segmenter.segments(slice) {
            Ok(spans) => spans,
            Err(err) => {
                tracing::warn!(%err, "could not find speech for a preview");
                return;
            }
        };

        let (audio, commit_through) = match decide(slice_seconds, &spans) {
            Tick::Skip => return,
            Tick::Live => (slice, None),
            Tick::Commit { through_seconds } => {
                let end = ((through_seconds * crate::TARGET_SAMPLE_RATE as f32) as usize)
                    .min(slice.len());
                (&slice[..end], Some(end))
            }
        };

        let language = self.config().language;
        let text = match self
            .inner
            .transcriber
            .transcribe(audio, language.as_deref())
        {
            Ok(text) => text.trim().to_string(),
            Err(err) => {
                tracing::warn!(%err, "could not decode a preview");
                return;
            }
        };

        // Built under the lock, emitted after releasing it: a subscriber that
        // calls back in while we still held it would deadlock.
        let display = {
            let mut preview = self.inner.preview.lock().expect("preview mutex poisoned");
            match commit_through {
                Some(end) => {
                    if !text.is_empty() {
                        if !preview.committed.is_empty() {
                            preview.committed.push(' ');
                        }
                        preview.committed.push_str(&text);
                    }
                    preview.committed_samples += end;
                    preview.committed.clone()
                }
                None if preview.committed.is_empty() => text,
                None => format!("{} {}", preview.committed, text),
            }
        };

        if display.is_empty() {
            return;
        }
        self.emit(Event::TranscriptPreview { text: display });
    }

    /// Hotkey released: stop capture, transcribe, inject.
    ///
    /// Blocking — transcription of a few seconds of audio takes a noticeable
    /// moment. Call it off your UI thread; see the Tauri shell's `bridge` for
    /// how that is wired.
    pub fn finish_recording(&self) -> Result<()> {
        {
            let state = self.inner.state.lock().expect("state mutex poisoned");
            if !matches!(*state, State::Recording) {
                return Ok(());
            }
        }

        let elapsed = self
            .inner
            .started_at
            .lock()
            .expect("started_at mutex poisoned")
            .take()
            .map(|start| start.elapsed().as_millis() as u64)
            .unwrap_or(0);

        let samples = match self.inner.audio.stop() {
            Ok(samples) => samples,
            Err(err) => {
                self.fail(err.clone());
                return Err(err);
            }
        };

        // An accidental tap: discard rather than injecting noise.
        if elapsed < self.config().min_recording_ms {
            self.transition(State::Idle);
            return Ok(());
        }

        self.transition(State::Transcribing);
        let language = self.config().language;
        let text = match self
            .inner
            .transcriber
            .transcribe(&samples, language.as_deref())
        {
            Ok(text) => text.trim().to_string(),
            Err(err) => {
                self.fail(err.clone());
                return Err(err);
            }
        };

        if text.is_empty() {
            self.transition(State::Idle);
            return Ok(());
        }
        self.emit(Event::Transcript { text: text.clone() });

        self.transition(State::Injecting);
        match self.inner.sink.inject(&text) {
            Ok(chars) => {
                self.emit(Event::Injected { chars });
                self.transition(State::Idle);
                Ok(())
            }
            Err(err) => {
                self.fail(err.clone());
                Err(err)
            }
        }
    }

    /// Abandon an in-flight recording without transcribing it.
    pub fn cancel(&self) {
        let _ = self.inner.audio.stop();
        *self
            .inner
            .started_at
            .lock()
            .expect("started_at mutex poisoned") = None;
        self.transition(State::Idle);
    }

    /// Every model the user could choose to download.
    pub fn catalog(&self) -> &'static [ModelInfo] {
        CATALOG.as_slice()
    }

    /// Catalog models already on disk.
    pub fn installed_models(&self) -> Vec<ModelId> {
        self.inner.models.installed()
    }

    pub fn model_status(&self) -> ModelStatus {
        self.inner.models.status()
    }

    /// Start downloading `id` and return immediately.
    ///
    /// Fire-and-forget, unlike [`Engine::finish_recording`]: there is no return
    /// value worth waiting for, because every outcome arrives as
    /// [`Event::ModelStatusChanged`]. That is also what lets a shell's IPC
    /// command stay a one-line delegation.
    ///
    /// Config is written *before* the download starts, because it records what
    /// the user wants rather than what succeeded — which is what makes an
    /// interrupted download resume on the next launch.
    pub fn install_model(&self, id: ModelId) {
        let previous = self.config().model;
        self.set_model_choice(ModelChoice::Catalog { id: id.clone() });

        let engine = self.clone();
        std::thread::spawn(move || match engine.inner.models.install(&id) {
            Ok(Some(_)) => {}
            // Cancelled: the user said never mind, so the desired model goes
            // back to what it was. Without this the next launch would resume
            // the transfer they just abandoned.
            Ok(None) => engine.set_model_choice(previous),
            // Failed: config deliberately keeps naming the wanted model, so a
            // retry is one click and the next launch tries again. But dictation
            // should not be dead in the meantime.
            Err(err) => {
                tracing::warn!(%err, %id, "could not install the model");
                engine.fall_back_to_any_installed();
            }
        });
    }

    /// Switch to a model that needs no download: a local file, or a catalog
    /// entry already on disk.
    pub fn select_model(&self, choice: ModelChoice) {
        self.set_model_choice(choice.clone());
        let Some(path) = self.inner.models.path_for(&choice) else {
            return;
        };
        if let Err(err) = self.inner.models.activate(&path, &label_for(&choice)) {
            tracing::warn!(%err, "could not switch models");
        }
    }

    pub fn cancel_model_download(&self) {
        self.inner.models.cancel();
    }

    pub fn remove_model(&self, id: &ModelId) -> Result<()> {
        self.inner.models.remove(id)
    }

    /// Reconcile desired state ([`Config::model`]) against what is on disk.
    ///
    /// Called once at startup. First run is not a special case: a fresh config
    /// wants the default catalog model, nothing is installed, so this downloads
    /// it — the same path a mid-session switch takes.
    pub fn ensure_model(&self) {
        let choice = self.config().model;
        let path = self.inner.models.path_for(&choice);

        if let Some(path) = &path {
            if path.is_file() {
                if self
                    .inner
                    .models
                    .activate(path, &label_for(&choice))
                    .is_ok()
                {
                    return;
                }
                // `activate` has already reported why.
                self.fall_back_to_any_installed();
                return;
            }
        }

        match choice {
            // Not on disk, and we know where to get it.
            ModelChoice::Catalog { id } => self.install_model(id),
            // A missing local file is nothing a download can fix. Let `activate`
            // report why, then keep dictation alive if anything else is here.
            ModelChoice::Local { .. } => {
                if let Some(path) = &path {
                    let _ = self.inner.models.activate(path, &label_for(&choice));
                }
                self.fall_back_to_any_installed();
            }
        }
    }

    /// Load whatever is installed so the hotkey still works.
    ///
    /// Transcribing on a model the user did not pick, while the status says so,
    /// beats not transcribing at all. Deliberately loads *without* touching the
    /// status: the user needs to keep seeing why they are not on the model they
    /// chose.
    fn fall_back_to_any_installed(&self) {
        let Some(id) = self.inner.models.installed().into_iter().next() else {
            return;
        };
        let choice = ModelChoice::Catalog { id: id.clone() };
        let Some(path) = self.inner.models.path_for(&choice) else {
            return;
        };

        match self.inner.models.load_only(&path) {
            Ok(()) => tracing::warn!(%id, "falling back to an installed model"),
            Err(err) => tracing::warn!(%err, %id, "the fallback model would not load either"),
        }
    }

    fn set_model_choice(&self, choice: ModelChoice) {
        {
            let mut config = self.inner.config.lock().expect("config mutex poisoned");
            if config.model == choice {
                return;
            }
            config.model = choice;
        }
        self.persist();
    }

    /// Current input level, for the recording indicator.
    pub fn audio_level(&self) -> f32 {
        self.inner.audio.level()
    }

    /// Current per-band energy, for a spectrum display. See
    /// [`crate::ports::AudioSource::spectrum`].
    pub fn audio_spectrum(&self) -> Vec<f32> {
        self.inner.audio.spectrum()
    }

    fn transition(&self, next: State) {
        {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            if *state == next {
                return;
            }
            *state = next.clone();
        }
        tracing::debug!(?next, "state changed");
        self.emit(Event::StateChanged { state: next });
    }

    fn fail(&self, error: Error) {
        tracing::warn!(%error, "engine failed");
        self.emit(Event::Failed {
            error: error.clone(),
        });
        self.transition(State::Failed { error });
    }

    fn emit(&self, event: Event) {
        self.inner.emit(event);
    }
}

/// How to name a model in the UI.
///
/// Lives here rather than in the shell because a catalog id and a local path
/// have to render in the same tray line, and every shell would otherwise write
/// this twice.
fn label_for(choice: &ModelChoice) -> String {
    match choice {
        // An id the catalog has never heard of still needs a name; the id
        // itself is the most honest one available.
        ModelChoice::Catalog { id } => catalog::find(id)
            .map(|info| info.label.to_string())
            .unwrap_or_else(|| id.to_string()),
        ModelChoice::Local { path } => std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone()),
    }
}

impl Inner {
    /// On `Inner` rather than `Engine` so the model status callback, which only
    /// holds a `Weak<Inner>`, can publish without an engine handle.
    fn emit(&self, event: Event) {
        let mut subscribers = self.subscribers.lock().expect("subscribers mutex poisoned");
        subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeAudio, FakeFetcher, FakeSink, FakeTranscriber};
    use crate::testing::TempDir;

    /// A models directory for tests that never touch one.
    ///
    /// Deliberately not created: `installed()` on a directory that does not
    /// exist is empty, which is what these tests want, and nothing here writes
    /// to it.
    fn unused_models_dir() -> PathBuf {
        std::env::temp_dir().join("speakeasy-engine-tests-no-models")
    }

    fn engine(transcript: &str) -> (Engine, Arc<FakeSink>) {
        let sink = Arc::new(FakeSink::default());
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Arc::new(FakeTranscriber::returning(transcript)),
            Box::new(sink.clone()),
            Box::new(FakeFetcher::serving(Vec::new())),
            unused_models_dir(),
            Config {
                min_recording_ms: 0,
                ..Config::default()
            },
        );
        (engine, sink)
    }

    fn engine_with_segmenter(
        transcript: &str,
        spans: Vec<crate::ports::SpeechSegment>,
    ) -> (Engine, Arc<FakeSink>) {
        let (engine, sink) = engine(transcript);
        (
            engine.with_segmenter(Arc::new(crate::fakes::FakeSegmenter::returning(spans))),
            sink,
        )
    }

    /// One second of speech in a one-second buffer: still in progress.
    fn running() -> Vec<crate::ports::SpeechSegment> {
        vec![crate::ports::SpeechSegment {
            start: 0.0,
            end: 0.95,
        }]
    }

    /// Speech that ended well before the buffer did: finished, so committable.
    fn settled() -> Vec<crate::ports::SpeechSegment> {
        vec![crate::ports::SpeechSegment {
            start: 0.0,
            end: 0.2,
        }]
    }

    #[test]
    fn a_tick_previews_text_while_recording() {
        let (engine, _sink) = engine_with_segmenter("hello there", running());
        let events = engine.subscribe();
        engine.begin_recording().expect("recording should start");

        engine.preview_tick();

        assert!(events.try_iter().any(
            |event| matches!(event, Event::TranscriptPreview { text } if text == "hello there")
        ));
    }

    /// The silence gate. Whisper invents words when handed a quiet buffer, so
    /// a pause must produce no preview at all rather than an empty one.
    #[test]
    fn silence_produces_no_preview() {
        let (engine, _sink) = engine("should never appear");
        let engine = engine.with_segmenter(Arc::new(crate::fakes::FakeSegmenter::silent()));
        let events = engine.subscribe();
        engine.begin_recording().expect("recording should start");

        engine.preview_tick();

        assert!(!events
            .try_iter()
            .any(|event| matches!(event, Event::TranscriptPreview { .. })));
    }

    /// The point of the whole design: text shown once does not change.
    #[test]
    fn committed_text_survives_a_later_tick() {
        let (engine, _sink) = engine_with_segmenter("first part", settled());
        let events = engine.subscribe();
        engine.begin_recording().expect("recording should start");

        engine.preview_tick();
        engine.preview_tick();

        let previews: Vec<String> = events
            .try_iter()
            .filter_map(|event| match event {
                Event::TranscriptPreview { text } => Some(text),
                _ => None,
            })
            .collect();
        assert!(!previews.is_empty(), "expected at least one preview");
        for preview in &previews {
            assert!(
                preview.starts_with("first part"),
                "committed text was rewritten: {preview}"
            );
        }
    }

    #[test]
    fn a_segmenter_failure_leaves_the_final_transcript_intact() {
        let (engine, sink) = engine("the real transcript");
        let engine = engine.with_segmenter(Arc::new(crate::fakes::FakeSegmenter::failing(
            Error::Other("vad exploded".to_string()),
        )));
        engine.begin_recording().expect("recording should start");

        engine.preview_tick();
        engine.finish_recording().expect("cycle should complete");

        assert_eq!(sink.injected(), vec!["the real transcript".to_string()]);
    }

    #[test]
    fn no_previews_without_a_segmenter() {
        let (engine, _sink) = engine("hello");
        let events = engine.subscribe();
        engine.begin_recording().expect("recording should start");

        engine.preview_tick();

        assert!(!events
            .try_iter()
            .any(|event| matches!(event, Event::TranscriptPreview { .. })));
    }

    /// Turning it off must stop the decoding, not merely hide the text.
    #[test]
    fn no_previews_when_the_setting_is_off() {
        let (engine, _sink) = engine_with_segmenter("hidden", running());
        engine.set_config(Config {
            transcript_preview: false,
            min_recording_ms: 0,
            ..Config::default()
        });
        let events = engine.subscribe();
        engine.begin_recording().expect("recording should start");

        engine.preview_tick();

        assert!(!events
            .try_iter()
            .any(|event| matches!(event, Event::TranscriptPreview { .. })));
    }

    #[test]
    fn a_preview_does_not_survive_a_cancel() {
        let (engine, sink) = engine_with_segmenter("abandoned", settled());
        engine.begin_recording().expect("recording should start");
        engine.preview_tick();

        engine.cancel();

        assert_eq!(engine.state(), State::Idle);
        assert!(sink.injected().is_empty());
    }

    /// A preview from the previous dictation must not leak into the next one.
    #[test]
    fn a_new_recording_starts_with_no_committed_text() {
        let (engine, _sink) = engine_with_segmenter("first", settled());
        engine.begin_recording().expect("recording should start");
        engine.preview_tick();
        engine.cancel();

        let events = engine.subscribe();
        engine.begin_recording().expect("second recording starts");
        engine.preview_tick();

        let previews: Vec<String> = events
            .try_iter()
            .filter_map(|event| match event {
                Event::TranscriptPreview { text } => Some(text),
                _ => None,
            })
            .collect();
        for preview in &previews {
            assert_eq!(preview, "first", "text carried over from the last take");
        }
    }

    #[test]
    fn setting_the_hotkey_persists_it_and_announces_the_change() {
        let (engine, _sink) = engine("anything");
        let events = engine.subscribe();

        let hotkey = Hotkey::Chord {
            mods: crate::ChordMods {
                command: true,
                ..Default::default()
            },
            key: crate::KeyCode::Space,
        };
        engine.set_hotkey(hotkey.clone());

        assert_eq!(engine.config().hotkey, hotkey);
        assert!(events
            .try_iter()
            .any(|event| matches!(event, Event::HotkeyChanged { hotkey: ref h } if *h == hotkey)));
    }

    /// Poll until `condition` holds, or fail.
    ///
    /// `install_model` spawns, so the assertions that follow it cannot run
    /// immediately. Polling with a generous ceiling beats a fixed sleep, which
    /// is either flaky or slow.
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn full_cycle_injects_transcript_and_returns_to_idle() {
        let (engine, sink) = engine("hello world");

        engine.begin_recording().expect("recording should start");
        assert_eq!(engine.state(), State::Recording);

        engine.finish_recording().expect("cycle should complete");
        assert_eq!(engine.state(), State::Idle);
        assert_eq!(sink.injected(), vec!["hello world".to_string()]);
    }

    #[test]
    fn key_repeat_does_not_restart_recording() {
        let (engine, _sink) = engine("hi");

        engine
            .begin_recording()
            .expect("first press starts recording");
        engine.begin_recording().expect("repeat press is a no-op");
        assert_eq!(engine.state(), State::Recording);
    }

    #[test]
    fn empty_transcript_is_not_injected() {
        let (engine, sink) = engine("   ");

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("cycle should complete");

        assert_eq!(engine.state(), State::Idle);
        assert!(sink.injected().is_empty());
    }

    #[test]
    fn release_without_press_is_a_no_op() {
        let (engine, sink) = engine("hello");

        engine.finish_recording().expect("stray release is ignored");
        assert_eq!(engine.state(), State::Idle);
        assert!(sink.injected().is_empty());
    }

    #[test]
    fn subscribers_observe_the_state_sequence() {
        let (engine, _sink) = engine("hello");
        let events = engine.subscribe();

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("cycle should complete");

        let states: Vec<State> = events
            .try_iter()
            .filter_map(|event| match event {
                Event::StateChanged { state } => Some(state),
                _ => None,
            })
            .collect();
        assert_eq!(
            states,
            vec![
                State::Recording,
                State::Transcribing,
                State::Injecting,
                State::Idle
            ]
        );
    }

    #[test]
    fn short_recording_is_discarded_as_an_accidental_tap() {
        let sink = Arc::new(FakeSink::default());
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Arc::new(FakeTranscriber::returning("oops")),
            Box::new(sink.clone()),
            Box::new(FakeFetcher::serving(Vec::new())),
            unused_models_dir(),
            Config {
                min_recording_ms: 60_000,
                ..Config::default()
            },
        );

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("tap should be discarded");

        assert_eq!(engine.state(), State::Idle);
        assert!(sink.injected().is_empty());
    }

    #[test]
    fn push_to_talk_records_between_press_and_release() {
        let (engine, _sink) = engine("hello");
        engine.set_config(Config {
            activation: ActivationMode::PushToTalk,
            ..Config::default()
        });

        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Pressed),
            HotkeyAction::Begin
        );
        engine.begin_recording().expect("recording should start");
        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Released),
            HotkeyAction::Finish
        );
    }

    #[test]
    fn toggle_alternates_on_press_and_ignores_release() {
        let (engine, _sink) = engine("hello");
        engine.set_config(Config {
            activation: ActivationMode::Toggle,
            min_recording_ms: 0,
            ..Config::default()
        });

        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Pressed),
            HotkeyAction::Begin
        );
        engine.begin_recording().expect("recording should start");

        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Released),
            HotkeyAction::Ignore,
            "releasing the key must not stop a toggled recording"
        );
        assert_eq!(engine.state(), State::Recording);

        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Pressed),
            HotkeyAction::Finish
        );
        engine.finish_recording().expect("cycle should complete");
        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Pressed),
            HotkeyAction::Begin
        );
    }

    #[test]
    fn toggle_ignores_a_press_while_transcription_is_in_flight() {
        let (engine, _sink) = engine("hello");
        engine.set_config(Config {
            activation: ActivationMode::Toggle,
            ..Config::default()
        });
        engine.transition(State::Transcribing);

        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Pressed),
            HotkeyAction::Ignore
        );
    }

    #[test]
    fn toggle_recovers_from_a_failed_state_on_the_next_press() {
        let (engine, _sink) = engine("hello");
        engine.set_config(Config {
            activation: ActivationMode::Toggle,
            ..Config::default()
        });
        engine.fail(Error::SecureInputActive);

        assert_eq!(
            engine.hotkey_action(HotkeyEvent::Pressed),
            HotkeyAction::Begin
        );
    }

    #[test]
    fn injection_failure_surfaces_as_failed_state() {
        let sink = Arc::new(FakeSink::failing(Error::SecureInputActive));
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Arc::new(FakeTranscriber::returning("hello")),
            Box::new(sink.clone()),
            Box::new(FakeFetcher::serving(Vec::new())),
            unused_models_dir(),
            Config {
                min_recording_ms: 0,
                ..Config::default()
            },
        );

        engine.begin_recording().expect("recording should start");
        let err = engine
            .finish_recording()
            .expect_err("injection should fail");

        assert_eq!(err, Error::SecureInputActive);
        assert_eq!(
            engine.state(),
            State::Failed {
                error: Error::SecureInputActive
            }
        );
    }

    /// Builds an engine over a real directory, so the model paths resolve.
    fn engine_with_models(
        dir: &TempDir,
        fetcher: FakeFetcher,
        config: Config,
    ) -> (Engine, Arc<FakeTranscriber>) {
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            transcriber.clone(),
            Box::new(FakeSink::default()),
            Box::new(fetcher),
            dir.0.clone(),
            config,
        );
        (engine, transcriber)
    }

    #[test]
    fn selecting_a_local_model_records_it_and_loads_it() {
        let dir = TempDir::new("engine-select-local");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let (engine, transcriber) =
            engine_with_models(&dir, FakeFetcher::serving(Vec::new()), Config::default());

        engine.select_model(ModelChoice::Local {
            path: weights.display().to_string(),
        });

        assert_eq!(
            engine.config().model,
            ModelChoice::Local {
                path: weights.display().to_string()
            }
        );
        assert_eq!(transcriber.loaded(), Some(weights));
        assert!(matches!(engine.model_status(), ModelStatus::Ready { .. }));
    }

    #[test]
    fn ensure_model_loads_a_model_that_is_already_on_disk() {
        let dir = TempDir::new("engine-ensure-present");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let (engine, transcriber) = engine_with_models(
            &dir,
            FakeFetcher::serving(Vec::new()),
            Config {
                model: ModelChoice::Local {
                    path: weights.display().to_string(),
                },
                ..Config::default()
            },
        );

        engine.ensure_model();

        assert_eq!(transcriber.loaded(), Some(weights));
        assert!(matches!(engine.model_status(), ModelStatus::Ready { .. }));
    }

    /// First run is not a special case: a fresh config wants a catalog model
    /// that is not on disk, so `ensure_model` fetches it.
    #[test]
    fn ensure_model_downloads_the_wanted_model_when_it_is_absent() {
        let dir = TempDir::new("engine-ensure-absent");
        // The catalog's real digest for base.en, so verification passes only if
        // the fetcher serves the real file — which it cannot. Serving the wrong
        // bytes here would make this a verification test instead, so assert on
        // the attempt rather than the outcome.
        let fetcher = Arc::new(FakeFetcher::serving(b"not the real weights".to_vec()));
        let transcriber = Arc::new(FakeTranscriber::not_ready());
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            transcriber.clone(),
            Box::new(FakeSink::default()),
            Box::new(fetcher.clone()),
            dir.0.clone(),
            Config::default(),
        );

        engine.ensure_model();

        wait_until("the download to be attempted", || fetcher.calls() == 1);
    }

    /// Dictation must not be left dead because one model could not be had.
    #[test]
    fn ensure_model_falls_back_to_an_installed_model_when_the_wanted_one_fails() {
        let dir = TempDir::new("engine-ensure-fallback");
        // A catalog model is on disk, but config wants a local file that is not.
        let installed = dir.join("ggml-base.en.bin");
        std::fs::write(&installed, b"weights").expect("write should succeed");
        let (engine, transcriber) = engine_with_models(
            &dir,
            FakeFetcher::serving(Vec::new()),
            Config {
                model: ModelChoice::Local {
                    path: "/nonexistent/ggml-gone.bin".to_string(),
                },
                ..Config::default()
            },
        );

        engine.ensure_model();

        assert_eq!(
            transcriber.loaded(),
            Some(installed),
            "the installed model was loaded so the hotkey still works"
        );
        assert!(
            matches!(engine.model_status(), ModelStatus::Failed { .. }),
            "but the status still says why this is not the chosen model"
        );
    }

    /// Cancel means "never mind". Without the revert, the next launch would
    /// resume the transfer the user just abandoned.
    #[test]
    fn cancelling_a_download_restores_the_previously_selected_model() {
        let dir = TempDir::new("engine-cancel-reverts");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let local = ModelChoice::Local {
            path: weights.display().to_string(),
        };
        let (engine, _transcriber) = engine_with_models(
            &dir,
            FakeFetcher::serving(b"some bytes".to_vec()),
            Config {
                model: local.clone(),
                ..Config::default()
            },
        );

        // Cancel before starting, so the fetcher observes the flag on its first
        // chunk and the install reports a cancel deterministically.
        engine.cancel_model_download();
        engine.install_model(ModelId::new("base.en"));

        wait_until("the config to revert", || engine.config().model == local);
    }

    #[test]
    fn a_model_status_change_reaches_subscribers() {
        let dir = TempDir::new("engine-status-event");
        let weights = dir.join("mine.bin");
        std::fs::write(&weights, b"weights").expect("write should succeed");
        let (engine, _transcriber) =
            engine_with_models(&dir, FakeFetcher::serving(Vec::new()), Config::default());
        let events = engine.subscribe();

        engine.select_model(ModelChoice::Local {
            path: weights.display().to_string(),
        });

        assert!(
            events
                .try_iter()
                .any(|event| matches!(event, Event::ModelStatusChanged { .. })),
            "a shell learns about model changes the same way it learns about state"
        );
    }

    #[test]
    fn recording_is_refused_when_no_model_is_loaded() {
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![])),
            Arc::new(FakeTranscriber::not_ready()),
            Box::new(FakeSink::default()),
            Box::new(FakeFetcher::serving(Vec::new())),
            unused_models_dir(),
            Config::default(),
        );

        let err = engine
            .begin_recording()
            .expect_err("should refuse to record");

        assert_eq!(err, Error::ModelNotLoaded);
        assert_eq!(
            engine.state(),
            State::Failed {
                error: Error::ModelNotLoaded
            }
        );
    }
}
