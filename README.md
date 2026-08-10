# Speakeasy

Local dictation. Tap a key and a floating widget appears while it listens; tap
again and the transcript is injected into whatever app has focus. Transcription
runs on-device. `Config::activation` switches the key back to hold-to-talk.

macOS first, X11 Linux second. Wayland is explicitly out of scope.

## Layout

```
crates/
  speakeasy-core/       ports + state machine. No tauri, no OS APIs, no whisper.
  speakeasy-platform/   the only crate allowed to use cfg(target_os).
  speakeasy-whisper/    Transcriber backed by whisper.cpp.
apps/
  desktop/              Tauri shell — tray, windows, IPC. Replaceable.
    src/                frontend (settings + widget)
    src-tauri/          shell backend
```

Dependencies point one way: `desktop` → `platform`/`whisper` → `core`. The core
depends on nothing but `serde`, `thiserror` and `tracing`.

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

The core needs four things from the outside world (`core/src/ports.rs`):

| Port             | Real implementation                | Test double     |
| ---------------- | ---------------------------------- | --------------- |
| `AudioSource`    | `platform::CpalAudioSource`        | `FakeAudio`     |
| `Transcriber`    | `whisper::WhisperTranscriber`      | `FakeTranscriber` |
| `TextSink`       | `platform::{MacTextSink, X11TextSink}` | `FakeSink`  |
| `HotkeyListener` | `platform::{MacHotkeys, X11Hotkeys}` | `FakeHotkeys` |

### Push-to-talk is a held modifier

The default binding is **Right Option**, held. That shape rules out every
ordinary global-shortcut library, for two reasons: an accelerator needs a
non-modifier key, and the usual APIs report "Option is down" without saying
which Option.

So `MacHotkeys` taps `FlagsChanged` through CoreGraphics and reads IOKit's
*device-dependent* modifier bits, which do encode the side. The tap is
`ListenOnly`, so Right Option still types whatever it normally types — nothing
is swallowed. This is why the listener lives in `platform` rather than the
shell, and why `tauri-plugin-global-shortcut` is no longer a dependency.

`Config::hotkey` is a `Hotkey` enum, not a string: `Modifier { key }` for the
held-modifier shape and `Accelerator { accelerator }` for a conventional chord.
macOS implements the first and rejects the second with a clear error.

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
memory-only, which is what the tests use. `SPEAKEASY_MODEL` still overrides
`model_path` at startup, so the pipeline can be exercised without touching the
file.

## Design decisions worth knowing

- **Record-then-transcribe, not streaming.** Push-to-talk records to a buffer
  and transcribes on release. Removes VAD and endpointing entirely. Adding
  streaming later is a new port, not a change to `Transcriber`.
- **Paste injection by default.** More reliable than synthetic key events in
  Electron apps and terminals, at the cost of clobbering the clipboard.
  Switchable via `Config::injection_mode`.
- **Secure input is a first-class error.** A focused password field disables
  event taps process-wide on macOS. `Error::SecureInputActive` exists so this
  surfaces to the user rather than silently dropping their dictation.

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

Verified against rustc 1.97.1: the workspace builds and links, `make check`
passes, and the frontend typechecks and bundles. Resolved versions are in
`Cargo.lock` — tauri 2.11.5, whisper-rs 0.14.4, cpal 0.15.3, rubato 0.15.0,
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
- XInput2 raw-key hotkeys (`platform/src/linux/hotkey.rs`)
- whisper.cpp loading and inference (`whisper/src/lib.rs`)
- TCC permission checks (`platform/src/macos/mod.rs`)
- model download and first-run flow
- settings UI: the pane is still a read-only log, so activation mode, hotkey and
  injection mode can only be changed by editing `config.json`
- signing, notarization, auto-update

`icons/icon.png` and `icons/tray.png` are generated placeholders — flat colour
squares, enough to satisfy `generate_context!`. Replace before shipping.
