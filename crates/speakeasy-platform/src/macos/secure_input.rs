//! Who, if anyone, is holding macOS secure input.
//!
//! Secure input is what a password field turns on. It disables event taps
//! process-wide, which kills both the hotkey and text injection — every app on
//! the machine, not just the one that asked. Nothing clears it but the holder
//! giving it up.
//!
//! [`IsSecureEventInputEnabled`] answers whether it is on. It does not say who
//! turned it on, and that is the part the user needs: "secure input is active"
//! is not something anyone can act on, where "1Password is blocking input" says
//! exactly which window to leave.
//!
//! The name comes from `kCGSSessionSecureInputPID` in the window server's
//! session dictionary, which is private API — hence `dlsym` rather than a link.
//! Everything here degrades to a name of `None` rather than failing: a missing
//! symbol on some future macOS should cost the user a name, never the check.

use std::ffi::{c_void, CString};

use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

extern "C" {
    fn dlopen(path: *const i8, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const i8) -> *mut c_void;
}

const RTLD_LAZY: i32 = 1;
const SKYLIGHT: &str = "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";

/// The app blocking input, or `None` if nothing is.
///
/// `Some` always carries a printable name: when secure input is on but the
/// holder cannot be identified, that is still a fact worth reporting, so it
/// comes back as a generic phrase rather than as `None`.
pub fn blocking_app() -> Option<String> {
    if !unsafe { IsSecureEventInputEnabled() } {
        return None;
    }

    let pid = holder_pid();
    let name = pid
        .and_then(app_name)
        // Not a placeholder to be filled in later: this is a real outcome, and
        // the sentence it lands in has to read properly without a name.
        .unwrap_or_else(|| "Another app".to_string());

    // Logged rather than left to the UI: when someone reports that dictation
    // went nowhere, this line is the difference between knowing secure input
    // was involved and guessing.
    tracing::info!(app = name, pid = pid.unwrap_or(-1), "secure input is held");
    Some(name)
}

/// Pid recorded against `kCGSSessionSecureInputPID`, if the window server
/// exposes one.
///
/// Note this is the *responsible* process, not always the one that made the
/// call — a command-line tool run from a terminal is reported as the terminal.
/// That is the right answer here: it names the window the user can see.
fn holder_pid() -> Option<i32> {
    let session = current_session()?;
    let key = CFString::from_static_string("kCGSSessionSecureInputPID");
    let value = session.find(key.as_CFTypeRef().cast())?;
    let number = unsafe { CFNumber::wrap_under_get_rule((*value).cast()) };
    number.to_i32()
}

fn current_session() -> Option<CFDictionary> {
    type Copy = unsafe extern "C" fn() -> *const c_void;

    unsafe {
        let path = CString::new(SKYLIGHT).expect("path has no NUL");
        let library = dlopen(path.as_ptr(), RTLD_LAZY);
        if library.is_null() {
            return None;
        }

        let symbol = CString::new("CGSCopyCurrentSessionDictionary").expect("symbol has no NUL");
        let address = dlsym(library, symbol.as_ptr());
        if address.is_null() {
            return None;
        }

        let copy: Copy = std::mem::transmute(address);
        let dictionary = copy();
        if dictionary.is_null() {
            return None;
        }
        // `Copy` in the name: we own this reference, so take it under the
        // create rule and let CFDictionary release it.
        Some(CFDictionary::wrap_under_create_rule(dictionary.cast()))
    }
}

/// Localized name of a running application, as the user would recognise it.
fn app_name(pid: i32) -> Option<String> {
    extern "C" {
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe {
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let send_i64: unsafe extern "C" fn(*mut c_void, *mut c_void, i64) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let sel = |name: &str| {
            let c = CString::new(name).expect("selector has no NUL");
            sel_registerName(c.as_ptr() as *const u8)
        };

        let class = CString::new("NSRunningApplication").expect("class name has no NUL");
        let running = objc_getClass(class.as_ptr() as *const u8);
        if running.is_null() {
            return None;
        }

        let app = send_i64(
            running,
            sel("runningApplicationWithProcessIdentifier:"),
            i64::from(pid),
        );
        if app.is_null() {
            return None;
        }

        let name = send(app, sel("localizedName"));
        if name.is_null() {
            return None;
        }
        let utf8 = send(name, sel("UTF8String"));
        if utf8.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(utf8 as *const i8)
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not an assertion about the machine's state — whether secure input
    /// happens to be on during a test run is not ours to decide. What this
    /// pins is that the lookup completes without tripping over the private
    /// symbol, and that a reported block always comes with something to print.
    #[test]
    fn the_lookup_returns_a_printable_name_or_nothing() {
        if let Some(name) = blocking_app() {
            assert!(!name.is_empty());
        }
    }

    /// The only test that actually exercises the private symbol, by becoming
    /// the holder itself.
    ///
    /// Ignored by default because it turns secure input on for the whole
    /// machine for the length of the call — every other app's event taps go
    /// dead while it runs, which is not a thing to do to someone in the
    /// background of `make check`.
    ///
    /// Run it with:
    /// `cargo test -p speakeasy-platform -- --ignored names_the_holder`
    #[test]
    #[ignore = "toggles secure input machine-wide"]
    fn names_the_holder_while_we_hold_it() {
        extern "C" {
            fn EnableSecureEventInput();
            fn DisableSecureEventInput();
        }

        assert_eq!(blocking_app(), None, "something else already holds it");

        unsafe { EnableSecureEventInput() };
        let held = blocking_app();
        unsafe { DisableSecureEventInput() };

        let name = held.expect("we are holding it, so someone should be named");
        assert_ne!(name, "Another app", "the pid lookup found nothing");
        println!("secure input attributed to: {name}");
    }
}
