//! The floating status widget.
//!
//! Shown while an utterance is in flight, hidden when idle.
//!
//! The hard requirement is that it must never take focus. Injection targets
//! "whatever had focus when the user spoke", so a widget that activated would
//! make Speakeasy paste into its own window. Hence `focus: false` in the config,
//! `set_focusable(false)` here, and `show()` without `set_focus()`.
//!
//! It also ignores the cursor, so it cannot swallow a click meant for whatever
//! is underneath.

use speakeasy_core::State;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow};

pub const LABEL: &str = "widget";

/// Gap between the widget and the bottom of the screen, in logical pixels.
const BOTTOM_MARGIN: f64 = 120.0;

/// Show or hide the widget to match engine state.
///
/// **Must be called on the main thread.** It touches NSWindow, and AppKit traps
/// the process rather than misbehaving if you call it from anywhere else. The
/// event bridge hops threads before calling this.
pub fn sync(app: &AppHandle, state: &State) {
    let Some(window) = app.get_webview_window(LABEL) else {
        tracing::warn!("no window labelled `{LABEL}`");
        return;
    };

    // Idle is the resting state; everything else is worth showing. `Failed`
    // stays up so the user actually sees what went wrong.
    let visible = !matches!(state, State::Idle);

    let result = if visible {
        position(&window)
            .and_then(|()| {
                #[cfg(target_os = "macos")]
                make_overlay(&window)?;
                Ok(())
            })
            .and_then(|()| window.show())
    } else {
        window.hide()
    };

    if let Err(err) = result {
        tracing::warn!(%err, visible, "could not update the widget");
        return;
    }

    tracing::debug!(
        visible,
        is_visible = ?window.is_visible(),
        position = ?window.outer_position(),
        size = ?window.outer_size(),
        "widget updated"
    );
}

/// Make the window a true overlay: visible over fullscreen apps, above
/// everything, and skipped by window cycling.
///
/// Tauri's `set_always_on_top` and `set_visible_on_all_workspaces` are not
/// enough. macOS keeps ordinary windows out of a fullscreen app's Space unless
/// the window opts in with `fullScreenAuxiliary`, and that behaviour is not
/// exposed by Tauri — so reach through to the NSWindow.
#[cfg(target_os = "macos")]
fn make_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    use std::ffi::{c_void, CString};

    extern "C" {
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    // NSWindowCollectionBehavior bits.
    const CAN_JOIN_ALL_SPACES: u64 = 1 << 0;
    const STATIONARY: u64 = 1 << 4;
    const IGNORES_CYCLE: u64 = 1 << 6;
    const FULL_SCREEN_AUXILIARY: u64 = 1 << 8;
    /// NSScreenSaverWindowLevel — above the fullscreen Space's own chrome.
    const SCREEN_SAVER_LEVEL: i64 = 1000;

    let ns_window = window.ns_window()?;

    unsafe {
        let send_u64: unsafe extern "C" fn(*mut c_void, *mut c_void, u64) =
            std::mem::transmute(objc_msgSend as *const ());
        let send_i64: unsafe extern "C" fn(*mut c_void, *mut c_void, i64) =
            std::mem::transmute(objc_msgSend as *const ());
        let sel = |name: &str| {
            let c = CString::new(name).expect("selector has no NUL");
            sel_registerName(c.as_ptr() as *const u8)
        };

        send_u64(
            ns_window,
            sel("setCollectionBehavior:"),
            CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY,
        );
        send_i64(ns_window, sel("setLevel:"), SCREEN_SAVER_LEVEL);
    }

    Ok(())
}

/// Park the widget bottom-centre of the active monitor.
///
/// Recomputed on every show rather than once, so it follows the user between
/// displays and survives resolution changes.
fn position(window: &WebviewWindow) -> tauri::Result<()> {
    window.set_focusable(false)?;
    window.set_ignore_cursor_events(true)?;
    window.set_always_on_top(true)?;
    // Without this the widget never appears over a fullscreen app, because
    // macOS confines ordinary windows to their own Space — and dictating into
    // a fullscreen editor or browser is a normal thing to want.
    window.set_visible_on_all_workspaces(true)?;

    let Some(monitor) = window.current_monitor()? else {
        return Ok(()); // no monitor info; leave it wherever it is
    };

    // All physical, deliberately: mixing logical and physical units here is
    // easy to get wrong and lands the widget somewhere arbitrary.
    let screen = monitor.size();
    let origin = monitor.position();
    let size = window.outer_size()?;
    let margin = (BOTTOM_MARGIN * monitor.scale_factor()) as i32;

    window.set_position(PhysicalPosition::new(
        origin.x + (screen.width as i32 - size.width as i32) / 2,
        origin.y + screen.height as i32 - size.height as i32 - margin,
    ))
}
