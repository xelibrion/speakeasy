# Esc Cancels Recording Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the user cancel an in-progress recording by pressing a bare Escape key, even though the widget showing it never takes keyboard focus.

**Architecture:** Esc-cancel rides the same macOS `CGEventTap` that already implements the activation hotkey (`speakeasy-platform/src/macos/hotkey.rs`), since that is the only thing in this app that sees keystrokes regardless of which app is focused. The engine gains a narrow `cancel_recording_if_active` method used only by this path; the existing `cancel()` (used by the widget's Dismiss button) is untouched. The `HotkeyListener` port gains a second callback, invoked on a bare Escape keydown, that the tap uses to decide whether to swallow the key.

**Tech Stack:** Rust workspace (`speakeasy-core`, `speakeasy-platform`, the `apps/desktop` Tauri shell), TypeScript/Vite frontend in `apps/desktop/src`.

## Global Constraints

- macOS-only: Linux hotkey listening (`speakeasy-platform/src/linux/hotkey.rs`) is an unimplemented stub; it must keep compiling but needs no real behavior.
- Esc cancels only from `State::Recording`. It does nothing during `Transcribing`, `Injecting`, `Unconfirmed`, `Failed`, or `Idle`.
- Esc only triggers cancel when pressed with **no modifiers held** (`mods.is_empty()`), so it never hijacks ⌘+Esc, Option+Esc, etc.
- If the currently bound activation hotkey is itself a `Hotkey::Chord` whose `key` is `KeyCode::Escape`, Esc-cancel is disabled entirely while that binding is active — the activation binding wins outright.
- Esc is swallowed (event nulled, including the matching key-up) only on the press that actually cancelled something. Every other Escape press passes through untouched.
- `helper` is a banned identifier anywhere in this codebase — do not name anything `*_helper`/`Helper`.
- Use `make test`, `make lint`, `make fmt-check` (or the underlying `cargo` commands they wrap) rather than ad hoc cargo invocations, per this repo's Makefile-first convention.

---

### Task 1: `Engine::cancel_recording_if_active`

**Files:**
- Modify: `crates/speakeasy-core/src/engine.rs` (new method near `cancel()` at line 622; new tests in the `mod tests` block starting at line 833)

**Interfaces:**
- Consumes: existing `Engine::state()` (`engine.rs:141`), `Engine::cancel()` (`engine.rs:622`), `State` (`event.rs`), the private test-only `Engine::transition()` (`engine.rs:779`) already used by other tests in this file, and the existing `engine(transcript: &str) -> (Engine, Arc<FakeSink>)` test fixture (`engine.rs:847`).
- Produces: `pub fn cancel_recording_if_active(&self) -> bool` on `Engine` — later tasks (the macOS tap, via the desktop shell's hotkey wrapper) call this and use the returned bool to decide whether to swallow a keystroke.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `crates/speakeasy-core/src/engine.rs` (near the other `State`-transition tests, e.g. after `toggle_ignores_a_press_while_transcription_is_in_flight`):

```rust
#[test]
fn cancel_recording_if_active_stops_a_live_recording() {
    let (engine, _sink) = engine("irrelevant");
    engine.begin_recording().expect("recording should start");

    let cancelled = engine.cancel_recording_if_active();

    assert!(cancelled);
    assert_eq!(engine.state(), State::Idle);
}

#[test]
fn cancel_recording_if_active_is_a_no_op_when_idle() {
    let (engine, _sink) = engine("irrelevant");

    let cancelled = engine.cancel_recording_if_active();

    assert!(!cancelled);
    assert_eq!(engine.state(), State::Idle);
}

#[test]
fn cancel_recording_if_active_is_a_no_op_outside_recording() {
    let (engine, _sink) = engine("irrelevant");
    let non_recording_states = [
        State::Transcribing,
        State::Injecting,
        State::Unconfirmed,
        State::Failed {
            error: Error::Injection("nope".to_string()),
        },
    ];

    for state in non_recording_states {
        engine.transition(state.clone());

        let cancelled = engine.cancel_recording_if_active();

        assert!(!cancelled, "expected no cancel from {state:?}");
        assert_eq!(engine.state(), state);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --package speakeasy-core cancel_recording_if_active`
Expected: FAIL with `no method named `cancel_recording_if_active` found`.

- [ ] **Step 3: Implement the method**

In `crates/speakeasy-core/src/engine.rs`, immediately after `pub fn cancel(&self) { ... }` (ends at line 630):

```rust
    /// Cancel a recording only if one is actually in progress; reports
    /// whether it did.
    ///
    /// Distinct from [`Engine::cancel`], which the widget's Dismiss button
    /// calls unconditionally from `Failed`/`Unconfirmed` — that contract must
    /// not gain a state check. This one backs the Esc-cancel hotkey, which
    /// only ever means "stop the recording," and the caller uses the return
    /// value to decide whether to swallow the keystroke that triggered it.
    pub fn cancel_recording_if_active(&self) -> bool {
        if self.state() != State::Recording {
            return false;
        }
        self.cancel();
        true
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --package speakeasy-core cancel_recording_if_active`
Expected: PASS (3 tests).

- [ ] **Step 5: Run the full core test suite and commit**

Run: `cargo test --package speakeasy-core`
Expected: PASS, no regressions.

```bash
git add crates/speakeasy-core/src/engine.rs
git commit -m "Add Engine::cancel_recording_if_active for the upcoming Esc-cancel hotkey"
```

---

### Task 2: Thread a cancel callback through `HotkeyListener` and act on it in the macOS tap

**Files:**
- Modify: `crates/speakeasy-core/src/ports.rs` (new `CancelCallback` type; extend `HotkeyListener::register`)
- Modify: `crates/speakeasy-core/src/lib.rs` (export `CancelCallback`)
- Modify: `crates/speakeasy-core/src/fakes.rs` (`FakeHotkeys` gains the parameter and a way for tests to fire it)
- Modify: `crates/speakeasy-platform/src/linux/hotkey.rs` (`X11Hotkeys::register` gains and ignores the parameter, same as it already ignores `hotkey`)
- Modify: `crates/speakeasy-platform/src/macos/hotkey.rs` (`MacHotkeys::register` gains the parameter and the tap callback acts on it)

**Interfaces:**
- Consumes: `Engine::cancel_recording_if_active` from Task 1 is **not** referenced here — this task stays inside `speakeasy-core`/`speakeasy-platform` and only threads a generic callback type. The engine is wired to it in Task 3.
- Produces:
  - `pub type CancelCallback = Box<dyn Fn() -> bool + Send + Sync>;` in `ports.rs`, re-exported from `lib.rs`.
  - `HotkeyListener::register(&self, hotkey: &Hotkey, on_event: HotkeyCallback, on_cancel: CancelCallback) -> Result<()>` — the new required signature every implementor (and every caller) must match.
  - `FakeHotkeys::fire_cancel(&self) -> bool` — test helper that invokes whatever `on_cancel` was last registered (panics via `.expect` if none is registered, matching the existing `fire`/`finish_capture` style... actually mirror `fire`'s silence-if-none behavior, see step below) and returns what it returned, for use in this crate's own tests and any future test in the desktop shell.

- [ ] **Step 1: Write the failing test for the fake's new capability**

Add to `crates/speakeasy-core/src/fakes.rs`, in its `mod tests` block (after `a_cancelled_capture_reports_nothing`):

```rust
    #[test]
    fn firing_cancel_invokes_whatever_on_cancel_was_registered() {
        let hotkeys = FakeHotkeys::default();
        hotkeys
            .register(
                &Hotkey::default(),
                Box::new(|_| {}),
                Box::new(|| true),
            )
            .expect("register succeeds");

        assert!(hotkeys.fire_cancel());
    }

    #[test]
    fn firing_cancel_with_nothing_registered_reports_false() {
        let hotkeys = FakeHotkeys::default();

        assert!(!hotkeys.fire_cancel());
    }
```

- [ ] **Step 2: Run the test to verify it fails to compile**

Run: `cargo test --package speakeasy-core firing_cancel`
Expected: FAIL to compile — `register` takes 2 arguments, `fire_cancel` does not exist.

- [ ] **Step 3: Add `CancelCallback` to the port and extend the trait**

In `crates/speakeasy-core/src/ports.rs`, right after the existing:
```rust
pub type HotkeyCallback = Box<dyn Fn(HotkeyEvent) + Send + Sync>;
```
add:
```rust
/// Invoked on a bare Escape keydown while a cancellable state is active.
/// Returns whether it consumed the event — the listener swallows the key
/// (and its matching key-up) only when this returns `true`.
pub type CancelCallback = Box<dyn Fn() -> bool + Send + Sync>;
```

Change the trait method:
```rust
    /// Register `hotkey` and invoke `on_event` for press and release.
    /// Replaces any previously registered binding.
    ///
    /// `on_cancel` is invoked separately, on a bare (unmodified) Escape
    /// keydown, so a recording can be cancelled with the keyboard even
    /// though `hotkey` itself is untouched. It is unrelated to `hotkey`
    /// except when `hotkey` is itself Escape-based, in which case an
    /// implementation must prefer `hotkey` and never call `on_cancel`.
    ///
    /// Implementations need not support every [`Hotkey`] shape; an unsupported
    /// one must fail with [`crate::Error::Hotkey`] rather than bind nothing.
    fn register(
        &self,
        hotkey: &Hotkey,
        on_event: HotkeyCallback,
        on_cancel: CancelCallback,
    ) -> Result<()>;
```

- [ ] **Step 4: Export `CancelCallback` from the crate root**

In `crates/speakeasy-core/src/lib.rs`, change:
```rust
pub use ports::{
    AudioSource, CaptureCallback, Delivery, HotkeyCallback, HotkeyEvent, HotkeyListener,
    SpeechSegment, SpeechSegmenter, TextSink, Transcriber,
};
```
to:
```rust
pub use ports::{
    AudioSource, CancelCallback, CaptureCallback, Delivery, HotkeyCallback, HotkeyEvent,
    HotkeyListener, SpeechSegment, SpeechSegmenter, TextSink, Transcriber,
};
```

- [ ] **Step 5: Update `FakeHotkeys` to store and expose the cancel callback**

In `crates/speakeasy-core/src/fakes.rs`, change the struct and both `register` impls:

```rust
#[derive(Default)]
pub struct FakeHotkeys {
    callback: Mutex<Option<HotkeyCallback>>,
    on_cancel: Mutex<Option<CancelCallback>>,
    bound: Mutex<Option<Hotkey>>,
    capturing: Mutex<Option<CaptureCallback>>,
}
```

Add alongside `fire`:
```rust
    /// Simulate a bare Escape keydown. Reports `false`, same as no key having
    /// been pressed, if nothing has registered a cancel callback yet.
    pub fn fire_cancel(&self) -> bool {
        self.on_cancel
            .lock()
            .expect("on_cancel mutex poisoned")
            .as_ref()
            .map(|on_cancel| on_cancel())
            .unwrap_or(false)
    }
```

Update the trait impl:
```rust
impl HotkeyListener for FakeHotkeys {
    fn register(
        &self,
        hotkey: &Hotkey,
        on_event: HotkeyCallback,
        on_cancel: CancelCallback,
    ) -> Result<()> {
        *self.bound.lock().expect("bound mutex poisoned") = Some(hotkey.clone());
        *self.callback.lock().expect("callback mutex poisoned") = Some(on_event);
        *self.on_cancel.lock().expect("on_cancel mutex poisoned") = Some(on_cancel);
        Ok(())
    }

    fn unregister(&self) -> Result<()> {
        *self.bound.lock().expect("bound mutex poisoned") = None;
        *self.callback.lock().expect("callback mutex poisoned") = None;
        *self.on_cancel.lock().expect("on_cancel mutex poisoned") = None;
        Ok(())
    }
    // ... capture / cancel_capture unchanged
}

impl HotkeyListener for Arc<FakeHotkeys> {
    fn register(
        &self,
        hotkey: &Hotkey,
        on_event: HotkeyCallback,
        on_cancel: CancelCallback,
    ) -> Result<()> {
        self.as_ref().register(hotkey, on_event, on_cancel)
    }
    // ... unregister / capture / cancel_capture unchanged, just delegate as today
}
```

- [ ] **Step 6: Run the fakes tests to verify they pass**

Run: `cargo test --package speakeasy-core fakes::`
Expected: still fails — the crate doesn't compile yet, because `speakeasy-platform`'s two `HotkeyListener` implementors haven't been updated. Continue to the next steps before re-running.

- [ ] **Step 7: Update the Linux stub**

In `crates/speakeasy-platform/src/linux/hotkey.rs`, change:
```rust
    fn register(&self, hotkey: &Hotkey, _on_event: HotkeyCallback) -> Result<()> {
        // TODO: ...
        let _ = hotkey;
        Err(Error::Hotkey(
            "X11 hotkey listening not implemented yet".to_string(),
        ))
    }
```
to:
```rust
    fn register(
        &self,
        hotkey: &Hotkey,
        _on_event: HotkeyCallback,
        _on_cancel: CancelCallback,
    ) -> Result<()> {
        // TODO: open an x11rb connection, XISelectEvents for raw key press and
        // release on the root window, and map ModifierKey to the keycodes for
        // Alt_L/Alt_R/Control_L/... via the keyboard mapping.
        let _ = hotkey;
        Err(Error::Hotkey(
            "X11 hotkey listening not implemented yet".to_string(),
        ))
    }
```
and add `CancelCallback` to its `use speakeasy_core::{...}` import list.

- [ ] **Step 8: Implement Escape-cancel in the macOS tap**

In `crates/speakeasy-platform/src/macos/hotkey.rs`:

Add `CancelCallback` to the `use speakeasy_core::{...}` import at the top, and add `KeyCode` (already available via `speakeasy_core::hotkey::{CaptureRecognizer, ChordMods, RawEventKind, RawKeyEvent}` — extend that import to also bring in `KeyCode`):
```rust
use speakeasy_core::hotkey::{CaptureRecognizer, ChordMods, KeyCode, RawEventKind, RawKeyEvent};
```

Change the trait method signature:
```rust
    fn register(
        &self,
        hotkey: &Hotkey,
        on_event: HotkeyCallback,
        on_cancel: CancelCallback,
    ) -> Result<()> {
```

Right after the existing:
```rust
        let is_down = Arc::new(AtomicBool::new(false));
```
add a second atomic and compute whether the bound hotkey itself claims Escape:
```rust
        // Tracks a swallowed Escape keydown through to its keyup, so the
        // up-stroke doesn't leak to whatever app is focused once the down-
        // stroke has already been consumed.
        let escape_swallowed = Arc::new(AtomicBool::new(false));
        // If the user's own binding is Escape-based, it wins outright: Esc
        // never also means cancel while that binding is live.
        let hotkey_claims_escape =
            matches!(hotkey, Hotkey::Chord { key: KeyCode::Escape, .. });
        let on_cancel = Arc::new(on_cancel);
```

Move the closures' captures: `on_cancel` needs to be cloned into the tap closure alongside the existing `port_for_callback`/`capture`/`callback` clones — add, near those `let ... = Arc::clone(...)` lines before `std::thread::Builder::new()`:
```rust
        let on_cancel_for_callback = Arc::clone(&on_cancel);
        let escape_swallowed_for_callback = Arc::clone(&escape_swallowed);
```

Inside the tap callback, after the existing capture-armed block (`if capture.armed() { ... return None; }`) and before `let edge = speakeasy_core::hotkey::match_event(&hotkey, raw)?;`, insert:
```rust
                        // A bare Escape is handled independently of the bound
                        // hotkey, unless the bound hotkey is itself
                        // Escape-based — that binding always wins.
                        if !hotkey_claims_escape && raw.key == Some(KeyCode::Escape) {
                            match event_type {
                                CGEventType::KeyDown if raw.mods.is_empty() => {
                                    let consumed = on_cancel_for_callback();
                                    escape_swallowed_for_callback
                                        .store(consumed, Ordering::SeqCst);
                                    if consumed {
                                        event.set_type(CGEventType::Null);
                                    }
                                    return None;
                                }
                                CGEventType::KeyUp
                                    if escape_swallowed_for_callback
                                        .swap(false, Ordering::SeqCst) =>
                                {
                                    event.set_type(CGEventType::Null);
                                    return None;
                                }
                                _ => {}
                            }
                        }
```

Note: `event_type`, `event`, and `raw` are already in scope in the existing closure (`raw` comes from `let raw = raw_event(event_type, event)?;` just above the capture-armed block). `hotkey_claims_escape` is `Copy`-free (`bool`), captured by the closure by copy automatically since it's `Copy`.

- [ ] **Step 9: Run the whole workspace build to confirm every implementor now matches**

Run: `cargo build --workspace`
Expected: PASS (compiles clean). If any implementor was missed, the compiler names it — fix and rerun.

- [ ] **Step 10: Run the fakes tests from Step 1**

Run: `cargo test --package speakeasy-core fakes::`
Expected: PASS.

- [ ] **Step 11: Run the full workspace test suite, lint, and commit**

Run: `make test && make lint`
Expected: both PASS.

```bash
git add crates/speakeasy-core/src/ports.rs crates/speakeasy-core/src/lib.rs \
        crates/speakeasy-core/src/fakes.rs crates/speakeasy-platform/src/linux/hotkey.rs \
        crates/speakeasy-platform/src/macos/hotkey.rs
git commit -m "Thread a cancel callback through HotkeyListener; swallow bare Escape on macOS"
```

---

### Task 3: Wire the desktop shell's hotkey registration to `Engine::cancel_recording_if_active`

**Files:**
- Modify: `apps/desktop/src-tauri/src/hotkey.rs:64-91` (the `register` function)

**Interfaces:**
- Consumes: `Engine::cancel_recording_if_active(&self) -> bool` (Task 1), `HotkeyListener::register(&self, hotkey, on_event, on_cancel)` (Task 2).
- Produces: nothing new for later tasks — this is the terminal wiring point for the backend half of the feature.

- [ ] **Step 1: Update the `register` function**

In `apps/desktop/src-tauri/src/hotkey.rs`, change:
```rust
pub fn register(engine: Engine, hotkey: &Hotkey) -> Result<(), Box<dyn std::error::Error>> {
    let listener = listener();

    listener.register(
        hotkey,
        Box::new(move |event| match engine.hotkey_action(event) {
            HotkeyAction::Begin => {
                if let Err(err) = engine.begin_recording() {
                    tracing::warn!(%err, "could not start recording");
                }
            }
            HotkeyAction::Finish => {
                // Transcription blocks; keep it off the tap's thread or the
                // event tap stalls and macOS eventually disables it.
                let engine = engine.clone();
                std::thread::spawn(move || {
                    if let Err(err) = engine.finish_recording() {
                        tracing::warn!(%err, "recording did not complete");
                    }
                });
            }
            HotkeyAction::Ignore => {}
        }),
    )?;

    BOUND.store(true, Ordering::SeqCst);
    Ok(())
}
```
to:
```rust
pub fn register(engine: Engine, hotkey: &Hotkey) -> Result<(), Box<dyn std::error::Error>> {
    let listener = listener();

    let cancel_engine = engine.clone();
    listener.register(
        hotkey,
        Box::new(move |event| match engine.hotkey_action(event) {
            HotkeyAction::Begin => {
                if let Err(err) = engine.begin_recording() {
                    tracing::warn!(%err, "could not start recording");
                }
            }
            HotkeyAction::Finish => {
                // Transcription blocks; keep it off the tap's thread or the
                // event tap stalls and macOS eventually disables it.
                let engine = engine.clone();
                std::thread::spawn(move || {
                    if let Err(err) = engine.finish_recording() {
                        tracing::warn!(%err, "recording did not complete");
                    }
                });
            }
            HotkeyAction::Ignore => {}
        }),
        Box::new(move || cancel_engine.cancel_recording_if_active()),
    )?;

    BOUND.store(true, Ordering::SeqCst);
    Ok(())
}
```

(`Engine` derives `Clone` — `engine.rs:47` — so cloning it once more here to move into the second closure is the same pattern already used for the `HotkeyAction::Finish` arm above.)

- [ ] **Step 2: Build the desktop shell to confirm it compiles**

Run: `cargo build --package speakeasy-desktop` (check the actual package name in `apps/desktop/src-tauri/Cargo.toml` if this fails — use whatever `[package].name` says)
Expected: PASS.

- [ ] **Step 3: Run the full workspace check and commit**

Run: `make check`
Expected: PASS.

```bash
git add apps/desktop/src-tauri/src/hotkey.rs
git commit -m "Cancel a live recording on Esc via the desktop shell's hotkey registration"
```

---

### Task 4: Mention Esc in the widget's recording hint

**Files:**
- Modify: `apps/desktop/src/engine.ts:257-260` (`stopHint`)

**Interfaces:**
- Consumes: existing `Config`, `Hotkey`, `hotkeyLabel` (all already in `engine.ts`).
- Produces: nothing new — this is the terminal, user-visible piece of the feature. `widget.ts` already calls `stopHint(config)` (`widget.ts:103`, `widget.ts:159`) and needs no changes; it just renders whatever string comes back.

- [ ] **Step 1: Update `stopHint`**

In `apps/desktop/src/engine.ts`, change:
```typescript
export function stopHint(config: Config): string {
  const key = hotkeyLabel(config.hotkey);
  return config.activation === "toggle" ? `Tap ${key} to stop` : `Release ${key} to insert`;
}
```
to:
```typescript
export function stopHint(config: Config): string {
  const key = hotkeyLabel(config.hotkey);
  const base = config.activation === "toggle" ? `Tap ${key} to stop` : `Release ${key} to insert`;

  // If the activation hotkey is itself Escape-based, Esc already means
  // something else while recording — see the matching precedence rule in
  // `speakeasy-platform`'s macOS hotkey tap. Mentioning cancel here too
  // would describe a binding that isn't actually live.
  const hotkeyIsEscape = config.hotkey.kind === "chord" && config.hotkey.key === "escape";
  return hotkeyIsEscape ? base : `${base} · Esc to cancel`;
}
```

- [ ] **Step 2: Type-check the frontend**

Run: `cd apps/desktop && npx tsc --noEmit`
Expected: PASS, no type errors.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/engine.ts
git commit -m "Mention Esc-to-cancel in the widget's recording hint"
```

---

### Task 5: Manual verification on a real Mac

This feature is a macOS `CGEventTap` behavior gated on Accessibility permission — nothing about it can be exercised in CI or a simulator. This task is the acceptance check for the whole plan.

**Files:** none — this task runs the built app, no code changes.

- [ ] **Step 1: Build and launch the dev app**

Run: `make dev`

- [ ] **Step 2: Confirm the hint text**

Start a recording (per your configured activation mode). Confirm the widget's hint line now reads e.g. `Release ⌥ to insert · Esc to cancel` (or the Tap-to-stop equivalent in toggle mode).

- [ ] **Step 3: Confirm Esc cancels a live recording**

While recording, press a bare Escape key. Confirm the widget disappears (state returns to idle) and no Escape keystroke reaches whatever app was focused underneath (e.g. it doesn't close a dialog or clear a text field there).

- [ ] **Step 4: Confirm Esc is inert once recording has moved on**

Start a recording, let it finish (transcribing/injecting), and press Escape immediately after — before and during that window Esc must do nothing to the dictation, and must reach the focused app normally.

- [ ] **Step 5: Confirm modified Escape passes through**

While recording, press ⌘+Escape (Force Quit dialog) and Option+Escape. Confirm the recording is unaffected in both cases and the modified chord behaves exactly as it does with Speakeasy not running (e.g. Force Quit still opens).

- [ ] **Step 6: Confirm the activation-hotkey-wins case**

In Settings, bind the activation hotkey to a chord that uses Escape (e.g. ⌘+Escape or a bare Escape-based chord, whatever the capture UI allows). Start a recording with it. Confirm:
- the hint no longer mentions "Esc to cancel"
- pressing the bound Escape-based chord behaves exactly as any other activation hotkey would (starts/stops per the configured mode) and does **not** additionally cancel via the Esc-cancel path.

Revert the hotkey binding back afterward if it was changed only for this test.

- [ ] **Step 7: Report results**

No commit for this task. If any check fails, return to the relevant earlier task, fix, and re-run its automated tests before re-verifying here.

---

## Self-Review Notes

- **Spec coverage:** Recording-only scope → Task 1 + constraint list. Bare-modifier requirement → Task 2 Step 8 (`raw.mods.is_empty()`). Swallow-only-on-actual-cancel + keyup swallow → Task 2 Step 8 (`escape_swallowed`). Activation-hotkey-wins → Task 2 Step 8 (`hotkey_claims_escape`) and Task 4 (hint suppression) and Task 5 Step 6 (manual check). Hint text → Task 4. macOS-only / Linux stub keeps compiling → Task 2 Step 7. Manual verification list from the spec's Testing section → Task 5.
- **Placeholder scan:** no TBD/TODO introduced (the one `TODO` retained in the Linux stub is pre-existing, untouched in meaning). All code steps show full code, not descriptions.
- **Type consistency:** `CancelCallback = Box<dyn Fn() -> bool + Send + Sync>` used identically in `ports.rs`, `lib.rs` export, `fakes.rs`, `linux/hotkey.rs`, `macos/hotkey.rs`, and `apps/desktop/src-tauri/src/hotkey.rs`. `cancel_recording_if_active` name and `-> bool` signature match between Task 1's definition and Task 3's call site.
