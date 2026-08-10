# Speakeasy

Local push-to-talk dictation. Hold a key, speak, release — the transcript is
injected into whatever app has focus. Transcription runs on-device.

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
| `HotkeyListener` | shell-provided (Tauri plugin today)| `FakeHotkeys`   |

`HotkeyListener` has no platform implementation yet: the Tauri shell registers
the shortcut through its own plugin and drives the engine directly
(`src-tauri/src/hotkey.rs`). The port exists so a SwiftUI shell — or a headless
test — can supply press/release from elsewhere. If the shell-owned approach
becomes awkward, move it into `platform` behind the existing trait.

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

One assumption is load-bearing and only partly verified: push-to-talk needs
key-*release* events. `ShortcutState` (re-exported from `global-hotkey` 0.8)
does have both `Pressed` and `Released`, but that the macOS backend actually
delivers `Released` in practice is still unconfirmed — check it before building
much on top. If it does not, move hotkeys into `speakeasy-platform` behind the
existing `HotkeyListener` port and drop the shell adapter.

## Not yet implemented

Every port implementation is a typed stub returning an error, with the intended
approach in a `TODO`. The core state machine is complete and tested.

- cpal capture + rubato resampling (`platform/src/audio.rs`)
- CGEvent / pasteboard injection (`platform/src/macos/text_sink.rs`)
- XTEST injection (`platform/src/linux/text_sink.rs`)
- whisper.cpp loading and inference (`whisper/src/lib.rs`)
- TCC permission checks (`platform/src/macos/mod.rs`)
- model download and first-run flow
- settings UI and the transcription widget window (the `widget` window is not
  declared yet, so it is absent from `capabilities/default.json`)
- signing, notarization, auto-update

`icons/icon.png` and `icons/tray.png` are generated placeholders — flat colour
squares, enough to satisfy `generate_context!`. Replace before shipping.
