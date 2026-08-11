# Streaming Preview Transcripts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show recognised text in the floating widget while the user is still speaking, with text that never rewrites itself once shown.

**Architecture:** A preview loop runs only while recording. Every 1.5s it decodes the audio captured since the last commit point and asks a voice-activity detector whether that audio is finished speech. Finished speech is decoded once, committed, and never revised; speech still in progress is shown as a live tail. Because the uncommitted region is bounded by how long the user speaks without pausing, decode cost stays flat.

**Tech Stack:** Rust workspace (`speakeasy-core`, `speakeasy-whisper`, `speakeasy-platform`), whisper.cpp via `whisper-rs` 0.16 with Metal, Silero VAD, Tauri 2 shell, TypeScript frontend.

**Spec:** `docs/superpowers/specs/2026-08-11-streaming-preview-transcripts-design.md`

---

## Before you start

Read the spec. In particular read "What the measurements say" — the numbers there are why this design exists, and several obvious-looking simplifications are ruled out by them.

Three conventions in this codebase you must follow:

1. **`make check` before every commit.** It runs `fmt-check`, `clippy -D warnings`, `test` and `boundary`. Clippy warnings are errors here.
2. **`speakeasy-core` must never gain a `tauri` dependency**, and must stay Swift-expressible: flat owned types, no generics or lifetimes in public signatures (`lib.rs:8-13`). `make boundary` enforces the first half; you enforce the second.
3. **`helper` is a banned word** in identifiers in this codebase.

Comments in this codebase explain *why*, not *what*. Match that or your code will look foreign.

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `apps/desktop/src-tauri/Cargo.toml` | Enables Metal on macOS | Modified (already, uncommitted) |
| `Cargo.toml` | Workspace dep versions | Modified — whisper-rs 0.16 |
| `crates/speakeasy-whisper/src/lib.rs` | Transcriber over whisper.cpp | Modified — 0.16 API |
| `crates/speakeasy-whisper/src/segmenter.rs` | **VAD over Silero** | Create |
| `crates/speakeasy-whisper/vendor/ggml-silero-v5.1.2.bin` | Bundled VAD weights | Create (binary) |
| `crates/speakeasy-core/src/ports.rs` | Port definitions | Modified — `SpeechSegmenter`, `AudioSource::snapshot` |
| `crates/speakeasy-core/src/event.rs` | Event stream | Modified — `TranscriptPreview` |
| `crates/speakeasy-core/src/config.rs` | Persisted settings | Modified — `transcript_preview` |
| `crates/speakeasy-core/src/preview.rs` | **Commit state machine** | Create |
| `crates/speakeasy-core/src/engine.rs` | State machine, preview loop | Modified |
| `crates/speakeasy-core/src/fakes.rs` | Test doubles | Modified — `FakeSegmenter`, growing `FakeAudio` |
| `crates/speakeasy-platform/src/audio/mod.rs` | cpal capture | Modified — `snapshot` |
| `apps/desktop/src-tauri/src/main.rs` | Wiring | Modified — build and attach segmenter |
| `apps/desktop/src/engine.ts` | Typed mirror of core | Modified |
| `apps/desktop/src/widget.ts` | The pill | Modified |
| `apps/desktop/src/settings.ts` | Dictation pane | Modified — checkbox |
| `apps/desktop/src/styles.css` | Widget styling | Modified |
| `apps/desktop/src-tauri/tauri.conf.json` | Widget window size | Modified — height |

**Why `preview.rs` is its own file:** the commit rule is the only genuinely subtle logic in this feature and it is worth testing in isolation, with no engine, no threads and no ports. `engine.rs` is already ~990 lines; putting a state machine plus its tests in there would make it worse.

---

## Task 1: Enable Metal

This change is already in your working tree, uncommitted. It is a prerequisite: at the pre-Metal 1.90s per decode, nothing else in this plan works.

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`

- [ ] **Step 1: Confirm the change is present**

```bash
git diff apps/desktop/src-tauri/Cargo.toml
```

Expected: a `[target.'cfg(target_os = "macos")'.dependencies]` block adding `speakeasy-whisper` with `features = ["metal"]`. If absent, add it — see the spec.

- [ ] **Step 2: Verify Metal actually links**

```bash
cargo build -p speakeasy-desktop && otool -L target/debug/speakeasy-desktop | grep -i metal
```

Expected: lines for `Metal.framework` and `MetalKit.framework`.

- [ ] **Step 3: Run the checks**

```bash
make check
```

Expected: exits 0.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml
git commit -m "Move whisper inference onto the GPU"
```

---

## Task 2: Upgrade whisper-rs to 0.16

The bundled whisper.cpp in 0.14 has no VAD support at all. 0.16 brings a standalone `WhisperVadContext`. Two API calls change.

**Files:**
- Modify: `Cargo.toml` (workspace deps)
- Modify: `crates/speakeasy-whisper/src/lib.rs:89-101`

- [ ] **Step 1: Bump the dependency**

In the workspace `Cargo.toml`, change `whisper-rs = "0.14"` to `whisper-rs = "0.16"`.

- [ ] **Step 2: Build and watch it fail**

```bash
cargo build -p speakeasy-whisper
```

Expected: FAIL. `full_get_segment_text_lossy` not found; `full_n_segments` type mismatch (it no longer returns `Result`).

This build compiles whisper.cpp from source and takes several minutes the first time.

- [ ] **Step 3: Fix the segment API**

In `transcribe`, replace the segment count and text extraction:

```rust
        let segments = state.full_n_segments();

        let mut text = String::new();
        for i in 0..segments {
            // `_lossy` so a stray byte in one segment cannot fail the whole
            // utterance — dropping a character beats dropping the dictation.
            let segment = state
                .get_segment(i)
                .ok_or_else(|| Error::Transcription(format!("segment {i} vanished")))?;
            let piece = segment.to_str_lossy().map_err(|err| {
                Error::Transcription(format!("could not read segment {i}: {err}"))
            })?;
            text.push_str(&piece);
        }
```

- [ ] **Step 4: Build**

```bash
cargo build -p speakeasy-whisper
```

Expected: PASS.

- [ ] **Step 5: Verify against real audio**

```bash
cargo test -p speakeasy-whisper
```

Expected: PASS. `tests/transcribe.rs` exercises a real model if one is present; if it skips, that is fine — note it and move on.

- [ ] **Step 6: Full checks and commit**

```bash
make check
git add Cargo.toml Cargo.lock crates/speakeasy-whisper/src/lib.rs
git commit -m "Upgrade whisper-rs for its voice activity detector"
```

---

## Task 3: The `SpeechSegmenter` port

**Files:**
- Modify: `crates/speakeasy-core/src/ports.rs`
- Modify: `crates/speakeasy-core/src/lib.rs:37-40`
- Modify: `crates/speakeasy-core/src/fakes.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/speakeasy-core/src/fakes.rs`, inside `mod tests`:

```rust
    #[test]
    fn a_fake_segmenter_reports_the_spans_it_was_given() {
        let segmenter = FakeSegmenter::returning(vec![SpeechSegment {
            start: 0.2,
            end: 1.4,
        }]);

        let spans = segmenter.segments(&[0.0; 16_000]).expect("spans");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].end, 1.4);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p speakeasy-core --features fakes a_fake_segmenter
```

Expected: FAIL — `FakeSegmenter` and `SpeechSegment` do not exist.

- [ ] **Step 3: Add the port**

In `crates/speakeasy-core/src/ports.rs`, after the `Transcriber` trait:

```rust
/// A span of speech within a buffer, in seconds from its start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeechSegment {
    pub start: f32,
    pub end: f32,
}

/// Finds speech within captured audio.
///
/// Separate from [`Transcriber`] rather than folded into it, per the contract
/// on that trait: streaming arrives as a new port. What a preview needs is not
/// more transcription but the answer to "has this utterance finished?", which
/// is what lets text be committed and never revised.
///
/// An energy threshold cannot answer it. Background noise keeps the level high
/// where the gate is most needed, and whisper hallucinates confidently on
/// non-speech audio.
pub trait SpeechSegmenter: Send + Sync {
    /// Speech spans within `samples`, which is mono f32 at
    /// [`crate::TARGET_SAMPLE_RATE`]. Empty means no speech.
    fn segments(&self, samples: &[f32]) -> Result<Vec<SpeechSegment>>;
}
```

- [ ] **Step 4: Export it**

In `crates/speakeasy-core/src/lib.rs`, add `SpeechSegment, SpeechSegmenter` to the `pub use ports::{...}` list, keeping it alphabetical.

- [ ] **Step 5: Add the fake**

In `crates/speakeasy-core/src/fakes.rs`, extend the `use crate::ports::{...}` list with `SpeechSegment, SpeechSegmenter`, then add:

```rust
/// Reports scripted spans, ignoring the audio.
pub struct FakeSegmenter {
    result: Result<Vec<SpeechSegment>>,
}

impl FakeSegmenter {
    pub fn returning(spans: Vec<SpeechSegment>) -> Self {
        Self { result: Ok(spans) }
    }

    /// No speech at all — what the detector reports during a pause.
    pub fn silent() -> Self {
        Self::returning(Vec::new())
    }

    pub fn failing(error: Error) -> Self {
        Self {
            result: Err(error),
        }
    }
}

impl SpeechSegmenter for FakeSegmenter {
    fn segments(&self, _samples: &[f32]) -> Result<Vec<SpeechSegment>> {
        self.result.clone()
    }
}
```

- [ ] **Step 6: Run the test**

```bash
cargo test -p speakeasy-core --features fakes a_fake_segmenter
```

Expected: PASS.

- [ ] **Step 7: Full checks and commit**

```bash
make check
git add crates/speakeasy-core/src/ports.rs crates/speakeasy-core/src/lib.rs crates/speakeasy-core/src/fakes.rs
git commit -m "Give the core a port for finding speech"
```

---

## Task 4: Reading audio without ending the recording

`AudioSource` currently only yields samples from `stop`, which drains the buffer. `spectrum()` (`audio/mod.rs:220-234`) is the precedent for the read this needs: copy out under the capture lock, release it, then do the expensive work off the audio callback's path.

**Files:**
- Modify: `crates/speakeasy-core/src/ports.rs:20-40`
- Modify: `crates/speakeasy-core/src/fakes.rs:47-79`
- Modify: `crates/speakeasy-platform/src/audio/mod.rs:181-235`

- [ ] **Step 1: Write the failing test**

In `crates/speakeasy-core/src/fakes.rs`, inside `mod tests`:

```rust
    /// A snapshot must not end the recording — that is the whole point of it.
    #[test]
    fn a_snapshot_leaves_the_recording_running() {
        let audio = FakeAudio::with_samples(vec![0.25; 16_000]);
        audio.start().expect("start");

        let snapshot = audio.snapshot().expect("snapshot");

        assert_eq!(snapshot.len(), 16_000);
        assert!(audio.is_recording(), "snapshot must not stop capture");
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p speakeasy-core --features fakes a_snapshot_leaves
```

Expected: FAIL — no method `snapshot`.

- [ ] **Step 3: Add the port method**

In `ports.rs`, inside `trait AudioSource`, after `stop`:

```rust
    /// Everything captured so far, at [`crate::TARGET_SAMPLE_RATE`], without
    /// ending the recording. Empty when not recording.
    ///
    /// Returns the whole buffer rather than taking an offset deliberately: an
    /// offset would have to be mapped between the device's native rate and
    /// 16 kHz on every call, and that arithmetic is where an off-by-one becomes
    /// a word cut in half. The caller slices what it wants.
    fn snapshot(&self) -> Result<Vec<f32>>;
```

- [ ] **Step 4: Implement it on the fake**

In `fakes.rs`, inside `impl AudioSource for FakeAudio`:

```rust
    fn snapshot(&self) -> Result<Vec<f32>> {
        if !self.is_recording() {
            return Ok(Vec::new());
        }
        Ok(self.samples.clone())
    }
```

- [ ] **Step 5: Implement it on cpal**

In `crates/speakeasy-platform/src/audio/mod.rs`, inside `impl AudioSource for CpalAudioSource`, after `stop`:

```rust
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
```

- [ ] **Step 6: Run the tests**

```bash
cargo test -p speakeasy-core --features fakes a_snapshot_leaves
cargo test -p speakeasy-platform
```

Expected: both PASS.

- [ ] **Step 7: Full checks and commit**

```bash
make check
git add crates/speakeasy-core/src/ports.rs crates/speakeasy-core/src/fakes.rs crates/speakeasy-platform/src/audio/mod.rs
git commit -m "Let a recording be read without ending it"
```

---

## Task 5: The commit state machine

The heart of the feature, in isolation: no engine, no threads, no ports. Given the length of a slice of audio and the speech spans within it, decide whether to commit or to show a live tail.

**Files:**
- Create: `crates/speakeasy-core/src/preview.rs`
- Modify: `crates/speakeasy-core/src/lib.rs:15-21`

- [ ] **Step 1: Write the failing tests**

Create `crates/speakeasy-core/src/preview.rs`:

```rust
//! What a preview tick should do with the audio it has.
//!
//! Split from the engine because this is the only genuinely subtle rule in the
//! feature — when text may be committed and never revised — and it is worth
//! deciding with no threads, no ports and no clock involved.

use crate::ports::SpeechSegment;

/// How much silence must follow speech before it counts as finished.
const SILENCE_TAIL_S: f32 = 0.5;

/// Below this there is not enough audio to be worth a decode.
const MIN_SLICE_S: f32 = 0.3;

/// Speech this long without a pause is committed anyway, so the slice never
/// approaches whisper's 30-second padded window — past which it processes a
/// second window and every decode costs roughly double.
const FORCE_COMMIT_S: f32 = 25.0;

/// Kept after the end of a span, so a commit does not clip the final consonant.
const COMMIT_PAD_S: f32 = 0.2;

/// What to do with the audio a tick is holding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tick {
    /// Not enough audio, or no speech in it. Decode nothing.
    Skip,
    /// Speech is still in progress. Decode all of it and show it as the live
    /// tail, committing nothing.
    Live,
    /// The utterance has finished. Decode up to `through_seconds`, commit that
    /// text permanently, and advance the offset past it.
    Commit { through_seconds: f32 },
}

/// Decide what a tick holding `slice_seconds` of audio containing `spans`
/// should do.
pub fn decide(slice_seconds: f32, spans: &[SpeechSegment]) -> Tick {
    if slice_seconds < MIN_SLICE_S {
        return Tick::Skip;
    }

    let Some(last) = spans.last() else {
        return Tick::Skip;
    };

    let settled = last.end + SILENCE_TAIL_S <= slice_seconds;
    if settled || slice_seconds >= FORCE_COMMIT_S {
        return Tick::Commit {
            through_seconds: (last.end + COMMIT_PAD_S).min(slice_seconds),
        };
    }

    Tick::Live
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f32, end: f32) -> SpeechSegment {
        SpeechSegment { start, end }
    }

    #[test]
    fn too_little_audio_is_not_worth_a_decode() {
        assert_eq!(decide(0.1, &[span(0.0, 0.1)]), Tick::Skip);
    }

    /// The silence gate. Without it whisper is handed a quiet buffer and
    /// invents words to fill it.
    #[test]
    fn silence_decodes_nothing() {
        assert_eq!(decide(3.0, &[]), Tick::Skip);
    }

    #[test]
    fn speech_still_running_shows_a_live_tail() {
        // Speech ends 0.1s before the buffer does — the user is mid-word.
        assert_eq!(decide(2.0, &[span(0.0, 1.9)]), Tick::Live);
    }

    #[test]
    fn speech_followed_by_silence_is_committed() {
        // Speech ended a full second ago.
        assert_eq!(
            decide(3.0, &[span(0.0, 2.0)]),
            Tick::Commit {
                through_seconds: 2.2
            }
        );
    }

    /// A long unbroken monologue must still commit, or the slice grows past
    /// whisper's 30-second window and every decode doubles in cost.
    #[test]
    fn unbroken_speech_is_committed_before_the_thirty_second_cliff() {
        assert!(matches!(
            decide(25.5, &[span(0.0, 25.4)]),
            Tick::Commit { .. }
        ));
    }

    /// Padding must never run off the end of the audio we actually hold.
    #[test]
    fn a_commit_never_reaches_past_the_audio() {
        let Tick::Commit { through_seconds } = decide(25.5, &[span(0.0, 25.45)]) else {
            panic!("expected a commit");
        };
        assert!(through_seconds <= 25.5);
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/speakeasy-core/src/lib.rs`, add `pub mod preview;` to the module list, alphabetically after `models`.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p speakeasy-core preview
```

Expected: PASS, six tests.

- [ ] **Step 4: Full checks and commit**

```bash
make check
git add crates/speakeasy-core/src/preview.rs crates/speakeasy-core/src/lib.rs
git commit -m "Decide when preview text may be committed"
```

---

## Task 6: The preview event

**Files:**
- Modify: `crates/speakeasy-core/src/event.rs:56-100`

- [ ] **Step 1: Add the variant**

In `event.rs`, after the `Transcript` variant:

```rust
    /// Text recognised so far, while the user is still speaking.
    ///
    /// Advisory and display-only. The committed prefix and the live tail are
    /// joined into one string because the widget renders only the tail of it
    /// and has no way to style them differently; if that changes, this grows a
    /// field rather than callers growing a rule.
    ///
    /// Never the text that gets injected — that is [`Event::Transcript`].
    TranscriptPreview {
        text: String,
    },
```

- [ ] **Step 2: Build**

```bash
cargo build -p speakeasy-core
```

Expected: PASS. Nothing matches exhaustively on `Event`, so no other code breaks.

- [ ] **Step 3: Commit**

```bash
make check
git add crates/speakeasy-core/src/event.rs
git commit -m "Announce preview text on the event stream"
```

---

## Task 7: The config setting

**Files:**
- Modify: `crates/speakeasy-core/src/config.rs:31-71`, `:294-307`

- [ ] **Step 1: Write the failing test**

In `config.rs`, inside `mod tests`:

```rust
    /// An existing config predates this field and must keep loading.
    #[test]
    fn a_config_without_the_preview_field_defaults_it_on() {
        let dir = TempDir::new("preview-default");
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"activation":"toggle"}"#).expect("write");

        let config = Config::load(&path).expect("old configs still load");

        assert!(config.transcript_preview);
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p speakeasy-core a_config_without_the_preview_field
```

Expected: FAIL — no field `transcript_preview`.

- [ ] **Step 3: Add the field**

In `struct Config`, after `min_recording_ms`:

```rust
    /// Whether to show recognised text in the widget while the user speaks.
    ///
    /// Costs a decode every 1.5s for the whole time the key is held, so it is
    /// worth being able to turn off on battery even though it defaults on.
    pub transcript_preview: bool,
```

And in `impl Default for Config`, after `min_recording_ms: 250,`:

```rust
            transcript_preview: true,
```

The struct already carries `#[serde(default, rename_all = "camelCase")]` (`config.rs:30`), so a missing field in an existing file takes the default with no extra attribute.

- [ ] **Step 4: Run the test**

```bash
cargo test -p speakeasy-core config
```

Expected: PASS.

- [ ] **Step 5: Full checks and commit**

```bash
make check
git add crates/speakeasy-core/src/config.rs
git commit -m "Let the preview be switched off"
```

---

## Task 8: Wire the preview into the engine

**Files:**
- Modify: `crates/speakeasy-core/src/engine.rs`

- [ ] **Step 1: Write the failing tests**

In `engine.rs`, inside `mod tests`, add:

```rust
    use crate::fakes::FakeSegmenter;
    use crate::ports::SpeechSegment;

    /// One second of audio, so a tick has something to slice.
    fn engine_with_segmenter(
        transcript: &str,
        spans: Vec<SpeechSegment>,
    ) -> (Engine, Arc<FakeSink>) {
        let (engine, sink) = engine(transcript);
        (
            engine.with_segmenter(Arc::new(FakeSegmenter::returning(spans))),
            sink,
        )
    }

    #[test]
    fn a_tick_previews_text_while_recording() {
        let (engine, _sink) = engine_with_segmenter("hello there", vec![SpeechSegment {
            start: 0.0,
            end: 0.9,
        }]);
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
        let engine = engine.with_segmenter(Arc::new(FakeSegmenter::silent()));
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
        // Speech ends well before the buffer does, so the tick commits.
        let (engine, _sink) = engine_with_segmenter("first part", vec![SpeechSegment {
            start: 0.0,
            end: 0.2,
        }]);
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
        let engine = engine.with_segmenter(Arc::new(FakeSegmenter::failing(Error::Other(
            "vad exploded".to_string(),
        ))));
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

    #[test]
    fn a_preview_does_not_survive_a_cancel() {
        let (engine, sink) = engine_with_segmenter("abandoned", vec![SpeechSegment {
            start: 0.0,
            end: 0.2,
        }]);
        engine.begin_recording().expect("recording should start");
        engine.preview_tick();

        engine.cancel();

        assert_eq!(engine.state(), State::Idle);
        assert!(sink.injected().is_empty());
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p speakeasy-core --features fakes engine
```

Expected: FAIL — no `with_segmenter`, no `preview_tick`.

- [ ] **Step 3: Add the state**

In `engine.rs`, extend the imports:

```rust
use crate::ports::{AudioSource, HotkeyEvent, ModelFetcher, SpeechSegmenter, TextSink, Transcriber};
use crate::preview::{decide, Tick};
```

Add near the other constants at the top of the file:

```rust
/// How often a preview decode runs while recording.
///
/// A decode costs ~0.31s on the measured hardware, so this is roughly a 20%
/// duty cycle. A constant rather than a setting until it has been lived with.
const PREVIEW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);
```

Add to `struct Inner`:

```rust
    /// `None` disables previews entirely, which is what a shell with no
    /// detector — or a headless test — gets by doing nothing.
    segmenter: Mutex<Option<Arc<dyn SpeechSegmenter>>>,
    preview: Mutex<Preview>,
```

And after `struct Inner`:

```rust
/// Preview progress within one recording.
#[derive(Default)]
struct Preview {
    /// Text from utterances that have finished. Never revised.
    committed: String,
    /// How much of the 16 kHz stream `committed` accounts for.
    committed_samples: usize,
}
```

Initialise both in `Engine::new`'s `Inner { ... }`:

```rust
            segmenter: Mutex::new(None),
            preview: Mutex::new(Preview::default()),
```

- [ ] **Step 4: Add `with_segmenter`**

After `persisting_to`:

```rust
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
```

- [ ] **Step 5: Add the tick**

```rust
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

        let language = self.config().language;
        let (audio, commit_through) = match decide(slice_seconds, &spans) {
            Tick::Skip => return,
            Tick::Live => (slice, None),
            Tick::Commit { through_seconds } => {
                let end = ((through_seconds * crate::TARGET_SAMPLE_RATE as f32) as usize)
                    .min(slice.len());
                (&slice[..end], Some(end))
            }
        };

        let text = match self.inner.transcriber.transcribe(audio, language.as_deref()) {
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
```

- [ ] **Step 6: Start and stop the loop**

In `begin_recording`, immediately before `self.transition(State::Recording);`, reset the preview state:

```rust
        *self.inner.preview.lock().expect("preview mutex poisoned") = Preview::default();
```

And immediately after `self.transition(State::Recording);`:

```rust
        self.spawn_preview_loop();
```

Then add:

```rust
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
```

- [ ] **Step 7: Run the tests**

```bash
cargo test -p speakeasy-core --features fakes
```

Expected: PASS, including every pre-existing engine test.

- [ ] **Step 8: Full checks and commit**

```bash
make check
git add crates/speakeasy-core/src/engine.rs
git commit -m "Preview the transcript while the user is still speaking"
```

---

## Task 9: The whisper segmenter

**Files:**
- Create: `crates/speakeasy-whisper/vendor/ggml-silero-v5.1.2.bin`
- Create: `crates/speakeasy-whisper/src/segmenter.rs`
- Modify: `crates/speakeasy-whisper/src/lib.rs:1-8`

- [ ] **Step 1: Vendor the model**

```bash
mkdir -p crates/speakeasy-whisper/vendor
curl -L -o crates/speakeasy-whisper/vendor/ggml-silero-v5.1.2.bin \
  https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin
shasum -a 256 crates/speakeasy-whisper/vendor/ggml-silero-v5.1.2.bin
```

Expected digest, exactly:

```
29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf
```

**Stop if it does not match.** The file is 885,098 bytes and MIT licensed.

- [ ] **Step 2: Write the failing test**

Create `crates/speakeasy-whisper/src/segmenter.rs`:

```rust
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
    /// Materialise the weights into `models_dir` if absent and open them.
    ///
    /// `WhisperVadContext` takes a path rather than bytes, which is the only
    /// reason this touches the filesystem at all.
    pub fn new(models_dir: &Path) -> Result<Self> {
        let path = Self::materialize(models_dir)?;
        let mut params = WhisperVadContextParams::new();
        params.set_use_gpu(true);

        let path = path.to_str().ok_or_else(|| {
            Error::Other(format!("vad model path is not UTF-8: {}", path.display()))
        })?;
        let context = WhisperVadContext::new(path, params)
            .map_err(|err| Error::Other(format!("could not load the vad model: {err}")))?;

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
        let Ok(wav) = std::env::var("SPEAKEASY_WAV") else {
            panic!("set SPEAKEASY_WAV");
        };
        let mut reader = hound::WavReader::open(&wav).expect("wav should open");
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.expect("sample") as f32 / i16::MAX as f32)
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
}
```

- [ ] **Step 3: Register the module**

In `crates/speakeasy-whisper/src/lib.rs`, after the module doc comment:

```rust
mod segmenter;

pub use segmenter::WhisperSegmenter;
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p speakeasy-whisper segmenter
```

Expected: PASS, two tests (the third is `#[ignore]`d).

`hound` is already a dev-dependency (`crates/speakeasy-whisper/Cargo.toml:15`), so the ignored test compiles without changes.

The `segments_from_samples(&mut self, params, samples)` signature was verified against `whisper-rs-0.16.0/src/whisper_vad.rs:211` while writing this plan, as were `num_segments` (`:253`) and `get_segment` (`:279`). If any disagree, read the source rather than guessing.

- [ ] **Step 4b: Run the unit check against real speech**

Record a few seconds of speech and convert it, per the recipe in
`crates/speakeasy-whisper/tests/transcribe.rs:15`:

```bash
SPEAKEASY_WAV=/tmp/speech.wav cargo test -p speakeasy-whisper -- --ignored
```

Expected: PASS. A failure naming the centisecond conversion means the divide by
100 in `segments` is missing or wrong. **Do not skip this step** — it is the
only automated check on the units, and getting them wrong breaks the feature
silently rather than loudly.

- [ ] **Step 5: Full checks and commit**

```bash
make check
git add crates/speakeasy-whisper/vendor crates/speakeasy-whisper/src/segmenter.rs crates/speakeasy-whisper/src/lib.rs
git commit -m "Detect speech with a bundled Silero model"
```

---

## Task 10: Attach the segmenter in the shell

**Files:**
- Modify: `apps/desktop/src-tauri/src/main.rs`

- [ ] **Step 1: Find where the engine is built**

```bash
grep -n "Engine::new\|persisting_to\|models_dir" apps/desktop/src-tauri/src/main.rs
```

- [ ] **Step 2: Attach it**

Where the engine is constructed, chain the segmenter on. A detector that will not load must not stop the app starting — dictation still works without previews:

```rust
    let engine = match speakeasy_whisper::WhisperSegmenter::new(&models_dir) {
        Ok(segmenter) => engine.with_segmenter(std::sync::Arc::new(segmenter)),
        Err(err) => {
            tracing::warn!(%err, "previews are off: the voice detector would not load");
            engine
        }
    };
```

Place it after `persisting_to` so the chain reads in the order things happen.

- [ ] **Step 3: Build and run**

```bash
cargo build -p speakeasy-desktop
make dev
```

Hold the hotkey and speak for five seconds. Expected in the log: `wrote the bundled vad model` on the first run only, and no warnings.

The widget shows nothing yet — that is Task 11.

- [ ] **Step 4: Full checks and commit**

```bash
make check
git add apps/desktop/src-tauri/src/main.rs
git commit -m "Give the desktop shell a voice detector"
```

---

## Task 11: Show the preview in the widget

**Files:**
- Modify: `apps/desktop/src/engine.ts:69-76`
- Modify: `apps/desktop/src/widget.ts`
- Modify: `apps/desktop/src/styles.css`
- Modify: `apps/desktop/src-tauri/tauri.conf.json:25-26`

- [ ] **Step 1: Mirror the event type**

In `engine.ts`, add to the `EngineEvent` union, after `transcript`:

```ts
  | { type: "transcriptPreview"; text: string }
```

And add to `Config`:

```ts
  transcriptPreview: boolean;
```

- [ ] **Step 2: Render it**

In `widget.ts`, add a preview element to the markup, after the `hint` div:

```html
      <div class="preview"></div>
```

Grab it alongside the others:

```ts
  const preview = root.querySelector<HTMLElement>(".preview")!;
```

In `render`, clear it whenever the state is not recording, since a discarded
short tap or a cancel must not leave text behind:

```ts
    if (!recording) preview.textContent = "";
```

Place that immediately after the existing `if (!recording) for (const bar of bars)` line.

Then handle the event, inside the existing `onEngineEvent` callback, before the
`stateChanged` guard:

```ts
    if (event.type === "transcriptPreview") {
      preview.textContent = event.text;
      // The box is clamped to three lines and the newest words are at the
      // bottom, so it has to be scrolled down to show them.
      preview.scrollTop = preview.scrollHeight;
      return;
    }
```

- [ ] **Step 3: Style it**

In `styles.css`, alongside the other widget rules:

```css
/* Three lines of the most recent text. Clamping alone would show the *first*
   three lines; the tail is what matters, so the box scrolls and widget.ts
   keeps it pinned to the bottom. */
.widget .preview:empty {
  display: none;
}

.widget .preview {
  max-height: calc(3 * 1.35em);
  overflow: hidden;
  font-size: 12px;
  line-height: 1.35;
  opacity: 0.85;
  text-align: left;
}
```

- [ ] **Step 4: Give the window room**

In `tauri.conf.json`, change the widget window's `"height": 112` to `"height": 150`.

- [ ] **Step 5: Verify by hand**

```bash
make dev
```

Hold the hotkey and speak two sentences with a clear pause between them. Expected: text appears within about two seconds; the first sentence stops changing once the second begins; the pill does not resize; releasing the key clears it and inserts the final text.

- [ ] **Step 6: Full checks and commit**

```bash
make check
git add apps/desktop/src/engine.ts apps/desktop/src/widget.ts apps/desktop/src/styles.css apps/desktop/src-tauri/tauri.conf.json
git commit -m "Show the preview transcript in the widget"
```

---

## Task 12: The settings checkbox

**Files:**
- Modify: `apps/desktop/src/settings.ts:145-157`

- [ ] **Step 1: Add a checkbox control**

Near `row()` at the bottom of `settings.ts`:

```ts
/** A labelled checkbox that writes straight back to config. */
function toggleRow(
  label: string,
  checked: boolean,
  onChange: (value: boolean) => void,
): HTMLElement {
  const line = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = label;

  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = checked;
  box.addEventListener("change", () => onChange(box.checked));

  line.append(name, box);
  return line;
}
```

- [ ] **Step 2: Use it**

In `mountDictation`, add to the `panel.append(...)` call after the hotkey row:

```ts
    toggleRow("Live preview", config.transcriptPreview, (value) => {
      void setConfig({ ...config, transcriptPreview: value });
    }),
```

Add `setConfig` to the imports from `./engine` if it is not already there.

- [ ] **Step 3: Verify by hand**

```bash
make dev
```

Open Settings, Dictation. Toggle the checkbox off, dictate, confirm no preview appears and the final text still inserts. Toggle back on, confirm it returns. Reopen settings and confirm the box remembers.

- [ ] **Step 4: Full checks and commit**

```bash
make check
git add apps/desktop/src/settings.ts
git commit -m "Let the live preview be turned off in settings"
```

---

## Task 13: Documentation

**Files:**
- Modify: `README.md:209` and its surrounding section

- [ ] **Step 1: Correct the streaming claim**

`README.md:209` says record-then-transcribe, "not streaming", and that streaming would be a new port. The second half held — `SpeechSegmenter` is a new port and `Transcriber` was untouched — but the first half is now wrong. Rewrite the paragraph to describe what exists: previews via VAD-segmented decoding, display-only, with the final transcription still authoritative.

- [ ] **Step 2: Note the Metal requirement**

Add a line where the build is described: Metal is enabled on macOS and is what makes previews viable — 0.31s per decode against 1.90s on the CPU.

- [ ] **Step 3: Commit**

```bash
make check
git add README.md
git commit -m "Document the live preview"
```

---

## Verification

After every task, by hand, on a Finder-launched build:

- [ ] Text appears while speaking, within roughly two seconds.
- [ ] Text already shown does not visibly rewrite itself.
- [ ] Pausing mid-dictation adds no invented words.
- [ ] The final inserted text is complete and correct.
- [ ] A short accidental tap inserts nothing and leaves no text in the pill.
- [ ] Cancelling from the tray leaves no text and inserts nothing.
- [ ] Toggle mode behaves the same as hold mode.
- [ ] The pill does not steal focus or resize while recording.

## Known gaps

Recorded so they are not mistaken for bugs:

- **Release can be delayed by up to ~0.31s**, because `WhisperTranscriber` holds one mutex for the whole decode (`lib.rs:63`) and a release landing mid-preview waits for it. Still far better than the ~2.0s before Metal. Removable later by giving previews their own decoder state; deliberately not done here.
- **Preview and final text can differ.** Two decodes over overlapping audio sometimes disagree.
- **The live tail still wobbles** within the current utterance. Only committed text is stable. `no_speech_probability()`, which arrives with the 0.16 upgrade, is the next lever if this is worse than expected.
