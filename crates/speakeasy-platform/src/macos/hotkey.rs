//! Push-to-talk on macOS, via a CoreGraphics event tap.
//!
//! An ordinary global-shortcut registration cannot express this binding: it
//! needs a non-modifier key, and it reports "Option is down" without saying
//! which Option. Push-to-talk wants exactly the opposite — a bare modifier,
//! side-specific, with separate press and release.
//!
//! So we tap `FlagsChanged` and read IOKit's *device-dependent* modifier bits,
//! which do encode the side. The tap is `ListenOnly`, so keystrokes are
//! observed and never swallowed: holding Right Option still types whatever it
//! normally types.
//!
//! Requires Accessibility permission. Without it `CGEventTap::new` fails, which
//! surfaces as [`Error::InputPermissionDenied`] rather than a binding that
//! silently never fires.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use core_foundation::base::TCFType;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopRef};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
};
use speakeasy_core::{
    Error, Hotkey, HotkeyCallback, HotkeyEvent, HotkeyListener, ModifierKey, Result,
};

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
        let key = match hotkey {
            Hotkey::Modifier { key } => *key,
            // Chords need KeyDown/KeyUp in the tap and the swallow path, which
            // land with the tap rework. Failing loudly is what the port
            // contract requires of a shape an implementation cannot bind.
            Hotkey::Chord { mods, key } => {
                return Err(Error::Hotkey(format!(
                    "chord `{}{}` cannot be bound yet",
                    mods.label(),
                    key.label()
                )));
            }
        };

        self.unregister()?;

        let mask = device_mask(key);
        let callback = Arc::new(on_event);
        // `CGEventTap::new` wants an `Fn`, so press state lives in an atomic
        // rather than a captured `mut`.
        let is_down = Arc::new(AtomicBool::new(false));

        // The tap must be created on the thread that runs the loop, so report
        // success or failure back to this one.
        let (ready_tx, ready_rx) = channel::<Result<RunLoopHandle>>();

        std::thread::Builder::new()
            .name("speakeasy-hotkey".into())
            .spawn(move || {
                let tap = CGEventTap::new(
                    CGEventTapLocation::HID,
                    CGEventTapPlacement::HeadInsertEventTap,
                    // Observe only: never consume the user's keystrokes.
                    CGEventTapOptions::ListenOnly,
                    vec![CGEventType::FlagsChanged],
                    move |_proxy, _event_type, event| {
                        let down = event.get_flags().bits() & mask != 0;
                        // FlagsChanged fires for every modifier, so only act on
                        // an actual transition of the one we care about.
                        if is_down.swap(down, Ordering::SeqCst) != down {
                            callback(if down {
                                HotkeyEvent::Pressed
                            } else {
                                HotkeyEvent::Released
                            });
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
        tracing::info!(hotkey = key.label(), "push-to-talk bound");
        Ok(())
    }

    fn unregister(&self) -> Result<()> {
        if let Some(handle) = self.running.lock().expect("running mutex poisoned").take() {
            handle.stop();
        }
        Ok(())
    }
}

impl Drop for MacHotkeys {
    fn drop(&mut self) {
        let _ = self.unregister();
    }
}
