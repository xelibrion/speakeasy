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
type IOHIDDeviceCallback = extern "C" fn(
    context: *mut c_void,
    result: IOReturn,
    sender: *mut c_void,
    device: IOHIDDeviceRef,
);

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

extern "C" fn on_matched(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    device: IOHIDDeviceRef,
) {
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

extern "C" fn on_removed(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    device: IOHIDDeviceRef,
) {
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
