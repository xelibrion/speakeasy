# Transcript history

Date: 2026-08-11
Status: approved, not yet planned

Depends on: `2026-08-11-model-selection-and-download-design.md`, which builds
the tabbed settings shell this adds a tab to.

## Problem

A transcript exists for exactly as long as it takes to inject it, and then it is
gone. Two things follow from that.

The one that matters: **a failed injection loses the user's words entirely.**
`Engine::finish_recording` transcribes, emits `Event::Transcript`, then calls
`sink.inject`. When that fails — `SecureInputActive` because a password field
has disabled event taps, or an `Injection` error from an app that refuses
synthetic events — the engine goes to `Failed`, the widget names the problem,
and the text the user just spoke is unrecoverable. They have to say it again,
and they only find out after the fact.

The lesser one: dictation lands in whatever had focus, and focus is not always
what the user thought it was. The paste goes into the wrong window and there is
no way to get the text back.

Both are solved by keeping the last few transcripts and letting the user copy
one.

## Goals

- Keep recent transcripts so a failed or misdirected injection is recoverable.
- Copy any of them to the clipboard.
- Show them in a History tab in the settings window.
- Record the transcript whether or not injection succeeded.

## Non-goals

- **Persistence across restarts.** History lives in memory and dies with the
  process. Dictation catches passwords, medical details and half-formed
  thoughts; writing that to disk creates a durable record the app would then
  have to defend, with retention, deletion and encryption questions attached.
  The value here is recovering something from the last few minutes, and memory
  covers that completely.
- **Search or filtering.** A hundred in-memory entries do not need it.
- **Re-injecting an entry.** Copy covers the need without re-entering the
  focus-stealing problem the widget already has to manage.
- **Recording which app a transcript went to.** Tempting given the focus
  tracking added in `25ddccf`, but it means threading app identity from the
  platform layer through the engine into every entry, and "which app did this go
  to" is not the question someone opening this tab is asking.
- **Editing an entry.**

## The clipboard port

History storage needs no port — it is a ring buffer in memory, and a trait with
one trivial implementation is ceremony rather than a seam. The port that is
load-bearing is the clipboard: core cannot touch the pasteboard.

`TextSink` is not it. `TextSink::inject` writes to the pasteboard *and*
synthesizes Cmd+V, and in `InjectionMode::Synthetic` it does not use the
pasteboard at all. A Copy button must do exactly one thing.

```rust
/// Puts text on the system clipboard, and nothing else.
///
/// Distinct from [`TextSink`], which delivers text to the focused app — on
/// macOS that means pasteboard *plus* a synthesized Cmd+V, and in
/// `InjectionMode::Synthetic` it does not touch the pasteboard at all.
pub trait Clipboard: Send + Sync {
    fn set_text(&self, text: &str) -> Result<()>;
}
```

Implemented in `speakeasy-platform` beside the existing pasteboard code in
`macos/text_sink.rs`, and faked in `core::fakes` so the copy path is testable.

This makes six ports. That is the right count if each one is a real capability
core lacks, and this one is.

## History in the engine

```rust
/// One completed transcription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transcript {
    pub text: String,
    /// When the transcription finished. `SystemTime` rather than `Instant`
    /// because the shell renders it as a wall-clock time.
    pub at: SystemTime,
    /// Whether injection succeeded. `false` is the interesting case: that
    /// entry is the only surviving copy of what the user said.
    pub delivered: bool,
}
```

A capped `VecDeque<Transcript>` on `Engine`'s inner state, newest last, holding
100 entries. Transcripts are small and nobody scrolls past a hundred.

**Recording happens where `Event::Transcript` is emitted** — after the empty
check, before `sink.inject` is attempted. That ordering is the whole point: an
entry exists before anything can go wrong with delivery. `delivered` starts
`false` and is set to `true` once `inject` returns `Ok`.

Engine surface:

```rust
fn history(&self) -> Vec<Transcript>          // newest first
fn clear_history(&self)
fn copy_transcript(&self, index: usize) -> Result<()>
```

`copy_transcript` takes an index into the newest-first list rather than the text
itself, so the shell cannot put something on the clipboard that the engine never
produced.

A new event, `Event::HistoryChanged`, so an open History tab updates as
dictation happens rather than only on reopen. It carries nothing — the tab
re-reads `history()`. A hundred small structs is not worth diffing.

## Shell

**IPC:** `history`, `clear_history`, `copy_transcript` — one-line delegations.

**History tab** in the settings shell built by the model plan: one entry in that
plan's `TABS` array plus a `mountHistory` function.

Rows newest-first: the text (clamped to three lines, with the full text in a
`title`), a relative time ("2 min ago"), a **Copy** button, and — for
`delivered: false` — a marker saying the text never reached its destination.
That marker is what makes the tab worth opening after a failure.

A **Clear** button at the top. Empty state says what the tab is for, since a
fresh install has nothing to show.

The tray is untouched. History is not a status.

## Testing

All in `speakeasy-core` against fakes, no platform involved:

- a completed dictation appends an entry with `delivered: true`
- a dictation whose injection fails appends an entry with `delivered: false` —
  the regression test for the problem this feature exists to solve
- an empty or whitespace-only transcript is not recorded, matching the existing
  rule that it is not injected either
- a discarded short recording is not recorded
- the buffer caps at 100 and drops the oldest
- `history()` returns newest first
- `copy_transcript` puts the right text on the fake clipboard
- `copy_transcript` with an out-of-range index errors rather than panicking
- `clear_history` empties it
- subscribers see `HistoryChanged`

`speakeasy-platform` gets no automated test for the real clipboard — it is a
three-line pasteboard call, and verifying it means asserting on the OS
clipboard, which is shared mutable state that other tests and the user's own
session both touch. Verify it by hand.
