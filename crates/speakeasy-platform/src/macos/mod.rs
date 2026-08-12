mod focus;
mod hotkey;
mod keycode;
mod secure_input;
mod text_sink;

use std::ffi::{c_void, CString};

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use speakeasy_core::config::InjectionMode;
use speakeasy_core::{HotkeyListener, TextSink};

pub use hotkey::MacHotkeys;
pub use text_sink::MacTextSink;

/// Name and pid of the frontmost application, as NSWorkspace sees it.
///
/// The pid is the part that matters: a name can be ambiguous, but comparing
/// against our own pid says without argument whether the app about to receive
/// synthetic keystrokes is us.
pub(crate) fn frontmost_app() -> (String, i64) {
    extern "C" {
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    unsafe {
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let read_i64: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i64 =
            std::mem::transmute(objc_msgSend as *const ());
        let sel = |name: &str| {
            let c = CString::new(name).expect("selector has no NUL");
            sel_registerName(c.as_ptr() as *const u8)
        };

        let class = CString::new("NSWorkspace").expect("class name has no NUL");
        let workspace_class = objc_getClass(class.as_ptr() as *const u8);
        if workspace_class.is_null() {
            return ("<no NSWorkspace>".to_string(), -1);
        }

        let workspace = send(workspace_class, sel("sharedWorkspace"));
        let app = send(workspace, sel("frontmostApplication"));
        if app.is_null() {
            return ("<none>".to_string(), -1);
        }

        let pid = read_i64(app, sel("processIdentifier"));
        let name = send(app, sel("localizedName"));
        let utf8 = if name.is_null() {
            std::ptr::null_mut()
        } else {
            send(name, sel("UTF8String"))
        };
        let name = if utf8.is_null() {
            "<unnamed>".to_string()
        } else {
            std::ffi::CStr::from_ptr(utf8 as *const i8)
                .to_string_lossy()
                .into_owned()
        };

        (name, pid)
    }
}

/// Pid of the app that was frontmost when dictation started, or -1.
///
/// Showing the widget makes Speakeasy the frontmost application — a panel at
/// screen-saver level, ordered front, is the frontmost window on the display,
/// and the window server follows it. That is fatal for injection: keystrokes
/// go to the frontmost app, and ours refuses key status, so they arrive
/// nowhere at all. The user sees the transcript vanish and hears a beep for
/// every shortcut afterwards.
static FOCUS_TARGET: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-1);

/// Remember who owned the focus, before the widget takes it.
pub fn remember_focus() {
    let (name, pid) = frontmost_app();
    if pid > 0 && pid != std::process::id() as i64 {
        FOCUS_TARGET.store(pid, std::sync::atomic::Ordering::SeqCst);
        tracing::debug!(app = name, pid, "remembered the injection target");
    }
}

/// Hand the focus back to that app, if we have taken it.
///
/// Returns whether it had to act, so the caller can let activation settle
/// before posting anything — it is asynchronous, and a paste that arrives
/// first lands in the wrong place, which is the whole bug.
pub fn restore_focus() -> bool {
    let (_, frontmost) = frontmost_app();
    if frontmost != std::process::id() as i64 {
        return false; // focus is already somewhere else; leave it alone
    }

    let target = FOCUS_TARGET.load(std::sync::atomic::Ordering::SeqCst);
    if target < 0 {
        return false;
    }

    extern "C" {
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    /// `NSApplicationActivateIgnoringOtherApps`. Required: we are asking on
    /// behalf of an app the user is not currently interacting with.
    const IGNORING_OTHER_APPS: u64 = 1 << 1;

    unsafe {
        let send_i64: unsafe extern "C" fn(*mut c_void, *mut c_void, i64) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let send_u64: unsafe extern "C" fn(*mut c_void, *mut c_void, u64) -> i8 =
            std::mem::transmute(objc_msgSend as *const ());
        let sel = |name: &str| {
            let c = CString::new(name).expect("selector has no NUL");
            sel_registerName(c.as_ptr() as *const u8)
        };

        let class = CString::new("NSRunningApplication").expect("class name has no NUL");
        let running = objc_getClass(class.as_ptr() as *const u8);
        if running.is_null() {
            return false;
        }

        let app = send_i64(
            running,
            sel("runningApplicationWithProcessIdentifier:"),
            target,
        );
        if app.is_null() {
            tracing::warn!(pid = target, "the injection target is gone");
            return false;
        }

        let ok = send_u64(app, sel("activateWithOptions:"), IGNORING_OTHER_APPS) != 0;
        tracing::debug!(pid = target, ok, "handed focus back before injecting");
        ok
    }
}

pub fn text_sink(mode: InjectionMode) -> Box<dyn TextSink> {
    Box::new(MacTextSink::new(mode))
}

pub fn hotkey_listener() -> Box<dyn HotkeyListener> {
    Box::new(MacHotkeys::new())
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;

    /// Key for the one option this call takes: whether to show the system
    /// prompt when the answer is "not trusted".
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

/// Whether Accessibility has been granted.
///
/// Answered by trying to create an event tap rather than by asking
/// `AXIsProcessTrustedWithOptions`, which caches its result for the life of the
/// process: grant the permission to a running app and that call goes on saying
/// "not trusted" until restart. Building a tap is the capability itself and
/// cannot be stale.
///
/// Only Accessibility. The microphone is a separate TCC grant, but macOS
/// prompts for it by itself the first time the audio device is opened, so
/// there is nothing here that asking early would improve.
pub fn permissions_granted() -> bool {
    hotkey::can_tap()
}

/// Ask for Accessibility, showing the system prompt if it is not granted.
///
/// The prompt is also what *registers* the app in System Settings › Privacy &
/// Security › Accessibility. An app that never calls this never appears in that
/// list at all, so there is nothing for the user to switch on — which reads as
/// the app being broken rather than unpermitted.
pub fn request_permissions() -> bool {
    accessibility_trusted(true)
}

fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let value = CFBoolean::from(prompt);
        let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

pub fn open_permission_settings() {
    // Ask first. If the grant is missing this both prompts and creates the
    // entry, so the pane we open next has a row to toggle.
    request_permissions();

    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}
