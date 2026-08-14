# Transcript History and Native Preferences Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the last 100 transcripts in memory so a failed or misdirected
injection is recoverable beyond the single most-recent one, show them in a new
History tab, and restyle the settings window in the System Settings sidebar
idiom (grouped inset cards, system accent color, native-looking switches, tab
glyphs).

**Architecture:** `Engine` replaces its single `last_transcript` slot with a
capped `VecDeque<Transcript>`; `recoverable_transcript()` / `copy_transcript()`
keep their exact signatures by reading the newest entry, so the widget's
rescue row needs no changes. Three new engine methods (`history`,
`copy_history_entry`, `clear_history`) and one new event
(`Event::HistoryChanged`) reach the shell through the usual one-line Tauri
command delegations. The settings window's CSS gets a shared row language
(`.card`, `.grouplabel`, `.switch`) applied to the two existing tabs, then a
third tab is added in the same language.

**Tech Stack:** Rust (`speakeasy-core`, `speakeasy-platform` unaffected),
Tauri 2 IPC, TypeScript (no framework — hand-rolled DOM), vanilla CSS.

## Global Constraints

- `speakeasy-core` must never depend on `tauri` — enforced by `make boundary`.
- Command bodies in `apps/desktop/src-tauri/src/commands.rs` are one-line
  delegations; a command with a branch means logic leaked out of the core.
- Nothing in `speakeasy-core`'s public API uses generics, lifetimes, or
  borrowed returns — flat owned types only, so a future Swift binding stays
  mechanical.
- No disk persistence for history — it lives in memory and dies with the
  process (spec non-goal; dictation catches passwords and half-formed
  thoughts).
- No native `NSSwitch` — the shell is a webview, so the switch is CSS built to
  read as native, not a real AppKit control (spec non-goal).
- `helper` is a banned name in this codebase.
- Prefer Makefile targets (`make test`, `make check`, `make fmt`, `make lint`)
  over ad hoc `cargo`/`npm` invocations for building, linting and formatting.

---

### Task 1: `Transcript` type and capped history buffer in the engine

**Files:**
- Modify: `crates/speakeasy-core/src/event.rs`
- Modify: `crates/speakeasy-core/src/engine.rs:6-20` (imports), `:47-72`
  (`Inner` struct), `:92-105` (`Inner` construction), `:481-518`
  (`finish_recording` tail), `:520-546` (`recoverable_transcript` /
  `copy_transcript`), `:759-787` (tests module, `engine()` helper)
- Modify: `crates/speakeasy-core/src/lib.rs:35`

**Interfaces:**
- Consumes: nothing new — `Error::Other`, `TextSink::copy` already exist.
- Produces: `pub struct Transcript { text: String, at: SystemTime, delivered:
  bool }` (re-exported at crate root), `Event::HistoryChanged`,
  `Engine::history(&self) -> Vec<Transcript>` (newest first). Keeps
  `Engine::recoverable_transcript(&self) -> Option<String>` and
  `Engine::copy_transcript(&self) -> Result<bool>` with unchanged behavior —
  both still describe "the newest entry."

- [ ] **Step 1: Add the `Transcript` type and `HistoryChanged` event**

In `crates/speakeasy-core/src/event.rs`, add near the top (after the existing
`use` lines):

```rust
use std::time::{SystemTime, UNIX_EPOCH};
```

Add this type after the `ModelStatus` enum and before the `Event` enum:

```rust
/// One completed transcription, kept so a failed or misdirected injection is
/// recoverable. In memory only — see the design doc's non-goals for why this
/// never touches disk.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Transcript {
    pub text: String,
    /// Milliseconds since the Unix epoch, not a `SystemTime`: the shell reads
    /// this as a plain number to build a relative time ("2 min ago"), and
    /// serde's built-in `SystemTime` representation is a `{secs, nanos}`
    /// struct no frontend wants to unpack.
    #[serde(serialize_with = "serialize_millis")]
    pub at: SystemTime,
    /// `false` only when `sink.inject` returned `Err` — a genuine failure.
    /// Never set for `State::Unconfirmed`: that detection is a known-wrong
    /// hint for apps that draw their own text (see `6701333`), and badging it
    /// here would mark rows that delivered fine.
    pub delivered: bool,
}

fn serialize_millis<S>(time: &SystemTime, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let millis = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    serializer.serialize_u64(millis)
}
```

Add a variant to the `Event` enum, after `ModelStatusChanged`:

```rust
    /// History gained or lost an entry. Carries nothing — an open History
    /// tab re-reads [`crate::Engine::history`] on receipt rather than
    /// diffing a hundred small structs.
    HistoryChanged,
```

- [ ] **Step 2: Export `Transcript` from the crate root**

In `crates/speakeasy-core/src/lib.rs:35`, change:

```rust
pub use event::{Event, ModelStatus, State};
```

to:

```rust
pub use event::{Event, ModelStatus, State, Transcript};
```

- [ ] **Step 3: Write the failing tests**

In `crates/speakeasy-core/src/engine.rs`, inside `mod tests` (after the
existing `use` lines at the top of the module, around line 762), the tests
below reference `engine.history()`, which does not exist yet — this is
expected to fail to compile. Add these tests after
`recording_refuses_to_start_while_secure_input_is_held` (around line 1352,
before `engine_with_models`):

```rust
    #[test]
    fn a_completed_dictation_is_recorded_as_delivered() {
        let (engine, _sink) = engine("landed safely");

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("injection should work");

        let history = engine.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].text, "landed safely");
        assert!(history[0].delivered);
    }

    #[test]
    fn a_failed_injection_is_recorded_as_not_delivered() {
        let sink = Arc::new(FakeSink::failing(Error::Injection("nope".to_string())));
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Arc::new(FakeTranscriber::returning("lost words")),
            Box::new(sink.clone()),
            Box::new(FakeFetcher::serving(Vec::new())),
            unused_models_dir(),
            Config {
                min_recording_ms: 0,
                ..Config::default()
            },
        );

        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect_err("injection should fail");

        let history = engine.history();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].text, "lost words");
        assert!(!history[0].delivered);
    }

    /// `Unconfirmed` is a hint known to be wrong for apps that draw their own
    /// text — see `6701333`. History must not treat it as a failure.
    #[test]
    fn an_unverified_injection_is_still_recorded_as_delivered() {
        let sink = Arc::new(FakeSink::unverified());
        let engine = Engine::new(
            Box::new(FakeAudio::with_samples(vec![0.0; 16_000])),
            Arc::new(FakeTranscriber::returning("into the void")),
            Box::new(sink.clone()),
            Box::new(FakeFetcher::serving(Vec::new())),
            unused_models_dir(),
            Config {
                min_recording_ms: 0,
                ..Config::default()
            },
        );

        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect("injection should not fail");

        let history = engine.history();
        assert_eq!(history.len(), 1);
        assert!(history[0].delivered);
    }

    #[test]
    fn an_empty_transcript_is_not_recorded() {
        let (engine, _sink) = engine("   ");

        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect("an empty transcript is not an error");

        assert!(engine.history().is_empty());
    }

    #[test]
    fn history_returns_newest_first() {
        let (engine, _sink) = engine("said twice");

        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect("first injection should work");
        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect("second injection should work");

        let history = engine.history();
        assert_eq!(history.len(), 2);
        assert!(
            history[0].at >= history[1].at,
            "the most recently recorded entry should be first"
        );
    }

    #[test]
    fn history_caps_at_100_and_drops_the_oldest() {
        let (engine, _sink) = engine("filler");

        for _ in 0..105 {
            engine.begin_recording().expect("recording should start");
            engine.finish_recording().expect("injection should work");
        }

        assert_eq!(engine.history().len(), 100);
    }

    /// Regression coverage for replacing `last_transcript` with the buffer:
    /// the rescue row's two methods must still describe the newest entry.
    #[test]
    fn recoverable_transcript_and_copy_transcript_still_track_the_newest_entry() {
        let (engine, sink) = engine("the newest one");

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("injection should work");

        assert_eq!(
            engine.recoverable_transcript(),
            Some("the newest one".to_string())
        );
        assert!(engine.copy_transcript().expect("copying should work"));
        assert_eq!(sink.copied(), vec!["the newest one".to_string()]);
    }
```

- [ ] **Step 4: Run the tests to confirm they fail to compile**

Run: `cargo test -p speakeasy-core history`
Expected: compile error — `no method named 'history' found for struct
'Engine'` (and similar for the fields/types not yet added).

- [ ] **Step 5: Replace `last_transcript` with the capped buffer**

In `crates/speakeasy-core/src/engine.rs:6-20`, change the imports:

```rust
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use std::path::PathBuf;

use crate::config::{ActivationMode, Config, Hotkey, ModelChoice, WidgetPosition};
use crate::error::{Error, Result};
use crate::event::{Event, ModelStatus, State, Transcript};
use crate::models::catalog::{self, ModelId, ModelInfo, CATALOG};
use crate::models::ModelManager;
use crate::ports::{
    AudioSource, HotkeyEvent, ModelFetcher, SpeechSegmenter, TextSink, Transcriber,
};
use crate::preview::{decide, Tick};
```

Add a constant near the top of the file, after the existing `PREVIEW_INTERVAL`
constant:

```rust
/// How many transcripts [`Engine::history`] keeps before dropping the
/// oldest. Transcripts are small and nobody scrolls past a hundred.
const HISTORY_CAP: usize = 100;
```

In the `Inner` struct (`:47-72`), replace:

```rust
    /// The last transcript that reached the injection step, kept so the user
    /// can still retrieve it when delivery went wrong.
    ///
    /// Held until the next recording rather than cleared on use: a transcript
    /// that failed to arrive is the one thing in this app that cannot be
    /// reproduced, and a Copy button that works exactly once is a trap.
    last_transcript: Mutex<Option<String>>,
```

with:

```rust
    /// Completed transcriptions, most recent last. Capped at
    /// [`HISTORY_CAP`] so a long-running session's memory use doesn't grow
    /// without bound.
    ///
    /// Held rather than cleared on use: a transcript that failed to arrive is
    /// the one thing in this app that cannot be reproduced, and a Copy button
    /// that works exactly once is a trap. Never persisted — see the design
    /// doc's non-goals.
    history: Mutex<VecDeque<Transcript>>,
```

In `Engine::new`'s `Inner` construction (`:92-105`), replace:

```rust
            last_transcript: Mutex::new(None),
```

with:

```rust
            history: Mutex::new(VecDeque::new()),
```

- [ ] **Step 6: Wire recording into the buffer**

In `crates/speakeasy-core/src/engine.rs`, in `finish_recording` (around
`:481-518`), replace:

```rust
        self.emit(Event::Transcript { text: text.clone() });
        // Recorded before the attempt, not after a failure: the failure path
        // returns early in several places, and a transcript saved only on the
        // branch someone remembered is a transcript that goes missing.
        *self
            .inner
            .last_transcript
            .lock()
            .expect("last_transcript mutex poisoned") = Some(text.clone());

        self.transition(State::Injecting);
        match self.inner.sink.inject(&text) {
            Ok(delivery) => {
                self.emit(Event::Injected {
                    chars: delivery.chars,
                });
                // Unverified does not mean failed, and the text is not resent:
                // it was posted either way, and posting twice into an app that
                // did receive it would duplicate the user's words. All this
                // changes is that the widget stays up with the transcript in
                // reach rather than dropping to `Idle` and taking it away.
                self.transition(if delivery.verified {
                    State::Idle
                } else {
                    State::Unconfirmed
                });
                Ok(())
            }
            Err(err) => {
                self.fail(err.clone());
                Err(err)
            }
        }
    }
```

with:

```rust
        self.emit(Event::Transcript { text: text.clone() });
        // Recorded before the attempt, not after a failure: the failure path
        // returns early in several places, and a transcript saved only on the
        // branch someone remembered is a transcript that goes missing.
        // `delivered` starts `true` — matching the assumption injection
        // works — and is flipped in the `Err` arm below, the one case any of
        // this can be sure about.
        self.push_history(Transcript {
            text: text.clone(),
            at: SystemTime::now(),
            delivered: true,
        });

        self.transition(State::Injecting);
        match self.inner.sink.inject(&text) {
            Ok(delivery) => {
                self.emit(Event::Injected {
                    chars: delivery.chars,
                });
                self.emit(Event::HistoryChanged);
                // Unverified does not mean failed, and the text is not resent:
                // it was posted either way, and posting twice into an app that
                // did receive it would duplicate the user's words. All this
                // changes is that the widget stays up with the transcript in
                // reach rather than dropping to `Idle` and taking it away.
                self.transition(if delivery.verified {
                    State::Idle
                } else {
                    State::Unconfirmed
                });
                Ok(())
            }
            Err(err) => {
                self.mark_last_undelivered();
                self.emit(Event::HistoryChanged);
                self.fail(err.clone());
                Err(err)
            }
        }
    }

    /// Appends to history, dropping the oldest entry once past
    /// [`HISTORY_CAP`].
    fn push_history(&self, transcript: Transcript) {
        let mut history = self.inner.history.lock().expect("history mutex poisoned");
        history.push_back(transcript);
        if history.len() > HISTORY_CAP {
            history.pop_front();
        }
    }

    /// Flips the most recently recorded entry to undelivered. Called only
    /// from the `sink.inject` error arm, immediately after the entry for
    /// this same dictation was pushed — so `back_mut` always finds it.
    fn mark_last_undelivered(&self) {
        let mut history = self.inner.history.lock().expect("history mutex poisoned");
        if let Some(last) = history.back_mut() {
            last.delivered = false;
        }
    }
```

- [ ] **Step 7: Update `recoverable_transcript` / `copy_transcript` and add `history`**

In `crates/speakeasy-core/src/engine.rs` (around `:520-546`), replace:

```rust
    /// Abandon an in-flight recording without transcribing it.
    /// The last transcript that reached the injection step, if any.
    ///
    /// What the widget asks to decide whether it has anything worth offering a
    /// Copy button for. `None` means there is nothing to recover — a failure
    /// before the microphone opened, say, where a Copy button would put an
    /// empty string on the clipboard.
    pub fn recoverable_transcript(&self) -> Option<String> {
        self.inner
            .last_transcript
            .lock()
            .expect("last_transcript mutex poisoned")
            .clone()
    }

    /// Put the last transcript on the clipboard. `false` if there was none.
    ///
    /// The way out when injection could not deliver: the clipboard needs
    /// nothing focused and survives secure input, so it works in every case
    /// [`TextSink::inject`] does not.
    pub fn copy_transcript(&self) -> Result<bool> {
        let Some(text) = self.recoverable_transcript() else {
            return Ok(false);
        };
        self.inner.sink.copy(&text)?;
        Ok(true)
    }
```

with:

```rust
    /// Abandon an in-flight recording without transcribing it.
    /// The newest entry in history, if any.
    ///
    /// What the widget asks to decide whether it has anything worth offering a
    /// Copy button for. `None` means there is nothing to recover — a failure
    /// before the microphone opened, say, where a Copy button would put an
    /// empty string on the clipboard.
    pub fn recoverable_transcript(&self) -> Option<String> {
        self.inner
            .history
            .lock()
            .expect("history mutex poisoned")
            .back()
            .map(|transcript| transcript.text.clone())
    }

    /// Put the newest transcript on the clipboard. `false` if there was none.
    ///
    /// The way out when injection could not deliver: the clipboard needs
    /// nothing focused and survives secure input, so it works in every case
    /// [`TextSink::inject`] does not.
    pub fn copy_transcript(&self) -> Result<bool> {
        let Some(text) = self.recoverable_transcript() else {
            return Ok(false);
        };
        self.inner.sink.copy(&text)?;
        Ok(true)
    }

    /// Every kept transcript, newest first.
    pub fn history(&self) -> Vec<Transcript> {
        self.inner
            .history
            .lock()
            .expect("history mutex poisoned")
            .iter()
            .rev()
            .cloned()
            .collect()
    }
```

(The stray `/// Abandon an in-flight recording without transcribing it.` line
above `recoverable_transcript` is a pre-existing doc-comment mismatch in the
file — leave it exactly as found; it belongs to `cancel()` below and is not
introduced or fixed by this task.)

- [ ] **Step 8: Run the tests to confirm they pass**

Run: `cargo test -p speakeasy-core`
Expected: PASS — every test in `crates/speakeasy-core/src/engine.rs`,
including the six new ones and the four pre-existing rescue-row tests
(`the_transcript_survives_being_copied`,
`a_failure_with_no_transcript_has_nothing_to_recover`,
`an_unverified_injection_keeps_the_widget_up`,
`a_verified_injection_returns_to_idle`), which needed no changes.

- [ ] **Step 9: Commit**

```bash
git add crates/speakeasy-core/src/event.rs crates/speakeasy-core/src/engine.rs crates/speakeasy-core/src/lib.rs
git commit -m "$(cat <<'EOF'
Keep more than one transcript in reach

Replaces the single last_transcript slot with a capped, newest-last
buffer. recoverable_transcript() and copy_transcript() keep their exact
behavior — both still mean "the newest entry" — so the widget's rescue
row needs no changes. Unconfirmed deliveries are recorded as delivered:
that detection is known wrong for apps that draw their own text, and a
History tab that badged it would cry wolf on rows that arrived fine.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `copy_history_entry` and `clear_history`

**Files:**
- Modify: `crates/speakeasy-core/src/engine.rs` (add methods after
  `history()`, add tests to `mod tests`)

**Interfaces:**
- Consumes: `Transcript`, `Engine::history()`, `Inner::history` from Task 1.
- Produces: `Engine::copy_history_entry(&self, index: usize) -> Result<()>`,
  `Engine::clear_history(&self)`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/speakeasy-core/src/engine.rs`'s `mod tests`, after
`history_caps_at_100_and_drops_the_oldest`:

```rust
    #[test]
    fn copy_history_entry_copies_the_right_text() {
        let (engine, sink) = engine("said twice");

        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect("first injection should work");
        engine.begin_recording().expect("recording should start");
        engine
            .finish_recording()
            .expect("second injection should work");

        // history() is newest first; index 1 is the older of the two.
        engine
            .copy_history_entry(1)
            .expect("copying an in-range entry should work");

        assert_eq!(
            sink.copied(),
            vec!["said twice".to_string()],
            "the entry at index 1 should have been copied, not the newest one"
        );
    }

    #[test]
    fn copy_history_entry_out_of_range_errors_rather_than_panics() {
        let (engine, _sink) = engine("only one");

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("injection should work");

        assert!(engine.copy_history_entry(5).is_err());
    }

    #[test]
    fn clear_history_empties_it() {
        let (engine, _sink) = engine("gone soon");

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("injection should work");
        assert_eq!(engine.history().len(), 1);

        engine.clear_history();

        assert!(engine.history().is_empty());
    }

    #[test]
    fn subscribers_see_history_changed() {
        let (engine, _sink) = engine("noticed");
        let events = engine.subscribe();

        engine.begin_recording().expect("recording should start");
        engine.finish_recording().expect("injection should work");

        assert!(events
            .try_iter()
            .any(|event| matches!(event, Event::HistoryChanged)));
    }
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test -p speakeasy-core copy_history_entry`
Expected: compile error — `no method named 'copy_history_entry' found for
struct 'Engine'`.

- [ ] **Step 3: Implement the methods**

In `crates/speakeasy-core/src/engine.rs`, add after the `history()` method
from Task 1:

```rust
    /// Put the transcript at `index` — into the newest-first list from
    /// [`Engine::history`] — on the clipboard.
    ///
    /// Takes an index rather than the text itself so the shell cannot put
    /// something on the clipboard the engine never produced.
    pub fn copy_history_entry(&self, index: usize) -> Result<()> {
        let text = {
            let history = self.inner.history.lock().expect("history mutex poisoned");
            history
                .iter()
                .rev()
                .nth(index)
                .map(|transcript| transcript.text.clone())
                .ok_or_else(|| Error::Other(format!("no history entry at index {index}")))?
        };
        self.inner.sink.copy(&text)
    }

    /// Empties history. Does not affect the current dictation cycle.
    pub fn clear_history(&self) {
        self.inner
            .history
            .lock()
            .expect("history mutex poisoned")
            .clear();
        self.emit(Event::HistoryChanged);
    }
```

- [ ] **Step 4: Run the tests to confirm they pass**

Run: `cargo test -p speakeasy-core`
Expected: PASS — all tests, including the four new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/speakeasy-core/src/engine.rs
git commit -m "$(cat <<'EOF'
Let the shell copy and clear history

copy_history_entry takes an index into the newest-first list rather
than text, so the shell cannot put something on the clipboard the
engine never produced. clear_history and every recorded dictation now
emit HistoryChanged, so an open History tab can stay live instead of
only updating on reopen.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: IPC commands and TypeScript bindings

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs:8` (import), add commands
  after `copy_transcript` (`:46-49`)
- Modify: `apps/desktop/src-tauri/src/main.rs:167-188` (`invoke_handler`)
- Modify: `apps/desktop/src/engine.ts:77-86` (`EngineEvent`), add `Transcript`
  type and IPC wrappers near `:263-267`

**Interfaces:**
- Consumes: `Engine::history`, `Engine::copy_history_entry`,
  `Engine::clear_history`, `Event::HistoryChanged`, `Transcript` from Tasks 1–2.
- Produces: Tauri commands `history`, `clear_history`, `copy_history_entry`;
  TS `export type Transcript = { text: string; at: number; delivered: boolean
  }`, `getHistory(): Promise<Transcript[]>`, `clearHistory(): Promise<void>`,
  `copyHistoryEntry(index: number): Promise<void>`, and `EngineEvent` variant
  `{ type: "historyChanged" }`.

- [ ] **Step 1: Add the Tauri commands**

In `apps/desktop/src-tauri/src/commands.rs:8`, change:

```rust
use speakeasy_core::{Config, Engine, Hotkey, ModelChoice, ModelId, ModelInfo, ModelStatus, State};
```

to:

```rust
use speakeasy_core::{
    Config, Engine, Hotkey, ModelChoice, ModelId, ModelInfo, ModelStatus, State, Transcript,
};
```

After `copy_transcript` (`:46-49`), add:

```rust
#[tauri::command]
pub fn history(engine: TauriState<'_, Engine>) -> Vec<Transcript> {
    engine.history()
}

#[tauri::command]
pub fn clear_history(engine: TauriState<'_, Engine>) {
    engine.clear_history();
}

#[tauri::command]
pub fn copy_history_entry(engine: TauriState<'_, Engine>, index: usize) -> Result<(), String> {
    engine
        .copy_history_entry(index)
        .map_err(|err| err.to_string())
}
```

- [ ] **Step 2: Register the commands**

In `apps/desktop/src-tauri/src/main.rs:167-188`, after `commands::copy_transcript,`
add:

```rust
            commands::history,
            commands::clear_history,
            commands::copy_history_entry,
```

- [ ] **Step 3: Build to confirm the Rust side compiles**

Run: `make build`
Expected: builds cleanly.

- [ ] **Step 4: Add the TypeScript type and bindings**

In `apps/desktop/src/engine.ts`, add after the `ModelStatus` type
(`:111-115`):

```ts
/**
 * One completed transcription. See `core::event::Transcript`.
 */
export type Transcript = {
  text: string;
  /** Milliseconds since the Unix epoch. */
  at: number;
  /** `false` only for a genuine injection failure — see `errorLabel`. */
  delivered: boolean;
};
```

In `EngineEvent` (`:77-86`), add a variant after `modelStatusChanged`:

```ts
export type EngineEvent =
  | { type: "stateChanged"; state: State }
  | { type: "audioLevel"; level: number }
  | { type: "transcript"; text: string }
  /** Advisory text while the user is still speaking. Never what gets injected. */
  | { type: "transcriptPreview"; text: string }
  | { type: "injected"; chars: number }
  | { type: "failed"; error: EngineError }
  | { type: "hotkeyChanged"; hotkey: Hotkey }
  | { type: "modelStatusChanged"; status: ModelStatus }
  | { type: "historyChanged" };
```

After `copyTranscript` (`:266-267`), add:

```ts
/** Every kept transcript, newest first. */
export const getHistory = () => invoke<Transcript[]>("history");

/** Empties history. */
export const clearHistory = () => invoke<void>("clear_history");

/** Put the entry at `index` — into the newest-first list from `getHistory` — on the clipboard. */
export const copyHistoryEntry = (index: number) =>
  invoke<void>("copy_history_entry", { index });
```

- [ ] **Step 5: Typecheck**

Run: `cd apps/desktop && npm run build`
Expected: `tsc --noEmit` and the Vite build both succeed with no errors.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src-tauri/src/main.rs apps/desktop/src/engine.ts
git commit -m "$(cat <<'EOF'
Expose history over IPC

Three one-line command delegations plus their TypeScript bindings.
copy_history_entry keeps taking an index for the same reason the core
method does — the shell should not be able to hand back text the
engine never produced.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Card row language, switch, and system accent color

**Files:**
- Modify: `apps/desktop/src/styles.css:24-187` (the whole settings section)
- Modify: `apps/desktop/src/settings.ts:146-161` (`mountDictation`),
  `:236-302` (`mountModels`, `localFileRow`), `:393-410` (`toggleRow`)

**Interfaces:**
- Consumes: nothing new.
- Produces: CSS classes `.card` and `.grouplabel`, usable by any tab; the
  `.switch` input styling, reused wherever `toggleRow` is called; a solid
  `AccentColor`-tinted selected-tab state. Task 6's History tab depends on
  `.card` and `.grouplabel` existing.

- [ ] **Step 1: Replace the settings CSS section**

In `apps/desktop/src/styles.css`, replace the entire block from `/* ---
settings ------------------------------------------------------- */`
(`:24`) through the end of the "read-only settings" section, i.e. the `.note`
rule (`:183-187`), with:

```css
/* --- settings ------------------------------------------------------- */

/* Two columns, with the banner spanning both. A missing Accessibility grant is
   the likeliest reason a fresh install does nothing at all, so it must not end
   up narrower than the pane it is warning about — or worse, behind a tab. */
.settings {
  display: grid;
  grid-template-columns: auto 1fr;
  align-items: start;
  gap: 0 20px;
  padding: 16px 20px 24px;
  font-size: 13px;
  line-height: 1.6;
}

.settings > .banner {
  grid-column: 1 / -1;
}

.settings button {
  padding: 5px 11px;
  border: 0;
  border-radius: 6px;
  background: AccentColor;
  color: AccentColorText;
  font-size: 12px;
}

.settings button:disabled {
  background: transparent;
  color: var(--muted);
}

.settings button.secondary {
  background: transparent;
  color: var(--danger);
}

/* Only present when a permission is missing, so it must read as a problem
   without shouting at someone who has already granted everything. */
.banner:not(:empty) {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 16px;
  padding: 10px 12px;
  border-radius: 8px;
  background: color-mix(in srgb, var(--danger) 12%, transparent);
}

.banner span {
  flex: 1;
}

.tabs {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 128px;
  border-right: 1px solid color-mix(in srgb, var(--fg) 12%, transparent);
  padding-right: 12px;
}

.settings .tab {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 10px;
  border-radius: 6px;
  background: transparent;
  color: var(--muted);
  font-size: 13px;
  text-align: left;
}

/* Solid, not tinted: this is what System Settings' own selected sidebar row
   looks like, and it reads clearly against a narrow, text-heavy list. */
.settings .tab[aria-selected="true"] {
  color: AccentColorText;
  background: AccentColor;
}

.panel {
  min-width: 0;
}

/* --- grouped cards ----------------------------------------------------
   The row language every tab shares: a raised card per group of settings,
   rows separated by a hairline instead of each owning its own border, and a
   small uppercase label above any card that isn't a tab's only group. */

.card {
  background: color-mix(in srgb, var(--fg) 3%, var(--bg));
  border-radius: 8px;
  overflow: hidden;
  box-shadow: 0 0 0 0.5px color-mix(in srgb, var(--fg) 12%, transparent);
  margin-bottom: 12px;
}

.card > * {
  padding: 8px 12px;
}

.card > * + * {
  box-shadow: inset 0 0.5px 0 color-mix(in srgb, var(--fg) 10%, transparent);
}

.grouplabel {
  margin: 14px 2px 5px;
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.02em;
  text-transform: uppercase;
  color: var(--muted);
}

.grouplabel:first-child {
  margin-top: 0;
}

/* --- dictation ------------------------------------------------------- */

.setting {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
}

/* This row has three children rather than two, and `space-between` would
   leave the value marooned in the middle. Pinning the label keeps the value
   beside the button it belongs to, and the tighter gap suits three items
   better than the two-item default. */
.setting-block .setting .setting-label {
  margin-right: auto;
}

.setting-block .setting {
  gap: 12px;
}

.setting-error:not(:empty) {
  padding-top: 4px;
  color: var(--danger);
  font-size: 12px;
}

/* A CSS switch standing in for `NSSwitch` — the shell is a webview, so there
   is no real one available. `AccentColor` keeps it tied to the system's own
   accent rather than a color picked for this app. */
.switch {
  appearance: none;
  flex: none;
  width: 30px;
  height: 18px;
  margin: 0;
  border-radius: 9px;
  background: color-mix(in srgb, var(--fg) 18%, transparent);
  position: relative;
  cursor: pointer;
  transition: background 120ms ease;
}

.switch::before {
  content: "";
  position: absolute;
  top: 1.5px;
  left: 1.5px;
  width: 15px;
  height: 15px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.25);
  transition: transform 120ms ease;
}

.switch:checked {
  background: AccentColor;
}

.switch:checked::before {
  transform: translateX(12px);
}

/* --- models ---------------------------------------------------------- */

.model {
  display: grid;
  grid-template-columns: 1fr auto auto;
  align-items: center;
  gap: 4px 8px;
}

.model-label {
  grid-column: 1;
  font-weight: 500;
}

.model-note {
  grid-column: 1;
  grid-row: 2;
  color: var(--muted);
  font-size: 12px;
}

.model-error {
  grid-column: 1;
  grid-row: 2;
  color: var(--danger);
  font-size: 12px;
}

.model progress {
  grid-row: 1 / span 2;
  width: 120px;
}

/* A local path can be long; it must not push the button off the row. */
.model.local .model-note {
  overflow-wrap: anywhere;
}

/* --- read-only settings ---------------------------------------------- */

.setting-label {
  color: var(--muted);
}

.note {
  margin-top: 16px;
  color: var(--muted);
  font-size: 12px;
}
```

This drops the old per-row `border-bottom` on `.model` and the accent
box-shadow on the selected tab, both superseded by `.card`'s hairline
dividers and the solid selected-tab background above.

- [ ] **Step 2: Wrap Dictation's rows in cards**

In `apps/desktop/src/settings.ts`, replace `mountDictation` (`:146-161`):

```ts
async function mountDictation(panel: HTMLElement): Promise<void> {
  const config = await getConfig();

  panel.append(
    await hotkeyRow(config),
    toggleRow("Live preview", config.transcriptPreview, (value) => {
      void setConfig({ ...config, transcriptPreview: value });
    }),
    row("Insertion", config.injectionMode === "paste" ? "Paste" : "Type each character"),
    row("Language", config.language ?? "Auto-detect"),
  );

  const note = element("p", "note");
  note.textContent = "Insertion and language are not editable yet. Edit config.json to change them.";
  panel.append(note);
}
```

with:

```ts
async function mountDictation(panel: HTMLElement): Promise<void> {
  const config = await getConfig();

  const primary = element("div", "card");
  primary.append(
    await hotkeyRow(config),
    toggleRow("Live preview", config.transcriptPreview, (value) => {
      void setConfig({ ...config, transcriptPreview: value });
    }),
  );

  const groupLabel = element("div", "grouplabel");
  groupLabel.textContent = "Text insertion";

  const secondary = element("div", "card");
  secondary.append(
    row("Insertion", config.injectionMode === "paste" ? "Paste" : "Type each character"),
    row("Language", config.language ?? "Auto-detect"),
  );

  panel.append(primary, groupLabel, secondary);

  const note = element("p", "note");
  note.textContent = "Insertion and language are not editable yet. Edit config.json to change them.";
  panel.append(note);
}
```

- [ ] **Step 3: Wrap Models' rows in cards**

In `apps/desktop/src/settings.ts`'s `mountModels` (`:236-253`), replace:

```ts
  const state = { config, installed: new Set(installed), status };
  const list = element("div", "models");
  const rows = new Map<ModelId, HTMLElement>();

  for (const info of catalog) {
    const row = element("div", "model");
    rows.set(info.id, row);
    list.append(row);
  }
  panel.append(list, localFileRow());
```

with:

```ts
  const state = { config, installed: new Set(installed), status };
  const list = element("div", "models card");
  const rows = new Map<ModelId, HTMLElement>();

  for (const info of catalog) {
    const row = element("div", "model");
    rows.set(info.id, row);
    list.append(row);
  }

  const localCard = element("div", "card");
  localCard.append(localFileRow());
  panel.append(list, localCard);
```

(`localFileRow` itself, at `:272-301`, is unchanged — it keeps its `"model
local"` class and is now the sole child of `localCard`, which is why it needs
no divider styling of its own.)

- [ ] **Step 4: Style the toggle as a switch**

In `apps/desktop/src/settings.ts`'s `toggleRow` (`:393-410`), change:

```ts
  const box = document.createElement("input");
  box.type = "checkbox";
  box.checked = checked;
```

to:

```ts
  const box = document.createElement("input");
  box.type = "checkbox";
  box.className = "switch";
  box.checked = checked;
```

- [ ] **Step 5: Typecheck**

Run: `cd apps/desktop && npm run build`
Expected: succeeds with no errors.

- [ ] **Step 6: Manually verify the visual change**

Run: `make dev`

In the settings window: confirm the Dictation tab shows two grouped cards
(hotkey + live preview; insertion + language) with a "TEXT INSERTION" label
between them, the live preview toggle renders as a sliding switch tinted with
your system's accent color when on, and the selected tab in the left rail is
filled with that same accent color. Confirm the Models tab's catalog list sits
in one card and "Use a local file…" sits in its own card below it. Toggle
System Settings' appearance between light and dark and confirm both tabs
still read correctly. Close the app when done (`Cmd+Q` from the tray, or however
`make dev` is normally stopped).

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/styles.css apps/desktop/src/settings.ts
git commit -m "$(cat <<'EOF'
Give the settings window a System Settings row language

Grouped inset cards with hairline dividers instead of bare rows, the
CSS AccentColor/AccentColorText system colors instead of a hardcoded
indigo, and a switch instead of a bare checkbox. Applied to the two
existing tabs so History lands in the same language rather than
needing a second pass.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Tab glyphs

**Files:**
- Modify: `apps/desktop/src/settings.ts:42-57` (`Tab` type, `TABS`),
  `:91-98` (tab button rendering)
- Modify: `apps/desktop/src/styles.css` (append after the `.tabs`/`.tab`
  rules added in Task 4)

**Interfaces:**
- Consumes: `.settings .tab` layout from Task 4.
- Produces: `Tab.icon: string` field. Task 6's new `TABS` entry for History
  must supply one.

- [ ] **Step 1: Add icon sizing and color CSS**

In `apps/desktop/src/styles.css`, immediately after the
`.settings .tab[aria-selected="true"]` rule added in Task 4, add:

```css
.settings .tab svg {
  flex: none;
  width: 16px;
  height: 16px;
  color: var(--muted);
}

.settings .tab[aria-selected="true"] svg {
  color: AccentColorText;
}
```

- [ ] **Step 2: Give `Tab` an `icon` field and add glyphs**

In `apps/desktop/src/settings.ts:42-57`, replace:

```ts
type Tab = {
  id: string;
  label: string;
  mount: (panel: HTMLElement) => void | Promise<void>;
};

/**
 * Adding a tab is one entry here plus its mount function.
 *
 * Data-driven so the shell never has to know what the tabs are — which is what
 * keeps this file from growing a branch per pane.
 */
const TABS: Tab[] = [
  { id: "models", label: "Models", mount: mountModels },
  { id: "dictation", label: "Dictation", mount: mountDictation },
];
```

with:

```ts
type Tab = {
  id: string;
  label: string;
  /**
   * Raw SVG markup, 16x16, single `currentColor` stroke. No icon asset
   * pipeline exists yet, so glyphs are inline rather than imported SF
   * Symbols; every value here is authored by us, never user input, so
   * setting it via `innerHTML` is safe.
   */
  icon: string;
  mount: (panel: HTMLElement) => void | Promise<void>;
};

/**
 * Adding a tab is one entry here plus its mount function.
 *
 * Data-driven so the shell never has to know what the tabs are — which is what
 * keeps this file from growing a branch per pane.
 */
const TABS: Tab[] = [
  {
    id: "models",
    label: "Models",
    icon: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><rect x="2" y="2" width="5" height="5" rx="1"/><rect x="9" y="2" width="5" height="5" rx="1"/><rect x="2" y="9" width="5" height="5" rx="1"/><rect x="9" y="9" width="5" height="5" rx="1"/></svg>`,
    mount: mountModels,
  },
  {
    id: "dictation",
    label: "Dictation",
    icon: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M8 2a2 2 0 0 1 2 2v4a2 2 0 1 1-4 0V4a2 2 0 0 1 2-2Z"/><path d="M4 8a4 4 0 0 0 8 0"/><path d="M8 12v2"/></svg>`,
    mount: mountDictation,
  },
];
```

- [ ] **Step 3: Render the glyph in each tab button**

In `apps/desktop/src/settings.ts:91-98`, replace:

```ts
  const entries = TABS.map((tab) => {
    const button = element("button", "tab");
    button.textContent = tab.label;
    button.setAttribute("role", "tab");
    button.addEventListener("click", () => void show(tab));
    return { tab, button };
  });
```

with:

```ts
  const entries = TABS.map((tab) => {
    const button = element("button", "tab");
    button.innerHTML = tab.icon;
    const text = document.createElement("span");
    text.textContent = tab.label;
    button.append(text);
    button.setAttribute("role", "tab");
    button.addEventListener("click", () => void show(tab));
    return { tab, button };
  });
```

- [ ] **Step 4: Typecheck**

Run: `cd apps/desktop && npm run build`
Expected: succeeds with no errors.

- [ ] **Step 5: Manually verify**

Run: `make dev`. Confirm both tabs in the left rail now show a small glyph
before their label, muted when unselected and tinted `AccentColorText` when
selected.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/settings.ts apps/desktop/src/styles.css
git commit -m "$(cat <<'EOF'
Give settings tabs a glyph

Inline SVG rather than an icon asset pipeline, matching System
Settings' single-color glyph style. Tab gains an icon field so the
next tab — History — carries its own glyph the same way.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: History tab

**Files:**
- Modify: `apps/desktop/src/engine.ts` (add `relativeTime` near
  `formatBytes`, `:293-299`)
- Modify: `apps/desktop/src/settings.ts` (imports at `:15-40`; add
  `mountHistory` and `historyRow`; add a `TABS` entry)
- Modify: `apps/desktop/src/styles.css` (append a history section)

**Interfaces:**
- Consumes: `Transcript`, `getHistory`, `clearHistory`, `copyHistoryEntry`,
  `EngineEvent` variant `historyChanged` from Task 3; `.card`, `.grouplabel`
  from Task 4; `Tab.icon` from Task 5.
- Produces: `mountHistory`, a fourth — no, third — `TABS` entry `{ id:
  "history", ... }`, `relativeTime(atMillis: number): string` in `engine.ts`.

- [ ] **Step 1: Add `relativeTime` to `engine.ts`**

In `apps/desktop/src/engine.ts`, add after `formatBytes` (`:293-299`):

```ts
/**
 * "2 min ago" — coarse on purpose. History entries are at most a few hours
 * old by the time anyone reads this, so a live-updating clock buys nothing.
 */
export function relativeTime(atMillis: number): string {
  const seconds = Math.max(0, Math.round((Date.now() - atMillis) / 1000));
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  return `${hours}h ago`;
}
```

- [ ] **Step 2: Import the new bindings in `settings.ts`**

In `apps/desktop/src/settings.ts:15-40`, the import block reads:

```ts
import {
  bindHotkey,
  cancelHotkeyCapture,
  cancelModelDownload,
  captureHotkey,
  hotkeyLabel,
  errorLabel,
  formatBytes,
  getConfig,
  setConfig,
  getInstalledModels,
  getModelCatalog,
  getModelStatus,
  installModel,
  isSelected,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  removeModel,
  selectModel,
  type Config,
  type Hotkey,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
} from "./engine";
```

Add four value imports after `cancelModelDownload,` and one type import after
`type Config,`, giving:

```ts
import {
  bindHotkey,
  cancelHotkeyCapture,
  cancelModelDownload,
  clearHistory,
  copyHistoryEntry,
  captureHotkey,
  hotkeyLabel,
  errorLabel,
  formatBytes,
  getConfig,
  setConfig,
  getHistory,
  getInstalledModels,
  getModelCatalog,
  getModelStatus,
  installModel,
  isSelected,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  relativeTime,
  removeModel,
  selectModel,
  type Config,
  type Hotkey,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
  type Transcript,
} from "./engine";
```

- [ ] **Step 3: Add `mountHistory` and `historyRow`**

In `apps/desktop/src/settings.ts`, add these functions after `mountModels`
and its helpers (after `renderModel`, before `function button(...)`):

```ts
async function mountHistory(panel: HTMLElement): Promise<void> {
  const list = element("div");
  panel.append(list);

  const draw = async () => {
    const entries = await getHistory();
    list.replaceChildren();

    if (entries.length === 0) {
      const empty = element("p", "note");
      empty.textContent =
        "Recent dictations appear here so a failed or misdirected paste is recoverable. Cleared when Speakeasy quits.";
      list.append(empty);
      return;
    }

    const clearRow = element("div", "history-clear");
    clearRow.append(
      button("Clear", () => {
        void (async () => {
          await clearHistory();
          await draw();
        })();
      }, "secondary"),
    );
    list.append(clearRow);

    const card = element("div", "card");
    entries.forEach((entry, index) => card.append(historyRow(entry, index)));
    list.append(card);
  };

  await draw();

  const unlisten = await onEngineEvent((event) => {
    if (event.type !== "historyChanged") return;
    void draw();
  });
  releasePanel = unlisten;
}

function historyRow(entry: Transcript, index: number): HTMLElement {
  const row = element("div", "history-row");

  const text = element("div", "history-text");
  text.textContent = entry.text;
  text.title = entry.text;

  const meta = element("div", "history-meta");
  const time = element("span");
  time.textContent = relativeTime(entry.at);
  meta.append(time);

  // Only a genuine injection failure is marked. `Unconfirmed` deliveries are
  // recorded as delivered — see `Transcript.delivered` — because that
  // detection is known wrong for apps that draw their own text, and this
  // marker would cry wolf on rows that arrived fine.
  if (!entry.delivered) {
    const marker = element("span", "history-undelivered");
    marker.textContent = "Not delivered";
    meta.append(marker);
  }

  meta.append(button("Copy", () => void copyHistoryEntry(index)));

  row.append(text, meta);
  return row;
}
```

- [ ] **Step 4: Add the History tab**

In `apps/desktop/src/settings.ts`, add a third entry to `TABS` (from Task 5),
after the `dictation` entry:

```ts
  {
    id: "history",
    label: "History",
    icon: `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 4h12M2 8h12M2 12h8"/></svg>`,
    mount: mountHistory,
  },
```

- [ ] **Step 5: Add history row CSS**

In `apps/desktop/src/styles.css`, append after the `.model.local .model-note`
rule (from Task 4's rewrite):

```css
/* --- history ----------------------------------------------------------- */

.history-clear {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 8px;
}

.history-row {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.history-text {
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  overflow-wrap: anywhere;
  font-size: 12.5px;
}

.history-meta {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11px;
  color: var(--muted);
}

.history-meta button {
  margin-left: auto;
}

.history-undelivered {
  color: var(--danger);
  font-weight: 500;
}
```

- [ ] **Step 6: Typecheck**

Run: `cd apps/desktop && npm run build`
Expected: succeeds with no errors.

- [ ] **Step 7: Manually verify**

Run: `make dev`. Open the History tab: with no dictations yet, confirm the
empty-state text renders. If a model is installed, dictate something and
confirm it appears at the top of the list with a relative time and a Copy
button; click Copy and confirm no error appears in the console. Click Clear
and confirm the list returns to the empty state. Confirm the tab's icon and
selected-state styling match the other two tabs.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/engine.ts apps/desktop/src/settings.ts apps/desktop/src/styles.css
git commit -m "$(cat <<'EOF'
Add the History tab

Newest-first, in the card row language the other two tabs already
use. Only a genuine injection failure gets a marker — the Unconfirmed
hint is known wrong for apps that draw their own text, and badging it
here would flag rows that delivered fine. History lives in memory and
clears on quit, same as the buffer behind it.

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: Final verification and documentation

**Files:**
- Modify: `README.md:300-307` (remove the "transcript history... not built"
  line from "Not yet implemented")

**Interfaces:**
- Consumes: everything from Tasks 1–6.
- Produces: nothing new — this task only verifies and documents.

- [ ] **Step 1: Update the README**

In `README.md`'s "Not yet implemented" section (around `:300-307`), remove
this line:

```markdown
- transcript history (designed in
  `docs/superpowers/specs/2026-08-11-transcript-history-design.md`, not built)
```

- [ ] **Step 2: Run the full Rust check**

Run: `make check`
Expected: `fmt-check`, `lint`, `test`, and `boundary` all pass. If `fmt-check`
or `lint` fail on formatting/lint nits introduced by earlier tasks, run `make
fmt` and re-run `make check`.

- [ ] **Step 3: Run the full frontend build**

Run: `cd apps/desktop && npm run build`
Expected: succeeds with no errors.

- [ ] **Step 4: Full manual pass across both themes**

Run: `make dev`. Click through Models, Dictation, and History. Toggle macOS
appearance (System Settings › Appearance) between Light and Dark while the
settings window is open and confirm every card, switch, tab glyph, and the
selected-tab background remain legible in both. Confirm the banner (if
Accessibility is not granted) still spans both columns above the tab rail.

- [ ] **Step 5: Commit the README update**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
Mark transcript history as built

Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>
EOF
)"
```
