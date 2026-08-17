# Per-keyboard hotkey bindings

## Problem

Speakeasy's push-to-talk binding is one global `Hotkey`. The default, Right
Option, is a good choice on a Mac's own keyboard but doesn't exist on a PC
keyboard; conversely a PC keyboard's Menu key (`KeyCode::ContextMenu`, already
modeled for exactly this) doesn't exist on a Mac keyboard. A user who switches
between a laptop's built-in keyboard and an attached PC keyboard today has to
manually re-bind every time they swap. The app should remember one binding for
the built-in keyboard and a separate one for an external keyboard, and switch
between them automatically as keyboards attach and detach.

## Behavior

- Two independent bindings: **built-in** and **external**. Defaults are Right
  Option (built-in, unchanged from today) and the Menu key (external).
- "External" is a single bucket, not per-model: any non-built-in keyboard
  attached selects the external binding. Attaching a second external keyboard
  changes nothing — the bucket is already external. This is a presence check,
  not an attribution of which physical keyboard produced a given keystroke.
- The active bucket is decided by presence at the moment: at least one
  external keyboard attached → `External`; none attached → `BuiltIn`. This is
  re-evaluated at launch (in case an external keyboard is already attached
  when the app starts) and on every attach/detach.
- Only the active bucket's binding is live. While an external keyboard is
  attached, the built-in binding does nothing — pressing Right Option does
  not start a recording — matching the mental model "attach a PC keyboard,
  it uses Menu now."
- Activation mode (push-to-talk vs. toggle) stays a single setting shared by
  both bindings. It describes *how* a key behaves, not *which* key, and there
  is no stated need for it to vary by keyboard.
- Settings shows both bindings as two always-visible rows, each independently
  capturable, with a small indicator on whichever one is currently active.
- macOS only. Linux's `HotkeyListener` (`X11Hotkeys`) is an unimplemented
  stub regardless of this feature, so bucket-switching has nothing to drive
  there; `KeyboardWatcher` gets a no-op Linux implementation for the same
  reason `X11Hotkeys::register` already errors out.

## Design

### Core (`speakeasy-core`)

**`config.rs`** — replace the single `hotkey: Hotkey` field with:

```rust
pub struct HotkeyBindings {
    pub built_in: Hotkey,
    pub external: Hotkey,
}

impl Default for HotkeyBindings {
    fn default() -> Self {
        Self {
            built_in: Hotkey::default(), // Modifier(RightOption), unchanged
            external: Hotkey::Chord {
                mods: ChordMods::default(),
                key: KeyCode::ContextMenu,
            },
        }
    }
}
```

`Config` gains `pub hotkeys: HotkeyBindings` in place of `pub hotkey: Hotkey`.

Migration, following the existing `model_path` pattern exactly
(deserialize-then-fold in `Config::migrate`): add
`#[serde(skip_serializing_if = "Option::is_none")] pub hotkey: Option<Hotkey>`
— a field that only ever gets populated by deserializing an old config file
that still has a top-level `hotkey` key, and is never written by this build.
In `migrate()`: if `self.hotkey` is `Some(h)` (via `.take()`) and `hotkeys ==
HotkeyBindings::default()`, set both `hotkeys.built_in` and
`hotkeys.external` to `h`. This preserves a deliberate custom binding (e.g. a
chord like ⌘⌥Space, which works identically on any keyboard) on both buckets,
the same "absent vs. explicitly default" guard `model_path` migration already
uses.

New enum, next to `Hotkey`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyboardBucket {
    BuiltIn,
    External,
}
```

`HotkeyBindings` gains `pub fn get(&self, bucket: KeyboardBucket) -> &Hotkey`
and a `set` counterpart, so callers don't hand-roll the match.

**`ports.rs`** — new port, parallel to `HotkeyListener`:

```rust
pub type KeyboardBucketCallback = Box<dyn Fn(KeyboardBucket) + Send + Sync>;

/// Reports which keyboard bucket is active, based on what's attached.
pub trait KeyboardWatcher: Send + Sync {
    /// Report the bucket that's active right now, then again on every
    /// attach/detach that changes it. Must call `on_change` once
    /// synchronously (or as soon as practical) with the starting bucket, so
    /// a caller never has to special-case "nothing has fired yet."
    fn watch(&self, on_change: KeyboardBucketCallback) -> Result<()>;
}
```

**`engine.rs`** — replace `set_hotkey` with:

```rust
pub fn set_hotkey(&self, bucket: KeyboardBucket, hotkey: Hotkey) {
    {
        let mut config = self.inner.config.lock().expect("config mutex poisoned");
        if config.hotkeys.get(bucket) == &hotkey {
            return;
        }
        config.hotkeys.set(bucket, hotkey.clone());
    }
    self.persist();
    self.emit(Event::HotkeyChanged { bucket, hotkey });
}
```

New `active_bucket: Mutex<KeyboardBucket>` field on `Inner` (default
`BuiltIn`, overwritten as soon as the real watcher reports in — see below),
and:

```rust
pub fn active_bucket(&self) -> KeyboardBucket { ... }

/// Called by the shell when the keyboard watcher reports a change.
pub fn set_active_bucket(&self, bucket: KeyboardBucket) {
    {
        let mut active = self.inner.active_bucket.lock().expect("active bucket mutex poisoned");
        if *active == bucket { return; }
        *active = bucket;
    }
    self.emit(Event::ActiveBucketChanged { bucket });
}
```

Not persisted — it's a live fact about what's plugged in, not a preference.

**`event.rs`** — `HotkeyChanged` gains a `bucket: KeyboardBucket` field. New
variant:

```rust
ActiveBucketChanged { bucket: KeyboardBucket },
```

**`fakes.rs`** — the existing `HotkeyListener` fake is untouched; add a
`FakeKeyboardWatcher` that calls back with a bucket fixed at construction
(tests don't need attach/detach simulation at the engine level — that logic
lives in the platform watcher, tested separately).

### macOS platform (`speakeasy-platform/src/macos`)

**New `keyboard_watch.rs`** — `IOHIDManager`-based watcher:

- Open an `IOHIDManager`, set a matching dictionary for
  `kIOHIDDeviceUsagePageKey = kHIDPage_GenericDesktop`,
  `kIOHIDDeviceUsageKey = kHIDUsage_GD_Keyboard`.
- Register `IOHIDManagerRegisterDeviceMatchingCallback` (attach) and
  `IOHIDManagerRegisterDeviceRemovalCallback` (detach), schedule on a run
  loop the same way `hotkey.rs` already spins one up on its own thread
  (`CFRunLoop::run_current()` after scheduling the manager with
  `IOHIDManagerScheduleWithRunLoop`).
- Each callback reads `kIOHIDBuiltInKey` off the `IOHIDDeviceRef`
  (`IOHIDDeviceGetProperty`); absent or `false` means external.
- Maintain an external-device count (`HashSet<IOHIDDeviceRef>` keyed by
  pointer, or a plain `AtomicUsize` incremented/decremented — a set is safer
  against a spurious double-attach callback). On any change where
  `count > 0` vs `count == 0` flips, invoke the callback with the new
  bucket. Also invoke it once immediately after the initial device
  enumeration, so `watch()`'s "report the starting bucket" contract holds
  even if no external keyboard is attached at launch.
- This logic — "given a stream of (device id, built-in?, attach|detach)
  events, does the external-count crossing zero change the bucket" — is
  small and pure enough to pull into a free function the module unit-tests
  directly, the same separation `speakeasy-core::hotkey::match_event` uses
  to keep matching logic out of the tap callback.

**`mod.rs`** — export `MacKeyboardWatcher`, add it to whatever constructs
`MacHotkeys` today (`speakeasy_platform::hotkey_listener()`); add a sibling
`speakeasy_platform::keyboard_watcher()`.

### Linux platform (`speakeasy-platform/src/linux`)

New `keyboard_watch.rs` with a stub `X11KeyboardWatcher` whose `watch` calls
`on_change(KeyboardBucket::BuiltIn)` once and returns `Ok(())` — there's
nothing to distinguish without real device enumeration, and `BuiltIn` keeps
existing single-binding behavior effectively intact for the platform that
doesn't have a working `HotkeyListener` yet anyway.

### Shell wiring (`apps/desktop/src-tauri`)

**`hotkey.rs`**:

- `register(engine, hotkey)` unchanged in shape, but every call site now
  passes `engine.config().hotkeys.get(engine.active_bucket())` instead of
  `engine.config().hotkey`.
- New `pub fn watch_keyboard(engine: Engine) -> Result<(), Box<dyn Error>>`,
  called once at startup (alongside the existing `bind_if_unbound` call
  path): gets the platform's `keyboard_watcher()`, calls `.watch()` with a
  closure that does `engine.set_active_bucket(bucket)` then
  `register(engine.clone(), engine.config().hotkeys.get(bucket))`.
- `bind_if_unbound` and `capture`'s `set_hotkey` call both need the active
  bucket: `capture` now calls `engine.set_hotkey(engine.active_bucket(),
  hotkey)` — you can only ever capture *into* whichever bucket is currently
  live, which is also the only one that makes sense to test against (you're
  holding whichever keyboard is attached).

**`bridge.rs`**: the `HotkeyChanged` handler adds a bucket check —
re-register only `if event.bucket == engine.active_bucket()` — since a
`HotkeyChanged` for the *inactive* bucket (the other Settings row) must not
tear down the tap that's currently working. New arm for
`Event::ActiveBucketChanged { bucket }`: re-register with
`engine.config().hotkeys.get(bucket)` unconditionally (this is what actually
performs the swap-tap-binding-on-attach behavior).

**`commands.rs`**:

- `set_hotkey(engine, hotkey: Hotkey)` → `set_hotkey(engine, bucket:
  KeyboardBucket, hotkey: Hotkey)`.
- `bind_hotkey` reads `engine.config().hotkeys.get(engine.active_bucket())`
  instead of `.hotkey`.
- New `#[tauri::command] pub fn active_keyboard_bucket(engine) ->
  KeyboardBucket` so Settings can render the initial "active" indicator
  without waiting for an event (mirrors how `getConfig` is a pull, while
  `HotkeyChanged` is the push for later changes).

### Frontend (`apps/desktop/src`)

**`settings.ts`**:

- `hotkeyRow(config)` becomes `hotkeyRow(config, bucket, label, active)`,
  parameterized the same way the existing function already reads/writes
  `config.hotkey` — swap that for `config.hotkeys[bucket]`, and
  `captureHotkey`/`setHotkey` calls gain the bucket argument.
- `mountDictation` renders two rows — `hotkeyRow(config, "builtIn", "Built-in
  keyboard", …)` and `hotkeyRow(config, "external", "External keyboard",
  …)` — instead of one, each independently capturable per the earlier
  decision (two rows, always visible).
- Active-bucket indicator: fetch `activeKeyboardBucket()` once on mount for
  the initial label, then listen for `activeBucketChanged` the same way the
  existing `onEngineEvent` listener already reacts to `hotkeyChanged` to
  keep the row's displayed value honest — add a small "Active" pill on
  whichever row currently matches.
- `onEngineEvent`'s `hotkeyChanged` handling gains a bucket check, updating
  only the matching row's `current`/`draw`.

**`engine.ts`** (Tauri command bindings): mirror the Rust command signature
changes — `setHotkey(bucket, hotkey)`, new `activeKeyboardBucket()`, new
`KeyboardBucket` type, new `activeBucketChanged` event shape.

## Testing

- **Core**: `config.rs` — round-trip test for `HotkeyBindings`; migration
  test asserting a legacy `{"hotkey": {...}}` file populates both `built_in`
  and `external` with the same value and drops the legacy field on next
  save (mirrors the existing `model_path` migration tests). `engine.rs` —
  `set_hotkey` persists into the right bucket only and emits
  `HotkeyChanged{bucket, hotkey}`; `set_active_bucket` emits
  `ActiveBucketChanged` and is idempotent (no event on a no-op set),
  following the existing `setting_the_hotkey_persists_it_and_announces_the_change`
  test shape.
- **Platform (macOS)**: pull the "does an attach/detach event flip the
  bucket" decision into a pure function over a device list/count, and unit
  test it directly (attach one external → External; attach a second
  external → stays External; detach one of two → stays External; detach the
  last → BuiltIn) without touching `IOHIDManager`. The `IOHIDManager` glue
  itself is untested, consistent with `hotkey.rs`'s own tap callback being
  untested today.
- **Manual verification on a real Mac** (required regardless, like the
  existing Esc-cancel feature): fresh install defaults to Right Option
  working on the built-in keyboard; attach a PC keyboard, confirm Right
  Option stops working and Menu starts; detach it, confirm the reverse;
  confirm Settings shows both rows, each independently rebindable, with the
  active indicator following attach/detach; confirm a config written by an
  older build (single `hotkey`) still loads and both new rows show that same
  binding.

## Out of scope

- Per-exact-device bindings (distinguishing one external keyboard model from
  another) — explicitly rejected in favor of a single built-in/external
  bucket.
- Any change to activation mode (push-to-talk vs. toggle) varying by
  keyboard.
- Linux/X11 real device-based bucket detection — the stub always reports
  `BuiltIn`, consistent with `X11Hotkeys::register` being unimplemented.
- Per-keystroke device attribution (rebinding based on which physical
  keyboard sent a given key, rather than which keyboards are currently
  attached).
