# Per-keyboard hotkey bindings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Speakeasy remembers a separate push-to-talk binding for the Mac's built-in keyboard and for any attached external keyboard, and switches between them automatically as keyboards attach and detach.

**Architecture:** `Config` grows a `hotkeys: HotkeyBindings { built_in, external }` in place of its single `hotkey` field. A new macOS-only `KeyboardWatcher` port (backed by `IOHIDManager`) reports which bucket is active; the shell rebinds the `CGEventTap` to that bucket's `Hotkey` whenever it changes. Settings shows both bindings as two independently editable rows.

**Tech Stack:** Rust workspace (`speakeasy-core`, `speakeasy-platform`, Tauri shell in `apps/desktop/src-tauri`), TypeScript/vanilla-DOM frontend in `apps/desktop/src`. macOS: `core-foundation`/`core-graphics` crates plus raw FFI to `IOKit`'s `IOHIDManager` (no existing Rust binding for it in this workspace).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-08-17-per-keyboard-hotkey-bindings-design.md`.
- `speakeasy-core` must never depend on `tauri` (enforced by `make boundary`).
- All `cfg(target_os)` branching stays inside `speakeasy-platform` (see that crate's `lib.rs` module doc).
- Formatting/lint gate: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` (Makefile: `fmt-check`, `lint`). Frontend gate: `npm run build` (runs `tsc --noEmit` then `vite build`) from `apps/desktop`.
- **This is a multi-crate change.** `speakeasy-core`'s public API changes in Tasks 1–3 before the Tauri shell (`apps/desktop/src-tauri`) is updated in Task 9 — so `cargo build --workspace` / `make check` will fail between those tasks. Every task below scopes its own build/test/lint commands to the crate(s) it touches (`-p speakeasy-core`, `-p speakeasy-platform`); only Task 9's final step runs the full `make check`, and only Task 12's final step runs the frontend build. Do not be alarmed by a red full-workspace build before then.
- macOS only for the watcher itself; Linux gets a stub (mirrors `X11Hotkeys::register`, already unimplemented there).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/speakeasy-core/src/config.rs` | `KeyboardBucket`, `HotkeyBindings`, `Config.hotkeys`, legacy-field migration |
| `crates/speakeasy-core/src/ports.rs` | `KeyboardWatcher` trait, `KeyboardBucketCallback` |
| `crates/speakeasy-core/src/event.rs` | `Event::HotkeyChanged` gains `bucket`; new `Event::ActiveBucketChanged` |
| `crates/speakeasy-core/src/engine.rs` | Bucket-aware `set_hotkey`; new `active_bucket`/`set_active_bucket` |
| `crates/speakeasy-core/src/lib.rs` | Re-exports |
| `crates/speakeasy-platform/src/macos/keyboard_watch.rs` (new) | `BucketTracker` (pure) + `MacKeyboardWatcher` (`IOHIDManager` FFI) |
| `crates/speakeasy-platform/src/macos/mod.rs` | Export `MacKeyboardWatcher`, wire it in |
| `crates/speakeasy-platform/src/linux/keyboard_watch.rs` (new) | `X11KeyboardWatcher` stub |
| `crates/speakeasy-platform/src/linux/mod.rs` | Wire the stub in |
| `crates/speakeasy-platform/src/lib.rs` | `pub fn keyboard_watcher()` |
| `apps/desktop/src-tauri/src/hotkey.rs` | Bucket-aware `capture`; new `watch_keyboard` |
| `apps/desktop/src-tauri/src/bridge.rs` | `HotkeyChanged`/`ActiveBucketChanged` handling |
| `apps/desktop/src-tauri/src/commands.rs` | `set_hotkey(bucket, hotkey)`, new `active_keyboard_bucket`, `bind_hotkey` |
| `apps/desktop/src-tauri/src/main.rs` | Startup wiring, `invoke_handler` list |
| `apps/desktop/src/engine.ts` | Typed mirror: `KeyboardBucket`, `Config.hotkeys`, events, commands, `stopHint`/`activationLabel` signatures |
| `apps/desktop/src/widget.ts` | Resolve the active bucket's hotkey for the recording hint |
| `apps/desktop/src/settings.ts` | Two hotkey rows with an active-bucket indicator |
| `apps/desktop/src/styles.css` | `.setting-active` badge style |

---

### Task 1: Core config — `KeyboardBucket` and `HotkeyBindings`

**Files:**
- Modify: `crates/speakeasy-core/src/config.rs`

**Interfaces:**
- Produces: `pub enum KeyboardBucket { BuiltIn, External }` (Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, `rename_all = "camelCase"`); `pub struct HotkeyBindings { pub built_in: Hotkey, pub external: Hotkey }` with `pub fn get(&self, bucket: KeyboardBucket) -> &Hotkey` and `pub fn set(&mut self, bucket: KeyboardBucket, hotkey: Hotkey)`; `Config.hotkeys: HotkeyBindings` replacing `Config.hotkey: Hotkey`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `config.rs`:

```rust
#[test]
fn hotkey_bindings_default_to_right_option_and_menu() {
    let bindings = HotkeyBindings::default();
    assert_eq!(bindings.built_in, Hotkey::Modifier { key: ModifierKey::RightOption });
    assert_eq!(
        bindings.external,
        Hotkey::Chord { mods: ChordMods::default(), key: KeyCode::ContextMenu }
    );
}

#[test]
fn hotkey_bindings_get_and_set_route_to_the_right_slot() {
    let mut bindings = HotkeyBindings::default();
    let chord = Hotkey::Chord {
        mods: ChordMods { command: true, ..Default::default() },
        key: KeyCode::Space,
    };
    bindings.set(KeyboardBucket::External, chord.clone());
    assert_eq!(bindings.get(KeyboardBucket::External), &chord);
    assert_eq!(bindings.get(KeyboardBucket::BuiltIn), &Hotkey::default());
}

#[test]
fn hotkey_bindings_round_trip_through_json() {
    let mut bindings = HotkeyBindings::default();
    bindings.set(
        KeyboardBucket::External,
        Hotkey::Chord { mods: ChordMods::default(), key: KeyCode::F13 },
    );
    let text = serde_json::to_string(&bindings).expect("serialises");
    let back: HotkeyBindings = serde_json::from_str(&text).expect("deserialises");
    assert_eq!(back, bindings);
}

/// A config written before this feature has a bare top-level `hotkey`, not
/// `hotkeys`. It must migrate into both buckets, preserving whatever custom
/// binding was there — the same "absent vs. explicitly default" guard
/// `model_path` migration already uses.
#[test]
fn a_legacy_single_hotkey_config_migrates_to_both_buckets() {
    let dir = TempDir::new("legacy-hotkey-migration");
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"hotkey":{"kind":"chord","mods":{"command":true,"option":true,"control":false,"shift":false},"key":"space"}}"#,
    )
    .expect("write should succeed");

    let config = Config::load(&path).expect("a legacy config should load");

    let expected = Hotkey::Chord {
        mods: ChordMods { command: true, option: true, control: false, shift: false },
        key: KeyCode::Space,
    };
    assert_eq!(config.hotkeys.built_in, expected);
    assert_eq!(config.hotkeys.external, expected);
    assert_eq!(config.hotkey, None, "the legacy field is consumed");
}

/// The legacy `Accelerator` shape maps to the ordinary default binding (see
/// the existing `HotkeyRepr` migration below), which then propagates to both
/// buckets the same way any other legacy value would.
#[test]
fn a_legacy_accelerator_config_falls_back_to_the_default_binding_on_both_buckets() {
    let dir = TempDir::new("legacy-hotkey");
    let path = dir.join("config.json");
    std::fs::write(
        &path,
        r#"{"hotkey":{"kind":"accelerator","accelerator":"CmdOrCtrl+Alt+Space"}}"#,
    )
    .expect("write should succeed");

    let config = Config::load(&path).expect("a legacy config should still load");
    assert_eq!(config.hotkeys.built_in, Hotkey::default());
    assert_eq!(config.hotkeys.external, Hotkey::default());
}

#[test]
fn a_saved_config_does_not_write_the_legacy_hotkey_field() {
    let dir = TempDir::new("no-legacy-hotkey-field");
    let path = dir.join("config.json");

    Config::default().save(&path).expect("save should succeed");

    let text = std::fs::read_to_string(&path).expect("read should succeed");
    assert!(
        !text.contains("\"hotkey\":"),
        "unexpected legacy field: {text}"
    );
}
```

Delete the now-obsolete `a_legacy_accelerator_config_falls_back_to_the_default_binding` test (it asserted against the old `config.hotkey` field, replaced by the two tests above).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p speakeasy-core --lib config:: -- --include-ignored`
Expected: compile error — `HotkeyBindings`, `KeyboardBucket` don't exist yet, `Config` has no `hotkeys` field.

- [ ] **Step 3: Implement `KeyboardBucket` and `HotkeyBindings`**

Add near the top of `config.rs`, after the `Hotkey` type:

```rust
/// Which physical keyboard is currently driving the binding.
///
/// Two buckets, not per-device identity: any external keyboard attached
/// selects [`KeyboardBucket::External`], regardless of which model it is or
/// how many are plugged in at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyboardBucket {
    BuiltIn,
    External,
}

/// A separate push-to-talk binding for each [`KeyboardBucket`].
///
/// Two bindings because the natural key differs by hardware: Right Option
/// (the default `built_in` binding) exists on every Mac keyboard and on no
/// PC keyboard, while the Menu key (the default `external` binding) is the
/// reverse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyBindings {
    pub built_in: Hotkey,
    pub external: Hotkey,
}

impl HotkeyBindings {
    pub fn get(&self, bucket: KeyboardBucket) -> &Hotkey {
        match bucket {
            KeyboardBucket::BuiltIn => &self.built_in,
            KeyboardBucket::External => &self.external,
        }
    }

    pub fn set(&mut self, bucket: KeyboardBucket, hotkey: Hotkey) {
        match bucket {
            KeyboardBucket::BuiltIn => self.built_in = hotkey,
            KeyboardBucket::External => self.external = hotkey,
        }
    }
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            built_in: Hotkey::default(),
            // No Apple keyboard has this key, which is exactly why it is a
            // good push-to-talk binding for anything else — see
            // `KeyCode::ContextMenu`'s own doc comment.
            external: Hotkey::Chord {
                mods: ChordMods::default(),
                key: KeyCode::ContextMenu,
            },
        }
    }
}
```

In `Config`, replace:

```rust
    /// The key that drives dictation.
    pub hotkey: Hotkey,
```

with:

```rust
    /// The key that drives dictation, one binding per [`KeyboardBucket`].
    pub hotkeys: HotkeyBindings,

    /// Pre-`HotkeyBindings` setting, read once and migrated into both
    /// buckets of [`Config::hotkeys`].
    ///
    /// Kept only so an existing config does not lose its binding on upgrade.
    /// [`Config::load`] clears it, so it disappears from the file on the next
    /// save — same treatment as [`Config::model_path`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<Hotkey>,
```

In `Config::migrate`, after the existing `model_path` block:

```rust
        if let Some(hotkey) = self.hotkey.take() {
            if self.hotkeys == HotkeyBindings::default() {
                self.hotkeys.built_in = hotkey.clone();
                self.hotkeys.external = hotkey;
            }
        }
```

In `impl Default for Config`, replace `hotkey: Hotkey::default(),` with:

```rust
            hotkeys: HotkeyBindings::default(),
            hotkey: None,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p speakeasy-core --lib config::`
Expected: PASS, including the pre-existing config tests (`config_survives_a_save_and_load`, `a_config_missing_new_fields_still_loads`, etc.) — none of them reference the old `hotkey` field directly, so they should be unaffected.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt -p speakeasy-core
cargo clippy -p speakeasy-core --all-targets -- -D warnings
git add crates/speakeasy-core/src/config.rs
git commit -m "core: replace Config::hotkey with per-keyboard-bucket bindings"
```

---

### Task 2: Core port — `KeyboardWatcher`

**Files:**
- Modify: `crates/speakeasy-core/src/ports.rs`
- Modify: `crates/speakeasy-core/src/lib.rs`

**Interfaces:**
- Consumes: `KeyboardBucket` from Task 1.
- Produces: `pub type KeyboardBucketCallback = Box<dyn Fn(KeyboardBucket) + Send + Sync>;` and `pub trait KeyboardWatcher: Send + Sync { fn watch(&self, on_change: KeyboardBucketCallback) -> Result<()>; }`, both re-exported from the crate root.

- [ ] **Step 1: Add the port**

In `ports.rs`, change the import line:

```rust
use crate::config::Hotkey;
```

to:

```rust
use crate::config::{Hotkey, KeyboardBucket};
```

Add near the bottom of the file, after the `TextSink` trait:

```rust
/// Invoked with the currently-active bucket, once at watch time and again on
/// every attach/detach that changes it.
pub type KeyboardBucketCallback = Box<dyn Fn(KeyboardBucket) + Send + Sync>;

/// Reports which keyboard bucket ([`KeyboardBucket::BuiltIn`] or
/// [`KeyboardBucket::External`]) is active, based on what's attached right
/// now.
///
/// A presence check, not a per-keystroke device attribution: any external
/// keyboard attached means [`KeyboardBucket::External`], regardless of how
/// many or which model. See `speakeasy-platform`'s macOS implementation for
/// how that's read off IOKit.
pub trait KeyboardWatcher: Send + Sync {
    /// Start watching. `on_change` must be called at least once with the
    /// bucket that's active when watching starts — implementations are not
    /// required to guarantee that call lands before `watch` returns, only
    /// that it happens once watching is live, the same way a caller must
    /// already tolerate any later attach/detach arriving asynchronously.
    fn watch(&self, on_change: KeyboardBucketCallback) -> Result<()>;
}
```

- [ ] **Step 2: Re-export from the crate root**

In `lib.rs`, update the `config` re-export line:

```rust
pub use config::{
    ActivationMode, Config, Hotkey, InjectionMode, ModelChoice, ModifierKey, WidgetPosition,
};
```

to:

```rust
pub use config::{
    ActivationMode, Config, Hotkey, HotkeyBindings, InjectionMode, KeyboardBucket, ModelChoice,
    ModifierKey, WidgetPosition,
};
```

and the `ports` re-export line:

```rust
pub use ports::{
    AudioSource, CancelCallback, CaptureCallback, Delivery, HotkeyCallback, HotkeyEvent,
    HotkeyListener, SpeechSegment, SpeechSegmenter, TextSink, Transcriber,
};
```

to:

```rust
pub use ports::{
    AudioSource, CancelCallback, CaptureCallback, Delivery, HotkeyCallback, HotkeyEvent,
    HotkeyListener, KeyboardBucketCallback, KeyboardWatcher, SpeechSegment, SpeechSegmenter,
    TextSink, Transcriber,
};
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p speakeasy-core`
Expected: builds clean (nothing constructs a `KeyboardWatcher` yet, but the trait itself is `pub`, so there's no dead-code warning).

- [ ] **Step 4: Format, lint, commit**

```bash
cargo fmt -p speakeasy-core
cargo clippy -p speakeasy-core --all-targets -- -D warnings
git add crates/speakeasy-core/src/ports.rs crates/speakeasy-core/src/lib.rs
git commit -m "core: add the KeyboardWatcher port"
```

---

### Task 3: Core engine — bucket-aware `set_hotkey`, `active_bucket`

**Files:**
- Modify: `crates/speakeasy-core/src/event.rs`
- Modify: `crates/speakeasy-core/src/engine.rs`

**Interfaces:**
- Consumes: `KeyboardBucket`, `HotkeyBindings` (Task 1).
- Produces: `Engine::set_hotkey(&self, bucket: KeyboardBucket, hotkey: Hotkey)`; `Engine::active_bucket(&self) -> KeyboardBucket`; `Engine::set_active_bucket(&self, bucket: KeyboardBucket)`; `Event::HotkeyChanged { bucket: KeyboardBucket, hotkey: Hotkey }`; `Event::ActiveBucketChanged { bucket: KeyboardBucket }`.

- [ ] **Step 1: Update `Event`**

In `event.rs`, change:

```rust
use crate::config::Hotkey;
```

to:

```rust
use crate::config::{Hotkey, KeyboardBucket};
```

Change the `HotkeyChanged` variant:

```rust
    HotkeyChanged {
        hotkey: Hotkey,
    },
```

to:

```rust
    HotkeyChanged {
        bucket: KeyboardBucket,
        hotkey: Hotkey,
    },
```

and add, right after it:

```rust
    /// Which keyboard bucket is driving the binding changed — an external
    /// keyboard attached or detached. The shell re-registers with that
    /// bucket's `Hotkey`; Settings uses it to move the "active" indicator.
    ActiveBucketChanged {
        bucket: KeyboardBucket,
    },
```

- [ ] **Step 2: Write the failing engine tests**

In `engine.rs`'s test module, replace `setting_the_hotkey_persists_it_and_announces_the_change`:

```rust
#[test]
fn setting_the_hotkey_persists_it_and_announces_the_change() {
    let (engine, _sink) = engine("anything");
    let events = engine.subscribe();

    let hotkey = Hotkey::Chord {
        mods: crate::ChordMods { command: true, ..Default::default() },
        key: crate::KeyCode::Space,
    };
    engine.set_hotkey(KeyboardBucket::BuiltIn, hotkey.clone());

    assert_eq!(engine.config().hotkeys.built_in, hotkey);
    assert!(events.try_iter().any(|event| matches!(
        event,
        Event::HotkeyChanged { bucket: KeyboardBucket::BuiltIn, hotkey: ref h } if *h == hotkey
    )));
}
```

and add:

```rust
#[test]
fn setting_the_hotkey_only_touches_the_given_bucket() {
    let (engine, _sink) = engine("anything");
    let before = engine.config().hotkeys.external.clone();

    engine.set_hotkey(
        KeyboardBucket::BuiltIn,
        Hotkey::Chord { mods: crate::ChordMods::default(), key: crate::KeyCode::F13 },
    );

    assert_eq!(engine.config().hotkeys.external, before);
}

#[test]
fn setting_the_hotkey_to_its_current_value_emits_nothing() {
    let (engine, _sink) = engine("anything");
    let current = engine.config().hotkeys.built_in.clone();
    let events = engine.subscribe();

    engine.set_hotkey(KeyboardBucket::BuiltIn, current);

    assert!(!events.try_iter().any(|event| matches!(event, Event::HotkeyChanged { .. })));
}

#[test]
fn the_active_bucket_starts_as_built_in() {
    let (engine, _sink) = engine("anything");
    assert_eq!(engine.active_bucket(), KeyboardBucket::BuiltIn);
}

#[test]
fn set_active_bucket_announces_the_change() {
    let (engine, _sink) = engine("anything");
    let events = engine.subscribe();

    engine.set_active_bucket(KeyboardBucket::External);

    assert_eq!(engine.active_bucket(), KeyboardBucket::External);
    assert!(events.try_iter().any(|event| matches!(
        event,
        Event::ActiveBucketChanged { bucket: KeyboardBucket::External }
    )));
}

#[test]
fn set_active_bucket_to_its_current_value_emits_nothing() {
    let (engine, _sink) = engine("anything");
    let events = engine.subscribe();

    engine.set_active_bucket(KeyboardBucket::BuiltIn);

    assert!(!events.try_iter().any(|event| matches!(event, Event::ActiveBucketChanged { .. })));
}
```

Add `KeyboardBucket` to the test module's imports (`use crate::config::{ActivationMode, Config, Hotkey, KeyboardBucket, ModelChoice, WidgetPosition};` at the top of `engine.rs` — see Step 3).

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p speakeasy-core --lib engine:: 2>&1 | head -30`
Expected: compile errors — `set_hotkey` takes one argument today, `active_bucket`/`set_active_bucket` don't exist, `KeyboardBucket` unimported.

- [ ] **Step 4: Implement**

In `engine.rs`, change the top import:

```rust
use crate::config::{ActivationMode, Config, Hotkey, ModelChoice, WidgetPosition};
```

to:

```rust
use crate::config::{ActivationMode, Config, Hotkey, KeyboardBucket, ModelChoice, WidgetPosition};
```

Add a field to `Inner`, next to `config`:

```rust
    /// Which keyboard bucket is driving the binding right now. Not
    /// persisted — this is a live fact about what's plugged in, not a
    /// preference. Starts `BuiltIn`; the shell corrects it as soon as its
    /// `KeyboardWatcher` reports in.
    active_bucket: Mutex<KeyboardBucket>,
```

Initialize it in `Engine::new`'s `Inner { .. }` construction:

```rust
            active_bucket: Mutex::new(KeyboardBucket::BuiltIn),
```

Replace `Engine::set_hotkey`:

```rust
    /// Rebind the dictation key for one keyboard bucket.
    ///
    /// Its own method rather than a `set_config` round-trip, for the same
    /// reason as [`Engine::set_widget_position`]: the settings window would
    /// otherwise read, mutate and write back a whole config that a capture in
    /// flight is also changing.
    ///
    /// Persisting is all this does. Actually binding the key belongs to
    /// whoever owns the listener, which is the shell — it re-registers when it
    /// sees the event, but only if `bucket` is the one currently active.
    pub fn set_hotkey(&self, bucket: KeyboardBucket, hotkey: Hotkey) {
        {
            let mut config = self.inner.config.lock().expect("config mutex poisoned");
            if config.hotkeys.get(bucket) == &hotkey {
                return;
            }
            config.hotkeys.set(bucket, hotkey.clone());
        }
        self.persist();
        // After the lock is released: a subscriber that calls back in while we
        // still held it would deadlock.
        self.emit(Event::HotkeyChanged { bucket, hotkey });
    }

    /// Which keyboard bucket is currently driving the binding.
    pub fn active_bucket(&self) -> KeyboardBucket {
        *self
            .inner
            .active_bucket
            .lock()
            .expect("active bucket mutex poisoned")
    }

    /// Record that a different keyboard bucket is now active — called by the
    /// shell when its `KeyboardWatcher` reports an attach/detach that flips
    /// the bucket.
    ///
    /// Not persisted; see the field doc on `Inner::active_bucket`.
    pub fn set_active_bucket(&self, bucket: KeyboardBucket) {
        {
            let mut active = self
                .inner
                .active_bucket
                .lock()
                .expect("active bucket mutex poisoned");
            if *active == bucket {
                return;
            }
            *active = bucket;
        }
        self.emit(Event::ActiveBucketChanged { bucket });
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p speakeasy-core --lib engine::`
Expected: PASS.

- [ ] **Step 6: Confirm the rest of `speakeasy-core` still passes**

Run: `cargo test -p speakeasy-core`
Expected: PASS (the `fakes` feature and every other module are untouched by this task).

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt -p speakeasy-core
cargo clippy -p speakeasy-core --all-targets -- -D warnings
git add crates/speakeasy-core/src/event.rs crates/speakeasy-core/src/engine.rs
git commit -m "core: bucket-aware hotkey binding and active-bucket tracking"
```

*(Reminder: `apps/desktop/src-tauri` will not compile again until Task 9 — it still calls the old `set_hotkey(hotkey)` / reads `config.hotkey`. Do not run `cargo build --workspace` until then.)*

---

### Task 4: macOS platform — keyboard-bucket detection

**Files:**
- Create: `crates/speakeasy-platform/src/macos/keyboard_watch.rs`
- Modify: `crates/speakeasy-platform/src/macos/mod.rs`
- Modify: `crates/speakeasy-platform/src/lib.rs`

**Interfaces:**
- Consumes: `KeyboardBucket`, `KeyboardBucketCallback`, `KeyboardWatcher`, `Error`, `Result` from `speakeasy_core`.
- Produces: `pub struct MacKeyboardWatcher` implementing `KeyboardWatcher`; `speakeasy_platform::keyboard_watcher() -> Box<dyn KeyboardWatcher>`.

This task is one commit because the pure bucket-decision logic (`BucketTracker`) has no caller until the `IOHIDManager` glue in the same file uses it — splitting them would leave `BucketTracker`'s methods unused outside `#[cfg(test)]`, which trips `clippy`'s dead-code lint (`pub(crate)` items, unlike `pub` ones, are checked).

- [ ] **Step 1: Write `BucketTracker` and its tests**

Create `crates/speakeasy-platform/src/macos/keyboard_watch.rs`:

```rust
//! Detects whether an external keyboard is attached, via IOKit's HID
//! manager.
//!
//! macOS marks the internal keyboard with `kIOHIDBuiltInKey`; anything else
//! reporting a Generic Desktop / Keyboard usage counts as external.
//! [`BucketTracker`] turns a stream of attach/detach events into the bucket
//! the hotkey tap should use, deliberately free of IOKit so it can be tested
//! without a run loop — the same split `speakeasy_core::hotkey::match_event`
//! uses to keep matching logic out of a tap callback.

use std::collections::HashSet;

use speakeasy_core::KeyboardBucket;

/// Opaque identity for one physical HID device, for as long as it stays
/// attached. [`MacKeyboardWatcher`] uses the device's pointer address; tests
/// use arbitrary small integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DeviceId(pub usize);

/// Tracks currently-attached external keyboards and decides the bucket.
#[derive(Debug, Default)]
pub(crate) struct BucketTracker {
    external: HashSet<DeviceId>,
}

impl BucketTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A keyboard attached. `built_in` is `kIOHIDBuiltInKey`'s value for it.
    pub(crate) fn attach(&mut self, id: DeviceId, built_in: bool) -> KeyboardBucket {
        if !built_in {
            self.external.insert(id);
        }
        self.bucket()
    }

    /// A keyboard detached. Detaching an id this tracker never recorded as
    /// external (the built-in keyboard, or a stray duplicate callback) is a
    /// no-op.
    pub(crate) fn detach(&mut self, id: DeviceId) -> KeyboardBucket {
        self.external.remove(&id);
        self.bucket()
    }

    pub(crate) fn bucket(&self) -> KeyboardBucket {
        if self.external.is_empty() {
            KeyboardBucket::BuiltIn
        } else {
            KeyboardBucket::External
        }
    }
}

#[cfg(test)]
mod tracker_tests {
    use super::*;

    #[test]
    fn no_devices_is_built_in() {
        assert_eq!(BucketTracker::new().bucket(), KeyboardBucket::BuiltIn);
    }

    #[test]
    fn the_built_in_keyboard_attaching_changes_nothing() {
        let mut tracker = BucketTracker::new();
        assert_eq!(tracker.attach(DeviceId(1), true), KeyboardBucket::BuiltIn);
    }

    #[test]
    fn an_external_keyboard_attaching_switches_to_external() {
        let mut tracker = BucketTracker::new();
        assert_eq!(tracker.attach(DeviceId(1), false), KeyboardBucket::External);
    }

    #[test]
    fn a_second_external_keyboard_stays_external() {
        let mut tracker = BucketTracker::new();
        tracker.attach(DeviceId(1), false);
        assert_eq!(tracker.attach(DeviceId(2), false), KeyboardBucket::External);
    }

    #[test]
    fn detaching_one_of_two_externals_stays_external() {
        let mut tracker = BucketTracker::new();
        tracker.attach(DeviceId(1), false);
        tracker.attach(DeviceId(2), false);
        assert_eq!(tracker.detach(DeviceId(1)), KeyboardBucket::External);
    }

    #[test]
    fn detaching_the_last_external_reverts_to_built_in() {
        let mut tracker = BucketTracker::new();
        tracker.attach(DeviceId(1), false);
        assert_eq!(tracker.detach(DeviceId(1)), KeyboardBucket::BuiltIn);
    }

    #[test]
    fn detaching_an_unknown_id_is_a_no_op() {
        let mut tracker = BucketTracker::new();
        tracker.attach(DeviceId(1), false);
        assert_eq!(tracker.detach(DeviceId(99)), KeyboardBucket::External);
    }
}
```

- [ ] **Step 2: Run the tracker tests**

Run: `cargo test -p speakeasy-platform --lib macos::keyboard_watch::tracker_tests`
Expected: PASS. (`mod keyboard_watch;` must be added to `macos/mod.rs` first — see Step 4 — or this won't compile; add just that one line now, nothing else, to unblock the test run.)

- [ ] **Step 3: Implement `MacKeyboardWatcher` over `IOHIDManager`**

Append to `keyboard_watch.rs`:

```rust
use std::ffi::c_void;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use core_foundation::base::{kCFAllocatorDefault, CFAllocatorRef, CFTypeRef, TCFType};
use core_foundation::boolean::{CFBoolean, CFBooleanRef};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopRef};
use core_foundation::string::{CFString, CFStringRef};
use speakeasy_core::{Error, KeyboardBucketCallback, KeyboardWatcher, Result};

type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOOptionBits = u32;
type IOReturn = i32;
type IOHIDDeviceCallback =
    extern "C" fn(context: *mut c_void, result: IOReturn, sender: *mut c_void, device: IOHIDDeviceRef);

/// `kHIDPage_GenericDesktop` / `kHIDUsage_GD_Keyboard`, from
/// `IOKit/hid/IOHIDUsageTables.h`. Plain integer macros in the C header, not
/// exported symbols, so they are copied here as constants.
const HID_PAGE_GENERIC_DESKTOP: i32 = 0x01;
const HID_USAGE_GD_KEYBOARD: i32 = 0x06;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: CFAllocatorRef, options: IOOptionBits) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: IOOptionBits) -> IOReturn;
    fn IOHIDManagerScheduleWithRunLoop(
        manager: IOHIDManagerRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn IOHIDManagerRegisterDeviceMatchingCallback(
        manager: IOHIDManagerRef,
        callback: IOHIDDeviceCallback,
        context: *mut c_void,
    );
    fn IOHIDManagerRegisterDeviceRemovalCallback(
        manager: IOHIDManagerRef,
        callback: IOHIDDeviceCallback,
        context: *mut c_void,
    );
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
}

/// Shared between the setup thread and the two IOKit callbacks, which run on
/// whatever thread IOKit chooses to deliver them on.
struct Watch {
    tracker: Mutex<BucketTracker>,
    on_change: KeyboardBucketCallback,
}

/// Reads `kIOHIDBuiltInKey` off a device. Missing, or not a boolean, reads as
/// external — every keyboard IOKit considers internal sets this explicitly.
fn is_built_in(device: IOHIDDeviceRef) -> bool {
    let key = CFString::new("BuiltIn");
    unsafe {
        let value = IOHIDDeviceGetProperty(device, key.as_concrete_TypeRef());
        if value.is_null() {
            return false;
        }
        let boolean: CFBoolean = TCFType::wrap_under_get_rule(value as CFBooleanRef);
        bool::from(boolean)
    }
}

extern "C" fn on_matched(context: *mut c_void, _result: IOReturn, _sender: *mut c_void, device: IOHIDDeviceRef) {
    // SAFETY: `context` is the `*const Watch` this watcher leaked in `watch`,
    // and it outlives every callback IOKit can deliver — there is no
    // `unwatch` that would free it first.
    let watch = unsafe { &*(context as *const Watch) };
    let built_in = is_built_in(device);
    let bucket = watch
        .tracker
        .lock()
        .expect("keyboard tracker mutex poisoned")
        .attach(DeviceId(device as usize), built_in);
    (watch.on_change)(bucket);
}

extern "C" fn on_removed(context: *mut c_void, _result: IOReturn, _sender: *mut c_void, device: IOHIDDeviceRef) {
    // SAFETY: see `on_matched`.
    let watch = unsafe { &*(context as *const Watch) };
    let bucket = watch
        .tracker
        .lock()
        .expect("keyboard tracker mutex poisoned")
        .detach(DeviceId(device as usize));
    (watch.on_change)(bucket);
}

#[derive(Default)]
pub struct MacKeyboardWatcher;

impl MacKeyboardWatcher {
    pub fn new() -> Self {
        Self
    }
}

impl KeyboardWatcher for MacKeyboardWatcher {
    fn watch(&self, on_change: KeyboardBucketCallback) -> Result<()> {
        let watch = Arc::new(Watch {
            tracker: Mutex::new(BucketTracker::new()),
            on_change,
        });

        let (ready_tx, ready_rx) = channel::<()>();

        std::thread::Builder::new()
            .name("speakeasy-keyboard-watch".into())
            .spawn(move || {
                let manager = unsafe { IOHIDManagerCreate(kCFAllocatorDefault, 0) };

                let matching = CFDictionary::from_CFType_pairs(&[
                    (
                        CFString::new("DeviceUsagePage").as_CFType(),
                        CFNumber::from(HID_PAGE_GENERIC_DESKTOP).as_CFType(),
                    ),
                    (
                        CFString::new("DeviceUsage").as_CFType(),
                        CFNumber::from(HID_USAGE_GD_KEYBOARD).as_CFType(),
                    ),
                ]);

                // Leaked on purpose: the callbacks read through this pointer
                // for the lifetime of the manager, which is the lifetime of
                // the process — there is no `unwatch`. Mirrors the tap port
                // `MacHotkeys` leaks in `hotkey.rs`.
                let context = Arc::into_raw(Arc::clone(&watch)) as *mut c_void;

                unsafe {
                    IOHIDManagerSetDeviceMatching(manager, matching.as_concrete_TypeRef());
                    IOHIDManagerRegisterDeviceMatchingCallback(manager, on_matched, context);
                    IOHIDManagerRegisterDeviceRemovalCallback(manager, on_removed, context);
                    IOHIDManagerScheduleWithRunLoop(
                        manager,
                        CFRunLoop::get_current().as_concrete_TypeRef(),
                        kCFRunLoopCommonModes,
                    );
                    IOHIDManagerOpen(manager, 0);
                }

                // The manager is fully set up; `watch` can return. Matching
                // callbacks for already-attached devices — including the
                // built-in keyboard — arrive once the run loop below starts
                // turning, which is what gives `on_change` its first report
                // of the real starting bucket.
                let _ = ready_tx.send(());
                CFRunLoop::run_current();
            })
            .map_err(|err| {
                Error::Hotkey(format!("could not start the keyboard-watch thread: {err}"))
            })?;

        ready_rx.recv().map_err(|_| {
            Error::Hotkey("the keyboard-watch thread stopped unexpectedly".to_string())
        })
    }
}
```

- [ ] **Step 4: Wire it into `macos/mod.rs` and `lib.rs`**

In `crates/speakeasy-platform/src/macos/mod.rs`, add the module declaration next to the others:

```rust
mod focus;
mod hotkey;
mod keyboard_watch;
mod keycode;
mod secure_input;
mod text_sink;
```

and export it next to `MacHotkeys`:

```rust
pub use hotkey::MacHotkeys;
pub use keyboard_watch::MacKeyboardWatcher;
```

Add a constructor function next to `hotkey_listener`:

```rust
pub fn keyboard_watcher() -> Box<dyn KeyboardWatcher> {
    Box::new(MacKeyboardWatcher::new())
}
```

and add `KeyboardWatcher` to that file's `use speakeasy_core::{HotkeyListener, TextSink};` line, making it `use speakeasy_core::{HotkeyListener, KeyboardWatcher, TextSink};`.

In `crates/speakeasy-platform/src/lib.rs`, add to the `use` line:

```rust
use speakeasy_core::{Config, HotkeyListener, KeyboardWatcher, TextSink};
```

and add, next to `hotkey_listener`:

```rust
/// Reports which keyboard bucket (built-in or external) is active right now.
pub fn keyboard_watcher() -> Box<dyn KeyboardWatcher> {
    sys::keyboard_watcher()
}
```

- [ ] **Step 5: Build and fix any FFI signature mismatches**

Run: `cargo build -p speakeasy-platform`

The hand-written `extern "C"` declarations above are written against the documented `IOHIDManager` C API, but this workspace has never linked `IOKit` before — treat compiler/linker errors here as expected, and fix them against what `core-foundation` 0.10.1 actually exposes (`cargo doc -p core-foundation --open` or read `~/.cargo/registry/src/*/core-foundation-0.10.1/src/{base,boolean,dictionary,number,runloop,string}.rs`) rather than reworking the approach.

Expected once fixed: builds clean.

- [ ] **Step 6: Run the tracker tests again (full file this time)**

Run: `cargo test -p speakeasy-platform --lib macos::keyboard_watch`
Expected: PASS — the `IOHIDManager` glue has no automated test (no seam to test it without real hardware and a run loop, same as `macos/hotkey.rs`'s tap callback today), so this only re-runs `tracker_tests`.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt -p speakeasy-platform
cargo clippy -p speakeasy-platform --all-targets -- -D warnings
git add crates/speakeasy-platform/src/macos/keyboard_watch.rs crates/speakeasy-platform/src/macos/mod.rs crates/speakeasy-platform/src/lib.rs
git commit -m "platform(macos): detect built-in vs external keyboard via IOHIDManager"
```

---

### Task 5: Linux platform — `KeyboardWatcher` stub

**Files:**
- Create: `crates/speakeasy-platform/src/linux/keyboard_watch.rs`
- Modify: `crates/speakeasy-platform/src/linux/mod.rs`

**Interfaces:**
- Consumes: `KeyboardBucket`, `KeyboardBucketCallback`, `KeyboardWatcher`, `Result` from `speakeasy_core`.
- Produces: `pub struct X11KeyboardWatcher` implementing `KeyboardWatcher`.

- [ ] **Step 1: Write the stub**

Create `crates/speakeasy-platform/src/linux/keyboard_watch.rs`:

```rust
//! No real device-based detection on X11 yet — mirrors `X11Hotkeys::register`
//! in `hotkey.rs`, which is also unimplemented for the same underlying
//! reason (no XInput2 device-enumeration wiring exists in this crate yet).
//! Always reports the built-in bucket, so the rest of the app still has a
//! bucket to bind against.

use speakeasy_core::{KeyboardBucket, KeyboardBucketCallback, KeyboardWatcher, Result};

#[derive(Default)]
pub struct X11KeyboardWatcher;

impl X11KeyboardWatcher {
    pub fn new() -> Self {
        Self
    }
}

impl KeyboardWatcher for X11KeyboardWatcher {
    fn watch(&self, on_change: KeyboardBucketCallback) -> Result<()> {
        on_change(KeyboardBucket::BuiltIn);
        Ok(())
    }
}
```

- [ ] **Step 2: Wire it into `linux/mod.rs`**

Update `crates/speakeasy-platform/src/linux/mod.rs`:

```rust
mod hotkey;
mod keyboard_watch;
mod text_sink;

use speakeasy_core::config::InjectionMode;
use speakeasy_core::{HotkeyListener, KeyboardWatcher, TextSink};

pub use hotkey::X11Hotkeys;
pub use keyboard_watch::X11KeyboardWatcher;
pub use text_sink::X11TextSink;

pub fn text_sink(mode: InjectionMode) -> Box<dyn TextSink> {
    Box::new(X11TextSink::new(mode))
}

pub fn hotkey_listener() -> Box<dyn HotkeyListener> {
    Box::new(X11Hotkeys::new())
}

pub fn keyboard_watcher() -> Box<dyn KeyboardWatcher> {
    Box::new(X11KeyboardWatcher::new())
}

/// X11 has no permission model for synthetic input or capture.
pub fn permissions_granted() -> bool {
    true
}

pub fn open_permission_settings() {}
```

- [ ] **Step 3: Verify it compiles**

This crate cross-compiles for Linux only with the right target installed; if this machine is macOS-only, skip an actual build and instead re-read the diff carefully against `macos/mod.rs`'s equivalent function for shape-consistency (same pattern, `Box::new(X11KeyboardWatcher::new())`). If a Linux target is available:

Run: `cargo build -p speakeasy-platform --target x86_64-unknown-linux-gnu`
Expected: builds clean.

- [ ] **Step 4: Format (macOS-visible files only), commit**

```bash
cargo fmt -p speakeasy-platform
git add crates/speakeasy-platform/src/linux/keyboard_watch.rs crates/speakeasy-platform/src/linux/mod.rs
git commit -m "platform(linux): stub KeyboardWatcher, always built-in"
```

---

### Task 6: Shell — bucket-aware hotkey registration and the keyboard watcher

**Files:**
- Modify: `apps/desktop/src-tauri/src/hotkey.rs`

**Interfaces:**
- Consumes: `Engine::active_bucket`, `Engine::set_hotkey`, `Engine::set_active_bucket`, `Engine::config` (Task 3); `speakeasy_platform::keyboard_watcher()` (Task 4/5).
- Produces: `pub fn watch_keyboard(engine: Engine) -> Result<(), Box<dyn std::error::Error>>`. `register`/`bind_if_unbound` keep their existing signatures (still take a resolved `&Hotkey` — bucket resolution happens at each call site). `capture` now resolves the bucket to write into from `engine.active_bucket()`.

There is no isolated automated test for this file (it is thin IPC-adjacent glue over the platform port, same as the rest of `hotkey.rs` today); verification is `cargo build`/`cargo clippy` plus the manual pass in Task 9.

- [ ] **Step 1: Update `capture` to write into the active bucket**

In `apps/desktop/src-tauri/src/hotkey.rs`, change:

```rust
pub fn capture(engine: Engine) -> Result<(), Box<dyn std::error::Error>> {
    listener().capture(Box::new(move |hotkey| {
        tracing::info!(hotkey = hotkey.label(), "captured a new binding");
        engine.set_hotkey(hotkey);
    }))?;
    Ok(())
}
```

to:

```rust
pub fn capture(engine: Engine) -> Result<(), Box<dyn std::error::Error>> {
    let bucket = engine.active_bucket();
    listener().capture(Box::new(move |hotkey| {
        tracing::info!(hotkey = hotkey.label(), ?bucket, "captured a new binding");
        engine.set_hotkey(bucket, hotkey);
    }))?;
    Ok(())
}
```

(This binds whatever the user is holding while capturing into whichever bucket is currently live — the one keyboard they could plausibly be pressing.)

- [ ] **Step 2: Add `watch_keyboard`**

Add, at the end of the file:

```rust
/// Starts the platform's keyboard-bucket watcher. Its callback only updates
/// `Engine::active_bucket` — the actual re-registration happens in
/// `bridge.rs`'s `ActiveBucketChanged` handler, the same "decide here, act
/// there" split `Engine::set_hotkey` already uses for `HotkeyChanged`.
///
/// Leaks the watcher: nothing ever tears it down for the life of the
/// process, the same trade `LISTENER` above already makes for the tap.
pub fn watch_keyboard(engine: Engine) -> Result<(), Box<dyn std::error::Error>> {
    let watcher: &'static dyn KeyboardWatcher =
        Box::leak(speakeasy_platform::keyboard_watcher());
    watcher.watch(Box::new(move |bucket| engine.set_active_bucket(bucket)))?;
    Ok(())
}
```

Add `KeyboardWatcher` to the top-level `use` line:

```rust
use speakeasy_core::{Engine, Hotkey, HotkeyAction, HotkeyListener, KeyboardWatcher};
```

- [ ] **Step 3: Build**

Run: `cargo build -p speakeasy-desktop 2>&1 | head -60` (the Tauri crate's package name — check `apps/desktop/src-tauri/Cargo.toml`'s `[package] name` if this doesn't match)

Expected: still fails — `main.rs`, `bridge.rs`, `commands.rs` reference the old `set_hotkey(hotkey)` / `config.hotkey` and haven't been updated yet (Tasks 7–8). Confirm the *only* errors are in those three files, not in `hotkey.rs` itself.

- [ ] **Step 4: Format, commit**

```bash
cargo fmt -p speakeasy-desktop
git add apps/desktop/src-tauri/src/hotkey.rs
git commit -m "shell: bucket-aware hotkey capture; add the keyboard watcher"
```

---

### Task 7: Shell — event forwarding

**Files:**
- Modify: `apps/desktop/src-tauri/src/bridge.rs`

**Interfaces:**
- Consumes: `Event::HotkeyChanged { bucket, hotkey }`, `Event::ActiveBucketChanged { bucket }` (Task 3); `crate::hotkey::register` (unchanged signature, Task 6); `Engine::active_bucket`, `Engine::config` (Task 3).

- [ ] **Step 1: Update the `HotkeyChanged` handler and add `ActiveBucketChanged`**

In `apps/desktop/src-tauri/src/bridge.rs`, change:

```rust
            // No main-thread hop: this installs an event tap, not an AppKit
            // window. Re-registering here rather than in the command is what
            // keeps a rebinding from settings and one from a capture on the
            // same path.
            if let Event::HotkeyChanged { hotkey } = &event {
                if let Err(err) = crate::hotkey::register(engine.clone(), hotkey) {
                    tracing::warn!(%err, "could not bind the new hotkey");
                }
            }
```

to:

```rust
            // No main-thread hop: this installs an event tap, not an AppKit
            // window. Re-registering here rather than in the command is what
            // keeps a rebinding from settings and one from a capture on the
            // same path.
            //
            // Only when `bucket` is the one currently active: a rebind of
            // the *other* row in Settings must not tear down the tap that is
            // actually working right now.
            if let Event::HotkeyChanged { bucket, hotkey } = &event {
                if *bucket == engine.active_bucket() {
                    if let Err(err) = crate::hotkey::register(engine.clone(), hotkey) {
                        tracing::warn!(%err, "could not bind the new hotkey");
                    }
                }
            }

            // The set of attached keyboards changed enough to flip the
            // bucket. This is what actually performs the swap-the-tap
            // behavior; `Engine::set_active_bucket` itself only records the
            // fact.
            if let Event::ActiveBucketChanged { bucket } = &event {
                let hotkey = engine.config().hotkeys.get(*bucket).clone();
                if let Err(err) = crate::hotkey::register(engine.clone(), &hotkey) {
                    tracing::warn!(%err, "could not bind the hotkey for the new keyboard");
                }
            }
```

- [ ] **Step 2: Build**

Run: `cargo build -p speakeasy-desktop 2>&1 | head -60`
Expected: remaining errors confined to `main.rs` and `commands.rs`.

- [ ] **Step 3: Format, commit**

```bash
cargo fmt -p speakeasy-desktop
git add apps/desktop/src-tauri/src/bridge.rs
git commit -m "shell: rebind on hotkey change only for the active bucket; rebind on bucket change"
```

---

### Task 8: Shell — commands and startup wiring

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/main.rs`

**Interfaces:**
- Consumes: everything from Tasks 3, 6, 7.
- Produces: `set_hotkey(engine, bucket: KeyboardBucket, hotkey: Hotkey)` Tauri command; new `active_keyboard_bucket(engine) -> KeyboardBucket` command; `bind_hotkey` reads the active bucket's binding.

- [ ] **Step 1: Update `commands.rs`**

Change the top import:

```rust
use speakeasy_core::{
    Config, Engine, Hotkey, ModelChoice, ModelId, ModelInfo, ModelStatus, State, Transcript,
};
```

to:

```rust
use speakeasy_core::{
    Config, Engine, Hotkey, KeyboardBucket, ModelChoice, ModelId, ModelInfo, ModelStatus, State,
    Transcript,
};
```

Change `set_hotkey`:

```rust
#[tauri::command]
pub fn set_hotkey(engine: TauriState<'_, Engine>, hotkey: Hotkey) {
    engine.set_hotkey(hotkey);
}
```

to:

```rust
#[tauri::command]
pub fn set_hotkey(engine: TauriState<'_, Engine>, bucket: KeyboardBucket, hotkey: Hotkey) {
    engine.set_hotkey(bucket, hotkey);
}
```

Add, next to it:

```rust
#[tauri::command]
pub fn active_keyboard_bucket(engine: TauriState<'_, Engine>) -> KeyboardBucket {
    engine.active_bucket()
}
```

Change `bind_hotkey`:

```rust
#[tauri::command]
pub fn bind_hotkey(engine: TauriState<'_, Engine>) -> Result<(), String> {
    crate::hotkey::bind_if_unbound(engine.inner().clone(), &engine.config().hotkey)
        .map_err(|err| err.to_string())
}
```

to:

```rust
#[tauri::command]
pub fn bind_hotkey(engine: TauriState<'_, Engine>) -> Result<(), String> {
    let bucket = engine.active_bucket();
    let hotkey = engine.config().hotkeys.get(bucket).clone();
    crate::hotkey::bind_if_unbound(engine.inner().clone(), &hotkey).map_err(|err| err.to_string())
}
```

- [ ] **Step 2: Update `main.rs`**

Change:

```rust
            if let Err(err) = hotkey::register(engine.clone(), &config.hotkey) {
                tracing::warn!(%err, "push-to-talk is not bound");
            }
```

to:

```rust
            // Bound synchronously against the built-in binding first, so
            // dictation works immediately even if the keyboard watcher below
            // is slow to report in; its first callback corrects this to
            // External if a PC keyboard is already attached at launch.
            let starting_hotkey = config.hotkeys.get(engine.active_bucket()).clone();
            if let Err(err) = hotkey::register(engine.clone(), &starting_hotkey) {
                tracing::warn!(%err, "push-to-talk is not bound");
            }
            if let Err(err) = hotkey::watch_keyboard(engine.clone()) {
                tracing::warn!(%err, "could not watch for keyboard changes");
            }
```

Add `commands::active_keyboard_bucket,` to the `invoke_handler(tauri::generate_handler![...])` list, next to `commands::bind_hotkey,`:

```rust
            commands::set_hotkey,
            commands::capture_hotkey,
            commands::cancel_hotkey_capture,
            commands::bind_hotkey,
            commands::active_keyboard_bucket,
            commands::permissions_granted,
```

- [ ] **Step 3: Build the whole workspace — this is the task that turns it green again**

Run: `cargo build --workspace 2>&1 | tail -60`
Expected: builds clean. If not, the remaining errors are almost certainly leftover references to the old `Config::hotkey`/`Engine::set_hotkey(hotkey)` shape outside the files this plan already touched — grep for them:

```bash
grep -rn "\.hotkey\b" apps/desktop/src-tauri/src crates --include=*.rs | grep -v "\.hotkeys\b"
```

- [ ] **Step 4: Run the full test suite and lints**

```bash
make test
make lint
make fmt-check
make boundary
```

Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src-tauri/src/main.rs
git commit -m "shell: bucket-aware set_hotkey command; bind and watch keyboards at startup"
```

---

### Task 9: Frontend — typed bindings

**Files:**
- Modify: `apps/desktop/src/engine.ts`

**Interfaces:**
- Produces: `export type KeyboardBucket = "builtIn" | "external";`; `Config.hotkeys: { builtIn: Hotkey; external: Hotkey }` (dropping `Config.hotkey`); `EngineEvent`'s `hotkeyChanged` gains `bucket: KeyboardBucket`; new `EngineEvent` variant `{ type: "activeBucketChanged"; bucket: KeyboardBucket }`; `setHotkey(bucket, hotkey)`; new `activeKeyboardBucket()`; `activationLabel(hotkey: Hotkey, activation: ActivationMode)`; `stopHint(hotkey: Hotkey, activation: ActivationMode)` (both dropping their `config: Config` parameter in favor of the already-resolved hotkey, since which `Config` field to read is no longer a single answer).

No test runner exists for this package; verification is `tsc --noEmit` (via `npm run build`), deferred to the end of Task 11 once every caller is updated — this task alone will not type-check standalone since `widget.ts`/`settings.ts` still call the old signatures. Read the diff carefully instead.

- [ ] **Step 1: Add `KeyboardBucket` and update `Config`**

In `apps/desktop/src/engine.ts`, add near the other small type aliases (by `ActivationMode`):

```ts
/** Which physical keyboard is driving the binding. See `core::config::KeyboardBucket`. */
export type KeyboardBucket = "builtIn" | "external";
```

Change:

```ts
export type Config = {
  hotkey: Hotkey;
  activation: ActivationMode;
```

to:

```ts
export type Config = {
  hotkeys: { builtIn: Hotkey; external: Hotkey };
  activation: ActivationMode;
```

- [ ] **Step 2: Update `EngineEvent`**

Change:

```ts
  | { type: "hotkeyChanged"; hotkey: Hotkey }
```

to:

```ts
  | { type: "hotkeyChanged"; bucket: KeyboardBucket; hotkey: Hotkey }
  | { type: "activeBucketChanged"; bucket: KeyboardBucket }
```

- [ ] **Step 3: Update `activationLabel` and `stopHint`**

Change:

```ts
/** How to describe the binding to the user, mode included. */
export function activationLabel(config: Config): string {
  const key = hotkeyLabel(config.hotkey);
  return config.activation === "toggle" ? `tap ${key} to start and stop` : `hold ${key} to talk`;
}
```

to:

```ts
/** How to describe the binding to the user, mode included. */
export function activationLabel(hotkey: Hotkey, activation: ActivationMode): string {
  const key = hotkeyLabel(hotkey);
  return activation === "toggle" ? `tap ${key} to start and stop` : `hold ${key} to talk`;
}
```

Change:

```ts
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

to:

```ts
export function stopHint(hotkey: Hotkey, activation: ActivationMode): string {
  const key = hotkeyLabel(hotkey);
  const base = activation === "toggle" ? `Tap ${key} to stop` : `Release ${key} to insert`;

  // If the activation hotkey is itself Escape-based, Esc already means
  // something else while recording — see the matching precedence rule in
  // `speakeasy-platform`'s macOS hotkey tap. Mentioning cancel here too
  // would describe a binding that isn't actually live.
  const hotkeyIsEscape = hotkey.kind === "chord" && hotkey.key === "escape";
  return hotkeyIsEscape ? base : `${base} · Esc to cancel`;
}
```

- [ ] **Step 4: Update the command bindings**

Change:

```ts
export const setHotkey = (hotkey: Hotkey) => invoke<void>("set_hotkey", { hotkey });
```

to:

```ts
export const setHotkey = (bucket: KeyboardBucket, hotkey: Hotkey) =>
  invoke<void>("set_hotkey", { bucket, hotkey });

/** Which keyboard bucket is driving the binding right now. */
export const activeKeyboardBucket = () => invoke<KeyboardBucket>("active_keyboard_bucket");
```

- [ ] **Step 5: Commit (staged, not yet type-checked — callers land in the next two tasks)**

```bash
git add apps/desktop/src/engine.ts
git commit -m "frontend: bucket-aware Config/events/commands for the hotkey bindings"
```

---

### Task 10: Frontend — widget hint uses the active bucket

**Files:**
- Modify: `apps/desktop/src/widget.ts`

**Interfaces:**
- Consumes: `activeKeyboardBucket`, `stopHint(hotkey, activation)`, `type KeyboardBucket` (Task 9).

- [ ] **Step 1: Track the active bucket alongside `config`**

In `apps/desktop/src/widget.ts`, add `activeKeyboardBucket` and `type KeyboardBucket` to the import list:

```ts
import {
  activeKeyboardBucket,
  cancel,
  copyTranscript,
  errorLabel,
  getAudioSpectrum,
  getConfig,
  getRecoverableTranscript,
  getState,
  onEngineEvent,
  stopHint,
  SPECTRUM_BANDS,
  type Config,
  type KeyboardBucket,
  type State,
} from "./engine";
```

Change:

```ts
  let recording = false;
  // The hotkey and the activation mode can both change at runtime, so the hint
  // is rebuilt from config rather than baked in at mount.
  let config: Config | null = null;
```

to:

```ts
  let recording = false;
  // The hotkey and the activation mode can both change at runtime, so the hint
  // is rebuilt from config rather than baked in at mount.
  let config: Config | null = null;
  let bucket: KeyboardBucket = "builtIn";

  const currentHint = () => (config ? stopHint(config.hotkeys[bucket], config.activation) : "");
```

- [ ] **Step 2: Use `currentHint()` and refresh the bucket alongside config**

Change:

```ts
    recording = state.state === "recording";
    hint.textContent = recording && config ? stopHint(config) : "";
```

to:

```ts
    recording = state.state === "recording";
    hint.textContent = recording ? currentHint() : "";
```

Change:

```ts
  const refreshConfig = async () => {
    config = await getConfig();
    if (recording) hint.textContent = stopHint(config);
  };
```

to:

```ts
  const refreshConfig = async () => {
    [config, bucket] = await Promise.all([getConfig(), activeKeyboardBucket()]);
    if (recording) hint.textContent = currentHint();
  };
```

(`refreshConfig` already runs at mount and again "once per dictation" on every `recording` transition — see the existing comment at its call site a few lines below — so this piggybacks on that cadence rather than adding a second live listener for `activeBucketChanged`; the widget only needs to be right by the time a recording's hint renders, not instantaneously on every attach/detach.)

- [ ] **Step 3: Type-check just this file's shape by eye against `engine.ts`**

Confirm `Config.hotkeys[bucket]` type-checks conceptually: `bucket: KeyboardBucket` (`"builtIn" | "external"`) indexing `{ builtIn: Hotkey; external: Hotkey }` is a valid TS index — this is standard string-literal-union indexing, no cast needed.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/widget.ts
git commit -m "frontend: resolve the widget's recording hint against the active keyboard bucket"
```

---

### Task 11: Frontend — Settings shows two hotkey rows

**Files:**
- Modify: `apps/desktop/src/settings.ts`
- Modify: `apps/desktop/src/styles.css`

**Interfaces:**
- Consumes: everything from Task 9 (`Config.hotkeys`, `setHotkey(bucket, hotkey)`, `activeKeyboardBucket()`, `activationLabel`/`stopHint` if used here — they are not; only `hotkeyRow` lives in this file).

This is also the task that finally makes the whole frontend type-check, so its last step runs `npm run build`.

- [ ] **Step 1: Update imports**

In `apps/desktop/src/settings.ts`, change:

```ts
import {
  bindHotkey,
  cancelHotkeyCapture,
  cancelModelDownload,
  clearHistory,
  copyHistoryEntry,
  captureHotkey,
  hotkeyLabel,
  errorLabel,
  formatBytes,
  getConfig,
  setConfig,
  getHistory,
  getInstalledModels,
  getModelCatalog,
  getModelStatus,
  installModel,
  isSelected,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  relativeTime,
  removeModel,
  selectModel,
  type Config,
  type Hotkey,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
  type Transcript,
} from "./engine";
```

to:

```ts
import {
  activeKeyboardBucket,
  bindHotkey,
  cancelHotkeyCapture,
  cancelModelDownload,
  clearHistory,
  copyHistoryEntry,
  captureHotkey,
  hotkeyLabel,
  errorLabel,
  formatBytes,
  getConfig,
  setConfig,
  getHistory,
  getInstalledModels,
  getModelCatalog,
  getModelStatus,
  installModel,
  isSelected,
  onEngineEvent,
  openPermissionSettings,
  permissionsGranted,
  relativeTime,
  removeModel,
  selectModel,
  setHotkey,
  type Config,
  type Hotkey,
  type KeyboardBucket,
  type ModelId,
  type ModelInfo,
  type ModelStatus,
  type Transcript,
} from "./engine";
```

(`setHotkey` was not previously imported here — the row's `Change` flow goes through `captureHotkey`, which the shell resolves into a `set_hotkey` call server-side. It remains unused directly by this file; do not add an unused import. Skip adding `setHotkey` if `tsc --noEmit` in Step 6 flags it as unused.)

- [ ] **Step 2: Replace `hotkeyRow` with a bucket-aware version returning `{ element, release }`**

Replace the existing `hotkeyRow` function:

```ts
/**
 * The binding, and a button that reads the next key pressed.
 *
 * No validity check here on purpose: whether a key can be bound is the
 * platform's answer, and a second opinion in the webview is exactly the kind of
 * drift this codebase is arranged to avoid. The pane renders what comes back.
 */
async function hotkeyRow(config: Config): Promise<HTMLElement> {
  const wrapper = element("div", "setting-block");
  const line = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = "Hotkey";
  const value = element("span", "setting-value");
  const action = document.createElement("button");
  line.append(name, value, action);

  const problem = element("div", "setting-error");
  wrapper.append(line, problem);

  let listening = false;

  const draw = (hotkey: Hotkey) => {
    value.textContent = listening ? "Press a key…" : hotkeyLabel(hotkey);
    action.textContent = listening ? "Cancel" : "Change";
  };

  let current = config.hotkey;
  draw(current);

  action.addEventListener("click", () => {
    void (async () => {
      problem.textContent = "";
      if (listening) {
        await cancelHotkeyCapture();
        listening = false;
        draw(current);
        return;
      }

      listening = true;
      draw(current);
      try {
        await captureHotkey();
      } catch (err) {
        // The likeliest cause is a missing Accessibility grant, which the
        // banner above already explains how to fix.
        listening = false;
        draw(current);
        problem.textContent = String(err);
      }
    })();
  });

  // The capture reports through the ordinary rebinding event, so this is also
  // what keeps the row honest if the binding changes from anywhere else.
  const unlisten = await onEngineEvent((event) => {
    if (event.type !== "hotkeyChanged") return;
    listening = false;
    current = event.hotkey;
    draw(current);
  });

  // A capture swallows keystrokes. Leaving one armed behind a tab the user has
  // navigated away from would half-deaden the keyboard with nothing on screen
  // to explain it.
  releasePanel = () => {
    unlisten();
    if (listening) void cancelHotkeyCapture();
  };

  return wrapper;
}
```

with:

```ts
type HotkeyRow = { element: HTMLElement; release: () => void };

/**
 * One binding — built-in or external — and a button that reads the next key
 * pressed.
 *
 * No validity check here on purpose: whether a key can be bound is the
 * platform's answer, and a second opinion in the webview is exactly the kind of
 * drift this codebase is arranged to avoid. The pane renders what comes back.
 *
 * `initialActive` is a one-time snapshot for the first paint; the row keeps
 * itself honest afterward via `activeBucketChanged`, the same way it already
 * does for `hotkeyChanged`.
 */
async function hotkeyRow(
  config: Config,
  bucket: KeyboardBucket,
  label: string,
  initialActive: KeyboardBucket,
): Promise<HotkeyRow> {
  const wrapper = element("div", "setting-block");
  const line = element("div", "setting");
  const name = element("span", "setting-label");
  name.textContent = label;
  const active = element("span", "setting-active");
  active.textContent = "Active";
  const value = element("span", "setting-value");
  const action = document.createElement("button");
  line.append(name, active, value, action);

  const problem = element("div", "setting-error");
  wrapper.append(line, problem);

  let listening = false;
  let isActive = bucket === initialActive;

  const draw = (hotkey: Hotkey) => {
    value.textContent = listening ? "Press a key…" : hotkeyLabel(hotkey);
    action.textContent = listening ? "Cancel" : "Change";
    active.hidden = !isActive;
  };

  let current = config.hotkeys[bucket];
  draw(current);

  action.addEventListener("click", () => {
    void (async () => {
      problem.textContent = "";
      if (listening) {
        await cancelHotkeyCapture();
        listening = false;
        draw(current);
        return;
      }

      listening = true;
      draw(current);
      try {
        await captureHotkey();
      } catch (err) {
        // The likeliest cause is a missing Accessibility grant, which the
        // banner above already explains how to fix.
        listening = false;
        draw(current);
        problem.textContent = String(err);
      }
    })();
  });

  // The capture reports through the ordinary rebinding event, so this is also
  // what keeps the row honest if the binding changes from anywhere else. A
  // capture always writes into whichever bucket was active when it started
  // (see the shell's `hotkey::capture`), so only react to a change for this
  // row's own bucket.
  const unlisten = await onEngineEvent((event) => {
    if (event.type === "hotkeyChanged" && event.bucket === bucket) {
      listening = false;
      current = event.hotkey;
      draw(current);
      return;
    }
    if (event.type === "activeBucketChanged") {
      isActive = event.bucket === bucket;
      draw(current);
    }
  });

  // A capture swallows keystrokes. Leaving one armed behind a tab the user has
  // navigated away from would half-deaden the keyboard with nothing on screen
  // to explain it.
  const release = () => {
    unlisten();
    if (listening) void cancelHotkeyCapture();
  };

  return { element: wrapper, release };
}
```

- [ ] **Step 3: Update `mountDictation` to render both rows**

Replace:

```ts
async function mountDictation(panel: HTMLElement): Promise<void> {
  const config = await getConfig();

  const primary = element("div", "card");
  primary.append(
    await hotkeyRow(config),
    toggleRow("Live preview", config.transcriptPreview, (value) => {
      void setConfig({ ...config, transcriptPreview: value });
    }),
  );
```

with:

```ts
async function mountDictation(panel: HTMLElement): Promise<void> {
  const config = await getConfig();
  const activeBucket = await activeKeyboardBucket();

  const builtIn = await hotkeyRow(config, "builtIn", "Built-in keyboard", activeBucket);
  const external = await hotkeyRow(config, "external", "External keyboard", activeBucket);

  const primary = element("div", "card");
  primary.append(
    builtIn.element,
    external.element,
    toggleRow("Live preview", config.transcriptPreview, (value) => {
      void setConfig({ ...config, transcriptPreview: value });
    }),
  );
```

and, further down in the same function, replace:

```ts
  const note = element("p", "note");
  note.textContent = "Insertion and language are not editable yet. Edit config.json to change them.";
  panel.append(note);
}
```

with:

```ts
  const note = element("p", "note");
  note.textContent = "Insertion and language are not editable yet. Edit config.json to change them.";
  panel.append(note);

  releasePanel = () => {
    builtIn.release();
    external.release();
  };
}
```

- [ ] **Step 4: Add the `.setting-active` badge style**

In `apps/desktop/src/styles.css`, add after the `.setting-error:not(:empty)` rule:

```css
.setting-active {
  font-size: 10px;
  font-weight: 600;
  letter-spacing: 0.02em;
  text-transform: uppercase;
  color: var(--accent);
}

.setting-active[hidden] {
  display: none;
}
```

- [ ] **Step 5: Type-check the whole frontend**

Run: `cd apps/desktop && npm run build`
Expected: `tsc --noEmit` passes with no errors, then `vite build` completes. Fix any remaining type errors — likely candidates: the `setHotkey` import from Step 1 if genuinely unused (remove it), or a stale `config.hotkey` reference elsewhere caught by `grep -rn "\.hotkey\b" apps/desktop/src --include=*.ts | grep -v "\.hotkeys\b"`.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/settings.ts apps/desktop/src/styles.css
git commit -m "frontend: two independently-editable hotkey rows with an active-bucket indicator"
```

---

### Task 12: Manual verification on a real Mac

**Files:** none (no code changes — this task is the spec's required manual pass, since event-tap + `IOHIDManager` + Accessibility-permission behavior cannot be exercised in CI).

- [ ] **Step 1: Build and run the dev app**

```bash
make dev
```

- [ ] **Step 2: Verify the default/fresh-install path**

Delete or rename the existing config (back it up first if it's real data), relaunch, and confirm Right Option starts/stops dictation on the built-in keyboard with no external keyboard attached.

- [ ] **Step 3: Verify attach/detach switching**

Attach a PC (or any non-Apple) keyboard. Confirm:
- Right Option stops working.
- The Menu key on the attached keyboard starts a recording.

Detach it. Confirm Right Option works again and the Menu key (if tested on a second keyboard) does not.

- [ ] **Step 4: Verify Settings**

Open Settings → Dictation. Confirm both "Built-in keyboard" and "External keyboard" rows are visible, each independently shows "Change"/"Cancel", each can be rebound via Change + pressing a key, and the "Active" badge sits next to whichever bucket matches what's currently attached — including moving live if you attach/detach while the window is open.

- [ ] **Step 5: Verify the widget hint**

Start a recording (toggle or push-to-talk, whichever `activation` is set) on each bucket and confirm the widget's hint line names the correct key for whichever keyboard is currently active.

- [ ] **Step 6: Verify old-config migration**

Hand-write a config file with the pre-feature shape (`{"hotkey": {"kind": "modifier", "key": "leftOption"}}`) at the app's config path, relaunch, and confirm both Settings rows show "Left Option" and dictation responds to Left Option on both keyboards until one binding is changed.

- [ ] **Step 7: No commit** — this task only confirms behavior. If anything above fails, fix it in the relevant earlier task's files and re-run `make check` (Task 8) / `npm run build` (Task 11) before re-testing.
