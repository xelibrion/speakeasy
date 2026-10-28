# Speakeasy

Local dictation. Tap a key and a floating widget appears while it listens; tap
again and the transcript is injected into whatever app has focus. Transcription
runs on-device. The key is set in Settings › Dictation by pressing it, and
`Config::activation` switches it back to hold-to-talk.

macOS first, X11 Linux second. Wayland is explicitly out of scope.

## Layout

```
crates/
  speakeasy-core/       ports + state machine. No tauri, no OS APIs, no whisper.
  speakeasy-models/     ModelFetcher over HTTP. Keeps TLS out of core.
  speakeasy-platform/   the only crate allowed to use cfg(target_os).
  speakeasy-whisper/    Transcriber backed by whisper.cpp.
apps/
  desktop/              Tauri shell — tray, windows, IPC. Replaceable.
    src/                frontend (settings + widget)
    src-tauri/          shell backend
```

Dependencies point one way: `desktop` → `models`/`platform`/`whisper` → `core`.
The core depends on nothing but `serde`, `serde_json`, `thiserror`, `tracing`
and `sha2` — the last only to verify a downloaded model, which is computation
rather than a capability.

## The invariant

**`speakeasy-core` must never depend on `tauri`.**

That single rule is what keeps a future SwiftUI shell cheap. The core owns the
state machine, the config, the error taxonomy and the event stream; the shell
subscribes and renders. Swapping Tauri for SwiftUI then means writing a new
shell against the same `Engine` — not a rewrite.

`make boundary` fails the build if the rule is broken. Two conventions keep it
honest in practice:

- Command bodies in `src-tauri/src/commands.rs` are one-line delegations. A
  command with a branch in it means logic has leaked out of the core.
- Nothing in `core`'s public API uses generics, lifetimes or borrowed returns.
  Flat owned types and plain error enums are what `uniffi`/`swift-bridge` can
  express, so the binding layer stays mechanical.

## Ports

The core needs five things from the outside world (`core/src/ports.rs`):

| Port             | Real implementation                | Test double     |
| ---------------- | ---------------------------------- | --------------- |
| `AudioSource`    | `platform::CpalAudioSource`        | `FakeAudio`     |
| `Transcriber`    | `whisper::WhisperTranscriber`      | `FakeTranscriber` |
| `TextSink`       | `platform::{MacTextSink, X11TextSink}` | `FakeSink`  |
| `HotkeyListener` | `platform::{MacHotkeys, X11Hotkeys}` | `FakeHotkeys` |
| `ModelFetcher`   | `models::HttpFetcher`              | `FakeFetcher`   |
| `SpeechSegmenter` | `whisper::WhisperSegmenter`       | `FakeSegmenter` |

`speakeasy-models` exists for the same reason `speakeasy-whisper` does: to keep
a heavy dependency — a TLS stack, here — out of the crate that is supposed to be
portable logic. It is one file implementing one trait. The payoff is that every
download branch (progress, a corrupt file, a cancel mid-transfer) is unit-tested
offline against `FakeFetcher`, with no network and no test server.

### Push-to-talk is a held modifier

The default binding is **Right Option**, held. That shape rules out every
ordinary global-shortcut library, for two reasons: an accelerator needs a
non-modifier key, and the usual APIs report "Option is down" without saying
which Option.

So `MacHotkeys` taps `FlagsChanged` through CoreGraphics and reads IOKit's
*device-dependent* modifier bits, which do encode the side. This is why the
listener lives in `platform` rather than the shell, and why
`tauri-plugin-global-shortcut` is no longer a dependency.

`Config::hotkey` is a `Hotkey` enum, not a string: `Modifier { key }` for the
held-modifier shape and `Chord { mods, key }` for a conventional chord such as
`⌘⌥Space`. Chords are side-agnostic; a bare modifier is not, because the side is
the whole point of that shape.

**Set it in Settings › Dictation by pressing the key you want.** Capture runs
through the same event tap that does the binding, so what can be captured and
what can be bound are the same set — including Fn, which a webview never sees.
Rebinding takes effect immediately: the core emits `HotkeyChanged` and the shell
re-registers.

### What gets swallowed

A bound **chord is consumed**, so binding `⌘⌥Space` does not also open
Spotlight every time you dictate. A bound **bare modifier never is** — the
default binding is Right Option, and swallowing it would break `⌥e → é` for
everyone who leaves the setting alone.

Two consequences worth knowing:

- The tap is `CGEventTapOptions::Default`, not `ListenOnly`. macOS can disable
  such a tap when it responds too slowly, so the callback handles
  `TapDisabledByTimeout`/`ByUserInput` and re-arms. Without that the app goes
  silently deaf until restarted.
- Suppression rewrites the event to `CGEventType::Null` in place. Returning
  `None` from the callback does *not* consume it: `core-graphics`'s trampoline
  reads `None` as "pass the original through" and cannot express the NULL that
  CoreGraphics treats as deletion.

**This needs Accessibility permission.** Without it `CGEventTap::new` fails and
registration reports `Error::InputPermissionDenied` rather than binding nothing
and going quiet. Startup logs `push-to-talk bound hotkey="Right Option"` on
success; a failure is a warning and the app keeps running, since the tray and
settings still work.

One caveat worth knowing: because it is a bare modifier, any use of Right
Option — including `⌥`-chords for special characters — starts a recording.
`min_recording_ms` discards taps shorter than 250 ms, which covers most
accidents in hold mode; in toggle mode an ⌥-chord starts a recording that stays
up until the next tap, which the widget at least makes obvious.

### Toggle or push-to-talk

`Config::activation` picks between the two. **Toggle is the default**: one tap
starts capture and raises the widget, the next transcribes and injects. Holding
a modifier through a long dictation is the worse ergonomic, and the widget is
what keeps a forgotten recording from going unnoticed.

The listener reports press and release regardless; only which edge the engine
acts on changes, so toggle costs the platform layer nothing. `Engine::hotkey_action`
maps `(mode, event, state)` to `Begin`, `Finish` or `Ignore`, and the shell just
carries that out — the decision stays in the core, where it can be tested. In
toggle mode the release edge is dropped and a press arriving while transcription
or injection is in flight is ignored, so an impatient second tap cannot queue a
recording behind the one being processed.

## The status widget

A floating pill, shown whenever the engine is not idle. It renders a twelve-band
spectrum of what the microphone is hearing, the current state, and — in toggle
mode — how to stop.

The spectrum is real, not a level meter dressed up: `AudioSource::spectrum`
takes a 1024-sample window of the tail of the capture buffer, applies a Hann
window, and runs a real FFT (`realfft`, already in the tree via rubato), then
groups the bins into log-spaced bands from 80 Hz to 8 kHz. Bars rise instantly
and decay slowly, which is what makes a 60 Hz readout legible. The FFT runs
outside the capture lock so the audio callback never waits on it.

The pill is draggable, and where you leave it is remembered. That is in real
tension with the widget's other rule — it must never take focus, or Speakeasy
pastes into its own window — because dragging means accepting clicks, and the
widget is only on screen *during* a dictation. `set_focusable(false)` is what
keeps a drag from making Speakeasy the active app. The cost is that the pill can
swallow a click meant for the window underneath, so it parks out of the way.

A drag emits a window event per frame; positions are written to disk only once
the widget has sat still for 600 ms.

## Config lives on disk

`~/Library/Application Support/com.speakeasy.app/config.json`, or
`$XDG_CONFIG_HOME/com.speakeasy.app/config.json` when that is set.

`Config::load` treats a missing file as a first run and returns the defaults; it
treats malformed JSON as an error, because silently overwriting a file someone
hand-edited is worse than starting from defaults and saying so. Saves go through
a temporary file and a rename, so an interrupted write cannot truncate a good
config. Every field is `#[serde(default)]`, so a file from an older build still
loads and picks up new settings at their defaults.

`Engine::persisting_to` is what turns saving on; without it the engine is
memory-only, which is what the tests use. `SPEAKEASY_MODEL` still overrides the
model at startup — it now maps to `ModelChoice::Local` — so the pipeline can be
exercised without touching the file.

## Choosing a model

`Config::model` is a `ModelChoice`: either a catalog id (`{"kind": "catalog",
"id": "base.en"}`) or a path to a `.bin` you already have. Downloaded weights
live in `models/` beside `config.json`, so they survive reinstalling the app.

The distinction that makes the rest of it simple:

- **`Config::model` is desired state** — what the user picked, persisted,
  written the moment they choose it. It may name a model that is not on disk.
- **`ModelStatus` is actual state** — what is loaded or downloading right now.
  In memory only, rebuilt at launch.

`Engine::ensure_model` reconciles them at startup, which is why first run is not
a special case: a fresh config wants `base.en`, nothing is installed, so it
downloads it. Identical machinery to a mid-session switch.

Three behaviours follow from that split. Quitting mid-download resumes on the
next launch, because config already names the wanted model. Cancelling reverts
`Config::model` to whatever is loaded, so the abandoned transfer does not come
back. And a download that fails does *not* revert — retrying is one click — but
`ensure_model` will load any other installed model so the hotkey keeps working,
with `ModelStatus::Failed` still saying why you are not on the model you chose.

Downloads go to a `.part` file, are checked against the SHA-256 in the catalog,
renamed into place, and only then loaded. Because the swap is last, the model
you are dictating with keeps working for the whole download, and every failure
short of that last step is a no-op. Sizes and digests in
`core::models::catalog` come from the upstream LFS metadata; the ignored test in
`speakeasy-models/tests/realfetch.rs` is what proves they still match.

A config from before the catalog carries a bare `modelPath`. `Config::load`
migrates it to a local choice and drops the old field on the next save.

## Design decisions worth knowing

- **Record-then-transcribe, with a live preview over the top.** Push-to-talk
  records to a buffer and transcribes on release; that final transcript is the
  only text ever injected. While recording, a preview loop decodes what has
  been said so far every 1.5s and shows it in the widget.

  Streaming arrived as a new port, `SpeechSegmenter`, exactly as this section
  used to predict — `Transcriber` was not touched. The detector answers "has
  this utterance finished?", which is what lets preview text be committed and
  then never revised; only the sentence in progress can still change. See
  `core::preview` for the rule and `docs/superpowers/specs/` for why.
- **Paste injection by default.** More reliable than synthetic key events in
  Electron apps and terminals, at the cost of clobbering the clipboard.
  Switchable via `Config::injection_mode`.
- **A dictation is never lost silently.** Posting keystrokes says nothing about
  whether anything read them, so `inject` returns a `Delivery` rather than a
  character count: `verified: false` means the Accessibility API positively
  reported nothing focused that could take text. The engine then rests in
  `State::Unconfirmed` instead of `Idle`, which keeps the widget up with the
  transcript and a Copy button rather than hiding it and taking the only copy.

  The detection is a hint and never a veto — see `macos::focus` for the measured
  table showing Emacs and Firefox reporting "not editable" while accepting typed
  text perfectly well. Injection always happens; the check only decides whether
  the transcript stays within reach. A wrong answer costs a button nobody
  needed, never the dictation.
- **Secure input is checked before the microphone opens.** A focused password
  field disables event taps process-wide on macOS, which makes the whole
  dictation undeliverable. `TextSink::blocked_by` is consulted in
  `begin_recording`, so the user is told immediately rather than after speaking
  a paragraph that then has nowhere to go. It is checked again at injection
  time, since secure input can arrive mid-dictation.

  `Error::SecureInputActive` carries the name of the app holding it — read from
  `kCGSSessionSecureInputPID` in the window server's session dictionary, which
  is private API, so `crates/speakeasy-platform/src/macos/secure_input.rs`
  reaches it by `dlsym` and degrades to "Another app" rather than failing. The
  name is the only actionable part: it tells the user which window to leave.

## Getting started

Prerequisites: a Rust toolchain, Node, and **cmake** — `whisper-rs` builds
whisper.cpp from source and its build script needs it.

```sh
brew install cmake
cd apps/desktop && npm install
```

Then:

```sh
make check   # fmt, clippy -D warnings, tests, boundary check
make dev     # run the app
```

On macOS the desktop app enables `speakeasy-whisper`'s `metal` feature, and it
is not optional in practice. Measured on an M2 with `base.en`, a decode takes
~1.90s on the CPU and ~0.31s on the GPU. The encoder runs on a padded
30-second window whatever the utterance length, so that cost is fixed per
decode rather than proportional to how much was said — which is why the live
preview is affordable at all, and why it would not be without Metal.

The voice detector deliberately runs on the CPU. Its graph contains an op the
Metal backend cannot execute, and whisper.cpp aborts the process rather than
falling back.

Verified against rustc 1.97.1: the workspace builds and links, `make check`
passes, and the frontend typechecks and bundles. Resolved versions are in
`Cargo.lock` — tauri 2.11.5, whisper-rs 0.16.0, cpal 0.15.3, rubato 0.15.0,
x11rb 0.13.2.

Push-to-talk is verified end to end on macOS: a synthesised Right Option press
reaches the tap, drives the engine through `begin_recording`, and fails with
`ModelNotLoaded` — the correct outcome until weights are loaded.

## Not yet implemented

The core state machine and the macOS hotkey listener are complete. The
remaining port implementations are typed stubs returning an error, with the
intended approach in a `TODO`.

- cpal capture + rubato resampling (`platform/src/audio.rs`)
- CGEvent / pasteboard injection (`platform/src/macos/text_sink.rs`)
- XTEST injection (`platform/src/linux/text_sink.rs`)
- XInput2 raw-key hotkeys (`platform/src/linux/hotkey.rs`). Chords are macOS
  only: `X11Hotkeys` returns `Error::Hotkey` for both `Hotkey::Chord` and
  capture, as the port contract requires of a shape it cannot bind
- whisper.cpp loading and inference (`whisper/src/lib.rs`)
- microphone permission status. Accessibility is checked and prompted for;
  macOS prompts for the microphone itself when the device is opened
- settings UI: Models and the hotkey binding are interactive. Activation mode,
  injection mode and language render read-only and can still only be changed by
  editing `config.json`
- resuming an interrupted download from where it stopped — a `.part` file is
  discarded and the retry starts over
- signing, notarization, auto-update

### Accessibility needs a stably signed build

macOS keys the Accessibility grant to the app's code signature. An **ad-hoc**
signed build has no stable identity, so the grant never matches on the next
launch and the app asks every time. `make deploy-mac` therefore signs with
`$APPLE_SIGNING_IDENTITY`, or the first Apple Development / Developer ID
identity in the keychain, and refuses to ship an ad-hoc bundle. A free
certificate is enough for the grant to stick; a Developer ID adds Gatekeeper
acceptance and notarization.

When moving a Mac from an ad-hoc build to a signed one, clear the old entries
first, or the stale row shows as ticked without working:

    tccutil reset Accessibility com.speakeasy.app
    tccutil reset ListenEvent com.speakeasy.app

**Test this by launching from Finder, never from a terminal.** A binary started
from a terminal has its TCC requests attributed to Terminal/iTerm, so it reports
`trusted=true` whenever the terminal has Accessibility — regardless of whether
the app itself was ever granted anything.

`make deploy-mac` also installs a LaunchAgent (`com.speakeasy.app.autostart`)
so the app starts at every login.

`icons/icon.png` and `icons/tray.png` are generated placeholders — flat colour
squares, enough to satisfy `generate_context!`. Replace before shipping.
