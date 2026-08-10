//! The floating status widget.
//!
//! Shown while an utterance is in flight, hidden when idle.
//!
//! The hard requirement is that it must never take focus. Injection targets
//! "whatever had focus when the user spoke", so a widget that activated would
//! make Speakeasy paste into its own window. Hence `focus: false` in the
//! config, `show()` without `set_focus()`, and on macOS a non-activating
//! NSPanel — see [`make_overlay`], which is also what gets the widget on screen
//! over a fullscreen app.
//!
//! It is draggable, which is in tension with that: accepting a click means the
//! pill can swallow one meant for the window underneath, and the widget is only
//! on screen *during* a dictation. Never becoming key is what keeps the drag
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

/// Whether the widget's NSWindow has been promoted to an NSPanel. The swap is
/// destructive and needs to happen exactly once. See `make_overlay`.
#[cfg(target_os = "macos")]
static PROMOTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
            .and_then(|()| window.show())
            .and_then(|()| {
                // After `show`, not before: it ends with `orderFrontRegardless`,
                // which is what actually puts the panel on screen, and ordering
                // a window front before showing it achieves nothing.
                #[cfg(target_os = "macos")]
                make_overlay(&window)?;
                Ok(())
            })
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
/// enough, and neither is `fullScreenAuxiliary` on its own. A plain NSWindow
/// belonging to a background app will not join another app's fullscreen Space
/// however it is flagged — it stays alive, reports itself visible, and draws
/// nowhere. Only an NSPanel will, so the window is promoted to one and the
/// remaining bits are set by hand, none of them exposed by Tauri.
///
/// **macOS only.** Everywhere else the Tauri calls in [`position`] are all
/// there is.
#[cfg(target_os = "macos")]
fn make_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    use std::ffi::{c_void, CString};

    extern "C" {
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn object_setClass(obj: *mut c_void, cls: *mut c_void) -> *mut c_void;
        fn objc_msgSend();
    }

    /// NSWindowStyleMaskNonactivatingPanel. Only meaningful on an NSPanel, and
    /// the reason the promotion is worth doing: it is what lets a window come
    /// forward without its app becoming active.
    const NONACTIVATING_PANEL: u64 = 1 << 7;

    // NSWindowCollectionBehavior bits.
    const CAN_JOIN_ALL_SPACES: u64 = 1 << 0;
    const STATIONARY: u64 = 1 << 4;
    const IGNORES_CYCLE: u64 = 1 << 6;
    const FULL_SCREEN_AUXILIARY: u64 = 1 << 8;
    /// NSScreenSaverWindowLevel — above the fullscreen Space's own chrome.
    const SCREEN_SAVER_LEVEL: i64 = 1000;

    let ns_window = window.ns_window()?;

    unsafe {
        let sel = |name: &str| {
            let c = CString::new(name).expect("selector has no NUL");
            sel_registerName(c.as_ptr() as *const u8)
        };
        let send_u64: unsafe extern "C" fn(*mut c_void, *mut c_void, u64) =
            std::mem::transmute(objc_msgSend as *const ());
        let send_i64: unsafe extern "C" fn(*mut c_void, *mut c_void, i64) =
            std::mem::transmute(objc_msgSend as *const ());
        let send_bool: unsafe extern "C" fn(*mut c_void, *mut c_void, i8) =
            std::mem::transmute(objc_msgSend as *const ());
        let send: unsafe extern "C" fn(*mut c_void, *mut c_void) =
            std::mem::transmute(objc_msgSend as *const ());
        let read_u64: unsafe extern "C" fn(*mut c_void, *mut c_void) -> u64 =
            std::mem::transmute(objc_msgSend as *const ());
        let read_i64: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i64 =
            std::mem::transmute(objc_msgSend as *const ());
        // BOOL is a signed char on arm64, not a word.
        let read_bool: unsafe extern "C" fn(*mut c_void, *mut c_void) -> i8 =
            std::mem::transmute(objc_msgSend as *const ());

        // Promote to NSPanel once, on the first show.
        //
        // Repeating it would be harmless in itself, but the swap is not free:
        // it replaces tao's own NSWindow subclass, so anything tao stored on
        // the instance goes with it. Doing it once, here, keeps that damage to
        // a single known point — see `position` for what it costs.
        if !PROMOTED.swap(true, Ordering::SeqCst) {
            let panel_name = CString::new("NSPanel").expect("class name has no NUL");
            let panel_class = objc_getClass(panel_name.as_ptr() as *const u8);
            if panel_class.is_null() {
                tracing::warn!("NSPanel class not found; widget stays a plain window");
            } else {
                object_setClass(ns_window, panel_class);

                let mask = read_u64(ns_window, sel("styleMask"));
                send_u64(ns_window, sel("setStyleMask:"), mask | NONACTIVATING_PANEL);

                // NSPanel defaults `hidesOnDeactivate` to true, and this app is
                // deactivated by definition. Left alone it would undo the whole
                // exercise: the panel would vanish the instant focus returned
                // to the app being dictated into.
                send_bool(ns_window, sel("setHidesOnDeactivate:"), 0);
                // Float above ordinary windows without demanding key status.
                send_bool(ns_window, sel("setFloatingPanel:"), 1);
                send_bool(ns_window, sel("setBecomesKeyOnlyIfNeeded:"), 1);
            }
        }

        send_u64(
            ns_window,
            sel("setCollectionBehavior:"),
            CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY,
        );
        send_i64(ns_window, sel("setLevel:"), SCREEN_SAVER_LEVEL);

        // `show` reaches AppKit as `orderFront:`, which is ignored when the
        // calling app is not the active one — and this app is never active by
        // construction. This is the version that says "front, even so".
        send(ns_window, sel("orderFrontRegardless"));

        // Read back rather than trust the writes: whether AppKit kept them is
        // the whole question when the widget fails to appear, and none of it is
        // visible from the outside.
        let behavior = read_u64(ns_window, sel("collectionBehavior"));
        tracing::debug!(
            level = read_i64(ns_window, sel("level")),
            expected_level = SCREEN_SAVER_LEVEL,
            behavior = format!("{behavior:#x}"),
            joins_all_spaces = behavior & CAN_JOIN_ALL_SPACES != 0,
            fullscreen_auxiliary = behavior & FULL_SCREEN_AUXILIARY != 0,
            // The telling one: a window that is "visible" but not on the active
            // Space is a window the user cannot see.
            on_active_space = read_bool(ns_window, sel("isOnActiveSpace")) != 0,
            ns_visible = read_bool(ns_window, sel("isVisible")) != 0,
            nonactivating_panel = read_u64(ns_window, sel("styleMask")) & NONACTIVATING_PANEL != 0,
            "widget overlay applied"
        );
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
    // Dragging needs clicks, so this cannot be true. See the module docs for
    // what that costs and why the window never taking focus is what makes it
    // safe.
    window.set_ignore_cursor_events(false)?;

    // On macOS `make_overlay` owns all three of focusable, always-on-top and
    // all-workspaces, and calling the Tauri versions here is not merely
    // redundant — `set_focusable` panics. It reads a `focusable` ivar off
    // tao's own NSWindow subclass, which the promotion to NSPanel replaced,
    // and a missing ivar is a hard panic rather than a no-op. The panel's
    // non-activating style mask is what supersedes it.
    #[cfg(not(target_os = "macos"))]
    {
        window.set_focusable(false)?;
        window.set_always_on_top(true)?;
        window.set_visible_on_all_workspaces(true)?;
    }

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
