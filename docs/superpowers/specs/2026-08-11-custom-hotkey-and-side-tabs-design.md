# Custom hotkey, tray Speak, and side tabs

Date: 2026-08-11
Status: approved, not yet implemented

## Problem

The dictation hotkey can only be changed by editing `config.json` and
restarting. The Dictation pane renders it read-only and says so: "These are not
editable yet." `Config::hotkey` already has two shapes, but only one of them
works — `speakeasy-platform`'s macOS listener hard-errors on
`Hotkey::Accelerator`, because the event tap watches `CGEventType::FlagsChanged`
and therefore sees modifiers going up and down and nothing else. So the real
choice available to a user today is one of nine bare modifiers, typed by hand
into a JSON file.

Two smaller gaps travel with it. There is no way to start dictation from the
menu bar, which is exactly what a user needs when the hotkey is misconfigured —
the case where the hotkey is least able to help. And the settings tabs run
across the top, which does not scale as panes are added.

This design makes the hotkey bindable by pressing it, extends the platform layer
to real chords, adds a tray item that starts and stops a recording, and moves
the tabs to the side.

## Goals

- Bind the hotkey from settings by pressing the key, not by picking from a list.
- Support bare modifiers (side-aware) and full chords such as `⌘⌥Space`.
- Consume a bound chord so it does not also fire in the focused app.
- Never consume a bare modifier.
- Start and stop a recording from the tray menu.
- Move the settings tabs to the side, with the permissions banner still above
  everything.
- Keep the match logic testable without an event tap.

## Non-goals

- Editors for the other dictation settings — activation mode, injection mode,
  language. They stay read-only; each is its own control and none was asked for.
- Detecting that a chosen chord collides with a system or third-party shortcut.
  The user finds out by trying it. Detection would mean enumerating other apps'
  bindings, which macOS does not expose.
- Per-application hotkeys.
- Linux/X11 chord support. The X11 listener keeps its current behaviour; the
  new `Hotkey::Chord` shape returns `Error::Hotkey` there until someone
  implements it, which is what the port contract already requires of an
  unsupported shape.
- Rebinding while a recording is in flight.

## The hotkey shape

`Hotkey::Accelerator { accelerator: String }` is replaced by a structured
chord:

```rust
pub enum Hotkey {
    Modifier { key: ModifierKey },
    Chord { mods: ChordMods, key: KeyCode },
}

pub struct ChordMods { command: bool, option: bool, control: bool, shift: bool }
pub enum KeyCode { /* A–Z, Digit0–9, F1–F20, Space, Return, Escape, … */ }
```

A string accelerator is the wrong intermediate for capture-driven binding: the
capture produces a keycode and a flag set, and a string would be formatted only
to be parsed back. It also cannot express what the tap actually sees.

Chords are side-agnostic and bare modifiers stay side-aware. `⌘⌥Space` should
not care which Command key; a modifier binding is the one case where the side is
the whole point, and IOKit's device-dependent flag bits are what make it
knowable. Fn stays modifier-only — macOS reports it in the flags and never as a
keycode.

Both variants stay flat owned enums with no generics, lifetimes or borrowed
returns, so the binding-layer constraint in the README still holds.

### Migration

A config carrying `{"kind": "accelerator", …}` deserialises to the default
binding, with a warning logged. This matches `load_config`'s existing stance
that a bad config starts from defaults rather than refusing to launch: a
menu-bar app that dies on a bad config leaves the user no UI to fix it with.
Nothing on macOS can be depending on the old variant, because registering it
returned `Error::Hotkey`.

## Matching, and where it lives

`speakeasy-core` gains a pure function:

```rust
pub fn match_event(hotkey: &Hotkey, event: RawKeyEvent) -> Option<HotkeyEvent>
```

`RawKeyEvent` is a flat description of what the tap saw — event kind, keycode,
flag bits — so core does not learn about CoreGraphics. The tap becomes a thin
adapter that translates a `CGEvent` into a `RawKeyEvent` and forwards the
result.

This placement is the point. If the match logic lived in the tap callback it
would be reachable only by pressing keys on a real Mac with Accessibility
granted, and chord matching is where the fiddly cases are: a chord fires only
when the modifier set matches exactly, so `⌘⌥Space` must not fire on
`⌘⇧⌥Space`; release is the key lifting *or* any required modifier lifting,
whichever comes first.

## The tap

In `crates/speakeasy-platform/src/macos/hotkey.rs`:

- Add `CGEventType::KeyDown` and `KeyUp` to the tap's event list.
- Switch `CGEventTapOptions::ListenOnly` to `Default`, so the callback can
  consume an event by returning `None`.
- Consume only when the binding is a `Chord` and it matched. A `Modifier`
  binding always passes the event through.
- Handle `kCGEventTapDisabledByTimeout` and `kCGEventTapDisabledByUserInput` by
  re-enabling the tap.

The pass-through rule is not a detail. The default binding is Right Option, and
swallowing it would break Option-based accented input (`⌥e → é`) for everyone
who never changes the setting.

The disabled-tap handling is new failure-mode surface introduced by this design.
A `Default` tap can be disabled by macOS under load, where the existing
`ListenOnly` tap could not be. Without re-arming, the app silently goes deaf —
the most likely "worked yesterday, dead today" report this feature can produce.

## Capture

`HotkeyListener` gains two methods:

```rust
fn capture(&self, on_captured: CaptureCallback) -> Result<()>;
fn cancel_capture(&self);
```

Capture reuses the same tap rather than listening in the webview. A webview
`keydown` listener would be much cheaper and `event.code` even distinguishes
`AltLeft` from `AltRight` — but macOS never delivers Fn to a webview, so a key
that can be bound would not be capturable. Reusing the tap makes "what capture
sees" and "what can be bound" the same set by construction.

While capture is armed every matching event is swallowed, so binding `⌘Q` does
not quit the app on the way in. Esc cancels, and so does a timeout, so a capture
cannot strand the keyboard if the settings window is closed mid-bind.

Capture requires Accessibility. Without it the tap cannot be created, `capture`
returns `Error::Hotkey`, and the pane says so rather than sitting silent — the
same failure the permissions banner already exists to explain.

## Re-registering

The engine does not own the hotkey listener; `apps/desktop/src-tauri/src/hotkey.rs`
does, and `main.rs` registers once during startup.

Two options were considered.

**Engine owns the listener.** Matches the ports table in the README, and makes
`set_hotkey` atomic: validate, bind, persist, roll back if binding fails. But
the registered callback calls back into the engine, so the engine would hold a
listener holding a callback holding the engine — an `Arc` cycle needing `Weak`
to break, for a leak that would not show up in any test.

**Shell re-registers on an event.** Core persists the new hotkey and emits
`HotkeyChanged`; `bridge.rs` re-registers, next to where it already updates the
tray on engine events. `register()` already calls `unregister()` first, so
rebinding works with no change to the listener's lifecycle.

The second is chosen. It is the smaller change, it introduces no cycle, and
re-registering is mechanical rather than a decision — the shell is still
deciding nothing, which is the rule that matters.

The engine surface is `Engine::set_hotkey(Hotkey) -> Result<()>`: validate,
persist, emit. The corresponding command stays a one-line delegation.

## Settings UI

The Dictation pane's hotkey row becomes editable and gains three states:

```
idle       Hotkey   Right Option           [ Change ]
listening  Hotkey   Press a key…           [ Cancel ]
rejected   Hotkey   Press a key…           [ Cancel ]
                    ⌃⌘ is modifiers only — add a key
```

The remaining rows stay read-only, and the note narrows to say so.

## Tray

The menu gains `Speak` and `Cancel`. Menu items cannot be reordered after
creation, so both are built up front and their label and enabled state are
rewritten from engine state by the same `render` path that already maintains
the status line:

```
── idle ──          ── recording ──
 Ready               Recording…
 ─────────           ─────────
 Speak               Stop and insert
 (Cancel, hidden)    Cancel
 Settings…           Settings…
 ─────────           ─────────
 Quit                Quit
```

`Speak` delegates to `begin_recording`, `Stop and insert` to
`finish_recording`, and — as in `hotkey.rs` — the finish path runs on a spawned
thread, because transcription blocks and must not stall the main thread.

Opening a menu takes key focus, but it returns to the previous app when the menu
closes, which happens before transcription finishes. The app is `Accessory` and
never activates, so the transcript still lands where the user was typing.

## Side tabs

`.settings` becomes a two-column grid with the banner spanning both columns, so
a missing Accessibility grant still cannot hide behind a tab. `.tabs` becomes a
column, and the selected indicator moves from `inset 0 -2px` to a left edge.

```
┌──────────────────────────────┐
│ ⚠ Needs Accessibility [Open] │
├──────────┬───────────────────┤
│ Models   │                   │
│ Dictation│   pane content    │
└──────────┴───────────────────┘
```

The TS change is small: the tab list is already data-driven and its `role` and
`aria-selected` attributes are already correct, so neither changes.

## Testing

- `match_event` against a table of chord and modifier cases: exact modifier
  match, superset rejection, release by key, release by modifier, unrelated
  keys ignored. This is where the logic risk is, and it needs no Mac.
- `Hotkey` serde round-trip for both variants, and a legacy `accelerator`
  config falling back to the default with the config still loading.
- `FakeHotkeys` gains capture support, so engine-level tests can drive a bind.
- `Engine::set_hotkey` persists and emits.
- Keycode mapping between `KeyCode` and `CGKeyCode` in the platform crate.
- The tap itself, the swallow behaviour, and the disabled-tap re-arm stay
  manually verified. They need a real event tap and a granted permission.

## Risks

- **The tap goes deaf.** Covered by re-arming on the disabled events, but it
  cannot be covered by a test.
- **A user binds something they need.** Swallowing is what makes this bite.
  `config.json` remains hand-editable, which is the escape hatch.
- **Chord release is ambiguous** when several modifiers lift at once. Resolved
  by treating the first release as the release and ignoring the rest until the
  next press.
