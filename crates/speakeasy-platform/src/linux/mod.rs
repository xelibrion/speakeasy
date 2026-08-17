mod hotkey;
mod keyboard_watch;
mod text_sink;

use speakeasy_core::config::InjectionMode;
use speakeasy_core::{HotkeyListener, KeyboardWatcher, TextSink};

pub use hotkey::X11Hotkeys;
pub use keyboard_watch::X11KeyboardWatcher;
pub use text_sink::X11TextSink;

pub fn text_sink(mode: InjectionMode) -> Box<dyn TextSink> {
    Box::new(X11TextSink::new(mode))
}

pub fn hotkey_listener() -> Box<dyn HotkeyListener> {
    Box::new(X11Hotkeys::new())
}

pub fn keyboard_watcher() -> Box<dyn KeyboardWatcher> {
    Box::new(X11KeyboardWatcher::new())
}

/// X11 has no permission model for synthetic input or capture.
pub fn permissions_granted() -> bool {
    true
}

pub fn open_permission_settings() {}
