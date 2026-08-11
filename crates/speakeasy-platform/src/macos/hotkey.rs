//! Push-to-talk on macOS, via a CoreGraphics event tap.
//!
//! An ordinary global-shortcut registration cannot express this binding: it
//! needs a non-modifier key, and it reports "Option is down" without saying
//! which Option. Push-to-talk wants exactly the opposite — a bare modifier,
//! side-specific, with separate press and release.
//!
//! So we tap `FlagsChanged` and read IOKit's *device-dependent* modifier bits,
//! which do encode the side. Chords need `KeyDown`/`KeyUp` as well, and the
//! decision of whether an event satisfies the binding lives in
//! `speakeasy_core::hotkey` — this file only translates and reacts.
//!
//! # Swallowing
//!
//! The tap is `Default` rather than `ListenOnly`, because a bound chord must
//! not also fire in the focused app: binding `⌘⌥Space` should not open
//! Spotlight every time you dictate.
//!
//! **A bare modifier is never swallowed.** The default binding is Right Option,
//! and consuming it would break Option-based accented input (`⌥e → é`) for
//! everyone who never changes the setting.
//!
//! Suppression is done by rewriting the event to [`CGEventType::Null`] in
//! place. The obvious approach — returning `None` from the callback — does not
//! work: `core-graphics`'s trampoline treats `None` as "pass the original
//! through" and offers no way to return the NULL that CoreGraphics reads as
//! "delete this event".
//!
//! # Cost of a `Default` tap
//!
//! macOS can disable one that takes too long to respond, which a `ListenOnly`
//! tap was never subject to. `TapDisabledByTimeout` and `TapDisabledByUserInput`
//! are delivered to the callback for exactly this, and are re-enabled there.
//! Without that the app goes silently deaf until it is restarted.
//!
//! Requires Accessibility permission. Without it `CGEventTap::new` fails, which
//! surfaces as [`Error::InputPermissionDenied`] rather than a binding that
//! silently never fires.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopRef};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, EventField,
};
use speakeasy_core::hotkey::{ChordMods, RawEventKind, RawKeyEvent};
use speakeasy_core::{
    CaptureCallback, Error, Hotkey, HotkeyCallback, HotkeyEvent, HotkeyListener, ModifierKey,
    Result,
};

use super::keycode;

extern "C" {
    /// Not exposed as a free function by `core-graphics` — only as
    /// `CGEventTap::enable`, which needs the tap, and the callback cannot hold
    /// the tap that owns it. The mach port is shared in instead.
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

/// Device-dependent modifier masks from IOKit's `NX_DEVICE*` constants. These
/// are the only way to tell left from right; `CGEventFlags`'s public constants
/// merge the two sides.
const fn device_mask(key: ModifierKey) -> u64 {
    match key {
        ModifierKey::LeftControl => 0x0000_0001,
        ModifierKey::LeftShift => 0x0000_0002,
        ModifierKey::RightShift => 0x0000_0004,
        ModifierKey::LeftCommand => 0x0000_0008,
        ModifierKey::RightCommand => 0x0000_0010,
        ModifierKey::LeftOption => 0x0000_0020,
        ModifierKey::RightOption => 0x0000_0040,
        ModifierKey::RightControl => 0x0000_2000,
        ModifierKey::Fn => 0x0080_0000,
    }
}

/// Every side-specific modifier currently held, read from the device-dependent
/// bits rather than `CGEventFlags`'s public constants, which merge the sides.
fn sides_down(flags: u64) -> Vec<ModifierKey> {
    [
        ModifierKey::LeftControl,
        ModifierKey::LeftShift,
        ModifierKey::RightShift,
        ModifierKey::LeftCommand,
        ModifierKey::RightCommand,
        ModifierKey::LeftOption,
        ModifierKey::RightOption,
        ModifierKey::RightControl,
        ModifierKey::Fn,
    ]
    .into_iter()
    .filter(|key| flags & device_mask(*key) != 0)
    .collect()
}

/// The side-agnostic view a chord is matched against.
fn chord_mods(flags: CGEventFlags) -> ChordMods {
    ChordMods {
        command: flags.contains(CGEventFlags::CGEventFlagCommand),
        option: flags.contains(CGEventFlags::CGEventFlagAlternate),
        control: flags.contains(CGEventFlags::CGEventFlagControl),
        shift: flags.contains(CGEventFlags::CGEventFlagShift),
    }
}

/// Flatten a `CGEvent` into the shape core matches against.
///
/// Returning `None` means "nothing core could act on" — a key we have no name
/// for, or an event type we did not ask for.
fn raw_event(event_type: CGEventType, event: &CGEvent) -> Option<RawKeyEvent> {
    let kind = match event_type {
        CGEventType::KeyDown => RawEventKind::KeyDown,
        CGEventType::KeyUp => RawEventKind::KeyUp,
        CGEventType::FlagsChanged => RawEventKind::FlagsChanged,
        _ => return None,
    };

    let flags = event.get_flags();
    let key = match kind {
        RawEventKind::FlagsChanged => None,
        _ => {
            let raw = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            // An unmapped key is not a match for anything, but it must still
            // reach the focused app — so this is `None`, not an early return
            // that would skip the pass-through.
            keycode::from_cg_keycode(raw)
        }
    };

    Some(RawKeyEvent {
        kind,
        key,
        mods: chord_mods(flags),
        sides: sides_down(flags.bits()),
    })
}

/// A run loop living on the tap's thread. `CFRunLoopStop` is safe to call from
/// another thread, which is how `unregister` gets the thread to exit.
struct RunLoopHandle(CFRunLoopRef);

// SAFETY: CFRunLoopRef is a CoreFoundation object; stopping a run loop from
// another thread is explicitly supported.
unsafe impl Send for RunLoopHandle {}

impl RunLoopHandle {
    fn stop(&self) {
        unsafe { CFRunLoop::wrap_under_get_rule(self.0) }.stop();
    }
}

#[derive(Default)]
pub struct MacHotkeys {
    running: Mutex<Option<RunLoopHandle>>,
}

impl MacHotkeys {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HotkeyListener for MacHotkeys {
    fn register(&self, hotkey: &Hotkey, on_event: HotkeyCallback) -> Result<()> {
        self.unregister()?;

        let hotkey = hotkey.clone();
        let label = hotkey.label();
        // Only a chord is taken from the app underneath; see the module docs.
        let swallows = matches!(hotkey, Hotkey::Chord { .. });
        let callback = Arc::new(on_event);
        // `CGEventTap::new` wants an `Fn`, so press state lives in an atomic
        // rather than a captured `mut`. This is also what makes `match_event`'s
        // statelessness safe: a held key repeats, and only a transition fires.
        let is_down = Arc::new(AtomicBool::new(false));

        // The callback cannot hold the tap that owns it, so the mach port is
        // published here once the tap exists and read back when macOS disables
        // it. A `usize` because the raw pointer is not `Send`, and only the
        // tap's own thread ever dereferences it.
        let port: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
        let port_for_callback = Arc::clone(&port);

        // The tap must be created on the thread that runs the loop, so report
        // success or failure back to this one.
        let (ready_tx, ready_rx) = channel::<Result<RunLoopHandle>>();

        std::thread::Builder::new()
            .name("speakeasy-hotkey".into())
            .spawn(move || {
                let tap = CGEventTap::new(
                    CGEventTapLocation::HID,
                    CGEventTapPlacement::HeadInsertEventTap,
                    // Default, not ListenOnly: a bound chord has to be taken
                    // from the app underneath. The cost is that macOS can
                    // disable this tap, handled below.
                    CGEventTapOptions::Default,
                    vec![
                        CGEventType::FlagsChanged,
                        CGEventType::KeyDown,
                        CGEventType::KeyUp,
                    ],
                    move |_proxy: CGEventTapProxy, event_type, event: &CGEvent| {
                        // Delivered when macOS has switched the tap off, which
                        // it can do to a Default tap that responds too slowly.
                        // Re-arming here is the difference between a hiccup and
                        // an app that is deaf until restarted.
                        if matches!(
                            event_type,
                            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                        ) {
                            tracing::warn!("the event tap was disabled by macOS; re-enabling");
                            if let Some(raw) =
                                *port_for_callback.lock().expect("tap port mutex poisoned")
                            {
                                // SAFETY: the port outlives the tap's run loop,
                                // and this runs on the tap's own thread.
                                unsafe { CGEventTapEnable(raw as CFMachPortRef, true) };
                            }
                            return None;
                        }

                        // `None` here means "pass it through untouched", which
                        // is the right answer for every key that is not bound.
                        let raw = raw_event(event_type, event)?;
                        let edge = speakeasy_core::hotkey::match_event(&hotkey, raw)?;

                        let down = edge == HotkeyEvent::Pressed;
                        // Only a transition is worth reporting: a held key
                        // repeats, and FlagsChanged fires for every modifier.
                        if is_down.swap(down, Ordering::SeqCst) != down {
                            callback(edge);
                        }

                        if swallows {
                            // `None` would pass the original through — the
                            // trampoline in core-graphics has no way to express
                            // deletion — so neuter the event instead.
                            event.set_type(CGEventType::Null);
                        }
                        None
                    },
                );

                let tap = match tap {
                    Ok(tap) => tap,
                    Err(()) => {
                        let _ = ready_tx.send(Err(Error::InputPermissionDenied(
                            "could not create an event tap; grant Speakeasy \
                             Accessibility access in System Settings › Privacy \
                             & Security › Accessibility"
                                .to_string(),
                        )));
                        return;
                    }
                };

                let source = match tap.mach_port.create_runloop_source(0) {
                    Ok(source) => source,
                    Err(()) => {
                        let _ = ready_tx.send(Err(Error::Hotkey(
                            "could not create a run loop source for the event tap".to_string(),
                        )));
                        return;
                    }
                };

                *port.lock().expect("tap port mutex poisoned") =
                    Some(tap.mach_port.as_concrete_TypeRef() as usize);

                let run_loop = CFRunLoop::get_current();
                run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
                tap.enable();

                if ready_tx
                    .send(Ok(RunLoopHandle(run_loop.as_concrete_TypeRef())))
                    .is_err()
                {
                    return; // caller gave up; drop the tap and exit
                }

                // Blocks until `unregister` stops it. `tap` stays alive for the
                // whole call, which is what keeps the tap installed.
                CFRunLoop::run_current();
            })
            .map_err(|err| Error::Hotkey(format!("could not start the hotkey thread: {err}")))?;

        let handle = ready_rx
            .recv()
            .map_err(|_| Error::Hotkey("the hotkey thread stopped unexpectedly".to_string()))??;

        *self.running.lock().expect("running mutex poisoned") = Some(handle);
        tracing::info!(hotkey = label, swallows, "push-to-talk bound");
        Ok(())
    }

    fn unregister(&self) -> Result<()> {
        if let Some(handle) = self.running.lock().expect("running mutex poisoned").take() {
            handle.stop();
        }
        Ok(())
    }

    /// Capture arrives with the tap rework; until then, fail rather than leave
    /// the settings window waiting on a report that never comes.
    fn capture(&self, _on_captured: CaptureCallback) -> Result<()> {
        Err(Error::Hotkey(
            "capturing a binding is not wired up yet".to_string(),
        ))
    }

    fn cancel_capture(&self) {}
}

impl Drop for MacHotkeys {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}
