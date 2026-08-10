//! The floating status widget.
//!
//! Shown while an utterance is in flight, hidden when idle.
//!
//! The hard requirement is that it must never take focus. Injection targets
//! "whatever had focus when the user spoke", so a widget that activated would
//! make Speakeasy paste into its own window. Hence `focus: false` in the config,
//! `set_focusable(false)` here, and `show()` without `set_focus()`.
//!
//! It is draggable, which is in tension with that: accepting a click means the
//! pill can swallow one meant for the window underneath, and the widget is only
//! on screen *during* a dictation. `set_focusable(false)` is what keeps the drag
//! from making Speakeasy the active app and redirecting the user's text. The
//! pill is small and parks out of the way, which is the rest of the mitigation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use speakeasy_core::{Engine, State, WidgetPosition};
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow, WindowEvent};

pub const LABEL: &str = "widget";

/// Gap between the widget and the bottom of the screen, in logical pixels.
const BOTTOM_MARGIN: f64 = 120.0;

/// How long the widget must sit still before its position is written to disk.
/// A drag emits a `Moved` per frame; without this, so would the config file.
const SETTLE: Duration = Duration::from_millis(600);

/// Bumped on every move. A pending save that finds it changed knows a newer
/// move is already waiting and drops out, so a drag costs exactly one write.
static MOVES: AtomicU64 = AtomicU64::new(0);

/// Persist the widget's position after the user drags it.
pub fn remember_moves(app: &AppHandle, engine: &Engine) {
    let Some(window) = app.get_webview_window(LABEL) else {
        tracing::warn!("no window labelled `{LABEL}`; positions will not be remembered");
        return;
    };

    let engine = Arc::new(engine.clone());
    window.on_window_event(move |event| {
        let WindowEvent::Moved(position) = event else {
            return;
        };
        let position = WidgetPosition {
            x: position.x,
            y: position.y,
        };

        let move_id = MOVES.fetch_add(1, Ordering::SeqCst) + 1;
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            if MOVES.load(Ordering::SeqCst) == move_id {
                // No-ops when this was our own `set_position` echoing back.
                engine.set_widget_position(position);
            }
        });
    });
}

/// Show or hide the widget to match engine state.
///
/// **Must be called on the main thread.** It touches NSWindow, and AppKit traps
/// the process rather than misbehaving if you call it from anywhere else. The
/// event bridge hops threads before calling this.
pub fn sync(app: &AppHandle, state: &State, saved: Option<WidgetPosition>) {
    let Some(window) = app.get_webview_window(LABEL) else {
        tracing::warn!("no window labelled `{LABEL}`");
        return;
    };

    // Idle is the resting state; everything else is worth showing. `Failed`
    // stays up so the user actually sees what went wrong.
    let visible = !matches!(state, State::Idle);

    let result = if visible {
        position(&window, saved)
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

/// Put the widget where the user left it, or bottom-centre of the active
/// monitor if they never moved it.
///
/// The default is recomputed on every show rather than once, so it follows the
/// user between displays and survives resolution changes. A saved position is
/// used as-is: second-guessing it is how a deliberately-placed widget creeps
/// back to the middle.
fn position(window: &WebviewWindow, saved: Option<WidgetPosition>) -> tauri::Result<()> {
    window.set_focusable(false)?;
    // Dragging needs clicks, so this cannot be true. See the module docs for
    // what that costs and why `set_focusable(false)` is what makes it safe.
    window.set_ignore_cursor_events(false)?;
    window.set_always_on_top(true)?;
    // Without this the widget never appears over a fullscreen app, because
    // macOS confines ordinary windows to their own Space — and dictating into
    // a fullscreen editor or browser is a normal thing to want.
    window.set_visible_on_all_workspaces(true)?;

    if let Some(saved) = saved {
        return window.set_position(PhysicalPosition::new(saved.x, saved.y));
    }

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
