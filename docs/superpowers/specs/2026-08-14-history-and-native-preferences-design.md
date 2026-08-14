# Transcript history and native preferences chrome

Date: 2026-08-14

Status: proposed

Supersedes `2026-08-11-transcript-history-design.md`. That spec predates two
things that change its design: `TextSink::copy()`, which makes its proposed
`Clipboard` port unnecessary, and `State::Unconfirmed`, which it did not
account for.

## Problem

Two gaps in the settings window, bundled into one spec because they touch the
same files (`settings.ts`, `styles.css`) and the History rows should be
designed against the row language they'll actually ship in, not restyled
afterward.

**No transcript survives past injection.** `Engine::finish_recording`
transcribes, emits `Event::Transcript`, then attempts `sink.inject`. The
engine already keeps the single most recent transcript — added in `6a8e4ec`
so the widget's rescue row can offer Copy after a failure — but it is
overwritten by the next dictation and invisible anywhere but that one row. A
failure two dictations ago is gone.

**The window doesn't look like it belongs on macOS.** Left-rail tabs are the
right shape, but the rows are flat text on white with a hardcoded indigo
accent and an HTML checkbox — closer to a generic web settings page than
System Settings.

## Goals

- Keep recent transcripts so a failed or misdirected injection is
  recoverable, beyond just the most recent one.
- Copy any of them to the clipboard.
- Show them in a History tab.
- Restyle the settings window in the System Settings sidebar idiom: icons on
  the tabs, grouped inset cards, system accent color, native-looking
  switches.

## Non-goals

(Carried over from the superseded spec — the reasoning still holds.)

- **Persistence across restarts.** History lives in memory and dies with the
  process. Dictation catches passwords, medical details and half-formed
  thoughts; writing that to disk creates a durable record the app would then
  have to defend, with retention, deletion and encryption questions attached.
  Recovering the last few minutes is the whole value, and memory covers that.
- **Search or filtering.** A hundred in-memory entries do not need it.
- **Re-injecting an entry.** Copy covers the need without re-entering the
  focus-stealing problem the widget already manages.
- **Recording which app a transcript went to.** Threading app identity from
  the platform layer into every entry is a lot of plumbing for a question
  nobody opening this tab is asking.
- **Editing an entry.**
- **Distinguishing "unconfirmed" from "delivered" in the UI.** The
  `Unconfirmed` detection is a hint that is known to be wrong for apps that
  draw their own text (Emacs, Firefox) — see `6701333`. Badging it in History
  would cry wolf on rows where the text arrived fine. Only a genuine
  injection error (`sink.inject` returning `Err`) is worth marking.
- **A resizing preferences window (toolbar-style chrome).** Considered and
  rejected in favor of the fixed-sidebar direction — see Approaches below.
- **Native `NSSwitch` / AppKit controls.** The shell is a webview; switches
  are CSS built to read as native, not real AppKit views. Revisit only if the
  shell itself stops being a webview.
- **Esc-to-cancel-recording.** Raised during this brainstorm but out of
  scope: the widget is a non-activating panel that refuses key status (see
  the rescue-row work in `6a8e4ec`), so no keystroke reaches it today.
  Fixing that is a separate design problem — either making the panel
  key-able (risking stolen focus from whatever the user is dictating into)
  or routing Escape through the global hotkey path instead of DOM key
  handling. Tracked as a follow-up brainstorm, not part of this spec.

## Approaches considered for the window chrome

**A — leave tabs, restyle rows only.** Smallest diff. Rejected: the tab
strip is half of what reads as "generic web app," and doing the row
restyle without it wastes the moment.

**B — System Settings sidebar (chosen).** Keep the left rail already built,
add a small glyph per tab, replace bare rows with grouped inset cards on a
tinted background, swap the hardcoded `--accent` for `accent-color: auto`
(the system's own), and swap the HTML checkbox for a CSS switch. Fixed
window size across tabs, as today.

**C — classic icon toolbar (Preferences-style).** Icon bar across the top,
window resizes to fit each pane — what Things and Fantastical do. Better
suited to History, which genuinely wants a different shape than Dictation.
Rejected: it needs the window-resize-on-tab-switch machinery in `main.rs`
that B doesn't, for a payoff (History's shape) that a scrollable fixed-height
list panel gets close enough to without it.

## Row language (applies to every tab)

- `.setting` rows move from bare flow to `.card` — a white (dark: elevated)
  panel, `border-radius: 8px`, each row separated by a `0.5px` inset hairline
  instead of the current bottom border per `.model` row.
- A `.grouplabel` (small, uppercase, muted) above each card where a tab has
  more than one group — Dictation gets "TEXT INSERTION" above Insertion and
  Language, matching System Settings' own grouping.
- `--accent: #4f46e5` is replaced by the CSS Color Module 4 system-color
  keywords `AccentColor` / `AccentColorText`, which WebKit (Safari 16.4+,
  comfortably within Tauri's WKWebView baseline) resolves to the user's
  actual OS accent color and which apply to arbitrary `background-color` /
  `color`, not just native form controls — so the custom switch and the
  selected-tab highlight pick up the real accent, not an approximation.
- The checkbox in `toggleRow` (Live preview) becomes a CSS switch: same
  markup role (`<input type="checkbox">` under the hood, for no-JS semantics
  and keyboard operation), styled as a pill with a sliding thumb via
  `appearance: none` and a `::before`/`:checked` pair — not a native
  `NSSwitch`, since the shell is a webview (see Non-goals).
- Tab buttons in `.tabs` gain a small leading glyph. No icon asset pipeline
  exists yet; inline SVG (16x16, `currentColor` stroke) per tab, matching
  System Settings' single-color glyph style rather than importing SF
  Symbols.

`styles.css`'s settings section (roughly lines 24–187) is rewritten along
these lines; the floating-widget section (189 on) is untouched — that pill
already has its own deliberate, non-System-Settings identity per its
comments, and stays that way.

## History in the engine

```rust
/// One completed transcription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transcript {
    pub text: String,
    /// When transcription finished. `SystemTime`, not `Instant`, because the
    /// shell renders it as a wall-clock relative time ("2 min ago").
    pub at: SystemTime,
    /// `false` only when `sink.inject` returned `Err` — a genuine failure,
    /// not the `Unconfirmed` hint. That is the one case History marks,
    /// because it is the one case that is actually true regardless of which
    /// app had focus.
    pub delivered: bool,
}
```

Replaces `last_transcript: Mutex<Option<String>>` with
`history: Mutex<VecDeque<Transcript>>`, capped at 100, newest pushed to the
back. Recorded at the same point `last_transcript` was — after the empty
check, before `sink.inject` is attempted — so an entry exists before
anything can go wrong with delivery. `delivered` starts `true` (matching
today's `Ok`-only assumption) and is set `false` only in the `Err` arm of
`finish_recording`'s inject match.

Engine surface — `recoverable_transcript()` and `copy_transcript()` keep
their exact signatures and behavior (both operate on the newest entry), so
the widget's rescue row (`widget.ts`, `showRescue`) needs no changes:

```rust
fn recoverable_transcript(&self) -> Option<String>      // unchanged: newest entry's text
fn copy_transcript(&self) -> Result<bool>                // unchanged: copies the newest entry
fn history(&self) -> Vec<Transcript>                      // new, newest first
fn copy_history_entry(&self, index: usize) -> Result<()>  // new, index into that list
fn clear_history(&self)                                   // new
```

`copy_history_entry` takes an index into the newest-first list, not the text
itself, so the shell cannot put something on the clipboard the engine never
produced — same reasoning the superseded spec gave for its
`copy_transcript(index)`, kept here under a distinct name so the existing
no-arg `copy_transcript` (rescue row) and the new indexed one (History tab)
don't collide.

A new event, `Event::HistoryChanged`, carrying nothing — an open History tab
re-reads `history()` on receipt rather than diffing a hundred small structs.

## Shell

**IPC** (`commands.rs`): `history`, `clear_history`, `copy_history_entry` —
one-line delegations alongside the existing `recoverable_transcript` /
`copy_transcript`.

**History tab**: third entry in `settings.ts`'s `TABS` array, a
`mountHistory` function. Rows newest-first inside the shared `.card` styling:
the text (clamped to three lines via the same clamp technique as the
floating widget's `.preview`, full text in a `title` attribute), a relative
time, a Copy button, and — only for `delivered: false` — a small marker
("Not delivered" or similar) that is the one thing distinguishing a row from
every other. A **Clear** action above the list. Empty state text explains
what the tab is for and that it clears on quit, since a fresh install (and
every restart) has nothing to show.

The tray is untouched — history is not a status indicator.

## Testing

**`speakeasy-core`**, against fakes, no platform involved:

- a completed dictation appends an entry with `delivered: true`
- a dictation whose injection fails appends an entry with `delivered: false`
- a dictation left `Unconfirmed` (posted, undetectable focus) still appends
  `delivered: true` — the regression test for not conflating the two
- an empty or whitespace-only transcript is not recorded, matching the
  existing rule that it is not injected either
- the buffer caps at 100 and drops the oldest
- `history()` returns newest first
- `copy_history_entry` puts the right text on the fake clipboard
- `copy_history_entry` with an out-of-range index errors rather than panics
- `clear_history` empties it
- subscribers see `HistoryChanged`
- `recoverable_transcript()` / `copy_transcript()` still report the newest
  entry and are unaffected by history's cap or clearing behavior of other
  entries (regression coverage for the replace-the-slot change)

**Shell**: no automated test for the CSS/visual change; verified by hand in
light and dark mode. `speakeasy-platform` gets no new test — no new platform
port is introduced (`TextSink::copy` already exists and is already exercised
by the rescue-row work).
