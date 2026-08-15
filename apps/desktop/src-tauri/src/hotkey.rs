//! Wires the platform's hotkey listener to the engine.
//!
//! This used to drive Tauri's global-shortcut plugin. It cannot: that plugin
//! needs an accelerator with a non-modifier key, and it reports "Option" with
//! no side. Push-to-talk on a bare Right Option needs the event tap in
//! `speakeasy-platform`, behind the `HotkeyListener` port.
//!
//! Push-to-talk versus toggle is not decided here — the engine reads the
//! configured activation mode and answers with a [`HotkeyAction`], so this
//! layer stays a pure translator.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use speakeasy_core::{Engine, Hotkey, HotkeyAction, HotkeyListener};

/// The listener owns an OS-level tap and must outlive `setup`.
static LISTENER: OnceLock<Box<dyn HotkeyListener>> = OnceLock::new();

/// Whether a binding is currently live.
///
/// Tracked here rather than asked of the listener, because the port has no way
/// to answer it and adding one would mean every implementation carrying state
/// only this caller wants.
static BOUND: AtomicBool = AtomicBool::new(false);

/// Bind only if nothing is bound — for a grant that arrived after startup.
///
/// Distinct from [`register`] because the settings window calls this every time
/// it regains focus. Re-registering there would tear down a working tap and
/// build a new one on each focus, leaving a gap where the hotkey is dead.
pub fn bind_if_unbound(engine: Engine, hotkey: &Hotkey) -> Result<(), Box<dyn std::error::Error>> {
    if BOUND.load(Ordering::SeqCst) {
        return Ok(());
    }
    register(engine, hotkey)
}

fn listener() -> &'static dyn HotkeyListener {
    LISTENER
        .get_or_init(speakeasy_platform::hotkey_listener)
        .as_ref()
}

/// Read one binding from the keyboard and adopt it.
///
/// The captured key goes straight to the engine rather than back to the
/// caller. `set_hotkey` then persists it and emits `HotkeyChanged`, which is
/// what both re-registers the binding and tells the settings window — so there
/// is one path for "the binding changed" instead of a second one that only
/// capture uses.
pub fn capture(engine: Engine) -> Result<(), Box<dyn std::error::Error>> {
    listener().capture(Box::new(move |hotkey| {
        tracing::info!(hotkey = hotkey.label(), "captured a new binding");
        engine.set_hotkey(hotkey);
    }))?;
    Ok(())
}

pub fn cancel_capture() {
    listener().cancel_capture();
}

pub fn register(engine: Engine, hotkey: &Hotkey) -> Result<(), Box<dyn std::error::Error>> {
    let listener = listener();

    let cancel_engine = engine.clone();
    listener.register(
        hotkey,
        Box::new(move |event| match engine.hotkey_action(event) {
            HotkeyAction::Begin => {
                if let Err(err) = engine.begin_recording() {
                    tracing::warn!(%err, "could not start recording");
                }
            }
            HotkeyAction::Finish => {
                // Transcription blocks; keep it off the tap's thread or the
                // event tap stalls and macOS eventually disables it.
                let engine = engine.clone();
                std::thread::spawn(move || {
                    if let Err(err) = engine.finish_recording() {
                        tracing::warn!(%err, "recording did not complete");
                    }
                });
            }
            HotkeyAction::Ignore => {}
        }),
        Box::new(move || cancel_engine.cancel_recording_if_active()),
    )?;

    BOUND.store(true, Ordering::SeqCst);
    Ok(())
}
