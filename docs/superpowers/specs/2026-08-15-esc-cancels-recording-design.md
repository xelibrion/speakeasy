# Esc cancels an active recording

## Problem

While the widget is up and recording, there's no keyboard way to bail out.
The only escape hatch today is the Dismiss button, which only appears once a
dictation has already failed. Users want to hit Esc mid-recording and have it
go away.

## Why this is not a DOM keydown handler

The widget window is deliberately never key (see `widget.rs` module docs):
injection targets "whatever had focus when the user spoke," and a widget that
could become key would steal that focus and paste into itself instead. As a
direct consequence, the widget's webview never receives keyboard events —
`widget.ts` already notes this is why Escape can't be wired up as a DOM
listener today.

The only thing that sees keystrokes regardless of focus is the macOS
`CGEventTap` that already backs the activation hotkey
(`speakeasy-platform/src/macos/hotkey.rs`). Esc-cancel has to ride that same
tap. Linux hotkey support (`speakeasy-platform/src/linux/hotkey.rs`) is an
unimplemented stub, so this feature lands macOS-only, consistent with
push-to-talk today.

## Behavior

- Esc cancels only while `State::Recording`. Once transcribing or injecting
  has started, Esc does nothing — cancelling mid-transcription or
  mid-injection is out of scope.
- Esc must be bare (no modifiers held) to trigger cancel. This keeps it from
  swallowing system/app shortcuts like ⌘+Esc (Force Quit) or an
  Option/Shift+Esc binding some other app owns.
- Esc is swallowed (consumed, not delivered to the focused app) only when it
  actually cancelled a recording. In every other case — idle, not bare,
  nothing to cancel — it passes through exactly as it does today.
- If the user's configured activation hotkey itself uses Escape as its key
  (a `Hotkey::Chord` with `key: KeyCode::Escape`), the activation binding
  wins outright: Esc-cancel is disabled for as long as that binding is
  active. No attempt is made to disambiguate a single Esc press between the
  two meanings.
- The widget's recording hint gains a `· Esc to cancel` suffix, suppressed
  under the same Escape-hotkey-clash condition above.

## Design

### Core (`speakeasy-core`)

- New port type in `ports.rs`:
  ```rust
  pub type CancelCallback = Box<dyn Fn() -> bool + Send + Sync>;
  ```
  Invoked on a bare Escape keydown; returns whether it consumed the event.
- `HotkeyListener::register` gains a third parameter:
  ```rust
  fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback, on_cancel: CancelCallback) -> Result<()>;
  ```
- New `Engine::cancel_recording_if_active(&self) -> bool` in `engine.rs`:
  no-ops and returns `false` unless `state() == State::Recording`; otherwise
  runs the same stop-audio / clear-`started_at` / transition-to-`Idle`
  sequence `cancel()` already runs, and returns `true`.

  Kept separate from the existing `cancel()` rather than adding a state
  guard there: `cancel()` is used unconditionally by the widget's Dismiss
  button from `Failed`/`Unconfirmed`, and that contract ("cancel whatever's
  happening, from wherever it's happening") must not change.

### macOS tap (`speakeasy-platform/src/macos/hotkey.rs`)

In the tap callback, after the capture-armed check and before matching the
bound hotkey:

- If the bound `hotkey` is not a `Chord` whose `key` is `KeyCode::Escape`,
  and the raw event is a bare (`mods.is_empty()`) Escape `KeyDown`, call
  `on_cancel()`.
  - If it returns `true`: null the event (swallow) and record that in a new
    `AtomicBool` (mirrors the existing `is_down` tracking for the bound
    hotkey), so the matching Escape `KeyUp` is also swallowed — otherwise a
    stray key-up would leak through to the focused app.
  - If it returns `false`: let the event pass through unmodified, same as
    any other unmatched key.
- This check runs independently of, and before, the existing
  `speakeasy_core::hotkey::match_event(&hotkey, raw)` call for the bound
  hotkey, since a bare Escape only ever matches a hotkey binding when that
  binding is itself Escape-based (excluded above).

### Other `HotkeyListener` implementors

- `X11Hotkeys` (Linux stub, `speakeasy-platform/src/linux/hotkey.rs`): gains
  the parameter, ignored, same as `hotkey`/`on_event` are today.
- The test fake in `speakeasy-core/src/fakes.rs`: gains the parameter,
  stored so tests can invoke it and assert on the returned bool.

### Wiring (`apps/desktop/src-tauri/src/hotkey.rs`)

`register()` builds `on_cancel` the same way it already builds `on_event` —
a fresh closure over `engine.clone()` calling
`engine.cancel_recording_if_active()` — and passes it to
`listener.register(hotkey, on_event, on_cancel)`.

### Frontend (`apps/desktop/src/engine.ts`)

`stopHint()` appends `" · Esc to cancel"` to its return value, suppressed
when `config.hotkey` is itself an Escape-based chord (mirrors the Rust-side
precedence rule). No changes to `widget.ts`: Esc never reaches the DOM, so
there's nothing there to listen for.

## Testing

- `speakeasy-core::hotkey` unit tests: none needed — no new matching logic
  lives there. The Escape-vs-bound-hotkey precedence check and the
  bare-modifier check are simple conditions inline in the platform tap
  callback, which has no existing unit-test seam (it's untested today,
  same as the rest of `speakeasy-platform/src/macos/hotkey.rs`) and isn't
  worth adding one for two conditionals.
- New unit test(s) in `speakeasy-core::engine`'s existing test module for
  `Engine::cancel_recording_if_active`: returns `true` and transitions to
  `Idle` from `State::Recording`; returns `false` and leaves state
  unchanged from `Idle`, `Transcribing`, `Injecting`, `Unconfirmed`, and
  `Failed`.
- Manual verification on a real Mac (required either way, since this is
  event-tap + Accessibility-permission behavior that cannot be exercised in
  CI): start a recording, press bare Esc, confirm the widget disappears and
  no Esc reaches the app that had focus; confirm Esc does nothing once
  transcribing/injecting has started; confirm ⌘+Esc / Option+Esc pass
  through untouched while recording; confirm behavior when the activation
  hotkey itself is bound to an Escape chord.

## Out of scope

- Cancelling during `Transcribing`/`Injecting`.
- Linux/X11 support (blocked on the pre-existing hotkey-listener stub).
- Any change to the Dismiss button or the `Failed`/`Unconfirmed` recovery
  flow.
