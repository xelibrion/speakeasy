//! The push-to-talk state machine.
//!
//! Idle → Recording → Transcribing → Injecting → Idle, with any step able to
//! fall into `Failed` and recover to `Idle` on the next press.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::event::{Event, State};
use crate::ports::{AudioSource, TextSink, Transcriber};

/// Owns application state and drives the ports. Cheap to clone — clones share
/// one engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    audio: Box<dyn AudioSource>,
    transcriber: Box<dyn Transcriber>,
    sink: Box<dyn TextSink>,
    config: Mutex<Config>,
    state: Mutex<State>,
    started_at: Mutex<Option<Instant>>,
    subscribers: Mutex<Vec<Sender<Event>>>,
}

impl Engine {
    pub fn new(
        audio: Box<dyn AudioSource>,
        transcriber: Box<dyn Transcriber>,
        sink: Box<dyn TextSink>,
        config: Config,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                audio,
                transcriber,
                sink,
                config: Mutex::new(config),
                state: Mutex::new(State::Idle),
                started_at: Mutex::new(None),
                subscribers: Mutex::new(Vec::new()),
            }),
        }
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

    pub fn set_config(&self, config: Config) {
        *self.inner.config.lock().expect("config mutex poisoned") = config;
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
        self.transition(State::Recording);
        Ok(())
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

    /// Current input level, for the recording indicator.
    pub fn audio_level(&self) -> f32 {
        self.inner.audio.level()
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
        let mut subscribers = self
            .inner
            .subscribers
            .lock()
            .expect("subscribers mutex poisoned");
        subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeAudio, FakeSink, FakeTranscriber};

    fn engine(transcript: &str) -> (Engine, Arc<FakeSink>) {
        let sink = Arc::new(FakeSink::default());
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Box::new(FakeTranscriber::returning(transcript)),
            Box::new(sink.clone()),
            Config {
                min_recording_ms: 0,
                ..Config::default()
            },
        );
        (engine, sink)
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
            Box::new(FakeTranscriber::returning("oops")),
            Box::new(sink.clone()),
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
    fn injection_failure_surfaces_as_failed_state() {
        let sink = Arc::new(FakeSink::failing(Error::SecureInputActive));
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Box::new(FakeTranscriber::returning("hello")),
            Box::new(sink.clone()),
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

    #[test]
    fn recording_is_refused_when_no_model_is_loaded() {
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![])),
            Box::new(FakeTranscriber::not_ready()),
            Box::new(FakeSink::default()),
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
