//! Whether anything on screen is ready to receive typed text.
//!
//! Injection posts keystrokes at whatever has focus. When nothing does — an
//! empty Space, a Finder window, a panel with no text field — the events go
//! nowhere and `inject` still reports success, because posting an event says
//! nothing about whether anyone read it. That silent loss is what this exists
//! to notice.
//!
//! # This is a hint, never a veto
//!
//! The Accessibility API answers well for apps built from native controls and
//! badly for apps that draw their own text. Measured against real apps:
//!
//! | App      | Focused role | `AXValue` settable |
//! |----------|--------------|--------------------|
//! | iTerm2   | `AXTextArea` | yes                |
//! | Messages | `AXTextArea` | yes                |
//! | Finder   | `AXGroup`    | no                 |
//! | Xcode    | `AXOutline`  | no                 |
//! | Emacs    | `AXWindow`   | no                 |
//! | Firefox  | `AXWindow`   | unsupported        |
//!
//! Emacs and Firefox accept typed text perfectly well and report otherwise.
//! Any use of this that *withholds* text would therefore break dictation into
//! them. So the caller injects regardless, and uses this only to decide whether
//! to keep the transcript within reach afterwards. The worst a wrong answer can
//! do is leave a Copy button on screen that nobody needed.
//!
//! [`Focus::Unknown`] exists for the same reason: an app that does not answer
//! must not be treated as an app with nowhere to type.

use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::CFString;

/// What the frontmost app's focused element looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// A focused element that accepts a value — a text field or text area.
    Editable,
    /// Focus exists but will not take text, or nothing is focused at all.
    NotEditable,
    /// The app did not answer: no Accessibility support, too slow, or busy.
    /// Indistinguishable from a working target, so treat it as one.
    Unknown,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementIsAttributeSettable(
        element: CFTypeRef,
        attribute: CFTypeRef,
        settable: *mut u8,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout: f32) -> i32;
}

extern "C" {
    fn CFRelease(cf: CFTypeRef);
}

const AX_SUCCESS: i32 = 0;

/// Seconds to wait for an app to answer.
///
/// This sits on the injection path, between the user releasing the key and
/// their text appearing, so it is a latency budget rather than a correctness
/// one. A busy app that blows through it is reported as [`Focus::Unknown`],
/// which is the harmless answer.
const TIMEOUT_S: f32 = 0.1;

/// Ask the frontmost app what has focus.
///
/// Deliberately per-application rather than through
/// `AXUIElementCreateSystemWide`: the system-wide element's
/// `AXFocusedUIElement` returns `kAXErrorCannotComplete` where the per-app
/// query succeeds.
pub fn focused_text_target() -> Focus {
    let (_, pid) = super::frontmost_app();
    if pid <= 0 {
        return Focus::Unknown;
    }

    unsafe {
        // `frontmost_app` reports an i64 for the same reason NSWorkspace
        // does; the AX API wants a pid_t.
        let app = AXUIElementCreateApplication(pid as i32);
        if app.is_null() {
            return Focus::Unknown;
        }
        // Released on every path below. The AX calls can each return early, so
        // this is the one thing that must not be forgotten.
        let _ = AXUIElementSetMessagingTimeout(app, TIMEOUT_S);

        let focused_key = CFString::from_static_string("AXFocusedUIElement");
        let mut focused: CFTypeRef = std::ptr::null();
        let status = AXUIElementCopyAttributeValue(app, focused_key.as_CFTypeRef(), &mut focused);
        CFRelease(app);

        if status != AX_SUCCESS || focused.is_null() {
            // Includes `kAXErrorNoValue` — nothing focused — which is a real
            // "nowhere to type", and the error codes that mean the app simply
            // did not answer. They are not worth separating: both leave the
            // transcript within reach, and neither withholds it.
            return if status == AX_SUCCESS {
                Focus::NotEditable
            } else {
                Focus::Unknown
            };
        }

        let value_key = CFString::from_static_string("AXValue");
        let mut settable: u8 = 0;
        let status =
            AXUIElementIsAttributeSettable(focused, value_key.as_CFTypeRef(), &mut settable);
        CFRelease(focused);

        // A settable `AXValue` is the test rather than the role: roles vary
        // wildly between toolkits, and "can I write to it?" is the question
        // actually being asked.
        match (status, settable) {
            (AX_SUCCESS, 0) => Focus::NotEditable,
            (AX_SUCCESS, _) => Focus::Editable,
            // Unsupported attribute — the element exists but has no value to
            // set, which is a real answer, not a failure to answer.
            _ => Focus::NotEditable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever is focused on the test machine is not ours to assert. What this
    /// pins is that the call returns rather than hanging on the messaging
    /// timeout, and that it never panics on a null element.
    #[test]
    fn the_lookup_answers_promptly() {
        let start = std::time::Instant::now();
        let _ = focused_text_target();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "the focus query should be bounded by its messaging timeout"
        );
    }
}
