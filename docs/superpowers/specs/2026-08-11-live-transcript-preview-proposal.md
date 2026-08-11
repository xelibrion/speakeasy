# Live Transcript Preview (Streaming) — Proposal

Show the user what they are saying *while* they are saying it, rather than only
after they release the key.

## Context Found

**The architecture explicitly anticipates this, and says how to do it.**
`README.md:209` — "Record-then-transcribe, not streaming… Adding streaming later
is a **new port**, not a change to `Transcriber`." The same sentence is repeated
as a contract on the port itself (`ports.rs:42-46`). Any option that widens
`Transcriber` is fighting a documented decision.

**The event stream is already the whole delivery mechanism.** `Engine::emit`
fans out to subscribers (`engine.rs:520-527`), the Tauri bridge re-emits every
event to the webview (`bridge.rs:74`), and the widget already listens
(`widget.ts:95`). A new `Event` variant reaches the UI with no new plumbing —
this is how `ModelStatusChanged` was added.

**Audio capture cannot currently be read without ending the recording.**
`AudioSource` exposes `start`, `stop`, `level`, `spectrum` (`ports.rs:20-40`);
only `stop` yields samples, and it drains the buffer (`audio/mod.rs:200`).
But `spectrum()` is precedent for exactly the read a preview needs: copy the
tail out under the capture lock, release it, then do the expensive work off the
audio callback's path (`audio/mod.rs:220-234`). Note `stop()` also does the
16 kHz resample (`audio/mod.rs:210-213`); a preview path has to resample too,
and `resample()` builds a fresh resampler per call (`audio/resample.rs`).

**The whisper context is a single mutex held across the entire decode.**
`WhisperTranscriber::transcribe` locks `context` for its whole body
(`lib.rs:63-109`). whisper.cpp lets several *states* decode against one context,
but this code does not — so any preview decode issued through the existing port
serialises against the final one. That lock is the central constraint on every
option below.

**The encoder cost is fixed per decode, and Metal is already being turned on.**
The uncommitted `Cargo.toml` change notes the encoder runs on a padded 30-second
window whatever the utterance length. That is the economics of preview: *N*
previews cost roughly *N* encoder passes regardless of how little new audio
arrived. Preview is only affordable if that fixed cost is on the GPU.

**Existing behaviour a preview must not corrupt:** recordings under
`min_recording_ms` are discarded silently (`engine.rs:279-282`), `cancel()`
abandons an utterance entirely (`engine.rs:319-327`), and `Event::Transcript` is
documented as the *final* text before injection (`event.rs:68`). The widget is a
small pill with a one-line label (`widget.ts:35-46`, `widget.rs` module docs) —
it has no room for a paragraph, and it must never take focus.

**Core API constraints:** `speakeasy-core` must stay free of tauri (`make
boundary`) and Swift-expressible — flat owned types, no generics or lifetimes in
signatures (`lib.rs:8-13`).

## Data flow (both options)

```
audio thread            preview thread              engine
────────────            ──────────────              ──────
callback → buffer  ──►  tail/whole read
                        resample to 16 kHz
                        decode ──────────────────►  Event::TranscriptPreview
                                                       │
                                          bridge ──────┴──► widget
release ──────────────────────────────────────────►  stop, final decode
                                                     Event::Transcript → inject
```

The preview thread lives for the duration of one recording: started on the
transition into `Recording`, stopped before the final decode. Everything it
produces is advisory; the final decode over the complete buffer stays
authoritative and is the only text that gets injected.

## Option A — Minimal / Fast: periodic whole-buffer re-decode

A timer thread inside the engine wakes every ~1.5 s while `Recording`, takes a
non-destructive copy of everything captured so far, and runs it through the
**existing** `Transcriber`. Each result replaces the previous preview.

New surface: one non-destructive read on `AudioSource`; one `Event` variant; a
preview thread in `Engine`. No new port, no new crate work.

What it does not solve:

- **Cost is quadratic in utterance length.** Decode *k* re-processes all audio
  so far, so a 30-second dictation pays for ~20 decodes of ever-growing audio.
- **It contends with the final decode on the whisper mutex** (`lib.rs:63`). A
  release lands while a preview is mid-flight, so the user's *final* transcript
  waits for a preview they will never see. That is latency added to exactly the
  moment that matters — unless the preview loop is stopped and joined before
  `finish_recording` proceeds, which costs the same wait by a different name.
- **Text flickers.** Whisper re-decoding a growing buffer freely rewrites
  earlier words; without a notion of a stable prefix the preview jitters.
- Greedy decode over a partial utterance ends mid-word and hallucinates
  endings; there is no seam here to suppress that.

Honest framing: this is the *display-only, short-utterance* version. For the
2–5 second dictations push-to-talk encourages, it is close to sufficient.

## Option B — Idiomatic / Scalable: a `StreamingTranscriber` port

Take the extension `README.md:209` describes. A second port alongside
`Transcriber`, modelling a **session**: opened when recording begins, fed newly
captured audio as it arrives, asked for the text so far, and closed when the
recording ends. Implemented in `speakeasy-whisper` over its own decoder state,
independent of the one `transcribe` uses.

Where this differs from A, structurally:

- **Sliding window, not whole buffer.** The session keeps only the audio since
  the last committed point, so per-decode cost is bounded rather than growing
  with the utterance.
- **A committed prefix.** The session distinguishes text it will not revise
  from the tail it may still rewrite. The preview event carries both, so the
  widget can render settled text calmly and the tail tentatively — this is what
  removes the flicker rather than hiding it.
- **No contention with the final decode**, provided the implementation owns a
  separate whisper state and does not take the mutex `transcribe` holds. That
  is an implementation constraint on `speakeasy-whisper`, not on the port.
- **Fakes make it testable.** A fake streaming transcriber in `core::fakes` lets
  the engine's preview lifecycle — start, cancel, discard-as-tap, failure —
  be tested with no model on disk, matching how `FakeFetcher` covers downloads.

What it costs: a new trait and a new fake in the core; a genuinely non-trivial
piece of work in `speakeasy-whisper` (windowing and prefix commitment are where
streaming whisper implementations actually get hard); a second decoder state
resident in memory for the duration of a recording; and a second code path that
can disagree with the final transcript in ways users will notice and report.

## Recommended Approach

**Hybrid: define Option B's port now, implement it first the way Option A
decodes.**

The port boundary is the expensive decision and the one `README.md:209` already
committed to — get it right while nothing depends on it. The windowing and
prefix-commitment machinery behind it is the expensive *work*, and it can start
as "re-decode what we have, commit nothing" without changing a single caller
later. The engine, the event, the widget and the tests are written once against
the final shape.

Two properties hold from day one regardless of the decoder's sophistication:
the preview never blocks the final decode (separate state, separate lock), and
the final transcript remains the only text injected.

Alongside it, preview should be **config-gated and default-off for large
models** — see the open questions.

## Open Questions

1. **Is the preview display-only, or does it change what gets injected?**
   Display-only in the widget is what this proposal assumes, and it keeps
   `Event::Transcript`'s contract (`event.rs:68`) intact. Progressive injection
   — typing into the target app as you speak — is a different product with a
   correction problem attached, and it would need a real answer for "we already
   pasted a word the final decode rewrote."

2. **What is the acceptable steady-state cost, and who pays it?** A preview
   decode every ~1.5 s on `small.en` is a continuous GPU/CPU load for the whole
   time the key is held. Should preview be off by default and opt-in, on by
   default only for the small models, or always on with the interval scaled to
   observed decode time?
