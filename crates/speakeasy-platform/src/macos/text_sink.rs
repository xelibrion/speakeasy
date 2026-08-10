//! Text injection on macOS.
//!
//! Two strategies, both going through CGEvent, and both with real caveats:
//!
//! - [`InjectionMode::Synthetic`] posts key events carrying a unicode string.
//!   Preserves the clipboard, but several apps (notably Electron and some
//!   terminals) drop or reorder characters under fast synthetic input.
//! - [`InjectionMode::Paste`] writes to the pasteboard and posts Cmd-V. Far
//!   more reliable, at the cost of clobbering the clipboard — so the previous
//!   contents are saved and restored.
//!
//! Both fail while *secure input* is active: a focused password field disables
//! event taps process-wide. That surfaces as [`Error::SecureInputActive`]
//! rather than silently dropping the user's dictation.

use std::ffi::c_void;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use speakeasy_core::config::InjectionMode;
use speakeasy_core::{Error, Result, TextSink};

const KEY_V: u16 = 9;
/// Longest run of UTF-16 units to attach to a single synthetic key event.
const CHUNK: usize = 20;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {
    fn objc_getClass(name: *const u8) -> *mut c_void;
}

pub struct MacTextSink {
    mode: InjectionMode,
}

impl MacTextSink {
    pub fn new(mode: InjectionMode) -> Self {
        Self { mode }
    }

    /// Check before every injection, not once at startup — secure input
    /// toggles as the user moves between fields.
    fn secure_input_active() -> bool {
        unsafe { IsSecureEventInputEnabled() }
    }

    fn source() -> Result<CGEventSource> {
        CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|()| Error::Injection("could not create an event source".to_string()))
    }

    fn type_unicode(text: &str) -> Result<usize> {
        let source = Self::source()?;
        let utf16: Vec<u16> = text.encode_utf16().collect();

        for chunk in utf16.chunks(CHUNK) {
            // Keycode 0 with an attached string: the key itself is ignored and
            // the unicode payload is what gets delivered.
            let down = CGEvent::new_keyboard_event(source.clone(), 0, true)
                .map_err(|()| Error::Injection("could not create a key event".to_string()))?;
            down.set_string_from_utf16_unchecked(chunk);
            down.post(CGEventTapLocation::HID);

            let up = CGEvent::new_keyboard_event(source.clone(), 0, false)
                .map_err(|()| Error::Injection("could not create a key event".to_string()))?;
            up.set_string_from_utf16_unchecked(chunk);
            up.post(CGEventTapLocation::HID);
        }

        Ok(text.chars().count())
    }

    fn paste(text: &str) -> Result<usize> {
        let previous = pasteboard::read();
        pasteboard::write(text)?;

        let source = Self::source()?;
        let down = CGEvent::new_keyboard_event(source.clone(), KEY_V, true)
            .map_err(|()| Error::Injection("could not create a key event".to_string()))?;
        down.set_flags(CGEventFlags::CGEventFlagCommand);
        down.post(CGEventTapLocation::HID);

        let up = CGEvent::new_keyboard_event(source, KEY_V, false)
            .map_err(|()| Error::Injection("could not create a key event".to_string()))?;
        up.set_flags(CGEventFlags::CGEventFlagCommand);
        up.post(CGEventTapLocation::HID);

        // Restore only after the paste has been consumed. This races with
        // clipboard managers, and with a target app that reads the pasteboard
        // late; 150 ms is the usual compromise.
        if let Some(previous) = previous {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(150));
                if let Err(err) = pasteboard::write(&previous) {
                    tracing::warn!(%err, "could not restore the clipboard");
                }
            });
        }

        Ok(text.chars().count())
    }
}

impl TextSink for MacTextSink {
    fn inject(&self, text: &str) -> Result<usize> {
        // Showing the widget can leave Speakeasy frontmost, and synthetic
        // keystrokes follow the frontmost app — into a panel that refuses key
        // status, which is to say nowhere. Put the focus back first.
        if super::restore_focus() {
            // Activation is asynchronous. Posting into the gap is exactly the
            // failure this is here to prevent.
            std::thread::sleep(std::time::Duration::from_millis(120));
        }

        // Where the text is actually about to go. Worth a line: injecting into
        // the wrong app is silent, and indistinguishable from dropping it.
        let (frontmost, frontmost_pid) = super::frontmost_app();
        tracing::debug!(frontmost, frontmost_pid, "injecting");

        if Self::secure_input_active() {
            return Err(Error::SecureInputActive);
        }
        if text.is_empty() {
            return Ok(0);
        }

        match self.mode {
            InjectionMode::Synthetic => Self::type_unicode(text),
            InjectionMode::Paste => Self::paste(text),
        }
    }
}

/// Minimal NSPasteboard access over the Objective-C runtime, to avoid pulling
/// in a full AppKit binding for two calls.
mod pasteboard {
    use super::*;
    use std::ffi::CString;

    extern "C" {
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSend0 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSend1 = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type MsgSend2 =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> bool;

    unsafe fn class(name: &str) -> *mut c_void {
        let c = CString::new(name).expect("class name has no NUL");
        objc_getClass(c.as_ptr() as *const u8)
    }

    unsafe fn sel(name: &str) -> *mut c_void {
        let c = CString::new(name).expect("selector has no NUL");
        sel_registerName(c.as_ptr() as *const u8)
    }

    unsafe fn general() -> *mut c_void {
        let send: MsgSend0 = std::mem::transmute(objc_msgSend as *const ());
        send(class("NSPasteboard"), sel("generalPasteboard"))
    }

    unsafe fn ns_string(text: &str) -> *mut c_void {
        let send1: MsgSend1 = std::mem::transmute(objc_msgSend as *const ());
        let c = CString::new(text).unwrap_or_else(|_| CString::new("").expect("empty CString"));
        send1(
            class("NSString"),
            sel("stringWithUTF8String:"),
            c.as_ptr() as *mut c_void,
        )
    }

    /// Current plain-text contents, if any.
    pub fn read() -> Option<String> {
        unsafe {
            let send1: MsgSend1 = std::mem::transmute(objc_msgSend as *const ());
            let send0: MsgSend0 = std::mem::transmute(objc_msgSend as *const ());

            let value = send1(
                general(),
                sel("stringForType:"),
                ns_string("public.utf8-plain-text"),
            );
            if value.is_null() {
                return None;
            }

            let utf8 = send0(value, sel("UTF8String"));
            if utf8.is_null() {
                return None;
            }
            std::ffi::CStr::from_ptr(utf8 as *const i8)
                .to_str()
                .ok()
                .map(str::to_owned)
        }
    }

    pub fn write(text: &str) -> Result<()> {
        unsafe {
            let send0: MsgSend0 = std::mem::transmute(objc_msgSend as *const ());
            let send2: MsgSend2 = std::mem::transmute(objc_msgSend as *const ());

            let board = general();
            send0(board, sel("clearContents"));
            let ok = send2(
                board,
                sel("setString:forType:"),
                ns_string(text),
                ns_string("public.utf8-plain-text"),
            );
            if ok {
                Ok(())
            } else {
                Err(Error::Injection(
                    "the pasteboard rejected the text".to_string(),
                ))
            }
        }
    }
}
