# Streaming preview transcripts

Date: 2026-08-11
Status: approved, not yet implemented

Supersedes the work-in-progress options analysis in
`2026-08-11-live-transcript-preview-proposal.md`, which predates the timing
measurements below and reaches a different conclusion. Its factual findings are
carried forward here. See "Relationship to the earlier proposal".

## Problem

Nothing appears until the key is released. The user holds the hotkey, speaks for
ten seconds with no feedback beyond a spectrum animation, and only then learns
whether the microphone was live, whether the model heard them, and what it
thought they said. A dictation that went wrong is discovered after it is over.

The floating widget should show the words as they are recognised, so the user
can see the transcript building while they speak.

## What the measurements say

This design rests on four numbers measured on the target machine (Apple M2,
`base.en`). They are recorded because every earlier attempt to reason about this
feature from first principles reached the wrong answer.

| Configuration | audio_s | elapsed_s |
| --- | --- | --- |
| `base.en`, CPU | 3.73 / 4.46 / 10.15 | 1.97 / 1.96 / 2.07 |
| `tiny`, CPU | 4.29 | 0.329 |
| `base.en`, Metal | 3.77 | 0.309 |

Two conclusions follow, and both are load-bearing.

**Decode cost is fixed, not proportional to audio length.** Least-squares over
the three CPU points gives `1.90s + 0.017s per second of audio`. More than
doubling the audio added 0.1s. whisper.cpp pads every input to a 30-second mel
window, so the encoder — which dominates — does the same work regardless of how
much was actually said. Designs that assume "less audio is cheaper" are wrong;
designs that re-decode a growing buffer are *not* quadratic.

**Metal was never enabled.** `speakeasy-whisper` declares `default = []` and
nothing turned on the `metal` feature, so inference ran on the CPU. Enabling it
takes `base.en` from ~1.90s to ~0.31s, a 6.4x improvement. This is a
prerequisite for the whole feature and is a change worth having on its own
merits: it also cuts the post-release wait from ~2.0s to ~0.3s.

A ~0.31s fixed decode cost is what makes previews affordable at all. At the
pre-Metal 1.90s, no preview design works.

## Goals

- Show recognised text in the widget while the user is still speaking.
- Once text is shown, do not rewrite it. Only the sentence in progress may
  change.
- Keep per-decode cost flat regardless of how long the user talks.
- Never let a preview change, delay materially, or corrupt the injected text.
- Keep `speakeasy-core` free of whisper, of tauri, and of anything not
  expressible across a Swift binding.

## Non-goals

- **Progressive injection.** Typing into the target app as the user speaks is a
  different product with a correction problem attached: it needs an answer for
  "we already pasted a word the final decode rewrote". Previews are display-only.
- **Auto-stop on silence.** VAD makes this nearly free, which is exactly why it
  should be resisted here — it changes when a dictation ends, which today only
  the hotkey does. Separate feature, separate decision.
- **Replacing the final transcription.** `Event::Transcript` keeps its contract
  as the final text before injection (`event.rs:68`).
- **A second, smaller model for previews.** Measured as unnecessary: `base.en`
  on Metal (0.309s) is as fast as `tiny` on CPU (0.329s).

## Approach

A preview loop runs only while recording. Every 1.5s it decodes the audio
captured since the last commit point, and uses voice-activity detection to
decide whether that audio is finished speech — in which case its text is
committed and never revised — or speech still in progress, in which case its
text is shown as the live tail.

```
audio     [======= committed =======][===== uncommitted =====]
                                     ^ only this is decoded, each tick
VAD                                              silence ^
commit    <-------- offset advances to here -------------+
```

Because the uncommitted region is bounded by how long the user speaks without
pausing, decodes stay at ~0.31s indefinitely. The 30-second padding cliff — past
which whisper processes a second window and cost roughly doubles — is never
reached in normal speech, and a guard handles the case where it would be.

### Why VAD rather than an energy gate

An RMS threshold over `AudioSource::level()` would be cheaper and need no
dependency, and it was the first proposal. It fails in two ways that matter.
Background noise keeps RMS high, so the gate never fires where it is most
needed; and whisper hallucinates confidently on non-speech audio, emitting
"Thank you." or similar into the pill during a pause. Distinguishing speech from
noise — not loud from quiet — is the actual requirement.

VAD also does something an energy gate cannot: it identifies where an utterance
*ended*, which is what makes committed text possible. Without it, every preview
is an independent decode of overlapping audio and the displayed text visibly
rewrites itself as whisper revises earlier words. Stable text is the main
user-visible benefit of this design, and VAD is the mechanism.

### The tick algorithm

Per recording the engine holds `committed_text: String` and
`committed_samples: usize`, an offset into the 16 kHz stream. Every 1.5s, while
the state is `Recording`:

1. `samples = audio.snapshot()`, sliced from `committed_samples` to the end.
2. If that slice is shorter than 300ms, return. Nothing useful to decode.
3. `spans = segmenter.segments(&slice)`.
4. If `spans` is empty, the user has not spoken since the last commit. Emit
   nothing and return — this is the silence gate.
5. Let `last` be the final span. If `last.end` is more than `SILENCE_TAIL`
   (500ms) before the end of the slice, the utterance is complete:
   - decode `slice[..last.end + pad]`, append the text to `committed_text`,
     advance `committed_samples`, and emit.
   - Otherwise the user is mid-utterance: decode the whole slice and emit
     `committed_text + live`, leaving the offset alone.
6. If the slice exceeds 25s without a commit, force a commit at `last.end`
   regardless. This is the 30-second cliff guard.

This is **one decode per tick**, not two. The branch decides what the decode
covers, not how many happen. At 1.5s ticks and ~0.31s per decode that is roughly
a 20% GPU duty cycle while speaking.

A commit decode always ends at a silence boundary, which is also where whisper
produces its best output — partial utterances cut mid-word are where greedy
decoding invents endings.

## Components

### `speakeasy-core`: a new port

`README.md:209` and `ports.rs:42-46` both state that streaming arrives as a new
port rather than a change to `Transcriber`. This honours that: `Transcriber` is
untouched.

```rust
/// A span of speech within a buffer, in seconds from its start.
pub struct SpeechSegment {
    pub start: f32,
    pub end: f32,
}

/// Finds speech within captured audio, so a preview can tell a finished
/// utterance from one still in progress.
pub trait SpeechSegmenter: Send + Sync {
    fn segments(&self, samples: &[f32]) -> Result<Vec<SpeechSegment>>;
}
```

Flat, owned, no generics or lifetimes — Swift-expressible, per `lib.rs:8-13`.

Attached with `Engine::with_segmenter(...)`, chained like `persisting_to`
(`engine.rs:120`) rather than growing the already six-argument constructor. No
segmenter means no previews, so every existing test is unaffected and a headless
shell can opt out by doing nothing.

### `AudioSource::snapshot()`

```rust
/// Everything captured so far, at TARGET_SAMPLE_RATE, without ending the
/// recording. Returns an empty vec when not recording.
fn snapshot(&self) -> Result<Vec<f32>>;
```

`spectrum()` (`audio/mod.rs:220-234`) is the precedent: copy out under the
capture lock, release it, then do the expensive work off the audio callback's
path. `stop()` already performs the 16 kHz resample (`audio/mod.rs:210-213`) and
this reuses that path.

Deliberately returns the whole buffer rather than taking an offset. Offsets
would have to be mapped between the device's native rate and 16 kHz, and that
arithmetic is a bug waiting to happen; the engine slices what it needs. 30s of
16 kHz mono f32 is ~1.9 MB per tick, which is negligible beside a 0.31s decode,
and `resample()` building a fresh resampler per call (`audio/resample.rs`) is
likewise noise at this cadence.

### `speakeasy-whisper`: upgrade and implement

**Upgrade `whisper-rs` 0.14 -> 0.16** (`whisper-rs-sys` 0.13 -> 0.15). The
bundled whisper.cpp in 0.14 has no VAD support at all — zero occurrences in
`whisper.h`. 0.16 exposes a standalone `WhisperVadContext` with
`segments_from_samples` and per-segment `start`/`end` timestamps, which is
precisely the port above.

The upgrade is contained to `speakeasy-whisper/src/lib.rs`:

- `state.full_get_segment_text_lossy(i)` becomes
  `state.get_segment(i).to_str_lossy()`.
- `full_n_segments()` no longer returns a `Result`.
- `full()`, `create_state()`, `FullParams`, the sampling strategy and
  `install_logging_hooks` are unchanged.

`WhisperSegment::no_speech_probability()` arrives with the upgrade and is a
direct hallucination signal. Not used in v1; noted because it is the obvious
lever if invented text still appears.

**`WhisperSegmenter`** wraps `WhisperVadContext` with its own context, so VAD
never contends with the transcription mutex.

### Bundling the VAD model

The model is required, not optional, so it must ship with the app rather than
have a first-run download that can fail.

Verified facts:

- Source: `https://huggingface.co/ggml-org/whisper-vad`,
  file `ggml-silero-v5.1.2.bin`
- Size: 885,098 bytes (865 KB)
- sha256: `29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf`
- Licence: MIT

Vendored into the repository and `include_bytes!` into `speakeasy-whisper`,
materialised to `models_dir/ggml-silero-v5.1.2.bin` on first use if absent —
`WhisperVadContext::new` takes a path, not bytes.

Inside the crate that uses it rather than as a Tauri resource, so every shell
gets it without bundler wiring, and reusing `models_dir` (already provided by
the shell) rather than inventing a location. Cost is 865 KB in git and in the
binary.

### `Event::TranscriptPreview { text }`

A new variant carrying the full display string, committed and live joined.
`bridge.rs:74` already forwards every engine event to the webview, so the shell
needs no change — this is how `ModelStatusChanged` was added.

Committed and live are joined rather than sent as separate fields: the widget
renders only the tail, so it has no way to style them differently, and the
stability is real regardless of whether the UI can see the seam. If the widget
later wants to render them distinctly, the variant grows a field.

### Widget

The preview text renders where the stop hint sits, tail-first, up to three
lines, cleared when recording ends. The window stays a fixed size and does not
resize while recording — it must never take focus, and a resizing pill would
fight the drag-to-position logic in `widget.rs`.

The height may need a one-time increase: 112pt is tight for the head, the
spectrum bars and three lines of text. Fixed during a recording is what matters,
not the specific number.

### `Config::transcript_preview: bool`

Default true, with a checkbox in the Dictation pane. Needs `#[serde(default)]`
so existing config files keep loading — the same care the `Hotkey` migration
required.

## Interaction with existing behaviour

- **`min_recording_ms`** (`engine.rs:279`): a recording under the threshold is
  discarded silently. A preview may already have been shown for it. The widget
  clears on the transition out of `Recording`, so the text disappears with the
  discard and nothing is injected.
- **`cancel()`** (`engine.rs:319`): the preview loop observes the state leaving
  `Recording` and exits. Committed text is dropped with the recording.
- **`Event::Transcript`**: unchanged, still the final text before injection.
  A preview never feeds it.
- **Toggle mode**: previews follow the `Recording` state, so they work
  identically in both activation modes with no extra handling.

## Error handling

Previews are strictly best-effort. A VAD failure, a decode failure, or a
snapshot failure is logged at `warn` and the tick skipped; the loop continues.
Nothing in this path can fail a dictation, transition the engine to `Failed`, or
alter the injected text.

A missing or corrupt VAD model disables previews for the session with one
warning, rather than failing startup — dictation must keep working.

## Testing

`FakeSegmenter` in `core::fakes`, returning scripted spans, alongside the
existing `FakeAudio` and `FakeTranscriber`. This matches how `FakeFetcher`
covers the download paths with nothing on disk.

The tick body is a private method called directly by tests, with the timer
thread doing nothing but calling it on an interval, so no test depends on
wall-clock timing.

Cases:

- Text once committed is never rewritten by a later tick.
- Silence produces no preview event (the gate), and no hallucinated text.
- A segmenter error leaves the final transcript intact.
- The 25s force-commit fires.
- A recording under `min_recording_ms` injects nothing despite a preview.
- `cancel()` mid-preview leaves the engine `Idle` and injects nothing.
- No preview events when no segmenter is attached.

Manual verification is required for what tests cannot cover: that the text does
not visibly wobble, and that the pill reads well at three lines.

## Risks

**Release latency.** `WhisperTranscriber::transcribe` holds a single mutex for
its whole body (`lib.rs:63-109`), so a release landing mid-preview waits for that
decode — up to ~0.31s added before the final transcription starts. Worst case
goes from today's ~2.0s to ~0.6s, so this is still a large net improvement, but
it is a real cost and the one place a preview can be felt in the final path.

whisper.cpp supports several states decoding against one context, so this is
removable later by giving previews their own state, without changing the port.
Deliberately not done in v1: it adds concurrency to the crate's most
safety-sensitive code, and 0.31s does not justify it yet. The earlier proposal
disagrees and treats it as must-fix.

**Dependency upgrade.** Moving whisper-rs two minor versions rebuilds
whisper.cpp and changes the segment API. Verified as contained to one file, but
it is the largest single source of unexpected work here.

**Metal in a bundle.** whisper.cpp normally embeds its Metal shader library, but
if a `make bundle-mac` build ships shaders as a loose file instead, the app
silently falls back to CPU on another machine — 6.4x slower, presenting as "it's
just slow on that Mac". Verify on a real bundle.

**Preview and final can disagree.** Two code paths decoding overlapping audio
will sometimes show one thing and inject another. Inherent to previews; worth
stating because users notice and report it.

## Relationship to the earlier proposal

The earlier work-in-progress proposal surveyed the same problem and recommended
defining a `StreamingTranscriber` session port immediately while implementing it
as a naive whole-buffer re-decode. Its factual findings — the mutex constraint,
the event plumbing, the `spectrum()` precedent, the core's Swift-expressibility
rules — are confirmed and reused here.

This design differs in three ways:

1. **A `SpeechSegmenter` port rather than a `StreamingTranscriber` session.**
   The commit logic then lives in the engine, where fakes make it testable with
   no model on disk. The proposal placed it in `speakeasy-whisper`, and
   identified prefix commitment as "where streaming whisper implementations
   actually get hard" without saying how it would work. VAD boundaries are a
   concrete answer to that question.
2. **Cost is fixed, not quadratic.** The proposal's critique of periodic
   re-decode assumed cost grows with utterance length. The measurements show it
   does not.
3. **Mutex contention is accepted for v1** rather than designed out, as argued
   under Risks.

## Open questions

None blocking. Two to revisit after use:

1. Is 1.5s the right interval? It is a constant, not a setting; easy to tune
   once the feature has been lived with.
2. Does the text still visibly wobble within the live tail? If so,
   `no_speech_probability()` and a stable-prefix rule inside the live region are
   the next levers.
