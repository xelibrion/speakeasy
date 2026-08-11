# Custom Hotkey, Tray Speak, and Side Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user bind the dictation hotkey by pressing it in settings — bare modifiers or full chords like `⌘⌥Space` — add a tray item that starts and stops a recording, and move the settings tabs to the side.

**Architecture:** `Hotkey::Accelerator { accelerator: String }` is replaced by a structured `Hotkey::Chord { mods, key }`. The decision "does this raw key event fire the hotkey?" moves into `speakeasy-core` as a pure function, so the fiddly chord cases are testable without a Mac; the macOS event tap becomes a thin adapter that translates `CGEvent` into a flat `RawKeyEvent`. The tap switches from `ListenOnly` to `Default` so a bound chord can be swallowed, which brings a new failure mode — macOS can disable a `Default` tap — handled by re-arming. Rebinding is shell-driven: core persists and emits `HotkeyChanged`, `bridge.rs` re-registers.

**Tech Stack:** Rust (4-crate workspace + Tauri shell), CoreGraphics event taps via `core-graphics`, TypeScript/Vite for the settings webview.

**Spec:** `docs/superpowers/specs/2026-08-11-custom-hotkey-and-side-tabs-design.md`

---

## Orientation for someone new to this codebase

Read these first. They are short and they explain the rules this plan obeys.

- `crates/speakeasy-core/src/lib.rs` — core is portable: no tauri, no cpal, no whisper, no OS APIs. The public surface is flat (owned types, no generics, no lifetimes, no borrowed returns) so it can be bound to Swift later. **Every type you add here must obey that.**
- `crates/speakeasy-core/src/ports.rs` — the five traits core needs from the outside world. You are extending `HotkeyListener`.
- `apps/desktop/src-tauri/src/commands.rs` — its doc comment says a non-trivial command body is a review failure. Every command you add is a one-line delegation.
- `Makefile` — `make check` runs fmt-check, clippy with `-D warnings`, tests, and `make boundary`. **Run `make check` before every commit.**

Conventions to match, visible throughout the existing code:

- Doc comments explain *why*, not *what*. `config.rs`'s comment on `Hotkey` is the register to write in.
- Test names are sentences: `a_chord_does_not_fire_when_an_extra_modifier_is_held`, not `test_chord`.
- Mutex locks use `.expect("<name> mutex poisoned")`.
- Errors are a flat enum with owned payloads.

**Three things that are easy to get wrong here:**

1. **Never swallow a bare modifier.** The default binding is Right Option, and consuming it breaks Option-based accented input (`⌥e → é`) for every user who never changes the setting.
2. **The tap callback runs on its own thread and must not block.** `hotkey.rs` already spawns a thread for `finish_recording` for exactly this reason — transcription blocks, and stalling the tap makes macOS disable it.
3. **AppKit calls must be on the main thread.** `bridge.rs` hops via `run_on_main_thread` before touching the tray. Skipping the hop is an immediate hard crash, not a race.

---

## File structure

**New files**

| Path | Responsibility |
|---|---|
| `crates/speakeasy-core/src/hotkey.rs` | `KeyCode`, `ChordMods`, `RawKeyEvent`, `match_event` — the pure matching logic |
| `crates/speakeasy-platform/src/macos/keycode.rs` | `KeyCode` ↔ `CGKeyCode` mapping, macOS-only |

**Modified files**

| Path | Change |
|---|---|
| `crates/speakeasy-core/src/config.rs` | `Hotkey::Chord` replaces `Accelerator`; legacy migration in deserialisation |
| `crates/speakeasy-core/src/ports.rs` | `capture` / `cancel_capture` on `HotkeyListener` |
| `crates/speakeasy-core/src/event.rs` | `Event::HotkeyChanged` |
| `crates/speakeasy-core/src/engine.rs` | `Engine::set_hotkey` |
| `crates/speakeasy-core/src/fakes.rs` | `FakeHotkeys` capture support |
| `crates/speakeasy-core/src/lib.rs` | Module + re-export wiring |
| `crates/speakeasy-platform/src/macos/hotkey.rs` | KeyDown/KeyUp, `Default` tap, swallow chords, re-arm, capture |
| `crates/speakeasy-platform/src/linux/hotkey.rs` | `capture` returning the not-implemented error |
| `apps/desktop/src-tauri/src/commands.rs` | `set_hotkey`, `capture_hotkey`, `cancel_hotkey_capture` |
| `apps/desktop/src-tauri/src/bridge.rs` | Re-register on `HotkeyChanged` |
| `apps/desktop/src-tauri/src/hotkey.rs` | `register` callable again post-startup |
| `apps/desktop/src-tauri/src/tray.rs` | `Speak` / `Stop and insert` / `Cancel` |
| `apps/desktop/src-tauri/src/main.rs` | Register the new commands |
| `apps/desktop/src/engine.ts` | `Hotkey` type, new invokes, `hotkeyLabel` |
| `apps/desktop/src/settings.ts` | Editable hotkey row; side-tab shell |
| `apps/desktop/src/styles.css` | Two-column grid, vertical tabs |
| `README.md` | Hotkey binding section |

---

## Task 1: The chord shape in config

**Files:**
- Create: `crates/speakeasy-core/src/hotkey.rs`
- Modify: `crates/speakeasy-core/src/config.rs:177-231`, `crates/speakeasy-core/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/speakeasy-core/src/config.rs`, in the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn a_chord_round_trips_through_json() {
    let hotkey = Hotkey::Chord {
        mods: ChordMods { command: true, option: true, control: false, shift: false },
        key: KeyCode::Space,
    };
    let text = serde_json::to_string(&hotkey).expect("serialises");
    let back: Hotkey = serde_json::from_str(&text).expect("deserialises");
    assert_eq!(back, hotkey);
}

#[test]
fn a_legacy_accelerator_config_falls_back_to_the_default_binding() {
    // Written by a version that had `Hotkey::Accelerator`. It never worked on
    // macOS — registering it returned Error::Hotkey — so there is nothing to
    // preserve, but the config as a whole must still load.
    let json = r#"{"hotkey":{"kind":"accelerator","accelerator":"CmdOrCtrl+Alt+Space"}}"#;
    let config: Config = serde_json::from_str(json).expect("config still loads");
    assert_eq!(config.hotkey, Config::default().hotkey);
}

#[test]
fn a_chord_labels_in_the_order_macos_writes_modifiers() {
    let hotkey = Hotkey::Chord {
        mods: ChordMods { command: true, option: true, control: true, shift: true },
        key: KeyCode::Space,
    };
    assert_eq!(hotkey.label(), "⌃⌥⇧⌘Space");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p speakeasy-core config`
Expected: FAIL — `ChordMods` and `KeyCode` do not exist.

- [ ] **Step 3: Create the hotkey module**

Create `crates/speakeasy-core/src/hotkey.rs` with `ChordMods` and `KeyCode`.

`ChordMods` is four bools rather than a bitflag set: flat owned fields are what `uniffi`/`swift-bridge` can express, and the binding layer has to stay mechanical.

```rust
//! The bindable key shapes, and the decision of whether a raw key event fires
//! the binding.
//!
//! This lives in core, not in the event tap, because chord matching is where
//! the fiddly cases are — an exact modifier match, a release that can arrive
//! as either the key or a modifier lifting — and logic inside a tap callback
//! is reachable only by pressing keys on a real Mac with Accessibility
//! granted. Here it is a pure function over a flat event.

/// The modifiers a chord requires, side-agnostic.
///
/// `⌘⌥Space` should not care which Command key. A bare modifier binding is the
/// one case where the side is the whole point, and that is `ModifierKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChordMods {
    pub command: bool,
    pub option: bool,
    pub control: bool,
    pub shift: bool,
}

/// A non-modifier key.
///
/// An enum rather than a raw keycode so the config is readable and portable:
/// a CGKeyCode means nothing on another platform, and nothing to a human
/// editing config.json. The mapping lives in the platform crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyCode {
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Digit0, Digit1, Digit2, Digit3, Digit4,
    Digit5, Digit6, Digit7, Digit8, Digit9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10,
    F11, F12, F13, F14, F15, F16, F17, F18, F19, F20,
    Space, Return, Escape, Tab, Delete,
    Left, Right, Up, Down,
}
```

Give `ChordMods` a `label()` returning the symbols in macOS order (`⌃⌥⇧⌘`) and `KeyCode` a `label()` returning its printable name (`Space`, `A`, `F13`).

- [ ] **Step 4: Replace the `Accelerator` variant**

In `config.rs`, replace the variant and update `Hotkey::label`:

```rust
    /// A conventional chord such as `⌘⌥Space`. Cannot distinguish sides, and
    /// unlike a bare modifier it is swallowed when bound — see the tap.
    Chord { mods: ChordMods, key: KeyCode },
```

```rust
            Self::Chord { mods, key } => format!("{}{}", mods.label(), key.label()),
```

- [ ] **Step 5: Make the legacy config load**

`Config::load` must not reject a config carrying the old variant. Add `#[serde(default = "default_hotkey")]` on the field and a custom deserialiser that maps an unknown `kind` to the default, logging a warning. This matches `load_config`'s existing stance: a menu-bar app that dies on a bad config leaves the user no UI to fix it with.

- [ ] **Step 6: Wire the module**

In `lib.rs`: `mod hotkey;` and re-export `ChordMods`, `KeyCode` alongside the existing `Hotkey` re-export.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p speakeasy-core config`
Expected: PASS, including the two pre-existing config tests.

- [ ] **Step 8: Commit**

```bash
make check
git add crates/speakeasy-core/
git commit -m "Replace the string accelerator with a structured chord"
```

---

## Task 2: Matching, as a pure function

This is the task that carries the logic risk. Take the tests seriously.

**Files:**
- Modify: `crates/speakeasy-core/src/hotkey.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Hotkey, ModifierKey};

    fn chord() -> Hotkey {
        Hotkey::Chord {
            mods: ChordMods { command: true, option: true, ..Default::default() },
            key: KeyCode::Space,
        }
    }

    fn modifier() -> Hotkey {
        Hotkey::Modifier { key: ModifierKey::RightOption }
    }

    #[test]
    fn a_chord_fires_when_the_key_goes_down_with_exactly_its_modifiers() {
        let event = RawKeyEvent::key_down(KeyCode::Space, ChordMods {
            command: true, option: true, ..Default::default()
        });
        assert_eq!(match_event(&chord(), event), Some(HotkeyEvent::Pressed));
    }

    #[test]
    fn a_chord_does_not_fire_when_an_extra_modifier_is_held() {
        // ⌘⌥Space must not fire on ⌘⇧⌥Space, or the binding collides with
        // every chord that merely contains it.
        let event = RawKeyEvent::key_down(KeyCode::Space, ChordMods {
            command: true, option: true, shift: true, ..Default::default()
        });
        assert_eq!(match_event(&chord(), event), None);
    }

    #[test]
    fn a_chord_does_not_fire_when_a_required_modifier_is_missing() {
        let event = RawKeyEvent::key_down(KeyCode::Space, ChordMods {
            command: true, ..Default::default()
        });
        assert_eq!(match_event(&chord(), event), None);
    }

    #[test]
    fn a_chord_releases_when_its_key_lifts() {
        let event = RawKeyEvent::key_up(KeyCode::Space, ChordMods {
            command: true, option: true, ..Default::default()
        });
        assert_eq!(match_event(&chord(), event), Some(HotkeyEvent::Released));
    }

    #[test]
    fn a_chord_releases_when_a_required_modifier_lifts_before_the_key() {
        // Lifting ⌘ while Space is still down ends the hold. Without this the
        // engine waits for a key-up that push-to-talk will never see.
        let event = RawKeyEvent::flags_changed(ChordMods {
            option: true, ..Default::default()
        });
        assert_eq!(match_event(&chord(), event), Some(HotkeyEvent::Released));
    }

    #[test]
    fn an_unrelated_key_is_ignored() {
        let event = RawKeyEvent::key_down(KeyCode::A, ChordMods {
            command: true, option: true, ..Default::default()
        });
        assert_eq!(match_event(&chord(), event), None);
    }

    #[test]
    fn a_modifier_binding_ignores_ordinary_keys() {
        let event = RawKeyEvent::key_down(KeyCode::Space, ChordMods::default());
        assert_eq!(match_event(&modifier(), event), None);
    }

    #[test]
    fn a_modifier_binding_fires_on_its_own_side_only() {
        let down = RawKeyEvent::modifier_changed(&[ModifierKey::RightOption]);
        assert_eq!(match_event(&modifier(), down), Some(HotkeyEvent::Pressed));

        let other = RawKeyEvent::modifier_changed(&[ModifierKey::LeftOption]);
        assert_eq!(match_event(&modifier(), other), None);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p speakeasy-core hotkey`
Expected: FAIL — `RawKeyEvent` and `match_event` do not exist.

- [ ] **Step 3: Implement `RawKeyEvent` and `match_event`**

`RawKeyEvent` is a flat description of what a tap saw — kind, optional keycode, modifier state — so core never learns about CoreGraphics.

`match_event` is stateless. It returns `Some(Pressed)` / `Some(Released)` / `None`. The **caller** suppresses repeats: the existing tap already holds an `AtomicBool` and only acts on a transition, and that stays its job. Document this on the function, because a stateless matcher that fires on every key-repeat is the obvious way to misuse it.

Release-by-modifier: a `FlagsChanged` event whose modifiers no longer satisfy the chord is a release. Per the spec, the first release wins — the tap's existing `is_down` transition check gives that for free, so the extra modifiers lifting afterwards produce no second event.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p speakeasy-core hotkey`
Expected: PASS, all 8.

- [ ] **Step 5: Commit**

```bash
make check
git add crates/speakeasy-core/src/hotkey.rs
git commit -m "Match hotkey events with a pure function in core"
```

---

## Task 3: `Engine::set_hotkey` and the `HotkeyChanged` event

**Files:**
- Modify: `crates/speakeasy-core/src/event.rs:57-90`, `crates/speakeasy-core/src/engine.rs`

- [ ] **Step 1: Write the failing test**

In `engine.rs`'s test module, following the existing fake-wiring pattern (see `unused_models_dir` and its neighbours):

```rust
#[test]
fn setting_the_hotkey_persists_it_and_announces_the_change() {
    let (engine, _fakes) = test_engine();
    let events = engine.subscribe();

    let hotkey = Hotkey::Chord {
        mods: ChordMods { command: true, ..Default::default() },
        key: KeyCode::Space,
    };
    engine.set_hotkey(hotkey.clone());

    assert_eq!(engine.config().hotkey, hotkey);
    assert!(events.try_iter().any(|event| matches!(
        event,
        Event::HotkeyChanged { hotkey: ref h } if *h == hotkey
    )));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p speakeasy-core setting_the_hotkey`
Expected: FAIL — no `set_hotkey`.

- [ ] **Step 3: Add the event**

In `event.rs`, on `Event`:

```rust
    /// The binding changed. The shell re-registers on this; core cannot,
    /// because the listener is owned by the shell.
    HotkeyChanged {
        hotkey: Hotkey,
    },
```

- [ ] **Step 4: Add `Engine::set_hotkey`**

Mirror `set_widget_position`'s shape — its own method rather than a `set_config` round-trip, so the settings window and the capture flow cannot race a read-modify-write. Mutate under the config lock, `persist()`, then emit. **Emit after releasing the lock**, as the existing methods do: emitting while holding it invites a deadlock through a subscriber that calls back in.

- [ ] **Step 5: Run the test**

Run: `cargo test -p speakeasy-core`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
make check
git add crates/speakeasy-core/
git commit -m "Add Engine::set_hotkey and the HotkeyChanged event"
```

---

## Task 4: Capture on the `HotkeyListener` port

**Files:**
- Modify: `crates/speakeasy-core/src/ports.rs:95-105`, `crates/speakeasy-core/src/fakes.rs:318-360`, `crates/speakeasy-platform/src/linux/hotkey.rs`

- [ ] **Step 1: Write the failing test**

In `fakes.rs`'s tests, or `engine.rs` if that is where listener tests live:

```rust
#[test]
fn a_capture_reports_the_key_that_was_pressed() {
    let hotkeys = Arc::new(FakeHotkeys::default());
    let captured = Arc::new(Mutex::new(None));

    let sink = Arc::clone(&captured);
    hotkeys
        .capture(Box::new(move |hotkey| {
            *sink.lock().expect("captured mutex poisoned") = Some(hotkey);
        }))
        .expect("capture arms");

    let pressed = Hotkey::Modifier { key: ModifierKey::LeftCommand };
    hotkeys.finish_capture(pressed.clone());

    assert_eq!(*captured.lock().expect("captured mutex poisoned"), Some(pressed));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p speakeasy-core capture`
Expected: FAIL — no `capture` on the trait.

- [ ] **Step 3: Extend the trait**

```rust
/// Invoked once with whatever the user pressed, then the capture disarms.
pub type CaptureCallback = Box<dyn Fn(Hotkey) + Send + Sync>;
```

```rust
    /// Listen for one binding and report it, instead of matching the bound one.
    ///
    /// While armed, matching events are swallowed — otherwise binding ⌘Q would
    /// quit the app on the way in. Implementations must disarm themselves after
    /// one capture, and must not leave the keyboard captured if the caller
    /// disappears: cancel on a timeout.
    fn capture(&self, on_captured: CaptureCallback) -> Result<()>;

    fn cancel_capture(&self);
```

- [ ] **Step 4: Implement for `FakeHotkeys` and the X11 listener**

`FakeHotkeys` stores the callback and gains `finish_capture(Hotkey)` for tests to drive it.

The X11 listener returns the same not-implemented error its `register` already returns. The port contract requires an unsupported shape to fail loudly rather than bind nothing.

- [ ] **Step 5: Run the tests**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
make check
git add crates/
git commit -m "Add hotkey capture to the HotkeyListener port"
```

---

## Task 5: Keycode mapping on macOS

**Files:**
- Create: `crates/speakeasy-platform/src/macos/keycode.rs`
- Modify: `crates/speakeasy-platform/src/macos/mod.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn every_keycode_maps_in_both_directions() {
    // A one-way table silently drops keys during capture: the user presses F13,
    // the tap reads 0x69, and nothing comes back.
    for key in KeyCode::ALL {
        let raw = to_cg_keycode(key);
        assert_eq!(from_cg_keycode(raw), Some(key), "{key:?} did not round-trip");
    }
}

#[test]
fn an_unmapped_keycode_is_none_rather_than_a_wrong_key() {
    assert_eq!(from_cg_keycode(0xFFFF), None);
}
```

Add a `KeyCode::ALL` constant in core alongside the enum for this.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p speakeasy-platform keycode`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Write the mapping**

Values come from `Carbon/HIToolbox/Events.h` (`kVK_ANSI_A = 0x00`, `kVK_Space = 0x31`, `kVK_F13 = 0x69`, …). Write both directions from one `const` table so they cannot drift.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p speakeasy-platform keycode`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
make check
git add crates/speakeasy-platform/
git commit -m "Map KeyCode to CGKeyCode on macOS"
```

---

## Task 6: The tap learns chords and swallowing

No automated tests — this needs a real tap and a granted permission. Verify by hand, and keep the callback thin so the logic under it stays the tested `match_event`.

**Files:**
- Modify: `crates/speakeasy-platform/src/macos/hotkey.rs`

- [ ] **Step 1: Widen the tap**

Add `CGEventType::KeyDown` and `CGEventType::KeyUp` to the event list beside `FlagsChanged`.

- [ ] **Step 2: Translate to `RawKeyEvent` and delegate**

Replace the inline flag comparison with: build a `RawKeyEvent` from the `CGEvent`, call `match_event`, keep the existing `AtomicBool` transition check so repeats are suppressed, and fire the callback.

- [ ] **Step 3: Swallow bound chords**

Switch `CGEventTapOptions::ListenOnly` to `Default`. Return `None` from the callback **only** when the binding is a `Chord` and it matched; otherwise return the event unchanged.

Update the module doc comment — it currently states the tap is `ListenOnly` and never swallows, which stops being true here. Say instead: chords are swallowed so they do not also fire in the focused app, bare modifiers never are, because the default binding is Right Option and consuming it would break `⌥e → é`.

- [ ] **Step 4: Re-arm a disabled tap**

Handle `CGEventType::TapDisabledByTimeout` and `TapDisabledByUserInput` by re-enabling the tap and logging at `warn`.

This is the whole reason a `Default` tap is riskier than the old one. Without this the app silently goes deaf and the only cure is a restart — the most likely "worked yesterday, dead today" report this feature can produce.

- [ ] **Step 5: Verify by hand**

```bash
make dev
```

- Bind stays Right Option: hold it, dictation starts, and `⌥e` still types `é`.
- Set `"hotkey": {"kind":"chord","mods":{"command":true,"option":true,"control":false,"shift":false},"key":"space"}` in `~/Library/Application Support/com.speakeasy.app/config.json`, restart, press `⌘⌥Space`: dictation starts and Spotlight does *not* open.
- `⌘⇧⌥Space` does nothing.

- [ ] **Step 6: Commit**

```bash
make check
git add crates/speakeasy-platform/
git commit -m "Bind chords in the macOS tap and swallow them"
```

---

## Task 7: Capture in the tap

**Files:**
- Modify: `crates/speakeasy-platform/src/macos/hotkey.rs`

- [ ] **Step 1: Implement `capture`**

Arm a flag the existing tap callback checks first. While armed, every `KeyDown` and every `FlagsChanged` that raises a modifier is turned into a `Hotkey` — a `KeyDown` becomes `Chord`, a lone modifier becomes `Modifier` — reported once, and swallowed.

Reuse the running tap rather than creating a second one. A webview `keydown` listener would be cheaper, but macOS never delivers Fn to a webview, so a bindable key would not be capturable.

- [ ] **Step 2: Disarm safely**

Disarm after one capture, on `cancel_capture`, and on a 10-second timeout. A capture that outlives its window would swallow the keyboard with no UI to stop it.

- [ ] **Step 3: Handle the no-permission case**

If the tap does not exist because Accessibility was never granted, `capture` returns `Error::InputPermissionDenied` — the same error `register` already returns, so the UI has one case to render.

- [ ] **Step 4: Verify by hand**

`make dev`, open settings, click Change, press Fn — it is reported. Press `⌘Q` — it is captured and the app does *not* quit. Press Esc — capture cancels.

- [ ] **Step 5: Commit**

```bash
make check
git add crates/speakeasy-platform/
git commit -m "Capture a binding through the existing tap"
```

---

## Task 8: Shell commands and re-registration

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`, `bridge.rs:20-67`, `hotkey.rs`, `main.rs`

- [ ] **Step 1: Add the commands**

One-line delegations, as the module doc requires:

```rust
#[tauri::command]
pub fn set_hotkey(engine: TauriState<'_, Engine>, hotkey: Hotkey) {
    engine.set_hotkey(hotkey);
}
```

`capture_hotkey` delegates to the listener and resolves with the captured `Hotkey`; `cancel_hotkey_capture` delegates to `cancel_capture`.

- [ ] **Step 2: Register them** in `main.rs`'s `invoke_handler`.

- [ ] **Step 3: Re-register on the event**

In `bridge.rs`, beside the existing `ModelStatusChanged` arm:

```rust
if let Event::HotkeyChanged { hotkey } = &event {
    if let Err(err) = crate::hotkey::register(engine.clone(), hotkey) {
        tracing::warn!(%err, "could not bind the new hotkey");
    }
}
```

No main-thread hop: this touches an event tap, not AppKit. `register` already calls `unregister` first, so rebinding needs no lifecycle change.

- [ ] **Step 4: Verify by hand**

`make dev`, change the binding in settings, confirm the new key works **without a restart** and the old one no longer does.

- [ ] **Step 5: Commit**

```bash
make check
git add apps/desktop/src-tauri/
git commit -m "Rebind the hotkey without a restart"
```

---

## Task 9: Tray Speak

**Files:**
- Modify: `apps/desktop/src-tauri/src/tray.rs`

- [ ] **Step 1: Add the items**

Menu items cannot be reordered after creation, so build `speak` and `cancel` up front, between the status line and Settings. Store both in statics beside `STATUS_ITEM`.

- [ ] **Step 2: Rewrite them from state**

Extend `render()`: while `State::Recording`, `speak` reads "Stop and insert" and `cancel` is enabled and visible; otherwise `speak` reads "Speak" and `cancel` is disabled. `render` is already called by `refresh` on every transition, so nothing new subscribes.

- [ ] **Step 3: Handle the clicks**

In `on_menu_event`: `"speak"` calls `begin_recording` or `finish_recording` depending on state, `"cancel"` calls `engine.cancel()`.

**The finish path must be spawned onto a thread**, exactly as `hotkey.rs` does — transcription blocks, and this handler runs on the main thread where blocking freezes the whole UI.

- [ ] **Step 4: Verify by hand**

`make dev`. Click Speak, talk, reopen the menu, click Stop and insert — text lands in the previously focused app. Check the transcript arrives in the right window: the menu takes key focus while open, and this confirms it hands it back.

- [ ] **Step 5: Commit**

```bash
make check
git add apps/desktop/src-tauri/src/tray.rs
git commit -m "Start and stop dictation from the tray"
```

---

## Task 10: Frontend types and invokes

**Files:**
- Modify: `apps/desktop/src/engine.ts:119-138`

- [ ] **Step 1: Update the `Hotkey` type**

```ts
export type ChordMods = { command: boolean; option: boolean; control: boolean; shift: boolean };

/** A held modifier, or a conventional chord. See `core::config::Hotkey`. */
export type Hotkey =
  | { kind: "modifier"; key: ModifierKey }
  | { kind: "chord"; mods: ChordMods; key: KeyCode };
```

- [ ] **Step 2: Add the invokes** — `setHotkey`, `captureHotkey`, `cancelHotkeyCapture`.

- [ ] **Step 3: Add `hotkeyLabel(hotkey)`**

Mirrors `Hotkey::label` in Rust: `⌃⌥⇧⌘` in macOS order, then the key name. Note in a comment that the two must agree, since the tray renders the Rust one and settings renders this one.

- [ ] **Step 4: Typecheck**

Run: `cd apps/desktop && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/engine.ts
git commit -m "Type the chord hotkey in the frontend"
```

---

## Task 11: The editable hotkey row

**Files:**
- Modify: `apps/desktop/src/settings.ts:123-136`

- [ ] **Step 1: Replace the read-only row**

Three states, per the spec:

```
idle       Hotkey   Right Option    [ Change ]
listening  Hotkey   Press a key…    [ Cancel ]
rejected   Hotkey   Press a key…    [ Cancel ]
                    <error text>
```

Clicking Change calls `captureHotkey`; the resolved binding goes straight to `setHotkey`. Keep the file's rule — render what the core reports, decide nothing. **No client-side validity check:** whether a binding is usable is the platform's answer, and duplicating it here is exactly the drift this codebase is arranged to avoid.

- [ ] **Step 2: Handle the permission failure**

If `captureHotkey` rejects with `InputPermissionDenied`, show the message under the row. The banner already explains the fix.

- [ ] **Step 3: Cancel on unmount**

Add a `releasePanel` teardown that calls `cancelHotkeyCapture` — otherwise switching tabs mid-capture leaves the tap armed and swallowing keys with nothing on screen to explain it.

- [ ] **Step 4: Narrow the note**

"These are not editable yet" now applies only to the remaining rows. Reword to say insertion and language are still config-file-only.

- [ ] **Step 5: Verify by hand**

`make dev`: bind Right Command, confirm it works; bind `⌘⌥Space`, confirm it works and Spotlight stays shut; start a capture and switch tabs, confirm the keyboard is released.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/settings.ts
git commit -m "Bind the hotkey by pressing it in settings"
```

---

## Task 12: Side tabs

**Files:**
- Modify: `apps/desktop/src/styles.css:26-88`, `apps/desktop/src/settings.ts:59-100`

- [ ] **Step 1: Grid the shell**

`.settings` becomes `display: grid; grid-template-columns: auto 1fr;` with the banner spanning both columns, so a missing Accessibility grant still cannot hide behind a tab.

- [ ] **Step 2: Turn the tab strip vertical**

`.tabs` becomes `flex-direction: column`, the `border-bottom` becomes a `border-right`, and the selected indicator moves from `inset 0 -2px 0` to `inset 2px 0 0`. Give `.tab` `text-align: left`.

- [ ] **Step 3: Check the TS**

The tab list is already data-driven and its `role`/`aria-selected` handling is already correct. Only the append order should need attention. Do not restructure `mountSettings`.

- [ ] **Step 4: Verify by hand**

`make dev`: tabs on the left, selection indicator on the correct edge, banner full width when Accessibility is revoked, window still usable at its 520px width.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/
git commit -m "Move the settings tabs to the side"
```

---

## Task 13: Documentation

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the hotkey description**

The README says `Config::activation` switches between tap and hold. Add that the binding itself is now set in Settings › Dictation by pressing the key, that chords are swallowed while bare modifiers are not, and why.

- [ ] **Step 2: Note the X11 gap** in whatever section covers Linux: chords are macOS-only for now, and the X11 listener returns `Error::Hotkey`.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "Document hotkey binding"
```

---

## Verification before calling this done

- [ ] `make check` passes.
- [ ] Default install still binds Right Option, and `⌥e` still types `é`.
- [ ] A chord binds, fires, and does not reach the focused app.
- [ ] `⌘⇧⌥Space` does not fire a `⌘⌥Space` binding.
- [ ] Rebinding works with no restart.
- [ ] Capture reports Fn.
- [ ] Capturing `⌘Q` does not quit the app.
- [ ] Switching tabs mid-capture releases the keyboard.
- [ ] Tray Speak starts, Stop and insert delivers text to the right app, Cancel discards.
- [ ] A config with the legacy `accelerator` hotkey still loads.
- [ ] Revoking Accessibility mid-session: the banner appears and capture reports the permission error rather than hanging.
