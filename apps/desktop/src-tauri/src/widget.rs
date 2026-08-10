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
        position(&window, saved).and_then(|()| {
            // On macOS `make_overlay` does the showing, via
            // `orderFrontRegardless`. Tauri's `show` is deliberately not called
            // here: it reaches AppKit as `makeKeyAndOrderFront:`, and a key
            // widget takes the focus the transcript is aimed at — the text ends
            // up nowhere and the next shortcut beeps against a panel that has
            // no use for it.
            #[cfg(target_os = "macos")]
            return make_overlay(&window);
            #[cfg(not(target_os = "macos"))]
            window.show()
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
        fn objc_allocateClassPair(
            superclass: *mut c_void,
            name: *const u8,
            extra: usize,
        ) -> *mut c_void;
        fn objc_registerClassPair(cls: *mut c_void);
        fn class_addMethod(
            cls: *mut c_void,
            name: *mut c_void,
            imp: *const c_void,
            types: *const u8,
        ) -> i8;
        fn objc_msgSend();
    }

    /// `NO`, as the implementation of a `-> BOOL` method.
    ///
    /// This is the whole point of the subclass below. Refusing key status has
    /// to be a property of the class, not a flag set after the fact: `show`
    /// reaches AppKit as `makeKeyAndOrderFront:`, and the only thing that stops
    /// that making the window key is `canBecomeKeyWindow` answering NO.
    extern "C" fn refuse(_: *mut c_void, _: *mut c_void) -> i8 {
        0
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
                // Subclass NSPanel to refuse key and main status outright.
                //
                // Setting a flag is not enough. tao's own NSWindow subclass had
                // exactly this, driven by `set_focusable`, and promoting the
                // window threw it away — which is how the widget started
                // stealing focus from the app being dictated into and eating
                // its keystrokes. Overriding the two methods puts the refusal
                // somewhere nothing can undo.
                let subclass_name =
                    CString::new("SpeakeasyWidgetPanel").expect("class name has no NUL");
                let subclass =
                    objc_allocateClassPair(panel_class, subclass_name.as_ptr() as *const u8, 0);

                let class = if subclass.is_null() {
                    // Already registered, which means a previous call got here.
                    tracing::warn!("could not allocate the panel subclass; using NSPanel");
                    panel_class
                } else {
                    // "c@:" — returns BOOL (a char), takes self and _cmd.
                    let types = CString::new("c@:").expect("type encoding has no NUL");
                    for method in ["canBecomeKeyWindow", "canBecomeMainWindow"] {
                        class_addMethod(
                            subclass,
                            sel(method),
                            refuse as *const c_void,
                            types.as_ptr() as *const u8,
                        );
                    }
                    objc_registerClassPair(subclass);
                    subclass
                };

                object_setClass(ns_window, class);

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

        // Read back rather than trust the writes. Every one of these settings
        // fails silently — the window stays alive and reports itself visible
        // while being on the wrong Space, or quietly accepting keystrokes the
        // user meant for something else. Two lines here cost nothing and are
        // the only way any of it is observable from outside the process.
        tracing::debug!(
            // False means the widget is drawn on a Space the user is not
            // looking at, which is indistinguishable from not showing at all.
            on_active_space = read_bool(ns_window, sel("isOnActiveSpace")) != 0,
            // Must stay false. A key panel takes the focus the transcript is
            // aimed at, and swallows the shortcuts meant for the app beneath.
            is_key = read_bool(ns_window, sel("isKeyWindow")) != 0,
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
